//! A small MCP server used by tests and as a demo target for the inspector
//! (`irs-mock-mcp` binary: stdio by default, `--http <port>` for Streamable HTTP).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::{
    Router,
    body::Body,
    extract::State,
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
    routing::post,
};
use serde_json::{Value, json};

/// What the server sends in reply to one inbound message.
#[derive(Debug, Default)]
pub struct Reply {
    /// Notifications emitted before the response (e.g. progress / log messages).
    pub before: Vec<Value>,
    pub response: Option<Value>,
}

pub fn tools_page(cursor: Option<&str>) -> Value {
    let page1 = json!([
        {
            "name": "get_weather",
            "title": "Get weather",
            "description": "Current weather for a **city**.\n\nReturns temperature and conditions.",
            "inputSchema": {
                "type": "object",
                "required": ["city"],
                "properties": {
                    "city": {"type": "string", "description": "City name, e.g. `Berlin`", "minLength": 1},
                    "units": {"type": "string", "enum": ["metric", "imperial"], "default": "metric", "description": "Unit system"}
                }
            },
            "annotations": {"readOnlyHint": true, "openWorldHint": true}
        },
        {
            "name": "create_issue",
            "description": "Create an issue in a repository.",
            "inputSchema": {
                "type": "object",
                "required": ["repo", "title"],
                "properties": {
                    "repo": {"type": "string", "pattern": "^[\\w-]+/[\\w-]+$", "description": "owner/name"},
                    "title": {"type": "string", "maxLength": 120},
                    "body": {"type": "string", "description": "Markdown body"},
                    "labels": {"type": "array", "items": {"type": "string"}, "uniqueItems": true},
                    "assignee": {"$ref": "#/$defs/user"},
                    "priority": {"type": "integer", "minimum": 1, "maximum": 5, "default": 3}
                },
                "$defs": {"user": {"type": "object", "required": ["login"], "properties": {
                    "login": {"type": "string", "description": "GitHub handle"},
                    "id": {"anyOf": [{"type": "integer"}, {"type": "null"}]}
                }}}
            },
            "annotations": {"destructiveHint": false, "idempotentHint": false, "openWorldHint": true}
        },
        {
            "name": "delete_repo",
            "description": "Permanently delete a repository.",
            "inputSchema": {"type": "object", "required": ["repo", "confirm"], "properties": {
                "repo": {"type": "string"}, "confirm": {"type": "boolean", "description": "Must be true"}
            }},
            "annotations": {"title": "Delete repository", "destructiveHint": true, "idempotentHint": true}
        }
    ]);
    let page2 = json!([
        {
            "name": "search",
            "description": "Full-text search with optional filters.",
            "inputSchema": {"type": "object", "required": ["query"], "properties": {
                "query": {"type": "string"},
                "limit": {"type": "integer", "minimum": 1, "maximum": 100, "default": 10},
                "filters": {"type": "object", "properties": {
                    "since": {"type": "string", "format": "date"},
                    "kinds": {"type": "array", "items": {"type": "string", "enum": ["issue", "pr", "commit"]}}
                }}
            }}
        },
        {
            "name": "echo",
            "description": "Echo the arguments back as text and structured content.",
            "inputSchema": {"type": "object", "properties": {"message": {"type": "string"}}},
            "outputSchema": {"type": "object", "properties": {"echo": {"type": "object"}}, "required": ["echo"]},
            "annotations": {"readOnlyHint": true, "openWorldHint": false}
        },
        {"name": "notify", "description": "Emits a log notification, then returns.", "inputSchema": {"type": "object"}},
        {"name": "fail", "description": "Always returns a tool error (isError: true).", "inputSchema": {"type": "object"}},
        {"name": "slow", "description": "Sleeps for `ms` milliseconds.", "inputSchema": {"type": "object", "properties": {"ms": {"type": "integer", "default": 1500}}}},
        {"name": "ask_roots", "description": "Asks the client for its roots (stdio only) and returns them.", "inputSchema": {"type": "object"}},
        {"name": "ask_llm", "description": "Asks the client's LLM via MCP sampling (stdio only).", "inputSchema": {"type": "object", "properties": {"prompt": {"type": "string"}}, "required": ["prompt"]}, "annotations": {"readOnlyHint": true}}
    ]);
    match cursor {
        Some("page2") => json!({ "tools": page2 }),
        _ => json!({ "tools": page1, "nextCursor": "page2" }),
    }
}

fn ok(id: &Value, result: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "result": result})
}

fn err(id: &Value, code: i64, message: &str) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
}

/// Handle one inbound frame. `ask_roots` is handled by the stdio loop.
pub async fn handle(msg: &Value) -> Reply {
    let Some(method) = msg.get("method").and_then(Value::as_str) else {
        return Reply::default();
    };
    let Some(id) = msg.get("id").filter(|i| !i.is_null()) else {
        return Reply::default();
    }; // notification
    let p = msg.get("params").cloned().unwrap_or(json!({}));
    let response = match method {
        "initialize" => ok(
            id,
            json!({
                "protocolVersion": p["protocolVersion"].as_str().unwrap_or(crate::LATEST_PROTOCOL_VERSION),
                "capabilities": {"tools": {"listChanged": false}, "resources": {}, "prompts": {}, "logging": {}},
                "serverInfo": {"name": "irs-mock-mcp", "title": "Mock MCP server", "version": "1.0.0"},
                "instructions": "Demo server for insomnia-rs. Try `get_weather` or `create_issue`."
            }),
        ),
        "ping" => ok(id, json!({})),
        "tools/list" => ok(id, tools_page(p["cursor"].as_str())),
        "tools/call" => {
            let args = p.get("arguments").cloned().unwrap_or(json!({}));
            match p["name"].as_str().unwrap_or("") {
                "echo" => ok(
                    id,
                    json!({
                        "content": [{"type": "text", "text": args.to_string()}],
                        "structuredContent": {"echo": args}
                    }),
                ),
                "get_weather" => {
                    let city = args["city"].as_str().unwrap_or("?");
                    ok(
                        id,
                        json!({"content": [{"type": "text", "text": format!("{city}: 21°C, clear sky")}]}),
                    )
                }
                "fail" => ok(
                    id,
                    json!({"content": [{"type": "text", "text": "something went wrong"}], "isError": true}),
                ),
                "slow" => {
                    tokio::time::sleep(Duration::from_millis(args["ms"].as_u64().unwrap_or(1500)))
                        .await;
                    ok(id, json!({"content": [{"type": "text", "text": "done"}]}))
                }
                "notify" => {
                    return Reply {
                        before: vec![json!({"jsonrpc": "2.0", "method": "notifications/message",
                            "params": {"level": "info", "logger": "mock", "data": "working on it"}})],
                        response: Some(ok(
                            id,
                            json!({"content": [{"type": "text", "text": "notified"}]}),
                        )),
                    };
                }
                "create_issue" | "delete_repo" | "search" => ok(
                    id,
                    json!({"content": [{"type": "text", "text": format!("ok: {args}")}]}),
                ),
                other => err(id, -32602, &format!("Unknown tool: {other}")),
            }
        }
        "resources/list" => ok(
            id,
            json!({"resources": [
                {"uri": "file:///readme.md", "name": "readme", "title": "README", "mimeType": "text/markdown"},
                {"uri": "mem://config", "name": "config", "mimeType": "application/json", "size": 42}
            ]}),
        ),
        "resources/templates/list" => ok(
            id,
            json!({"resourceTemplates": [
                {"uriTemplate": "repo://{owner}/{name}/issues", "name": "issues", "description": "Issues of a repository"}
            ]}),
        ),
        "resources/read" => ok(
            id,
            json!({"contents": [{"uri": p["uri"], "mimeType": "text/plain", "text": format!("contents of {}", p["uri"].as_str().unwrap_or(""))}]}),
        ),
        "prompts/list" => ok(
            id,
            json!({"prompts": [
                {"name": "review_code", "title": "Review code", "description": "Ask for a code review",
                 "arguments": [{"name": "code", "required": true, "description": "The code"}, {"name": "focus"}]}
            ]}),
        ),
        "prompts/get" => ok(
            id,
            json!({"messages": [{"role": "user", "content": {"type": "text",
            "text": format!("Please review: {}", p["arguments"]["code"].as_str().unwrap_or(""))}}]}),
        ),
        _ => err(id, -32601, &format!("Method not found: {method}")),
    };
    Reply {
        before: vec![],
        response: Some(response),
    }
}

// ------------------------------------------------------------------ HTTP

#[derive(Clone, Default)]
pub struct HttpState {
    sessions: Arc<Mutex<HashMap<String, ()>>>,
    required_token: Option<String>,
    /// Server→client requests awaiting the client's POSTed response, by id.
    waiting: Arc<Mutex<HashMap<String, tokio::sync::oneshot::Sender<Value>>>>,
}

impl HttpState {
    /// Require `Authorization: Bearer <token>` on every request.
    pub fn with_token(token: impl Into<String>) -> Self {
        Self {
            required_token: Some(token.into()),
            ..Default::default()
        }
    }
}

pub fn http_router(state: HttpState) -> Router {
    Router::new()
        .route("/mcp", post(http_post).delete(http_delete))
        .with_state(state)
}

/// Bind on 127.0.0.1:`port` (0 = random) and serve in the background.
pub async fn spawn_http(state: HttpState, port: u16) -> std::io::Result<String> {
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", port)).await?;
    let addr = listener.local_addr()?;
    tokio::spawn(async move {
        let _ = axum::serve(listener, http_router(state)).await;
    });
    Ok(format!("http://{addr}/mcp"))
}

async fn http_post(State(st): State<HttpState>, headers: HeaderMap, body: String) -> Response {
    if let Some(tok) = &st.required_token {
        let ok = headers
            .get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            == Some(&format!("Bearer {tok}"));
        if !ok {
            return (StatusCode::UNAUTHORIZED, "missing or invalid bearer token").into_response();
        }
    }
    let Ok(msg) = serde_json::from_str::<Value>(&body) else {
        return (StatusCode::BAD_REQUEST, "invalid JSON").into_response();
    };
    let is_init = msg["method"] == "initialize";
    let sid = headers
        .get("mcp-session-id")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    if !is_init {
        match &sid {
            Some(s) if st.sessions.lock().unwrap().contains_key(s) => {}
            Some(_) => return (StatusCode::NOT_FOUND, "unknown session").into_response(),
            None => return (StatusCode::BAD_REQUEST, "missing mcp-session-id").into_response(),
        }
    }
    // A client's response to one of our server→client requests (e.g. sampling).
    if msg.get("method").is_none()
        && let Some(id) = msg["id"].as_str()
    {
        if let Some(tx) = st.waiting.lock().unwrap().remove(id) {
            let _ = tx.send(msg.clone());
        }
        return StatusCode::ACCEPTED.into_response();
    }
    if msg["method"] == "tools/call" && msg["params"]["name"] == "ask_llm" {
        return ask_llm_over_sse(&st, &msg, sid);
    }
    let reply = handle(&msg).await;
    let Some(resp) = reply.response else {
        return StatusCode::ACCEPTED.into_response();
    };

    let mut builder = Response::builder();
    if is_init {
        let new_sid = format!(
            "sess-{}",
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
        );
        st.sessions.lock().unwrap().insert(new_sid.clone(), ());
        builder = builder.header("mcp-session-id", new_sid);
    }
    let wants_sse = headers
        .get(header::ACCEPT)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|a| a.contains("text/event-stream"));
    // Stream tool calls as SSE (to exercise that path); everything else is plain JSON.
    if wants_sse && msg["method"] == "tools/call" {
        let mut sse = String::new();
        for n in reply.before.iter().chain(std::iter::once(&resp)) {
            sse.push_str(&format!("event: message\ndata: {n}\n\n"));
        }
        return builder
            .header(header::CONTENT_TYPE, "text/event-stream")
            .body(Body::from(sse))
            .unwrap();
    }
    builder
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(resp.to_string()))
        .unwrap()
}

/// Stream a `sampling/createMessage` request to the client, wait for its
/// answer (POSTed separately), then stream the tool result.
fn ask_llm_over_sse(st: &HttpState, msg: &Value, sid: Option<String>) -> Response {
    let req_id = format!(
        "srv-{}",
        chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
    );
    let (tx, rx) = tokio::sync::oneshot::channel::<Value>();
    st.waiting.lock().unwrap().insert(req_id.clone(), tx);
    let prompt = msg["params"]["arguments"]["prompt"]
        .as_str()
        .unwrap_or("hello")
        .to_string();
    let sampling = serde_json::json!({"jsonrpc": "2.0", "id": req_id, "method": "sampling/createMessage", "params": {
        "messages": [{"role": "user", "content": {"type": "text", "text": prompt}}],
        "systemPrompt": "Answer briefly.", "maxTokens": 200
    }});
    let call_id = msg["id"].clone();
    let (out_tx, out_rx) = tokio::sync::mpsc::channel::<Result<String, std::io::Error>>(4);
    tokio::spawn(async move {
        let _ = out_tx
            .send(Ok(format!("event: message\ndata: {sampling}\n\n")))
            .await;
        let result = match tokio::time::timeout(Duration::from_secs(30), rx).await {
            Ok(Ok(v)) if v.get("result").is_some() => {
                serde_json::json!({"content": [{"type": "text", "text": format!("LLM said: {}", v["result"]["content"]["text"].as_str().unwrap_or(""))}]})
            }
            Ok(Ok(v)) => {
                serde_json::json!({"content": [{"type": "text", "text": format!("sampling failed: {}", v["error"]["message"])}], "isError": true})
            }
            _ => {
                serde_json::json!({"content": [{"type": "text", "text": "sampling timed out"}], "isError": true})
            }
        };
        let final_msg = serde_json::json!({"jsonrpc": "2.0", "id": call_id, "result": result});
        let _ = out_tx
            .send(Ok(format!("event: message\ndata: {final_msg}\n\n")))
            .await;
    });
    let stream =
        futures::stream::unfold(
            out_rx,
            |mut rx| async move { rx.recv().await.map(|x| (x, rx)) },
        );
    let mut b = Response::builder().header(header::CONTENT_TYPE, "text/event-stream");
    if let Some(s) = sid {
        b = b.header("mcp-session-id", s);
    }
    b.body(Body::from_stream(stream)).unwrap()
}

async fn http_delete(State(st): State<HttpState>, headers: HeaderMap) -> StatusCode {
    match headers.get("mcp-session-id").and_then(|v| v.to_str().ok()) {
        Some(s) if st.sessions.lock().unwrap().remove(s).is_some() => StatusCode::OK,
        _ => StatusCode::NOT_FOUND,
    }
}
