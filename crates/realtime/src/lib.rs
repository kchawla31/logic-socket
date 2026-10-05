//! Realtime connections — WebSocket, Server-Sent Events and Socket.IO (v4 over
//! WebSocket) — sharing one session model and an event log for the UI/CLI.

pub mod mock;
mod socketio;
mod sse;
mod ws;

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use serde::Serialize;
use serde_json::Value;
use tokio::sync::{broadcast, mpsc};

pub use socketio::parse_packet as parse_socketio_packet;

#[derive(Debug, Clone, thiserror::Error, PartialEq)]
pub enum RtError {
    #[error("invalid URL '{0}': {1}")]
    Url(String, String),
    #[error("could not connect: {0}")]
    Connect(String),
    #[error("not connected")]
    Closed,
    #[error("{0}")]
    Protocol(String),
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Direction {
    Out,
    In,
    Info,
    Error,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RtEvent {
    pub seq: u64,
    pub direction: Direction,
    pub timestamp_ms: i64,
    /// `open` | `close` | `text` | `binary` | `ping` | `pong` | `event` (SSE/Socket.IO) | `ack` | `error`
    pub kind: String,
    /// SSE `event:` name or Socket.IO event name.
    pub name: Option<String>,
    /// Text payload (binary shown as hex preview).
    pub data: String,
    pub size: usize,
}

/// Append-only, broadcastable event log for one connection.
#[derive(Clone)]
pub struct EventLog {
    entries: Arc<Mutex<Vec<RtEvent>>>,
    seq: Arc<AtomicU64>,
    tx: broadcast::Sender<RtEvent>,
}

impl Default for EventLog {
    fn default() -> Self {
        let (tx, _) = broadcast::channel(2048);
        Self {
            entries: Arc::default(),
            seq: Arc::default(),
            tx,
        }
    }
}

impl EventLog {
    pub fn push(
        &self,
        direction: Direction,
        kind: &str,
        name: Option<String>,
        data: impl Into<String>,
        size: usize,
    ) {
        let e = RtEvent {
            seq: self.seq.fetch_add(1, Ordering::SeqCst) + 1,
            direction,
            timestamp_ms: chrono::Utc::now().timestamp_millis(),
            kind: kind.to_string(),
            name,
            data: data.into(),
            size,
        };
        self.entries.lock().unwrap().push(e.clone());
        let _ = self.tx.send(e);
    }
    pub fn info(&self, kind: &str, text: impl Into<String>) {
        let t = text.into();
        let n = t.len();
        self.push(Direction::Info, kind, None, t, n);
    }
    pub fn error(&self, text: impl Into<String>) {
        let t = text.into();
        let n = t.len();
        self.push(Direction::Error, "error", None, t, n);
    }
    pub fn entries(&self) -> Vec<RtEvent> {
        self.entries.lock().unwrap().clone()
    }
    pub fn subscribe(&self) -> broadcast::Receiver<RtEvent> {
        self.tx.subscribe()
    }
    pub fn clear(&self) {
        self.entries.lock().unwrap().clear();
    }
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq, Default, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Kind {
    #[default]
    Websocket,
    Sse,
    Socketio,
}

#[derive(Debug, Clone, Default)]
pub struct ConnectOptions {
    pub kind: Kind,
    pub url: String,
    pub headers: Vec<(String, String)>,
    /// WebSocket subprotocols (Sec-WebSocket-Protocol).
    pub subprotocols: Vec<String>,
    /// SSE: HTTP method + body (most servers use GET).
    pub method: Option<String>,
    pub body: Option<String>,
    /// Socket.IO namespace (default `/`) and `auth` payload.
    pub namespace: Option<String>,
    pub auth: Option<Value>,
    pub validate_certificates: bool,
}

#[derive(Debug)]
pub(crate) enum Outgoing {
    Text(String),
    Binary(Vec<u8>),
    /// Socket.IO emit: event, args, optional ack id.
    Emit {
        event: String,
        args: Vec<Value>,
        ack: Option<u64>,
    },
    Close,
}

/// A live connection. Dropping it does not close the socket; call [`Session::close`].
pub struct Session {
    pub kind: Kind,
    pub log: EventLog,
    tx: mpsc::UnboundedSender<Outgoing>,
    ack_seq: AtomicU64,
    connected: Arc<std::sync::atomic::AtomicBool>,
}

impl Session {
    pub fn is_connected(&self) -> bool {
        self.connected.load(Ordering::SeqCst)
    }

    pub fn send_text(&self, text: impl Into<String>) -> Result<(), RtError> {
        match self.kind {
            Kind::Sse => Err(RtError::Protocol(
                "Server-Sent Events are receive-only".into(),
            )),
            _ if !self.is_connected() => Err(RtError::Closed),
            Kind::Websocket => self
                .tx
                .send(Outgoing::Text(text.into()))
                .map_err(|_| RtError::Closed),
            Kind::Socketio => Err(RtError::Protocol(
                "use emit(event, args) for Socket.IO".into(),
            )),
        }
    }

    pub fn send_binary(&self, data: Vec<u8>) -> Result<(), RtError> {
        if self.kind != Kind::Websocket {
            return Err(RtError::Protocol(
                "binary frames are only supported on WebSocket".into(),
            ));
        }
        if !self.is_connected() {
            return Err(RtError::Closed);
        }
        self.tx
            .send(Outgoing::Binary(data))
            .map_err(|_| RtError::Closed)
    }

    /// Socket.IO: emit `event` with `args`; `want_ack` requests an acknowledgement.
    pub fn emit(
        &self,
        event: &str,
        args: Vec<Value>,
        want_ack: bool,
    ) -> Result<Option<u64>, RtError> {
        if self.kind != Kind::Socketio {
            return Err(RtError::Protocol(
                "emit is only available for Socket.IO".into(),
            ));
        }
        if !self.is_connected() {
            return Err(RtError::Closed);
        }
        let ack = want_ack.then(|| self.ack_seq.fetch_add(1, Ordering::SeqCst));
        self.tx
            .send(Outgoing::Emit {
                event: event.to_string(),
                args,
                ack,
            })
            .map_err(|_| RtError::Closed)?;
        Ok(ack)
    }

    pub fn close(&self) {
        let _ = self.tx.send(Outgoing::Close);
    }
}

/// Connect and start pumping events into the session log.
pub async fn connect(opts: ConnectOptions) -> Result<Session, RtError> {
    let log = EventLog::default();
    let (tx, rx) = mpsc::unbounded_channel();
    let connected = Arc::new(std::sync::atomic::AtomicBool::new(false));
    match opts.kind {
        Kind::Websocket => ws::start(&opts, log.clone(), rx, connected.clone()).await?,
        Kind::Sse => sse::start(&opts, log.clone(), rx, connected.clone()).await?,
        Kind::Socketio => socketio::start(&opts, log.clone(), rx, connected.clone()).await?,
    }
    Ok(Session {
        kind: opts.kind,
        log,
        tx,
        ack_seq: AtomicU64::new(0),
        connected,
    })
}

pub(crate) fn preview_binary(b: &[u8]) -> String {
    let hex: Vec<String> = b.iter().take(64).map(|x| format!("{x:02x}")).collect();
    format!("{}{}", hex.join(" "), if b.len() > 64 { " …" } else { "" })
}

#[cfg(test)]
mod tests;
