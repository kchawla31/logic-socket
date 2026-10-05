//! Protocol log: every JSON-RPC frame in either direction, with latency for
//! responses. Shared by the CLI (`--log`) and the desktop Protocol Log panel.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use serde::Serialize;
use serde_json::Value;
use tokio::sync::broadcast;

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Direction {
    /// Client → server.
    Out,
    /// Server → client.
    In,
    /// Transport-level note (HTTP status, session id, process exit, ...).
    Info,
    /// Server stderr (stdio transport).
    Stderr,
    Error,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum FrameKind {
    Request,
    Response,
    Notification,
    Error,
    Other,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LogEntry {
    pub seq: u64,
    pub direction: Direction,
    pub kind: FrameKind,
    pub timestamp_ms: i64,
    /// Method name; for responses, the method of the matching request.
    pub method: Option<String>,
    pub id: Option<Value>,
    /// Round-trip time, set on responses.
    pub latency_ms: Option<f64>,
    /// Full JSON frame, or `{"text": "..."}` for Info/Stderr/Error lines.
    pub message: Value,
}

pub fn classify(msg: &Value) -> FrameKind {
    let has_method = msg.get("method").is_some();
    let has_id = msg.get("id").is_some_and(|i| !i.is_null());
    match (has_method, has_id) {
        (true, true) => FrameKind::Request,
        (true, false) => FrameKind::Notification,
        (false, true) if msg.get("error").is_some() => FrameKind::Error,
        (false, true) => FrameKind::Response,
        _ => FrameKind::Other,
    }
}

#[derive(Clone)]
pub struct ProtocolLog {
    entries: Arc<Mutex<Vec<LogEntry>>>,
    seq: Arc<AtomicU64>,
    tx: broadcast::Sender<LogEntry>,
}

impl Default for ProtocolLog {
    fn default() -> Self {
        let (tx, _) = broadcast::channel(1024);
        Self { entries: Arc::default(), seq: Arc::default(), tx }
    }
}

impl ProtocolLog {
    pub fn push_frame(&self, direction: Direction, msg: &Value, method: Option<String>, latency_ms: Option<f64>) {
        let kind = classify(msg);
        let method = method.or_else(|| msg.get("method").and_then(Value::as_str).map(str::to_string));
        self.push(LogEntry {
            seq: 0,
            direction,
            kind,
            timestamp_ms: chrono::Utc::now().timestamp_millis(),
            method,
            id: msg.get("id").cloned().filter(|v| !v.is_null()),
            latency_ms,
            message: msg.clone(),
        });
    }

    pub fn push_text(&self, direction: Direction, text: impl Into<String>) {
        self.push(LogEntry {
            seq: 0,
            direction,
            kind: FrameKind::Other,
            timestamp_ms: chrono::Utc::now().timestamp_millis(),
            method: None,
            id: None,
            latency_ms: None,
            message: serde_json::json!({ "text": text.into() }),
        });
    }

    fn push(&self, mut e: LogEntry) {
        let mut entries = self.entries.lock().unwrap();
        e.seq = self.seq.fetch_add(1, Ordering::SeqCst) + 1;
        entries.push(e.clone());
        drop(entries);
        let _ = self.tx.send(e);
    }

    pub fn entries(&self) -> Vec<LogEntry> {
        self.entries.lock().unwrap().clone()
    }

    pub fn since(&self, seq: u64) -> Vec<LogEntry> {
        self.entries.lock().unwrap().iter().filter(|e| e.seq > seq).cloned().collect()
    }

    pub fn subscribe(&self) -> broadcast::Receiver<LogEntry> {
        self.tx.subscribe()
    }

    pub fn clear(&self) {
        self.entries.lock().unwrap().clear();
    }
}
