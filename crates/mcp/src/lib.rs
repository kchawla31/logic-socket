//! MCP client (JSON-RPC 2.0 over stdio or Streamable HTTP) with a full
//! protocol log. See docs/DECISIONS.md D3 for why this is hand-written.

pub mod log;
pub mod mock;
pub mod schema;
pub mod transport;
pub mod types;

use std::collections::HashMap;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use tokio::sync::{mpsc, oneshot};

pub use log::{Direction, FrameKind, LogEntry, ProtocolLog};
pub use transport::TransportConfig;
use transport::{Inbound, Transport};
pub use types::*;

#[derive(Debug, Clone, thiserror::Error, PartialEq)]
pub enum McpError {
    #[error("transport error: {0}")]
    Transport(String),
    #[error("server error {code}: {message}")]
    Rpc {
        code: i64,
        message: String,
        data: Option<Value>,
    },
    #[error("'{method}' timed out after {ms} ms")]
    Timeout { method: String, ms: u64 },
    #[error("connection closed: {0}")]
    Closed(String),
    #[error("unexpected response for '{method}': {detail}")]
    Decode { method: String, detail: String },
    #[error("server does not support {0}")]
    Unsupported(&'static str),
}

/// Answers server-initiated `sampling/createMessage` requests (e.g. with an LLM).
pub trait SamplingHandler: Send + Sync {
    /// `params` is the request's params; return the result object.
    fn create_message(
        &self,
        params: Value,
    ) -> futures::future::BoxFuture<'static, Result<Value, String>>;
}

#[derive(Clone)]
pub struct ConnectOptions {
    pub transport: TransportConfig,
    /// Roots returned to `roots/list`: (uri, optional name).
    pub root_uris: Vec<(String, Option<String>)>,
    pub request_timeout: Duration,
    pub client_name: String,
    /// When set, the client advertises `sampling` and answers server requests with it.
    pub sampling: Option<Arc<dyn SamplingHandler>>,
}

impl std::fmt::Debug for ConnectOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ConnectOptions")
            .field("transport", &self.transport)
            .field("root_uris", &self.root_uris)
            .field("request_timeout", &self.request_timeout)
            .field("sampling", &self.sampling.is_some())
            .finish()
    }
}

impl ConnectOptions {
    pub fn new(transport: TransportConfig) -> Self {
        Self {
            transport,
            root_uris: vec![],
            request_timeout: Duration::from_secs(60),
            client_name: "insomnia-rs".into(),
            sampling: None,
        }
    }
}

/// A notification received from the server (e.g. `notifications/message`).
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Notification {
    pub method: String,
    pub params: Value,
    pub timestamp_ms: i64,
}

type Pending =
    Arc<Mutex<HashMap<i64, (String, Instant, oneshot::Sender<Result<Value, McpError>>)>>>;

pub struct Client {
    transport: Arc<Transport>,
    next_id: AtomicI64,
    pending: Pending,
    log: ProtocolLog,
    notifications: Arc<Mutex<Vec<Notification>>>,
    timeout: Duration,
    init: InitializeResult,
}

impl Client {
    /// Start the transport, run `initialize` + `notifications/initialized`.
    pub async fn connect(opts: ConnectOptions) -> Result<Client, McpError> {
        Self::connect_with_log(opts, ProtocolLog::default()).await
    }

    pub async fn connect_with_log(
        opts: ConnectOptions,
        log: ProtocolLog,
    ) -> Result<Client, McpError> {
        let (tx, rx) = mpsc::unbounded_channel();
        match &opts.transport {
            TransportConfig::Stdio { command, args, .. } => log.push_text(
                Direction::Info,
                format!("starting stdio server: {command} {}", args.join(" ")),
            ),
            TransportConfig::Http { url, .. } => {
                log.push_text(Direction::Info, format!("connecting to {url}"))
            }
        }
        let transport = match Transport::start(&opts.transport, tx).await {
            Ok(t) => Arc::new(t),
            Err(e) => {
                log.push_text(Direction::Error, e.to_string());
                return Err(e);
            }
        };
        let pending: Pending = Arc::default();
        let notifications: Arc<Mutex<Vec<Notification>>> = Arc::default();
        let roots: Vec<Value> = opts
            .root_uris
            .iter()
            .map(|(uri, name)| match name {
                Some(n) => json!({"uri": uri, "name": n}),
                None => json!({"uri": uri}),
            })
            .collect();
        tokio::spawn(dispatch(
            rx,
            transport.clone(),
            pending.clone(),
            log.clone(),
            notifications.clone(),
            roots,
            opts.sampling.clone(),
        ));

        let mut client = Client {
            transport,
            next_id: AtomicI64::new(1),
            pending,
            log,
            notifications,
            timeout: opts.request_timeout,
            init: InitializeResult {
                protocol_version: String::new(),
                capabilities: Value::Null,
                server_info: Implementation {
                    name: String::new(),
                    version: String::new(),
                    title: None,
                    extra: Default::default(),
                },
                instructions: None,
            },
        };
        let mut capabilities = json!({ "roots": { "listChanged": true } });
        if opts.sampling.is_some() {
            capabilities["sampling"] = json!({});
        }
        let params = json!({
            "protocolVersion": LATEST_PROTOCOL_VERSION,
            "capabilities": capabilities,
            "clientInfo": { "name": opts.client_name, "version": env!("CARGO_PKG_VERSION") },
        });
        let init: InitializeResult = match client.request_typed("initialize", params).await {
            Ok(i) => i,
            Err(e) => {
                client.transport.close().await;
                return Err(e);
            }
        };
        client
            .transport
            .set_protocol_version(&init.protocol_version);
        client.init = init;
        client.notify("notifications/initialized", None).await?;
        Ok(client)
    }

    pub fn server(&self) -> &InitializeResult {
        &self.init
    }

    pub fn log(&self) -> &ProtocolLog {
        &self.log
    }

    pub fn session_id(&self) -> Option<String> {
        self.transport.session_id()
    }

    pub fn notifications(&self) -> Vec<Notification> {
        self.notifications.lock().unwrap().clone()
    }

    fn has_capability(&self, name: &str) -> bool {
        self.init.capabilities.get(name).is_some()
    }

    /// Send a request and wait for its result.
    pub async fn request(&self, method: &str, params: Value) -> Result<Value, McpError> {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let mut msg = json!({ "jsonrpc": "2.0", "id": id, "method": method });
        if !params.is_null() {
            msg["params"] = params;
        }
        let (tx, rx) = oneshot::channel();
        self.pending
            .lock()
            .unwrap()
            .insert(id, (method.to_string(), Instant::now(), tx));
        self.log.push_frame(Direction::Out, &msg, None, None);
        if let Err(e) = self.transport.send(&msg).await {
            self.pending.lock().unwrap().remove(&id);
            self.log.push_text(Direction::Error, e.to_string());
            return Err(e);
        }
        match tokio::time::timeout(self.timeout, rx).await {
            Ok(Ok(r)) => r,
            Ok(Err(_)) => Err(McpError::Closed("dispatcher stopped".into())),
            Err(_) => {
                self.pending.lock().unwrap().remove(&id);
                let ms = self.timeout.as_millis() as u64;
                self.log.push_text(
                    Direction::Error,
                    format!("{method} (id {id}) timed out after {ms} ms"),
                );
                // Tell the server we gave up (spec: notifications/cancelled).
                let _ = self
                    .notify(
                        "notifications/cancelled",
                        Some(json!({"requestId": id, "reason": "timeout"})),
                    )
                    .await;
                Err(McpError::Timeout {
                    method: method.to_string(),
                    ms,
                })
            }
        }
    }

    pub async fn request_typed<T: DeserializeOwned>(
        &self,
        method: &str,
        params: Value,
    ) -> Result<T, McpError> {
        let v = self.request(method, params).await?;
        serde_json::from_value(v).map_err(|e| McpError::Decode {
            method: method.into(),
            detail: e.to_string(),
        })
    }

    pub async fn notify(&self, method: &str, params: Option<Value>) -> Result<(), McpError> {
        let mut msg = json!({ "jsonrpc": "2.0", "method": method });
        if let Some(p) = params {
            msg["params"] = p;
        }
        self.log.push_frame(Direction::Out, &msg, None, None);
        self.transport.send(&msg).await
    }

    pub async fn ping(&self) -> Result<f64, McpError> {
        let t = Instant::now();
        self.request("ping", Value::Null).await?;
        Ok(t.elapsed().as_secs_f64() * 1000.0)
    }

    /// Follow `nextCursor` until exhausted (capped at 100 pages).
    async fn list_all<T: DeserializeOwned>(
        &self,
        method: &str,
        field: &str,
    ) -> Result<Listed<T>, McpError> {
        let start = Instant::now();
        let mut items = vec![];
        let mut cursor: Option<String> = None;
        let mut pages = 0;
        loop {
            let params = match &cursor {
                Some(c) => json!({ "cursor": c }),
                None => json!({}),
            };
            let page = self.request(method, params).await?;
            pages += 1;
            let arr = page.get(field).cloned().unwrap_or(Value::Array(vec![]));
            let mut batch: Vec<T> = serde_json::from_value(arr).map_err(|e| McpError::Decode {
                method: method.into(),
                detail: format!("{field}: {e}"),
            })?;
            items.append(&mut batch);
            cursor = page
                .get("nextCursor")
                .and_then(Value::as_str)
                .filter(|c| !c.is_empty())
                .map(str::to_string);
            if cursor.is_none() || pages >= 100 {
                break;
            }
        }
        Ok(Listed {
            items,
            pages,
            elapsed_ms: start.elapsed().as_secs_f64() * 1000.0,
        })
    }

    pub async fn list_tools(&self) -> Result<Listed<Tool>, McpError> {
        if !self.has_capability("tools") {
            return Err(McpError::Unsupported("tools"));
        }
        self.list_all("tools/list", "tools").await
    }

    pub async fn list_resources(&self) -> Result<Listed<Resource>, McpError> {
        if !self.has_capability("resources") {
            return Err(McpError::Unsupported("resources"));
        }
        self.list_all("resources/list", "resources").await
    }

    pub async fn list_resource_templates(&self) -> Result<Listed<ResourceTemplate>, McpError> {
        if !self.has_capability("resources") {
            return Err(McpError::Unsupported("resources"));
        }
        self.list_all("resources/templates/list", "resourceTemplates")
            .await
    }

    pub async fn list_prompts(&self) -> Result<Listed<Prompt>, McpError> {
        if !self.has_capability("prompts") {
            return Err(McpError::Unsupported("prompts"));
        }
        self.list_all("prompts/list", "prompts").await
    }

    pub async fn call_tool(
        &self,
        name: &str,
        arguments: Value,
    ) -> Result<CallToolResult, McpError> {
        let args = if arguments.is_null() {
            json!({})
        } else {
            arguments
        };
        self.request_typed("tools/call", json!({ "name": name, "arguments": args }))
            .await
    }

    pub async fn read_resource(&self, uri: &str) -> Result<Value, McpError> {
        self.request("resources/read", json!({ "uri": uri })).await
    }

    pub async fn get_prompt(&self, name: &str, arguments: Value) -> Result<Value, McpError> {
        let args = if arguments.is_null() {
            json!({})
        } else {
            arguments
        };
        self.request("prompts/get", json!({ "name": name, "arguments": args }))
            .await
    }

    pub async fn close(&self) {
        self.log.push_text(Direction::Info, "closing connection");
        self.transport.close().await;
        for (_, (_, _, tx)) in self.pending.lock().unwrap().drain() {
            let _ = tx.send(Err(McpError::Closed("client closed".into())));
        }
    }
}

async fn dispatch(
    mut rx: mpsc::UnboundedReceiver<Inbound>,
    transport: Arc<Transport>,
    pending: Pending,
    log: ProtocolLog,
    notifications: Arc<Mutex<Vec<Notification>>>,
    roots: Vec<Value>,
    sampling: Option<Arc<dyn SamplingHandler>>,
) {
    while let Some(ev) = rx.recv().await {
        match ev {
            Inbound::Stderr(line) => log.push_text(Direction::Stderr, line),
            Inbound::Info(text) => log.push_text(Direction::Info, text),
            Inbound::Closed(reason) => {
                log.push_text(Direction::Info, reason.clone());
                for (_, (_, _, tx)) in pending.lock().unwrap().drain() {
                    let _ = tx.send(Err(McpError::Closed(reason.clone())));
                }
            }
            Inbound::Message(msg) => match log::classify(&msg) {
                FrameKind::Response | FrameKind::Error => {
                    let id = msg["id"]
                        .as_i64()
                        .or_else(|| msg["id"].as_str().and_then(|s| s.parse().ok()));
                    let entry = id.and_then(|id| pending.lock().unwrap().remove(&id));
                    match entry {
                        Some((method, started, tx)) => {
                            let latency = started.elapsed().as_secs_f64() * 1000.0;
                            log.push_frame(
                                Direction::In,
                                &msg,
                                Some(method),
                                Some((latency * 100.0).round() / 100.0),
                            );
                            let result = match msg.get("error") {
                                Some(err) => Err(McpError::Rpc {
                                    code: err["code"].as_i64().unwrap_or(0),
                                    message: err["message"].as_str().unwrap_or("").to_string(),
                                    data: err.get("data").cloned(),
                                }),
                                None => Ok(msg.get("result").cloned().unwrap_or(Value::Null)),
                            };
                            let _ = tx.send(result);
                        }
                        None => log.push_frame(Direction::In, &msg, None, None),
                    }
                }
                FrameKind::Notification => {
                    log.push_frame(Direction::In, &msg, None, None);
                    notifications.lock().unwrap().push(Notification {
                        method: msg["method"].as_str().unwrap_or("").to_string(),
                        params: msg.get("params").cloned().unwrap_or(Value::Null),
                        timestamp_ms: chrono::Utc::now().timestamp_millis(),
                    });
                }
                FrameKind::Request => {
                    log.push_frame(Direction::In, &msg, None, None);
                    let method = msg["method"].as_str().unwrap_or("");
                    if method == "sampling/createMessage"
                        && let Some(handler) = sampling.clone()
                    {
                        // May take seconds (LLM call): answer from a task so the dispatcher keeps running.
                        let (transport, log, id) =
                            (transport.clone(), log.clone(), msg["id"].clone());
                        let fut = handler
                            .create_message(msg.get("params").cloned().unwrap_or(Value::Null));
                        tokio::spawn(async move {
                            let started = Instant::now();
                            let reply = match fut.await {
                                Ok(result) => json!({"jsonrpc": "2.0", "id": id, "result": result}),
                                Err(e) => {
                                    json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32603, "message": e}})
                                }
                            };
                            let ms = (started.elapsed().as_secs_f64() * 100_000.0).round() / 100.0;
                            log.push_frame(
                                Direction::Out,
                                &reply,
                                Some("sampling/createMessage".into()),
                                Some(ms),
                            );
                            if let Err(e) = transport.send(&reply).await {
                                log.push_text(Direction::Error, e.to_string());
                            }
                        });
                        continue;
                    }
                    let reply = match method {
                        "ping" => json!({"jsonrpc": "2.0", "id": msg["id"], "result": {}}),
                        "roots/list" => {
                            json!({"jsonrpc": "2.0", "id": msg["id"], "result": {"roots": roots}})
                        }
                        _ => json!({"jsonrpc": "2.0", "id": msg["id"], "error": {
                            "code": -32601,
                            "message": if method == "sampling/createMessage" {
                                "Sampling is disabled for this server (enable it and pick an AI provider in the server settings)".to_string()
                            } else {
                                format!("insomnia-rs does not support '{method}' yet")
                            },
                        }}),
                    };
                    log.push_frame(Direction::Out, &reply, Some(method.to_string()), None);
                    if let Err(e) = transport.send(&reply).await {
                        log.push_text(Direction::Error, e.to_string());
                    }
                }
                FrameKind::Other => log.push_frame(Direction::In, &msg, None, None),
            },
        }
    }
}
