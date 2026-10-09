//! Scripted mock LLM speaking both the Anthropic Messages and OpenAI Chat
//! Completions streaming formats. Used by tests and for offline demos
//! (`lsock llm mock-server`).
//!
//! Behavior, based on the last message:
//! - it contains a tool result → answer "Based on the tool: <result>"
//! - the user mentions "weather" and a `*get_weather` tool exists → call it with `{"city": "<capitalised word after 'in'>"}`
//! - the user mentions "delete" and a `*delete_repo` tool exists → call it
//! - otherwise → stream "Echo: <user text>" in small chunks
//!
//! Requires the API key `test-key` (header `x-api-key` or `Authorization: Bearer`).

use std::sync::{Arc, Mutex};

use axum::{
    Json, Router,
    body::Body,
    extract::State,
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde_json::{Value, json};

pub const MOCK_KEY: &str = "test-key";

#[derive(Clone, Default)]
pub struct MockLlm {
    /// Every request body received, in order.
    pub requests: Arc<Mutex<Vec<Value>>>,
}

enum Plan {
    Text(String),
    Tool { name: String, input: Value },
}

fn last_user_text(messages: &[Value]) -> String {
    for m in messages.iter().rev() {
        if m["role"] == "user" {
            match &m["content"] {
                Value::String(s) => return s.clone(),
                Value::Array(a) => {
                    let t: Vec<&str> = a
                        .iter()
                        .filter(|b| b["type"] == "text")
                        .filter_map(|b| b["text"].as_str())
                        .collect();
                    if !t.is_empty() {
                        return t.join(" ");
                    }
                }
                _ => {}
            }
        }
    }
    String::new()
}

fn tool_result_text(messages: &[Value]) -> Option<String> {
    let last = messages.last()?;
    if last["role"] == "tool" {
        return last["content"].as_str().map(str::to_string);
    }
    last["content"]
        .as_array()?
        .iter()
        .find(|b| b["type"] == "tool_result")
        .map(|b| match &b["content"] {
            Value::String(s) => s.clone(),
            v => v.to_string(),
        })
}

fn plan(messages: &[Value], tool_names: &[String]) -> Plan {
    if let Some(r) = tool_result_text(messages) {
        return Plan::Text(format!("Based on the tool: {r}"));
    }
    let text = last_user_text(messages);
    let lower = text.to_lowercase();
    // Tool classification (see lsock_mcp::action::classify_prompt): a stand-in that
    // reads each description with simple word rules and answers in JSON.
    if lower.starts_with("classify each mcp tool") {
        let mut out = serde_json::Map::new();
        for line in text.lines().filter_map(|l| l.strip_prefix("- ")) {
            let Some((name, desc)) = line.split_once(": ") else {
                continue;
            };
            let d = desc.to_lowercase();
            let has = |ws: &[&str]| ws.iter().any(|w| d.contains(w));
            let action = if has(&["delete", "remove", "cancel"]) {
                "delete"
            } else if has(&["update", "change", "edit", "rename"]) {
                "update"
            } else if has(&["submit", "place", "buy", "sell", "send", "create", "order"]) {
                "create"
            } else {
                "read"
            };
            out.insert(name.to_string(), json!(action));
        }
        return Plan::Text(Value::Object(out).to_string());
    }
    if lower.contains("weather")
        && let Some(t) = tool_names.iter().find(|n| n.ends_with("get_weather"))
    {
        let city = text
            .split_whitespace()
            .skip_while(|w| !w.eq_ignore_ascii_case("in"))
            .nth(1)
            .map(|w| w.trim_matches(|c: char| !c.is_alphanumeric()).to_string())
            .unwrap_or_else(|| "Berlin".into());
        return Plan::Tool {
            name: t.clone(),
            input: json!({ "city": city }),
        };
    }
    if lower.contains("delete")
        && let Some(t) = tool_names.iter().find(|n| n.ends_with("delete_repo"))
    {
        return Plan::Tool {
            name: t.clone(),
            input: json!({ "repo": "acme/old", "confirm": true }),
        };
    }
    Plan::Text(format!("Echo: {text}"))
}

fn authorized(h: &HeaderMap) -> bool {
    h.get("x-api-key").and_then(|v| v.to_str().ok()) == Some(MOCK_KEY)
        || h.get(header::AUTHORIZATION).and_then(|v| v.to_str().ok())
            == Some(&format!("Bearer {MOCK_KEY}"))
}

fn chunks(s: &str) -> Vec<String> {
    let chars: Vec<char> = s.chars().collect();
    chars.chunks(7).map(|c| c.iter().collect()).collect()
}

/// Split near the middle on a char boundary.
fn halves(s: &str) -> (&str, &str) {
    let mut mid = s.len() / 2;
    while !s.is_char_boundary(mid) {
        mid += 1;
    }
    s.split_at(mid)
}

fn sse(events: Vec<(Option<&str>, Value)>, done: bool) -> Response {
    let mut body = String::new();
    for (name, data) in events {
        if let Some(n) = name {
            body.push_str(&format!("event: {n}\n"));
        }
        body.push_str(&format!("data: {data}\n\n"));
    }
    if done {
        body.push_str("data: [DONE]\n\n");
    }
    Response::builder()
        .header(header::CONTENT_TYPE, "text/event-stream")
        .body(Body::from(body))
        .unwrap()
}

async fn anthropic(
    State(st): State<MockLlm>,
    headers: HeaderMap,
    Json(req): Json<Value>,
) -> Response {
    st.requests.lock().unwrap().push(req.clone());
    if !authorized(&headers) {
        return (StatusCode::UNAUTHORIZED, Json(json!({"type": "error", "error": {"type": "authentication_error", "message": "invalid x-api-key"}}))).into_response();
    }
    let messages = req["messages"].as_array().cloned().unwrap_or_default();
    let tools: Vec<String> = req["tools"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|t| t["name"].as_str().map(str::to_string))
        .collect();
    let model = req["model"].as_str().unwrap_or("mock").to_string();
    let mut ev: Vec<(Option<&str>, Value)> = vec![(
        Some("message_start"),
        json!({"type": "message_start", "message": {"id": "msg_mock", "type": "message", "role": "assistant", "model": model, "content": [], "stop_reason": null, "usage": {"input_tokens": 12, "output_tokens": 1}}}),
    )];
    ev.push((Some("ping"), json!({"type": "ping"})));
    // a short thinking block first, to exercise signature round-tripping
    ev.push((Some("content_block_start"), json!({"type": "content_block_start", "index": 0, "content_block": {"type": "thinking", "thinking": ""}})));
    ev.push((Some("content_block_delta"), json!({"type": "content_block_delta", "index": 0, "delta": {"type": "thinking_delta", "thinking": "Let me think."}})));
    ev.push((Some("content_block_delta"), json!({"type": "content_block_delta", "index": 0, "delta": {"type": "signature_delta", "signature": "sig-abc"}})));
    ev.push((
        Some("content_block_stop"),
        json!({"type": "content_block_stop", "index": 0}),
    ));
    let stop = match plan(&messages, &tools) {
        Plan::Text(t) => {
            ev.push((Some("content_block_start"), json!({"type": "content_block_start", "index": 1, "content_block": {"type": "text", "text": ""}})));
            for c in chunks(&t) {
                ev.push((Some("content_block_delta"), json!({"type": "content_block_delta", "index": 1, "delta": {"type": "text_delta", "text": c}})));
            }
            ev.push((
                Some("content_block_stop"),
                json!({"type": "content_block_stop", "index": 1}),
            ));
            "end_turn"
        }
        Plan::Tool { name, input } => {
            ev.push((Some("content_block_start"), json!({"type": "content_block_start", "index": 1, "content_block": {"type": "text", "text": ""}})));
            ev.push((Some("content_block_delta"), json!({"type": "content_block_delta", "index": 1, "delta": {"type": "text_delta", "text": "Let me check."}})));
            ev.push((
                Some("content_block_stop"),
                json!({"type": "content_block_stop", "index": 1}),
            ));
            ev.push((Some("content_block_start"), json!({"type": "content_block_start", "index": 2, "content_block": {"type": "tool_use", "id": "toolu_mock1", "name": name, "input": {}}})));
            let s = input.to_string();
            let (a, b) = halves(&s);
            for part in [a, b] {
                ev.push((Some("content_block_delta"), json!({"type": "content_block_delta", "index": 2, "delta": {"type": "input_json_delta", "partial_json": part}})));
            }
            ev.push((
                Some("content_block_stop"),
                json!({"type": "content_block_stop", "index": 2}),
            ));
            "tool_use"
        }
    };
    ev.push((Some("message_delta"), json!({"type": "message_delta", "delta": {"stop_reason": stop, "stop_sequence": null}, "usage": {"output_tokens": 21}})));
    ev.push((Some("message_stop"), json!({"type": "message_stop"})));
    sse(ev, false)
}

async fn openai(State(st): State<MockLlm>, headers: HeaderMap, Json(req): Json<Value>) -> Response {
    st.requests.lock().unwrap().push(req.clone());
    if !authorized(&headers) {
        return (StatusCode::UNAUTHORIZED, Json(json!({"error": {"message": "Incorrect API key provided", "type": "invalid_request_error"}}))).into_response();
    }
    let messages = req["messages"].as_array().cloned().unwrap_or_default();
    let tools: Vec<String> = req["tools"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|t| t["function"]["name"].as_str().map(str::to_string))
        .collect();
    let model = req["model"].as_str().unwrap_or("mock").to_string();
    let chunk = |delta: Value, finish: Value| json!({"id": "chatcmpl-mock", "object": "chat.completion.chunk", "model": model, "choices": [{"index": 0, "delta": delta, "finish_reason": finish}]});
    let mut ev: Vec<(Option<&str>, Value)> = vec![(
        None,
        chunk(json!({"role": "assistant", "content": ""}), Value::Null),
    )];
    let finish = match plan(&messages, &tools) {
        Plan::Text(t) => {
            for c in chunks(&t) {
                ev.push((None, chunk(json!({"content": c}), Value::Null)));
            }
            "stop"
        }
        Plan::Tool { name, input } => {
            ev.push((None, chunk(json!({"tool_calls": [{"index": 0, "id": "call_mock1", "type": "function", "function": {"name": name, "arguments": ""}}]}), Value::Null)));
            let s = input.to_string();
            let (a, b) = halves(&s);
            for part in [a, b] {
                ev.push((
                    None,
                    chunk(
                        json!({"tool_calls": [{"index": 0, "function": {"arguments": part}}]}),
                        Value::Null,
                    ),
                ));
            }
            "tool_calls"
        }
    };
    ev.push((None, chunk(json!({}), json!(finish))));
    ev.push((None, json!({"id": "chatcmpl-mock", "object": "chat.completion.chunk", "model": model, "choices": [], "usage": {"prompt_tokens": 12, "completion_tokens": 21}})));
    sse(ev, true)
}

async fn models() -> Json<Value> {
    Json(json!({"data": [{"id": "mock-large"}, {"id": "mock-small"}]}))
}

pub fn router(state: MockLlm) -> Router {
    Router::new()
        .route("/v1/messages", post(anthropic))
        .route("/v1/models", get(models))
        .route("/v1/chat/completions", post(openai))
        .with_state(state)
}

/// Serve on 127.0.0.1:`port` (0 = random). Returns the base URL (without `/v1`).
pub async fn spawn(state: MockLlm, port: u16) -> std::io::Result<String> {
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", port)).await?;
    let addr = listener.local_addr()?;
    tokio::spawn(async move {
        let _ = axum::serve(listener, router(state)).await;
    });
    Ok(format!("http://{addr}"))
}
