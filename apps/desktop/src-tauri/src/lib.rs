//! Tauri shell: thin commands over irs-engine / irs-mcp. The UI never touches
//! SQLite or the network directly, so the CLI and desktop behave identically.

mod ai;

use std::collections::HashMap;
use std::sync::Arc;

use base64::Engine as _;
use irs_core::{
    CookieJar, Doc, Environment, Folder, McpServer, McpTransport, RawDoc, Request, Response,
    Settings, Workspace,
};
use irs_engine::Engine;
use irs_mcp::schema::{self, ParamRow};
use irs_mcp::{Client, Hints, InitializeResult, LogEntry, Notification, ProtocolLog, Tool};
use serde::Serialize;
use serde_json::{Value, json};
use tauri::{Emitter, Manager, State};
use tokio::sync::Mutex;

type CmdResult<T> = Result<T, String>;

fn e<E: std::fmt::Display>(err: E) -> String {
    err.to_string()
}

pub struct AppState {
    engine: Engine,
    mcp: Mutex<HashMap<String, Arc<Client>>>,
    logs: std::sync::Mutex<HashMap<String, ProtocolLog>>,
    runs: std::sync::Mutex<HashMap<String, RunSlot>>,
    ai_runs: std::sync::Mutex<HashMap<String, ai::AiRunSlot>>,
}

#[derive(Default)]
struct RunSlot {
    cancel: Arc<std::sync::atomic::AtomicBool>,
    summary: Option<irs_runner::Summary>,
}

impl AppState {
    fn log_for(&self, server_id: &str) -> ProtocolLog {
        self.logs
            .lock()
            .unwrap()
            .entry(server_id.to_string())
            .or_default()
            .clone()
    }
}

// ------------------------------------------------------------------ tree

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct TreeNode {
    id: String,
    kind: String,
    name: String,
    method: Option<String>,
    transport: Option<String>,
    sort_key: f64,
    children: Vec<TreeNode>,
}

fn tree(engine: &Engine, parent: &str) -> CmdResult<Vec<TreeNode>> {
    let mut out = vec![];
    for d in engine.store.all_children(parent).map_err(e)? {
        let name = d.data["name"].as_str().unwrap_or("").to_string();
        let sort_key = d.meta.sort_key;
        let node = match d.meta.kind.as_str() {
            "Folder" => TreeNode {
                children: tree(engine, &d.meta.id)?,
                id: d.meta.id,
                kind: "folder".into(),
                sort_key,
                name,
                method: None,
                transport: None,
            },
            "Request" => TreeNode {
                method: d.data["method"].as_str().map(str::to_string),
                id: d.meta.id,
                kind: "request".into(),
                sort_key,
                name,
                transport: None,
                children: vec![],
            },
            "LlmRequest" => TreeNode {
                transport: d.data["model"].as_str().map(str::to_string),
                id: d.meta.id,
                kind: "llm".into(),
                sort_key,
                name,
                method: None,
                children: vec![],
            },
            "McpServer" => TreeNode {
                transport: d.data["transport"]["kind"].as_str().map(str::to_string),
                id: d.meta.id,
                kind: "mcp".into(),
                sort_key,
                name,
                method: None,
                children: vec![],
            },
            _ => continue,
        };
        out.push(node);
    }
    Ok(out)
}

#[tauri::command]
fn tree_get(state: State<'_, AppState>, workspace_id: String) -> CmdResult<Vec<TreeNode>> {
    tree(&state.engine, &workspace_id)
}

// ------------------------------------------------------------------ workspaces

#[tauri::command]
fn workspace_list(state: State<'_, AppState>) -> CmdResult<Vec<Doc<Workspace>>> {
    let all = state.engine.store.all_of::<Workspace>().map_err(e)?;
    Ok(all
        .into_iter()
        .filter(|w| w.scope != irs_core::WorkspaceScope::Environment)
        .collect())
}

#[tauri::command]
fn workspace_create(state: State<'_, AppState>, name: String) -> CmdResult<Doc<Workspace>> {
    let w = state
        .engine
        .store
        .insert(
            None,
            Workspace {
                name,
                ..Default::default()
            },
        )
        .map_err(e)?;
    state.engine.base_environment(w.id()).map_err(e)?;
    Ok(w)
}

#[tauri::command]
fn workspace_update(state: State<'_, AppState>, doc: Doc<Workspace>) -> CmdResult<Doc<Workspace>> {
    state.engine.store.update(&doc).map_err(e)
}

// ------------------------------------------------------------------ generic items

#[tauri::command]
fn doc_get(state: State<'_, AppState>, id: String) -> CmdResult<RawDoc> {
    state
        .engine
        .store
        .raw(&id)
        .map_err(e)?
        .ok_or_else(|| format!("{id} not found"))
}

#[tauri::command]
fn item_delete(state: State<'_, AppState>, id: String) -> CmdResult<usize> {
    state.engine.store.delete(&id).map_err(e)
}

#[tauri::command]
fn item_rename(state: State<'_, AppState>, id: String, name: String) -> CmdResult<()> {
    let mut d = state.engine.store.raw(&id).map_err(e)?.ok_or("not found")?;
    d.data["name"] = Value::String(name);
    state.engine.store.batch(|tx| tx.update_raw(&d)).map_err(e)
}

#[tauri::command]
fn item_move(
    state: State<'_, AppState>,
    id: String,
    parent_id: String,
    sort_key: f64,
) -> CmdResult<()> {
    if id == parent_id
        || state
            .engine
            .store
            .ancestors(&parent_id)
            .map_err(e)?
            .iter()
            .any(|a| a.meta.id == id)
    {
        return Err("cannot move an item into itself".into());
    }
    state
        .engine
        .store
        .batch(|tx| tx.move_to(&id, Some(&parent_id), sort_key))
        .map_err(e)
}

#[tauri::command]
fn item_duplicate(state: State<'_, AppState>, id: String) -> CmdResult<String> {
    let d = state.engine.store.raw(&id).map_err(e)?.ok_or("not found")?;
    let parent = d.meta.parent_id.clone();
    let mut data = d.data.clone();
    data["name"] = Value::String(format!("{} (copy)", data["name"].as_str().unwrap_or("")));
    let store = &state.engine.store;
    let new_id = match d.meta.kind.as_str() {
        "Request" => {
            store
                .insert(
                    parent.as_deref(),
                    serde_json::from_value::<Request>(data).map_err(e)?,
                )
                .map_err(e)?
                .meta
                .id
        }
        "McpServer" => {
            store
                .insert(
                    parent.as_deref(),
                    serde_json::from_value::<McpServer>(data).map_err(e)?,
                )
                .map_err(e)?
                .meta
                .id
        }
        "LlmRequest" => {
            store
                .insert(
                    parent.as_deref(),
                    serde_json::from_value::<irs_core::LlmRequest>(data).map_err(e)?,
                )
                .map_err(e)?
                .meta
                .id
        }
        "Folder" => {
            // Deep copy: folder + all requests/folders/servers under it.
            fn copy(store: &irs_core::Store, src: &str, dst: &str) -> CmdResult<()> {
                for c in store.all_children(src).map_err(e)? {
                    match c.meta.kind.as_str() {
                        "Request" => {
                            store
                                .insert(
                                    Some(dst),
                                    serde_json::from_value::<Request>(c.data).map_err(e)?,
                                )
                                .map_err(e)?;
                        }
                        "McpServer" => {
                            store
                                .insert(
                                    Some(dst),
                                    serde_json::from_value::<McpServer>(c.data).map_err(e)?,
                                )
                                .map_err(e)?;
                        }
                        "LlmRequest" => {
                            store
                                .insert(
                                    Some(dst),
                                    serde_json::from_value::<irs_core::LlmRequest>(c.data)
                                        .map_err(e)?,
                                )
                                .map_err(e)?;
                        }
                        "Folder" => {
                            let f = store
                                .insert(
                                    Some(dst),
                                    serde_json::from_value::<Folder>(c.data).map_err(e)?,
                                )
                                .map_err(e)?;
                            copy(store, &c.meta.id, f.id())?;
                        }
                        _ => {}
                    }
                }
                Ok(())
            }
            let f = store
                .insert(
                    parent.as_deref(),
                    serde_json::from_value::<Folder>(data).map_err(e)?,
                )
                .map_err(e)?;
            copy(store, &id, f.id())?;
            f.meta.id
        }
        other => return Err(format!("cannot duplicate {other}")),
    };
    Ok(new_id)
}

// ------------------------------------------------------------------ requests / folders / servers

#[tauri::command]
fn request_create(
    state: State<'_, AppState>,
    parent_id: String,
    request: Option<Request>,
) -> CmdResult<Doc<Request>> {
    state
        .engine
        .store
        .insert(Some(&parent_id), request.unwrap_or_default())
        .map_err(e)
}

#[tauri::command]
fn request_update(state: State<'_, AppState>, doc: Doc<Request>) -> CmdResult<Doc<Request>> {
    state.engine.store.update(&doc).map_err(e)
}

#[tauri::command]
fn folder_create(
    state: State<'_, AppState>,
    parent_id: String,
    name: String,
) -> CmdResult<Doc<Folder>> {
    state
        .engine
        .store
        .insert(
            Some(&parent_id),
            Folder {
                name,
                ..Default::default()
            },
        )
        .map_err(e)
}

#[tauri::command]
fn folder_update(state: State<'_, AppState>, doc: Doc<Folder>) -> CmdResult<Doc<Folder>> {
    state.engine.store.update(&doc).map_err(e)
}

#[tauri::command]
fn mcp_server_create(
    state: State<'_, AppState>,
    parent_id: String,
    name: String,
) -> CmdResult<Doc<McpServer>> {
    let s = McpServer {
        name,
        transport: McpTransport::StreamableHttp {
            url: "http://localhost:3333/mcp".into(),
        },
        ..Default::default()
    };
    state.engine.store.insert(Some(&parent_id), s).map_err(e)
}

#[tauri::command]
fn mcp_server_update(state: State<'_, AppState>, doc: Doc<McpServer>) -> CmdResult<Doc<McpServer>> {
    state.engine.store.update(&doc).map_err(e)
}

#[tauri::command]
fn curl_parse(text: String) -> CmdResult<Request> {
    irs_engine::curl::parse(&text).map_err(e)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ResponseView {
    #[serde(flatten)]
    doc: Doc<Response>,
    /// UTF-8 body (lossy), capped at 5 MB for display.
    body_text: String,
    body_truncated: bool,
    /// Base64 body for images.
    body_image: Option<String>,
}

fn view(engine: &Engine, mut doc: Doc<Response>) -> ResponseView {
    let bytes = engine.response_body(&doc);
    doc.body_b64 = None;
    let limit = 5 * 1024 * 1024;
    let body_image = doc
        .content_type
        .starts_with("image/")
        .then(|| base64::engine::general_purpose::STANDARD.encode(&bytes));
    ResponseView {
        body_truncated: bytes.len() > limit,
        body_text: String::from_utf8_lossy(&bytes[..bytes.len().min(limit)]).into_owned(),
        body_image,
        doc,
    }
}

#[tauri::command]
async fn request_send(state: State<'_, AppState>, request_id: String) -> CmdResult<ResponseView> {
    let resp = state.engine.send(&request_id).await.map_err(e)?;
    Ok(view(&state.engine, resp))
}

#[tauri::command]
fn response_list(state: State<'_, AppState>, request_id: String) -> CmdResult<Vec<Value>> {
    Ok(state
        .engine
        .responses(&request_id)
        .map_err(e)?
        .into_iter()
        .map(|r| {
            json!({
                "id": r.meta.id, "created": r.meta.created, "statusCode": r.status_code,
                "statusMessage": r.status_message, "error": r.error, "totalMs": r.timings.total_ms,
                "bytes": r.bytes, "method": r.method, "url": r.url,
            })
        })
        .collect())
}

#[tauri::command]
fn response_get(state: State<'_, AppState>, id: String) -> CmdResult<ResponseView> {
    let doc = state.engine.store.get::<Response>(&id).map_err(e)?;
    Ok(view(&state.engine, doc))
}

#[tauri::command]
fn response_clear(state: State<'_, AppState>, request_id: String) -> CmdResult<()> {
    let ids: Vec<String> = state
        .engine
        .responses(&request_id)
        .map_err(e)?
        .into_iter()
        .map(|r| r.meta.id)
        .collect();
    state
        .engine
        .store
        .batch(|tx| {
            for id in &ids {
                tx.delete(id)?;
            }
            Ok(())
        })
        .map_err(e)
}

// ------------------------------------------------------------------ environments

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct EnvList {
    base: Doc<Environment>,
    subs: Vec<Doc<Environment>>,
    active_id: Option<String>,
}

#[tauri::command]
fn env_list(state: State<'_, AppState>, workspace_id: String) -> CmdResult<EnvList> {
    let ws = state
        .engine
        .store
        .get::<Workspace>(&workspace_id)
        .map_err(e)?;
    Ok(EnvList {
        base: state.engine.base_environment(&workspace_id).map_err(e)?,
        subs: state.engine.sub_environments(&workspace_id).map_err(e)?,
        active_id: ws.active_environment_id.clone(),
    })
}

#[tauri::command]
fn env_create(
    state: State<'_, AppState>,
    workspace_id: String,
    name: String,
) -> CmdResult<Doc<Environment>> {
    let base = state.engine.base_environment(&workspace_id).map_err(e)?;
    state
        .engine
        .store
        .insert(
            Some(base.id()),
            Environment {
                name,
                ..Default::default()
            },
        )
        .map_err(e)
}

#[tauri::command]
fn env_update(state: State<'_, AppState>, doc: Doc<Environment>) -> CmdResult<Doc<Environment>> {
    state.engine.store.update(&doc).map_err(e)
}

#[tauri::command]
fn env_set_active(
    state: State<'_, AppState>,
    workspace_id: String,
    env_id: Option<String>,
) -> CmdResult<()> {
    let mut ws = state
        .engine
        .store
        .get::<Workspace>(&workspace_id)
        .map_err(e)?;
    ws.active_environment_id = env_id;
    state.engine.store.update(&ws).map(|_| ()).map_err(e)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Preview {
    rendered: Option<String>,
    error: Option<String>,
    refs: Vec<irs_templating::VarRef>,
}

#[tauri::command]
fn render_preview(state: State<'_, AppState>, id: String, text: String) -> CmdResult<Preview> {
    let (r, refs) = state.engine.preview(&id, &text).map_err(e)?;
    Ok(match r {
        Ok(s) => Preview {
            rendered: Some(s),
            error: None,
            refs,
        },
        Err(err) => Preview {
            rendered: None,
            error: Some(err.to_string()),
            refs,
        },
    })
}

#[derive(Serialize)]
struct VarInfo {
    name: String,
    value: Value,
    source: String,
}

#[tauri::command]
fn context_vars(state: State<'_, AppState>, id: String) -> CmdResult<Vec<VarInfo>> {
    let ctx = state.engine.context(&id).map_err(e)?;
    Ok(ctx
        .vars
        .iter()
        .map(|(k, v)| VarInfo {
            name: k.clone(),
            value: v.clone(),
            source: ctx.sources.get(k).cloned().unwrap_or_default(),
        })
        .collect())
}

#[tauri::command]
fn cookies_get(state: State<'_, AppState>, workspace_id: String) -> CmdResult<Doc<CookieJar>> {
    state.engine.cookie_jar(&workspace_id).map_err(e)
}

#[tauri::command]
fn cookies_update(state: State<'_, AppState>, doc: Doc<CookieJar>) -> CmdResult<Doc<CookieJar>> {
    state.engine.store.update(&doc).map_err(e)
}

#[tauri::command]
fn settings_get(state: State<'_, AppState>) -> CmdResult<Doc<Settings>> {
    state.engine.store.settings().map_err(e)
}

#[tauri::command]
fn settings_update(state: State<'_, AppState>, doc: Doc<Settings>) -> CmdResult<Doc<Settings>> {
    state.engine.store.update(&doc).map_err(e)
}

// ------------------------------------------------------------------ MCP

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct McpStatus {
    connected: bool,
    server: Option<InitializeResult>,
    session_id: Option<String>,
}

#[tauri::command]
async fn mcp_connect<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    state: State<'_, AppState>,
    server_id: String,
) -> CmdResult<McpStatus> {
    if let Some(old) = state.mcp.lock().await.remove(&server_id) {
        old.close().await;
    }
    let opts = state.engine.mcp_connect_options(&server_id).map_err(e)?;
    let log = state.log_for(&server_id);
    let mut rx = log.subscribe();
    let sid = server_id.clone();
    let app2 = app.clone();
    tokio::spawn(async move {
        while let Ok(entry) = rx.recv().await {
            let _ = app2.emit("mcp-log", json!({ "serverId": sid, "entry": entry }));
        }
    });
    let client = Client::connect_with_log(opts, log).await.map_err(e)?;
    let status = McpStatus {
        connected: true,
        server: Some(client.server().clone()),
        session_id: client.session_id(),
    };
    state.mcp.lock().await.insert(server_id, Arc::new(client));
    Ok(status)
}

#[tauri::command]
async fn mcp_disconnect(state: State<'_, AppState>, server_id: String) -> CmdResult<()> {
    if let Some(c) = state.mcp.lock().await.remove(&server_id) {
        c.close().await;
    }
    Ok(())
}

#[tauri::command]
async fn mcp_status(state: State<'_, AppState>, server_id: String) -> CmdResult<McpStatus> {
    Ok(match state.mcp.lock().await.get(&server_id) {
        Some(c) => McpStatus {
            connected: true,
            server: Some(c.server().clone()),
            session_id: c.session_id(),
        },
        None => McpStatus {
            connected: false,
            server: None,
            session_id: None,
        },
    })
}

async fn client(state: &AppState, server_id: &str) -> CmdResult<Arc<Client>> {
    state
        .mcp
        .lock()
        .await
        .get(server_id)
        .cloned()
        .ok_or_else(|| "Not connected — press Connect first".to_string())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ToolView {
    #[serde(flatten)]
    tool: Tool,
    display_name: String,
    hints: Hints,
    params: Vec<ParamRow>,
    output_params: Vec<ParamRow>,
    example: Value,
}

#[tauri::command]
async fn mcp_list(state: State<'_, AppState>, server_id: String, kind: String) -> CmdResult<Value> {
    let c = client(&state, &server_id).await?;
    let out = match kind.as_str() {
        "tools" => {
            let l = c.list_tools().await.map_err(e)?;
            let items: Vec<ToolView> = l
                .items
                .into_iter()
                .map(|t| ToolView {
                    display_name: t.display_name().to_string(),
                    hints: t.hints(),
                    params: schema::param_rows(&t.input_schema),
                    output_params: t
                        .output_schema
                        .as_ref()
                        .map(schema::param_rows)
                        .unwrap_or_default(),
                    example: schema::example_args(&t.input_schema),
                    tool: t,
                })
                .collect();
            json!({ "items": items, "pages": l.pages, "elapsedMs": l.elapsed_ms })
        }
        "resources" => serde_json::to_value(c.list_resources().await.map_err(e)?).map_err(e)?,
        "templates" => {
            serde_json::to_value(c.list_resource_templates().await.map_err(e)?).map_err(e)?
        }
        "prompts" => serde_json::to_value(c.list_prompts().await.map_err(e)?).map_err(e)?,
        other => return Err(format!("unknown list kind {other}")),
    };
    Ok(out)
}

#[tauri::command]
async fn mcp_call_tool(
    state: State<'_, AppState>,
    server_id: String,
    name: String,
    args: Value,
) -> CmdResult<Value> {
    let c = client(&state, &server_id).await?;
    let t = std::time::Instant::now();
    let r = c.call_tool(&name, args).await.map_err(e)?;
    Ok(json!({ "result": r, "latencyMs": t.elapsed().as_secs_f64() * 1000.0 }))
}

#[tauri::command]
async fn mcp_read_resource(
    state: State<'_, AppState>,
    server_id: String,
    uri: String,
) -> CmdResult<Value> {
    client(&state, &server_id)
        .await?
        .read_resource(&uri)
        .await
        .map_err(e)
}

#[tauri::command]
async fn mcp_get_prompt(
    state: State<'_, AppState>,
    server_id: String,
    name: String,
    args: Value,
) -> CmdResult<Value> {
    client(&state, &server_id)
        .await?
        .get_prompt(&name, args)
        .await
        .map_err(e)
}

#[tauri::command]
async fn mcp_ping(state: State<'_, AppState>, server_id: String) -> CmdResult<f64> {
    client(&state, &server_id).await?.ping().await.map_err(e)
}

#[tauri::command]
fn mcp_validate(schema: Value, args: Value) -> Vec<String> {
    schema::validate(&schema, &args)
}

#[tauri::command]
fn mcp_log(state: State<'_, AppState>, server_id: String) -> Vec<LogEntry> {
    state.log_for(&server_id).entries()
}

#[tauri::command]
fn mcp_log_clear(state: State<'_, AppState>, server_id: String) {
    state.log_for(&server_id).clear();
}

#[tauri::command]
async fn mcp_notifications(
    state: State<'_, AppState>,
    server_id: String,
) -> CmdResult<Vec<Notification>> {
    Ok(match state.mcp.lock().await.get(&server_id) {
        Some(c) => c.notifications(),
        None => vec![],
    })
}

// ------------------------------------------------------------------ runner

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct RunRequest {
    /// Client-chosen id so events can't arrive before the UI knows it.
    run_id: Option<String>,
    request_ids: Vec<String>,
    iterations: u32,
    delay_ms: u64,
    bail: bool,
    /// CSV or JSON data file contents.
    data_text: Option<String>,
}

#[tauri::command]
async fn runner_start<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    state: State<'_, AppState>,
    run: RunRequest,
) -> CmdResult<String> {
    let data = match run.data_text.as_deref().filter(|t| !t.trim().is_empty()) {
        Some(t) => irs_runner::parse_data(t).map_err(e)?,
        None => vec![],
    };
    if run.request_ids.is_empty() {
        return Err("Select at least one request to run".into());
    }
    let run_id = run
        .run_id
        .clone()
        .filter(|r| !r.is_empty())
        .unwrap_or_else(|| irs_core::new_id("run"));
    let cancel = Arc::new(std::sync::atomic::AtomicBool::new(false));
    state.runs.lock().unwrap().insert(
        run_id.clone(),
        RunSlot {
            cancel: cancel.clone(),
            summary: None,
        },
    );
    let opts = irs_runner::RunOptions {
        iterations: run.iterations.max(1),
        delay_ms: run.delay_ms,
        data,
        bail: run.bail,
        cancel: Some(cancel),
        ..Default::default()
    };
    let engine = state.engine.clone();
    let id = run_id.clone();
    tauri::async_runtime::spawn(async move {
        let summary = irs_runner::run(&engine, &run.request_ids, &opts, |ev| {
            let _ = app.emit("runner-event", json!({ "runId": id, "event": ev }));
        })
        .await;
        if let Some(state) = app.try_state::<AppState>()
            && let Some(slot) = state.runs.lock().unwrap().get_mut(&id)
        {
            slot.summary = Some(summary);
        }
    });
    Ok(run_id)
}

#[tauri::command]
fn runner_cancel(state: State<'_, AppState>, run_id: String) {
    if let Some(slot) = state.runs.lock().unwrap().get(&run_id) {
        slot.cancel.store(true, std::sync::atomic::Ordering::SeqCst);
    }
}

/// Write a finished run's report to `dir` (default: Downloads) and return the path.
#[tauri::command]
fn runner_export(
    state: State<'_, AppState>,
    run_id: String,
    reporter: String,
    dir: Option<String>,
) -> CmdResult<String> {
    let runs = state.runs.lock().unwrap();
    let summary = runs
        .get(&run_id)
        .and_then(|s| s.summary.as_ref())
        .ok_or("The run has not finished yet")?;
    let text = irs_runner::report::render(summary, &reporter).ok_or("Unknown report format")?;
    let ext = match reporter.as_str() {
        "junit" => "xml",
        "json" => "json",
        _ => "txt",
    };
    let home = std::env::var_os("HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_default();
    let dir = match dir {
        Some(d) => std::path::PathBuf::from(d),
        None => [home.join("Downloads"), home.clone()]
            .into_iter()
            .find(|d| d.is_dir())
            .unwrap_or(home),
    };
    let stamp = chrono_like_stamp();
    let path = dir.join(format!("insomnia-rs-run-{stamp}.{ext}"));
    std::fs::write(&path, text).map_err(e)?;
    Ok(path.to_string_lossy().into_owned())
}

fn chrono_like_stamp() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    secs.to_string()
}

// ------------------------------------------------------------------ app

fn seed_if_empty(engine: &Engine) -> irs_core::store::Result<()> {
    if !engine.store.all_of::<Workspace>()?.is_empty() {
        return Ok(());
    }
    let ws = engine.store.insert(
        None,
        Workspace {
            name: "My Collection".into(),
            ..Default::default()
        },
    )?;
    let mut base =
        engine
            .base_environment(ws.id())
            .map_err(|_| irs_core::StoreError::NotFound {
                kind: "Environment",
                id: "base".into(),
            })?;
    base.data
        .insert("base_url".into(), "https://httpbin.org".into());
    engine.store.update(&base)?;
    let folder = engine.store.insert(
        Some(ws.id()),
        Folder {
            name: "Examples".into(),
            ..Default::default()
        },
    )?;
    engine.store.insert(
        Some(folder.id()),
        Request {
            name: "Get JSON".into(),
            url: "{{ _.base_url }}/json".into(),
            ..Default::default()
        },
    )?;
    engine.store.insert(
        Some(folder.id()),
        Request {
            name: "Post body".into(),
            method: "POST".into(),
            url: "{{ _.base_url }}/post".into(),
            body: irs_core::Body {
                mime_type: Some(irs_core::mime::JSON.into()),
                text: Some("{\n  \"id\": \"{% uuid 'v4' %}\",\n  \"hello\": \"world\"\n}".into()),
                ..Default::default()
            },
            ..Default::default()
        },
    )?;
    Ok(())
}

/// Register state, events and commands. Shared by `run` and the IPC tests.
pub fn build<R: tauri::Runtime>(builder: tauri::Builder<R>, engine: Engine) -> tauri::Builder<R> {
    builder
        .manage(AppState {
            engine: engine.clone(),
            mcp: Mutex::default(),
            logs: Default::default(),
            runs: Default::default(),
            ai_runs: Default::default(),
        })
        .setup(move |app| {
            let handle = app.handle().clone();
            engine.store.subscribe(move |changes| {
                let _ = handle.emit("db-changed", changes);
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            tree_get,
            workspace_list,
            workspace_create,
            workspace_update,
            doc_get,
            item_delete,
            item_rename,
            item_move,
            item_duplicate,
            request_create,
            request_update,
            folder_create,
            folder_update,
            mcp_server_create,
            mcp_server_update,
            curl_parse,
            request_send,
            response_list,
            response_get,
            response_clear,
            env_list,
            env_create,
            env_update,
            env_set_active,
            render_preview,
            context_vars,
            cookies_get,
            cookies_update,
            settings_get,
            settings_update,
            mcp_connect,
            mcp_disconnect,
            mcp_status,
            mcp_list,
            mcp_call_tool,
            mcp_read_resource,
            mcp_get_prompt,
            mcp_ping,
            mcp_validate,
            mcp_log,
            mcp_log_clear,
            mcp_notifications,
            runner_start,
            runner_cancel,
            runner_export,
            ai::llm_provider_list,
            ai::llm_provider_create,
            ai::llm_provider_update,
            ai::llm_provider_delete,
            ai::llm_provider_set_key,
            ai::llm_models,
            ai::llm_request_create,
            ai::llm_request_update,
            ai::llm_runs,
            ai::llm_run_start,
            ai::llm_approve,
            ai::llm_cancel,
        ])
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let engine = Engine::open(irs_engine::default_data_dir())
        .expect("could not open the insomnia-rs database");
    let _ = seed_if_empty(&engine);
    build(tauri::Builder::default(), engine)
        .run(tauri::generate_context!())
        .expect("error while running insomnia-rs");
}

#[cfg(test)]
mod ipc_tests;
