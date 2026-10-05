//! stdio and Streamable HTTP transports. Both push every inbound frame into
//! one channel consumed by the client's dispatcher.

use std::collections::HashMap;
use std::process::Stdio as ProcStdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::StreamExt;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::mpsc::UnboundedSender;

use crate::McpError;

pub const SESSION_HEADER: &str = "mcp-session-id";
pub const PROTOCOL_HEADER: &str = "mcp-protocol-version";

#[derive(Debug)]
pub enum Inbound {
    Message(Value),
    Stderr(String),
    Info(String),
    Closed(String),
}

#[derive(Debug, Clone)]
pub enum TransportConfig {
    Stdio {
        command: String,
        args: Vec<String>,
        env: HashMap<String, String>,
        cwd: Option<String>,
    },
    Http {
        url: String,
        headers: Vec<(String, String)>,
        validate_certificates: bool,
    },
}

pub enum Transport {
    Stdio(StdioTransport),
    Http(HttpTransport),
}

impl Transport {
    pub async fn start(
        cfg: &TransportConfig,
        inbound: UnboundedSender<Inbound>,
    ) -> Result<Self, McpError> {
        match cfg {
            TransportConfig::Stdio {
                command,
                args,
                env,
                cwd,
            } => Ok(Transport::Stdio(StdioTransport::spawn(
                command,
                args,
                env,
                cwd.as_deref(),
                inbound,
            )?)),
            TransportConfig::Http {
                url,
                headers,
                validate_certificates,
            } => Ok(Transport::Http(HttpTransport::new(
                url,
                headers,
                *validate_certificates,
                inbound,
            )?)),
        }
    }

    pub async fn send(&self, msg: &Value) -> Result<(), McpError> {
        match self {
            Transport::Stdio(t) => t.send(msg).await,
            Transport::Http(t) => t.send(msg).await,
        }
    }

    pub fn set_protocol_version(&self, v: &str) {
        if let Transport::Http(t) = self {
            *t.protocol_version.lock().unwrap() = Some(v.to_string());
        }
    }

    pub fn session_id(&self) -> Option<String> {
        match self {
            Transport::Http(t) => t.session_id.lock().unwrap().clone(),
            Transport::Stdio(_) => None,
        }
    }

    pub async fn close(&self) {
        match self {
            Transport::Stdio(t) => t.close().await,
            Transport::Http(t) => t.close().await,
        }
    }
}

// ------------------------------------------------------------------ stdio

pub struct StdioTransport {
    stdin: tokio::sync::Mutex<Option<ChildStdin>>,
    child: tokio::sync::Mutex<Option<Child>>,
}

impl StdioTransport {
    fn spawn(
        command: &str,
        args: &[String],
        env: &HashMap<String, String>,
        cwd: Option<&str>,
        inbound: UnboundedSender<Inbound>,
    ) -> Result<Self, McpError> {
        let mut cmd = Command::new(command);
        cmd.args(args)
            .envs(env)
            .stdin(ProcStdio::piped())
            .stdout(ProcStdio::piped())
            .stderr(ProcStdio::piped())
            .kill_on_drop(true);
        if let Some(dir) = cwd.filter(|d| !d.is_empty()) {
            cmd.current_dir(dir);
        }
        let mut child = cmd
            .spawn()
            .map_err(|e| McpError::Transport(format!("failed to start '{command}': {e}")))?;
        let stdout = child.stdout.take().expect("piped stdout");
        let stderr = child.stderr.take().expect("piped stderr");
        let stdin = child.stdin.take();

        let tx = inbound.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(stdout).lines();
            loop {
                match lines.next_line().await {
                    Ok(Some(line)) if line.trim().is_empty() => {}
                    Ok(Some(line)) => match serde_json::from_str::<Value>(&line) {
                        Ok(v) => {
                            let _ = tx.send(Inbound::Message(v));
                        }
                        Err(_) => {
                            let _ = tx.send(Inbound::Stderr(format!("(stdout, not JSON) {line}")));
                        }
                    },
                    Ok(None) => {
                        let _ = tx.send(Inbound::Closed("server process closed stdout".into()));
                        break;
                    }
                    Err(e) => {
                        let _ = tx.send(Inbound::Closed(format!("stdout read error: {e}")));
                        break;
                    }
                }
            }
        });
        let tx = inbound;
        tokio::spawn(async move {
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                let _ = tx.send(Inbound::Stderr(line));
            }
        });
        Ok(Self {
            stdin: tokio::sync::Mutex::new(stdin),
            child: tokio::sync::Mutex::new(Some(child)),
        })
    }

    async fn send(&self, msg: &Value) -> Result<(), McpError> {
        let mut guard = self.stdin.lock().await;
        let stdin = guard
            .as_mut()
            .ok_or_else(|| McpError::Transport("stdin closed".into()))?;
        let mut line = serde_json::to_vec(msg).map_err(|e| McpError::Transport(e.to_string()))?;
        line.push(b'\n');
        stdin
            .write_all(&line)
            .await
            .map_err(|e| McpError::Transport(format!("write to server failed: {e}")))?;
        stdin
            .flush()
            .await
            .map_err(|e| McpError::Transport(e.to_string()))
    }

    async fn close(&self) {
        // Closing stdin lets well-behaved servers exit; then make sure.
        self.stdin.lock().await.take();
        if let Some(mut child) = self.child.lock().await.take()
            && tokio::time::timeout(Duration::from_millis(500), child.wait())
                .await
                .is_err()
        {
            let _ = child.kill().await;
        }
    }
}

// ------------------------------------------------------------------ Streamable HTTP

pub struct HttpTransport {
    url: String,
    headers: Vec<(String, String)>,
    client: reqwest::Client,
    session_id: Arc<Mutex<Option<String>>>,
    protocol_version: Arc<Mutex<Option<String>>>,
    inbound: UnboundedSender<Inbound>,
}

impl HttpTransport {
    fn new(
        url: &str,
        headers: &[(String, String)],
        validate_certificates: bool,
        inbound: UnboundedSender<Inbound>,
    ) -> Result<Self, McpError> {
        reqwest::Url::parse(url)
            .map_err(|e| McpError::Transport(format!("invalid URL '{url}': {e}")))?;
        let client = reqwest::Client::builder()
            .danger_accept_invalid_certs(!validate_certificates)
            .connect_timeout(Duration::from_secs(15))
            .build()
            .map_err(|e| McpError::Transport(e.to_string()))?;
        Ok(Self {
            url: url.to_string(),
            headers: headers.to_vec(),
            client,
            session_id: Arc::default(),
            protocol_version: Arc::default(),
            inbound,
        })
    }

    fn request(&self, method: reqwest::Method) -> reqwest::RequestBuilder {
        let mut rb = self.client.request(method, &self.url);
        for (k, v) in &self.headers {
            rb = rb.header(k, v);
        }
        if let Some(s) = self.session_id.lock().unwrap().as_deref() {
            rb = rb.header(SESSION_HEADER, s);
        }
        if let Some(v) = self.protocol_version.lock().unwrap().as_deref() {
            rb = rb.header(PROTOCOL_HEADER, v);
        }
        rb
    }

    async fn send(&self, msg: &Value) -> Result<(), McpError> {
        let id = msg
            .get("id")
            .cloned()
            .filter(|_| msg.get("method").is_some());
        let resp = self
            .request(reqwest::Method::POST)
            .header("accept", "application/json, text/event-stream")
            .header("content-type", "application/json")
            .body(msg.to_string())
            .send()
            .await
            .map_err(|e| McpError::Transport(format!("POST {} failed: {e}", self.url)))?;

        if let Some(sid) = resp
            .headers()
            .get(SESSION_HEADER)
            .and_then(|v| v.to_str().ok())
        {
            let mut cur = self.session_id.lock().unwrap();
            if cur.as_deref() != Some(sid) {
                let _ = self
                    .inbound
                    .send(Inbound::Info(format!("session id: {sid}")));
                *cur = Some(sid.to_string());
            }
        }
        let status = resp.status();
        let ctype = resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_ascii_lowercase();

        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            let hint = match status.as_u16() {
                401 | 403 => " (server requires authentication — add an Authorization header)",
                404 if self.session_id.lock().unwrap().is_some() => {
                    " (session expired — reconnect)"
                }
                _ => "",
            };
            let text = format!(
                "HTTP {}{hint}: {}",
                status,
                body.chars().take(500).collect::<String>()
            );
            return match id {
                // Surface the failure as the JSON-RPC error of the pending request.
                Some(id) => {
                    let _ = self.inbound.send(Inbound::Info(text.clone()));
                    let _ = self.inbound.send(Inbound::Message(
                        json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32000, "message": text}}),
                    ));
                    Ok(())
                }
                None => Err(McpError::Transport(text)),
            };
        }
        if status == reqwest::StatusCode::ACCEPTED || id.is_none() {
            return Ok(());
        }

        if ctype.starts_with("text/event-stream") {
            let tx = self.inbound.clone();
            let mut stream = resp.bytes_stream();
            tokio::spawn(async move {
                let mut parser = SseParser::default();
                while let Some(chunk) = stream.next().await {
                    match chunk {
                        Ok(bytes) => {
                            for data in parser.feed(&bytes) {
                                forward_json(&tx, &data);
                            }
                        }
                        Err(e) => {
                            let _ = tx.send(Inbound::Info(format!("SSE stream error: {e}")));
                            break;
                        }
                    }
                }
                for data in parser.finish() {
                    forward_json(&tx, &data);
                }
            });
        } else {
            let body = resp
                .text()
                .await
                .map_err(|e| McpError::Transport(e.to_string()))?;
            forward_json(&self.inbound, &body);
        }
        Ok(())
    }

    async fn close(&self) {
        if self.session_id.lock().unwrap().is_some() {
            let _ = tokio::time::timeout(
                Duration::from_secs(2),
                self.request(reqwest::Method::DELETE).send(),
            )
            .await;
        }
    }
}

fn forward_json(tx: &UnboundedSender<Inbound>, data: &str) {
    match serde_json::from_str::<Value>(data) {
        Ok(Value::Array(batch)) => {
            for m in batch {
                let _ = tx.send(Inbound::Message(m));
            }
        }
        Ok(v) => {
            let _ = tx.send(Inbound::Message(v));
        }
        Err(e) => {
            let _ = tx.send(Inbound::Info(format!(
                "unparseable server payload ({e}): {}",
                &data[..data.len().min(200)]
            )));
        }
    }
}

/// Incremental Server-Sent Events parser returning the `data` of each event.
#[derive(Default)]
pub struct SseParser {
    buf: String,
}

impl SseParser {
    pub fn feed(&mut self, bytes: &[u8]) -> Vec<String> {
        self.buf.push_str(&String::from_utf8_lossy(bytes));
        self.buf = self.buf.replace("\r\n", "\n");
        let mut out = vec![];
        while let Some(i) = self.buf.find("\n\n") {
            let event: String = self.buf.drain(..i + 2).collect();
            if let Some(d) = event_data(&event) {
                out.push(d);
            }
        }
        out
    }

    pub fn finish(&mut self) -> Vec<String> {
        let rest = std::mem::take(&mut self.buf);
        event_data(&rest).into_iter().collect()
    }
}

fn event_data(event: &str) -> Option<String> {
    let mut data = vec![];
    let mut kind = "message";
    for line in event.lines() {
        if let Some(d) = line.strip_prefix("data:") {
            data.push(d.strip_prefix(' ').unwrap_or(d));
        } else if let Some(e) = line.strip_prefix("event:") {
            kind = e.trim();
        }
    }
    (kind == "message" && !data.is_empty()).then(|| data.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::SseParser;

    #[test]
    fn sse_parser_handles_split_chunks_multiline_and_crlf() {
        let mut p = SseParser::default();
        assert!(p.feed(b"event: message\r\ndata: {\"a\":").is_empty());
        assert_eq!(
            p.feed(b"1}\r\n\r\n: comment\n\ndata: x\ndata: y\n\nevent: ping\ndata: z\n\n"),
            vec!["{\"a\":1}", "x\ny"]
        );
        assert!(p.feed(b"data: tail").is_empty());
        assert_eq!(p.finish(), vec!["tail"]);
    }
}
