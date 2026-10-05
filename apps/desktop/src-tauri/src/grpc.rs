//! gRPC commands: proto files, method discovery, unary and streaming calls.

use std::sync::{Arc, Mutex};

use irs_core::{Doc, GrpcRequest, ProtoFile};
use irs_grpc::{ServiceInfo, StreamCall, UnaryResult};
use irs_realtime::RtEvent;
use serde_json::json;
use tauri::{Emitter, State};

use crate::{AppState, CmdResult, e};

#[tauri::command]
pub fn proto_file_list(state: State<'_, AppState>, workspace_id: String) -> CmdResult<Vec<Doc<ProtoFile>>> {
    state.engine.proto_files(&workspace_id).map_err(e)
}

#[tauri::command]
pub fn proto_file_create(state: State<'_, AppState>, workspace_id: String, name: String, contents: String) -> CmdResult<Doc<ProtoFile>> {
    state.engine.store.insert(Some(&workspace_id), ProtoFile { name, contents }).map_err(e)
}

#[tauri::command]
pub fn proto_file_update(state: State<'_, AppState>, doc: Doc<ProtoFile>) -> CmdResult<Doc<ProtoFile>> {
    state.grpc_schemas.lock().unwrap().clear(); // protos changed: recompile on next use
    state.engine.store.update(&doc).map_err(e)
}

#[tauri::command]
pub fn grpc_create(state: State<'_, AppState>, parent_id: String) -> CmdResult<Doc<GrpcRequest>> {
    state.engine.store.insert(Some(&parent_id), GrpcRequest::default()).map_err(e)
}

#[tauri::command]
pub fn grpc_update(state: State<'_, AppState>, doc: Doc<GrpcRequest>) -> CmdResult<Doc<GrpcRequest>> {
    state.engine.store.update(&doc).map_err(e)
}

/// Services and methods (cached per request; `refresh` reloads protos/reflection).
#[tauri::command]
pub async fn grpc_methods(state: State<'_, AppState>, id: String, refresh: bool) -> CmdResult<Vec<ServiceInfo>> {
    if !refresh && let Some(s) = state.grpc_schemas.lock().unwrap().get(&id) {
        return Ok(s.services());
    }
    let schema = state.engine.grpc_schema(&id).await.map_err(e)?;
    let services = schema.services();
    state.grpc_schemas.lock().unwrap().insert(id, schema);
    Ok(services)
}

async fn method(state: &AppState, id: &str, path: &str) -> CmdResult<prost_reflect::MethodDescriptor> {
    if path.is_empty() {
        return Err("Pick a method first".into());
    }
    let cached = state.grpc_schemas.lock().unwrap().get(id).cloned();
    let schema = match cached {
        Some(s) => s,
        None => {
            let s = state.engine.grpc_schema(id).await.map_err(e)?;
            state.grpc_schemas.lock().unwrap().insert(id.to_string(), s.clone());
            s
        }
    };
    schema.method(path).map_err(e)
}

#[tauri::command]
pub async fn grpc_invoke(state: State<'_, AppState>, id: String) -> CmdResult<UnaryResult> {
    let p = state.engine.grpc_prepare(&id).map_err(e)?;
    let m = method(&state, &id, &p.method).await?;
    if m.is_client_streaming() || m.is_server_streaming() {
        return Err("This is a streaming method — use Start".into());
    }
    let channel = irs_grpc::connect(&p.url).await.map_err(e)?;
    irs_grpc::unary(channel, &m, &p.body, &p.metadata, p.timeout).await.map_err(e)
}

#[tauri::command]
pub async fn grpc_stream_start<R: tauri::Runtime>(app: tauri::AppHandle<R>, state: State<'_, AppState>, id: String) -> CmdResult<()> {
    if let Some(old) = state.grpc_calls.lock().unwrap().remove(&id) {
        old.lock().unwrap().cancel();
    }
    let p = state.engine.grpc_prepare(&id).map_err(e)?;
    let m = method(&state, &id, &p.method).await?;
    let channel = irs_grpc::connect(&p.url).await.map_err(e)?;
    let call = irs_grpc::start_stream(channel, &m, Some(&p.body), &p.metadata).await.map_err(e)?;
    let mut rx = call.log.subscribe();
    for ev in call.log.entries() {
        let _ = app.emit("grpc-event", json!({ "id": id, "event": ev }));
    }
    let id2 = id.clone();
    tauri::async_runtime::spawn(async move {
        while let Ok(ev) = rx.recv().await {
            let _ = app.emit("grpc-event", json!({ "id": id2, "event": ev }));
        }
    });
    state.grpc_calls.lock().unwrap().insert(id, Arc::new(Mutex::new(call)));
    Ok(())
}

fn call(state: &AppState, id: &str) -> CmdResult<Arc<Mutex<StreamCall>>> {
    state.grpc_calls.lock().unwrap().get(id).cloned().ok_or_else(|| "No active stream — press Start".to_string())
}

/// Send the request's current message (rendered) on a client/bidi stream.
#[tauri::command]
pub fn grpc_send(state: State<'_, AppState>, id: String) -> CmdResult<()> {
    let p = state.engine.grpc_prepare(&id).map_err(e)?;
    let c = call(&state, &id)?;
    let c = c.lock().unwrap();
    if c.is_done() {
        return Err("The stream has ended".into());
    }
    c.send(&p.body).map_err(e)
}

#[tauri::command]
pub fn grpc_commit(state: State<'_, AppState>, id: String) -> CmdResult<()> {
    call(&state, &id)?.lock().unwrap().commit();
    Ok(())
}

#[tauri::command]
pub fn grpc_cancel(state: State<'_, AppState>, id: String) {
    if let Some(c) = state.grpc_calls.lock().unwrap().remove(&id) {
        c.lock().unwrap().cancel();
    }
}

#[tauri::command]
pub fn grpc_log(state: State<'_, AppState>, id: String) -> Vec<RtEvent> {
    state.grpc_calls.lock().unwrap().get(&id).map(|c| c.lock().unwrap().log.entries()).unwrap_or_default()
}
