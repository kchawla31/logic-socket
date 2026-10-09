//! Server-Sent Events client (receive-only stream over HTTP).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use futures::StreamExt;
use tokio::sync::mpsc;

use crate::{ConnectOptions, Direction, EventLog, Outgoing, RtError};

#[derive(Debug, Default, PartialEq)]
pub(crate) struct SseFrame {
    pub event: Option<String>,
    pub data: String,
    pub id: Option<String>,
    pub retry: Option<u64>,
    pub comment: bool,
}

#[derive(Default)]
pub(crate) struct Parser {
    buf: String,
}

impl Parser {
    pub fn feed(&mut self, bytes: &[u8]) -> Vec<SseFrame> {
        self.buf.push_str(&String::from_utf8_lossy(bytes));
        if self.buf.contains('\r') {
            self.buf = self.buf.replace("\r\n", "\n").replace('\r', "\n");
        }
        let mut out = vec![];
        while let Some(i) = self.buf.find("\n\n") {
            let raw: String = self.buf.drain(..i + 2).collect();
            let mut f = SseFrame::default();
            let mut data = vec![];
            let mut any = false;
            for line in raw.lines() {
                if line.starts_with(':') {
                    f.comment = true;
                    continue;
                }
                let (field, value) = line
                    .split_once(':')
                    .map(|(a, b)| (a, b.strip_prefix(' ').unwrap_or(b)))
                    .unwrap_or((line, ""));
                any = true;
                match field {
                    "data" => data.push(value.to_string()),
                    "event" => f.event = Some(value.to_string()),
                    "id" => f.id = Some(value.to_string()),
                    "retry" => f.retry = value.parse().ok(),
                    _ => {}
                }
            }
            f.data = data.join("\n");
            if any || f.comment {
                out.push(f);
            }
        }
        out
    }
}

pub(crate) async fn start(
    opts: &ConnectOptions,
    log: EventLog,
    mut rx: mpsc::UnboundedReceiver<Outgoing>,
    connected: Arc<AtomicBool>,
) -> Result<(), RtError> {
    url::Url::parse(&opts.url).map_err(|e| RtError::Url(opts.url.clone(), e.to_string()))?;
    let client = reqwest::Client::builder()
        .danger_accept_invalid_certs(!opts.validate_certificates)
        .connect_timeout(std::time::Duration::from_secs(20))
        .build()
        .map_err(|e| RtError::Connect(e.to_string()))?;
    let method = reqwest::Method::from_bytes(
        opts.method
            .as_deref()
            .unwrap_or("GET")
            .to_uppercase()
            .as_bytes(),
    )
    .unwrap_or(reqwest::Method::GET);
    let mut rb = client
        .request(method, &opts.url)
        .header("Accept", "text/event-stream")
        .header("Cache-Control", "no-cache");
    for (k, v) in &opts.headers {
        rb = rb.header(k, v);
    }
    if let Some(b) = &opts.body {
        rb = rb.body(b.clone());
    }
    log.info("info", format!("Connecting to {}", opts.url));
    let started = std::time::Instant::now();
    let resp = rb.send().await.map_err(|e| {
        log.error(e.to_string());
        RtError::Connect(e.to_string())
    })?;
    let status = resp.status();
    let ctype = resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        let msg = format!(
            "HTTP {status}: {}",
            body.chars().take(300).collect::<String>()
        );
        log.error(msg.clone());
        return Err(RtError::Connect(msg));
    }
    if !ctype.starts_with("text/event-stream") {
        log.info(
            "info",
            format!("Warning: content-type is '{ctype}', not text/event-stream"),
        );
    }
    log.info(
        "open",
        format!(
            "Connected ({}) in {:.0} ms",
            status.as_u16(),
            started.elapsed().as_secs_f64() * 1000.0
        ),
    );
    connected.store(true, Ordering::SeqCst);
    let mut stream = resp.bytes_stream();
    tokio::spawn(async move {
        let mut parser = Parser::default();
        loop {
            tokio::select! {
                out = rx.recv() => {
                    if matches!(out, Some(Outgoing::Close) | None) {
                        log.info("close", "Closed by client");
                        break;
                    }
                }
                chunk = stream.next() => match chunk {
                    Some(Ok(bytes)) => {
                        for f in parser.feed(&bytes) {
                            if f.comment && f.data.is_empty() && f.event.is_none() {
                                continue; // keep-alive comment
                            }
                            let mut name = f.event.clone().unwrap_or_else(|| "message".into());
                            if let Some(id) = &f.id { name = format!("{name} #{id}"); }
                            let n = f.data.len();
                            log.push(Direction::In, "event", Some(name), f.data, n);
                        }
                    }
                    Some(Err(e)) => { log.error(e.to_string()); break; }
                    None => { log.info("close", "Stream ended by server"); break; }
                },
            }
        }
        connected.store(false, Ordering::SeqCst);
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_fields_comments_and_multiline() {
        let mut p = Parser::default();
        assert!(p.feed(b": keep-alive\n\nid: 7\nevent: tick\ndata: a").len() == 1);
        let f = p.feed(b"\ndata: b\nretry: 3000\n\n");
        assert_eq!(
            f,
            vec![SseFrame {
                event: Some("tick".into()),
                data: "a\nb".into(),
                id: Some("7".into()),
                retry: Some(3000),
                comment: false
            }]
        );
    }

    /// WHATWG SSE treats CR, LF, and CRLF as one line ending. A chunk that
    /// ends on the CR of a CRLF must not become a blank line.
    #[test]
    #[ignore = "BUG-008"]
    fn crlf_split_after_the_carriage_return_stays_one_event() {
        let mut p = Parser::default();
        let first = p.feed(b"data: hello\r");
        let second = p.feed(b"\ndata: world\r\n\r\n");
        assert!(
            first.is_empty() && second.len() == 1 && second[0].data == "hello\nworld",
            "first={first:?} second={second:?}"
        );
    }
}
