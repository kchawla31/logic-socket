//! Realtime commands (WebSocket / SSE / Socket.IO). One live session per request.

use std::sync::Arc;

use irs_core::{Doc, RealtimeRequest};
use irs_realtime::{Kind, RtEvent, Session};
use serde::Serialize;
use serde_json::{Value, json};
use tauri::{Emitter, State};

use crate::{AppState, CmdResult, e};

#[tauri::command]
pub fn rt_create(
    state: State<'_, AppState>,
    parent_id: String,
    kind: String,
) -> CmdResult<Doc<RealtimeRequest>> {
    let (name, url) = match kind.as_str() {
        "sse" => ("New Event Stream", "https://sse.dev/test"),
        "socketio" => ("New Socket.IO", "http://localhost:3000"),
        _ => ("New WebSocket", "wss://echo.websocket.org"),
    };
    let r = RealtimeRequest {
        name: name.into(),
        kind,
        url: url.into(),
        ..Default::default()
    };
    state.engine.store.insert(Some(&parent_id), r).map_err(e)
}

#[tauri::command]
pub fn rt_update(
    state: State<'_, AppState>,
    doc: Doc<RealtimeRequest>,
) -> CmdResult<Doc<RealtimeRequest>> {
    state.engine.store.update(&doc).map_err(e)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RtStatus {
    connected: bool,
    kind: Option<Kind>,
}

#[tauri::command]
pub async fn rt_connect<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    state: State<'_, AppState>,
    id: String,
) -> CmdResult<RtStatus> {
    if let Some(old) = state.rt.lock().unwrap().remove(&id) {
        old.close();
    }
    let opts = state.engine.realtime_options(&id).map_err(e)?;
    let kind = opts.kind;
    let session = irs_realtime::connect(opts).await.map_err(e)?;
    let mut rx = session.log.subscribe();
    // replay what happened during the handshake, then stream
    for ev in session.log.entries() {
        let _ = app.emit("rt-event", json!({ "id": id, "event": ev }));
    }
    let id2 = id.clone();
    let app2 = app.clone();
    tauri::async_runtime::spawn(async move {
        while let Ok(ev) = rx.recv().await {
            let _ = app2.emit("rt-event", json!({ "id": id2, "event": ev }));
        }
    });
    state.rt.lock().unwrap().insert(id, Arc::new(session));
    Ok(RtStatus {
        connected: true,
        kind: Some(kind),
    })
}

#[tauri::command]
pub fn rt_status(state: State<'_, AppState>, id: String) -> RtStatus {
    match state.rt.lock().unwrap().get(&id) {
        Some(s) => RtStatus {
            connected: s.is_connected(),
            kind: Some(s.kind),
        },
        None => RtStatus {
            connected: false,
            kind: None,
        },
    }
}

fn session(state: &AppState, id: &str) -> CmdResult<Arc<Session>> {
    state
        .rt
        .lock()
        .unwrap()
        .get(id)
        .cloned()
        .filter(|s| s.is_connected())
        .ok_or_else(|| "Not connected — press Connect first".to_string())
}

/// Send the composer text (variables rendered). For Socket.IO, a JSON array
/// payload becomes the event arguments; anything else is a single argument.
#[tauri::command]
pub fn rt_send(
    state: State<'_, AppState>,
    id: String,
    text: String,
    event: Option<String>,
    ack: bool,
) -> CmdResult<Option<u64>> {
    let s = session(&state, &id)?;
    let rendered = state.engine.realtime_payload(&id, &text).map_err(e)?;
    match s.kind {
        Kind::Socketio => {
            let args = match serde_json::from_str::<Value>(&rendered) {
                Ok(Value::Array(a)) => a,
                Ok(v) => vec![v],
                Err(_) if rendered.trim().is_empty() => vec![],
                Err(_) => vec![Value::String(rendered)],
            };
            s.emit(
                event
                    .as_deref()
                    .filter(|e| !e.is_empty())
                    .unwrap_or("message"),
                args,
                ack,
            )
            .map_err(e)
        }
        _ => s.send_text(rendered).map(|_| None).map_err(e),
    }
}

#[tauri::command]
pub fn rt_disconnect(state: State<'_, AppState>, id: String) {
    if let Some(s) = state.rt.lock().unwrap().remove(&id) {
        s.close();
    }
}

#[tauri::command]
pub fn rt_log(state: State<'_, AppState>, id: String) -> Vec<RtEvent> {
    state
        .rt
        .lock()
        .unwrap()
        .get(&id)
        .map(|s| s.log.entries())
        .unwrap_or_default()
}
