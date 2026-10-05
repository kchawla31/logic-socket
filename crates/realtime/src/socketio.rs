//! Socket.IO v4 client over the engine.io v4 WebSocket transport.
//!
//! engine.io packets: `0` open, `2` ping, `3` pong, `4` message, `1` close.
//! Socket.IO packets (inside `4`): `0` connect, `1` disconnect, `2` event,
//! `3` ack, `4` connect_error; optional `/namespace,` and ack id before the JSON.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use futures::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::protocol::Message;

use crate::{ConnectOptions, Direction, EventLog, Outgoing, RtError};

/// A decoded Socket.IO packet.
#[derive(Debug, Clone, PartialEq)]
pub struct Packet {
    pub kind: u8,
    pub namespace: String,
    pub ack: Option<u64>,
    pub data: Option<Value>,
}

/// Parse the Socket.IO part of an engine.io message (without the leading `4`).
pub fn parse_packet(s: &str) -> Option<Packet> {
    let mut chars = s.char_indices().peekable();
    let (_, k) = chars.next()?;
    let kind = k.to_digit(10)? as u8;
    let mut rest = &s[k.len_utf8()..];
    if kind == 5 || kind == 6 {
        // binary event/ack: "<attachments>-" prefix
        if let Some(i) = rest.find('-') {
            rest = &rest[i + 1..];
        }
    }
    let mut namespace = "/".to_string();
    if rest.starts_with('/') {
        let end = rest.find(',').unwrap_or(rest.len());
        namespace = rest[..end].to_string();
        rest = rest.get(end + 1..).unwrap_or("");
    }
    let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    let ack = (!digits.is_empty()).then(|| digits.parse().ok()).flatten();
    rest = &rest[digits.len()..];
    let data = if rest.is_empty() {
        None
    } else {
        serde_json::from_str(rest).ok()
    };
    Some(Packet {
        kind,
        namespace,
        ack,
        data,
    })
}

fn encode(kind: u8, namespace: &str, ack: Option<u64>, data: Option<&Value>) -> String {
    let mut s = format!("4{kind}");
    if namespace != "/" {
        s.push_str(namespace);
        s.push(',');
    }
    if let Some(a) = ack {
        s.push_str(&a.to_string());
    }
    if let Some(d) = data {
        s.push_str(&d.to_string());
    }
    s
}

fn engine_url(raw: &str) -> Result<String, RtError> {
    let mut u = url::Url::parse(raw).map_err(|e| RtError::Url(raw.to_string(), e.to_string()))?;
    let scheme = match u.scheme() {
        "http" | "ws" => "ws",
        "https" | "wss" => "wss",
        other => {
            return Err(RtError::Url(
                raw.to_string(),
                format!("unsupported scheme {other}"),
            ));
        }
    };
    u.set_scheme(scheme)
        .map_err(|_| RtError::Url(raw.to_string(), "bad scheme".into()))?;
    if u.path() == "/" || u.path().is_empty() {
        u.set_path("/socket.io/");
    }
    u.query_pairs_mut()
        .append_pair("EIO", "4")
        .append_pair("transport", "websocket");
    Ok(u.to_string())
}

pub(crate) async fn start(
    opts: &ConnectOptions,
    log: EventLog,
    mut rx: mpsc::UnboundedReceiver<Outgoing>,
    connected: Arc<AtomicBool>,
) -> Result<(), RtError> {
    let url = engine_url(&opts.url)?;
    let namespace = opts
        .namespace
        .clone()
        .filter(|n| !n.is_empty())
        .map(|n| {
            if n.starts_with('/') {
                n
            } else {
                format!("/{n}")
            }
        })
        .unwrap_or("/".into());
    let mut stream = crate::ws::open(&url, opts, &log).await?;

    // engine.io open packet
    let open = tokio::time::timeout(std::time::Duration::from_secs(10), stream.next())
        .await
        .map_err(|_| {
            RtError::Protocol(
                "no engine.io handshake within 10 s (is this a Socket.IO v4 server?)".into(),
            )
        })?;
    let handshake = match open {
        Some(Ok(Message::Text(t))) if t.starts_with('0') => {
            serde_json::from_str::<Value>(&t[1..]).unwrap_or(Value::Null)
        }
        other => {
            return Err(RtError::Protocol(format!(
                "unexpected first frame: {other:?}"
            )));
        }
    };
    log.info(
        "info",
        format!(
            "engine.io session {} (ping every {} ms)",
            handshake["sid"].as_str().unwrap_or("?"),
            handshake["pingInterval"]
        ),
    );

    // Socket.IO connect to the namespace
    let connect = encode(0, &namespace, None, opts.auth.as_ref());
    stream
        .send(Message::Text(connect.clone().into()))
        .await
        .map_err(|e| RtError::Connect(e.to_string()))?;
    let ack = tokio::time::timeout(std::time::Duration::from_secs(10), async {
        while let Some(m) = stream.next().await {
            match m {
                Ok(Message::Text(t)) if t.starts_with('4') => return parse_packet(&t[1..]),
                Ok(Message::Text(t)) if t == "2" => {
                    let _ = stream.send(Message::Text("3".into())).await;
                }
                Ok(_) => {}
                Err(_) => return None,
            }
        }
        None
    })
    .await
    .map_err(|_| RtError::Protocol("no CONNECT answer from the namespace".into()))?;
    match ack {
        Some(Packet { kind: 0, data, .. }) => log.info(
            "open",
            format!(
                "Joined namespace {namespace} (socket id {})",
                data.as_ref().and_then(|d| d["sid"].as_str()).unwrap_or("?")
            ),
        ),
        Some(Packet { kind: 4, data, .. }) => {
            let msg = data
                .as_ref()
                .and_then(|d| d["message"].as_str().map(str::to_string))
                .unwrap_or_else(|| format!("{data:?}"));
            log.error(format!("Namespace refused the connection: {msg}"));
            return Err(RtError::Connect(format!("connect_error: {msg}")));
        }
        other => {
            return Err(RtError::Protocol(format!(
                "unexpected answer to CONNECT: {other:?}"
            )));
        }
    }
    connected.store(true, Ordering::SeqCst);

    let (mut sink, mut source) = stream.split();
    tokio::spawn(async move {
        loop {
            tokio::select! {
                out = rx.recv() => match out {
                    Some(Outgoing::Emit { event, args, ack }) => {
                        let mut arr = vec![json!(event)];
                        arr.extend(args);
                        let payload = Value::Array(arr);
                        let frame = encode(2, &namespace, ack, Some(&payload));
                        if sink.send(Message::Text(frame.into())).await.is_err() { break; }
                        let data = payload.as_array().map(|a| Value::Array(a[1..].to_vec()).to_string()).unwrap_or_default();
                        let label = match ack { Some(a) => format!("{event} (ack #{a})"), None => event };
                        let n = data.len();
                        log.push(Direction::Out, "event", Some(label), data, n);
                    }
                    Some(Outgoing::Close) | None => {
                        let _ = sink.send(Message::Text(encode(1, &namespace, None, None).into())).await;
                        let _ = sink.send(Message::Close(None)).await;
                        log.info("close", "Disconnected by client");
                        break;
                    }
                    Some(_) => {}
                },
                msg = source.next() => match msg {
                    Some(Ok(Message::Text(t))) => {
                        let t = t.to_string();
                        if t == "2" {
                            let _ = sink.send(Message::Text("3".into())).await; // engine.io heartbeat
                            continue;
                        }
                        if t == "1" { log.info("close", "Server closed the engine.io session"); break; }
                        let Some(p) = t.strip_prefix('4').and_then(parse_packet) else { continue };
                        if p.namespace != namespace { continue; }
                        match p.kind {
                            2 | 5 => {
                                let arr = p.data.and_then(|d| d.as_array().cloned()).unwrap_or_default();
                                let name = arr.first().and_then(|v| v.as_str()).unwrap_or("?").to_string();
                                let data = Value::Array(arr.into_iter().skip(1).collect()).to_string();
                                let label = match p.ack { Some(a) => format!("{name} (wants ack #{a})"), None => name };
                                let n = data.len();
                                log.push(Direction::In, "event", Some(label), data, n);
                            }
                            3 | 6 => {
                                let data = p.data.map(|d| d.to_string()).unwrap_or_default();
                                let n = data.len();
                                log.push(Direction::In, "ack", Some(format!("ack #{}", p.ack.unwrap_or(0))), data, n);
                            }
                            1 => { log.info("close", "Server disconnected this socket"); break; }
                            4 => log.error(format!("connect_error: {}", p.data.map(|d| d.to_string()).unwrap_or_default())),
                            _ => {}
                        }
                    }
                    Some(Ok(Message::Close(_))) | None => { log.info("close", "Connection closed"); break; }
                    Some(Ok(_)) => {}
                    Some(Err(e)) => { log.error(e.to_string()); break; }
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
    fn packets_round_trip() {
        assert_eq!(
            parse_packet(r#"2["chat",{"a":1}]"#).unwrap(),
            Packet {
                kind: 2,
                namespace: "/".into(),
                ack: None,
                data: Some(json!(["chat", {"a": 1}]))
            }
        );
        assert_eq!(
            parse_packet(r#"2/admin,12["x"]"#).unwrap(),
            Packet {
                kind: 2,
                namespace: "/admin".into(),
                ack: Some(12),
                data: Some(json!(["x"]))
            }
        );
        assert_eq!(parse_packet("0/admin,").unwrap().namespace, "/admin");
        let bin = parse_packet(r#"51-["x",{"_placeholder":true,"num":0}]"#).unwrap();
        assert_eq!((bin.kind, bin.data.unwrap()[0].clone()), (5, json!("x")));
        assert_eq!(
            encode(2, "/chat", Some(3), Some(&json!(["hi"]))),
            r#"42/chat,3["hi"]"#
        );
        assert_eq!(
            engine_url("https://x.io").unwrap(),
            "wss://x.io/socket.io/?EIO=4&transport=websocket"
        );
    }
}
