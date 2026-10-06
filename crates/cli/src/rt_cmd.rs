//! `lsock rt` — WebSocket / SSE / Socket.IO from the terminal.

use anyhow::{Result, anyhow};
use clap::Args;
use lsock_core::RealtimeRequest;
use lsock_engine::Engine;
use lsock_realtime::{ConnectOptions, Direction, Kind, RtEvent};

use crate::out::*;

#[derive(Args, Debug)]
pub struct RtArgs {
    /// Saved realtime request (name or id) or a URL
    pub target: String,
    /// websocket | sse | socketio (for URLs; default: from the scheme, ws/wss → websocket)
    #[arg(short, long)]
    pub kind: Option<String>,
    /// Header "Name: value" (repeatable, URLs only)
    #[arg(short = 'H', long = "header")]
    pub headers: Vec<String>,
    /// Text message to send after connecting (repeatable)
    #[arg(short, long)]
    pub send: Vec<String>,
    /// Socket.IO event to emit (with --args)
    #[arg(long)]
    pub emit: Option<String>,
    /// Socket.IO arguments as a JSON array
    #[arg(long, default_value = "[]")]
    pub args: String,
    /// Seconds to stay connected (0 = until Ctrl-C)
    #[arg(short, long, default_value_t = 5)]
    pub wait: u64,
    /// Socket.IO namespace
    #[arg(long)]
    pub namespace: Option<String>,
}

fn print(e: &RtEvent) {
    let arrow = match e.direction {
        Direction::Out => blue("↑"),
        Direction::In => green("↓"),
        Direction::Info => dim("·"),
        Direction::Error => red("✗"),
    };
    let name = e
        .name
        .as_ref()
        .map(|n| format!("{} ", magenta(n)))
        .unwrap_or_default();
    let data = if e.direction == Direction::Info {
        dim(&e.data)
    } else {
        e.data.clone()
    };
    println!("{arrow} {name}{data}");
}

pub async fn run(engine: &Engine, a: RtArgs) -> Result<()> {
    let (opts, saved_id) = match crate::find::<RealtimeRequest>(engine, &a.target) {
        Ok(r) => (engine.realtime_options(r.id())?, Some(r.meta.id.clone())),
        Err(_) if a.target.contains("://") => {
            let kind = match a.kind.as_deref() {
                Some("sse") => Kind::Sse,
                Some("socketio") => Kind::Socketio,
                Some("websocket") | None => Kind::Websocket,
                Some(k) => return Err(anyhow!("unknown kind '{k}' (websocket, sse, socketio)")),
            };
            let headers = a
                .headers
                .iter()
                .map(|h| {
                    h.split_once(':')
                        .map(|(k, v)| (k.trim().to_string(), v.trim().to_string()))
                        .ok_or_else(|| anyhow!("header must be 'Name: value'"))
                })
                .collect::<Result<_>>()?;
            (
                ConnectOptions {
                    kind,
                    url: a.target.clone(),
                    headers,
                    namespace: a.namespace.clone(),
                    validate_certificates: true,
                    ..Default::default()
                },
                None,
            )
        }
        Err(e) => return Err(e),
    };
    let session = match lsock_realtime::connect(opts).await {
        Ok(s) => s,
        Err(e) => return Err(anyhow!("{e}")),
    };
    let mut rx = session.log.subscribe();
    for e in session.log.entries() {
        print(&e);
    }
    let printer = tokio::spawn(async move {
        while let Ok(e) = rx.recv().await {
            print(&e);
        }
    });
    for m in &a.send {
        let text = match &saved_id {
            Some(id) => engine.realtime_payload(id, m)?,
            None => m.clone(),
        };
        session.send_text(text).map_err(|e| anyhow!("{e}"))?;
    }
    if let Some(ev) = &a.emit {
        let args: serde_json::Value = serde_json::from_str(&a.args)
            .map_err(|e| anyhow!("--args must be a JSON array: {e}"))?;
        session
            .emit(
                ev,
                args.as_array().cloned().unwrap_or_else(|| vec![args]),
                false,
            )
            .map_err(|e| anyhow!("{e}"))?;
    }
    if a.wait == 0 {
        tokio::signal::ctrl_c().await?;
    } else {
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(a.wait);
        while tokio::time::Instant::now() < deadline && session.is_connected() {
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
    }
    session.close();
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    printer.abort();
    Ok(())
}
