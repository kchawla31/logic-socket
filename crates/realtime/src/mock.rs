//! Demo/test servers: WebSocket echo (`/ws`), SSE (`/events`) and a minimal
//! Socket.IO v4 server (`/socket.io/`, namespace `/admin` needs `{"token":"s3cret"}`).

use axum::{
    Router,
    extract::ws::{Message as WsMsg, WebSocket, WebSocketUpgrade},
    http::HeaderMap,
    response::{IntoResponse, Response},
    routing::get,
};
use serde_json::json;

async fn ws_echo(ws: WebSocketUpgrade, headers: HeaderMap) -> Response {
    let token = headers.get("x-token").and_then(|v| v.to_str().ok()).unwrap_or("none").to_string();
    ws.protocols(["chat.v1"]).on_upgrade(move |mut socket: WebSocket| async move {
        let _ = socket.send(WsMsg::Text(format!("welcome {token}").into())).await;
        while let Some(Ok(m)) = socket.recv().await {
            match m {
                WsMsg::Text(t) if t.as_str() == "bye" => {
                    let _ = socket.send(WsMsg::Close(Some(axum::extract::ws::CloseFrame { code: 1000, reason: "see you".into() }))).await;
                    break;
                }
                WsMsg::Text(t) => {
                    let _ = socket.send(WsMsg::Text(format!("echo: {t}").into())).await;
                }
                WsMsg::Binary(b) => {
                    let _ = socket.send(WsMsg::Binary(b)).await;
                }
                _ => {}
            }
        }
    })
}

async fn sse() -> impl IntoResponse {
    let body = ": keep-alive\n\nevent: greeting\ndata: hello\n\nid: 2\ndata: {\"n\":1}\ndata: line2\n\n";
    ([("content-type", "text/event-stream")], body)
}

/// Minimal Socket.IO v4 server: echoes events, answers acks, `/admin` needs auth.
async fn socketio(ws: WebSocketUpgrade) -> Response {
    ws.on_upgrade(|mut socket: WebSocket| async move {
        let _ = socket.send(WsMsg::Text(r#"0{"sid":"eng1","upgrades":[],"pingInterval":25000,"pingTimeout":20000,"maxPayload":1000000}"#.into())).await;
        let _ = socket.send(WsMsg::Text("2".into())).await; // heartbeat before connect
        while let Some(Ok(WsMsg::Text(t))) = socket.recv().await {
            let t = t.to_string();
            if t == "3" {
                continue;
            }
            let Some(p) = t.strip_prefix('4').and_then(crate::parse_socketio_packet) else { continue };
            let ns = if p.namespace == "/" { String::new() } else { format!("{},", p.namespace) };
            match p.kind {
                0 if p.namespace == "/admin" && p.data.as_ref().and_then(|d| d["token"].as_str()) != Some("s3cret") => {
                    let _ = socket.send(WsMsg::Text(format!(r#"44{ns}{{"message":"not authorized"}}"#).into())).await;
                }
                0 => {
                    let _ = socket.send(WsMsg::Text(format!(r#"40{ns}{{"sid":"sock1"}}"#).into())).await;
                    let _ = socket.send(WsMsg::Text(format!(r#"42{ns}["hello",{{"motd":"hi"}}]"#).into())).await;
                }
                2 => {
                    let arr = p.data.unwrap();
                    let args: Vec<_> = arr.as_array().unwrap()[1..].to_vec();
                    match p.ack {
                        Some(id) => {
                            let _ = socket.send(WsMsg::Text(format!("43{ns}{id}{}", json!(["got", args])).into())).await;
                        }
                        None => {
                            let mut out = vec![json!("echo")];
                            out.extend(args);
                            let _ = socket.send(WsMsg::Text(format!("42{ns}{}", json!(out)).into())).await;
                        }
                    }
                }
                1 => break,
                _ => {}
            }
        }
    })
}

pub fn router() -> Router {
    Router::new().route("/ws", get(ws_echo)).route("/events", get(sse)).route("/socket.io/", get(socketio))
}

/// Serve on 127.0.0.1:`port` (0 = random); returns `http://host:port`.
pub async fn spawn(port: u16) -> std::io::Result<String> {
    let l = tokio::net::TcpListener::bind(("127.0.0.1", port)).await?;
    let addr = l.local_addr()?;
    tokio::spawn(async move {
        let _ = axum::serve(l, router()).await;
    });
    Ok(format!("http://{addr}"))
}

