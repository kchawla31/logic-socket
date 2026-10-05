//! WebSocket client (tokio-tungstenite).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use futures::{SinkExt, StreamExt};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::HeaderValue;
use tokio_tungstenite::tungstenite::protocol::Message;

use crate::{ConnectOptions, Direction, EventLog, Outgoing, RtError, preview_binary};

pub(crate) type WsStream = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

/// Open the socket, log the handshake, and return the stream.
pub(crate) async fn open(url: &str, opts: &ConnectOptions, log: &EventLog) -> Result<WsStream, RtError> {
    let mut req = url.into_client_request().map_err(|e| RtError::Url(url.to_string(), e.to_string()))?;
    for (k, v) in &opts.headers {
        let name = tokio_tungstenite::tungstenite::http::HeaderName::from_bytes(k.as_bytes()).map_err(|e| RtError::Protocol(format!("header {k}: {e}")))?;
        let value = HeaderValue::from_str(v).map_err(|e| RtError::Protocol(format!("header {k}: {e}")))?;
        req.headers_mut().insert(name, value);
    }
    if !opts.subprotocols.is_empty() {
        req.headers_mut().insert("Sec-WebSocket-Protocol", HeaderValue::from_str(&opts.subprotocols.join(", ")).map_err(|e| RtError::Protocol(e.to_string()))?);
    }
    log.info("info", format!("Connecting to {url}"));
    let started = std::time::Instant::now();
    let (stream, resp) = tokio::time::timeout(std::time::Duration::from_secs(20), tokio_tungstenite::connect_async(req))
        .await
        .map_err(|_| RtError::Connect("timed out after 20 s".into()))?
        .map_err(|e| {
            let msg = match &e {
                tokio_tungstenite::tungstenite::Error::Http(r) => format!("server answered HTTP {} instead of upgrading", r.status()),
                other => other.to_string(),
            };
            log.error(msg.clone());
            RtError::Connect(msg)
        })?;
    let proto = resp.headers().get("sec-websocket-protocol").and_then(|v| v.to_str().ok()).map(|p| format!(" · subprotocol {p}")).unwrap_or_default();
    log.info("open", format!("Connected ({} {}) in {:.0} ms{proto}", resp.status().as_u16(), resp.status().canonical_reason().unwrap_or(""), started.elapsed().as_secs_f64() * 1000.0));
    Ok(stream)
}

pub(crate) async fn start(
    opts: &ConnectOptions,
    log: EventLog,
    mut rx: mpsc::UnboundedReceiver<Outgoing>,
    connected: Arc<AtomicBool>,
) -> Result<(), RtError> {
    let stream = open(&opts.url, opts, &log).await?;
    connected.store(true, Ordering::SeqCst);
    let (mut sink, mut source) = stream.split();
    tokio::spawn(async move {
        loop {
            tokio::select! {
                out = rx.recv() => match out {
                    Some(Outgoing::Text(t)) => {
                        let n = t.len();
                        if sink.send(Message::Text(t.clone().into())).await.is_err() { break; }
                        log.push(Direction::Out, "text", None, t, n);
                    }
                    Some(Outgoing::Binary(b)) => {
                        let n = b.len();
                        let p = preview_binary(&b);
                        if sink.send(Message::Binary(b.into())).await.is_err() { break; }
                        log.push(Direction::Out, "binary", None, p, n);
                    }
                    Some(Outgoing::Close) | None => {
                        let _ = sink.send(Message::Close(None)).await;
                        log.info("close", "Closed by client");
                        break;
                    }
                    Some(Outgoing::Emit { .. }) => {}
                },
                msg = source.next() => match msg {
                    Some(Ok(Message::Text(t))) => { let n = t.len(); log.push(Direction::In, "text", None, t.to_string(), n); }
                    Some(Ok(Message::Binary(b))) => log.push(Direction::In, "binary", None, preview_binary(&b), b.len()),
                    Some(Ok(Message::Ping(p))) => log.push(Direction::In, "ping", None, String::from_utf8_lossy(&p).into_owned(), p.len()),
                    Some(Ok(Message::Pong(p))) => log.push(Direction::In, "pong", None, String::from_utf8_lossy(&p).into_owned(), p.len()),
                    Some(Ok(Message::Close(frame))) => {
                        let why = frame.map(|f| format!("{} {}", u16::from(f.code), f.reason)).unwrap_or_else(|| "no reason".into());
                        log.info("close", format!("Closed by server ({why})"));
                        break;
                    }
                    Some(Ok(Message::Frame(_))) => {}
                    Some(Err(e)) => { log.error(e.to_string()); break; }
                    None => { log.info("close", "Connection ended"); break; }
                },
            }
        }
        connected.store(false, Ordering::SeqCst);
    });
    Ok(())
}
