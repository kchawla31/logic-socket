//! AI commands: providers, AI requests, streamed runs with tool approvals.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use futures::future::BoxFuture;
use lsock_core::{Doc, KeySource, LlmProvider, LlmRequest, LlmRun};
use lsock_engine::llm::parse_kind;
use lsock_llm::agent::{Approval, Approver, ToolCallInfo, ToolSource};
use serde::Serialize;
use serde_json::{Value, json};
use tauri::{Emitter, Manager, State};

use crate::{AppState, CmdResult, e};

/// Per-run control: cancel flag + tool calls waiting for the user.
#[derive(Default)]
pub struct AiRunSlot {
    cancel: Arc<AtomicBool>,
    approvals: Arc<std::sync::Mutex<HashMap<String, tokio::sync::oneshot::Sender<Approval>>>>,
}

struct UiApprover {
    approvals: Arc<std::sync::Mutex<HashMap<String, tokio::sync::oneshot::Sender<Approval>>>>,
    cancel: Arc<AtomicBool>,
}

impl Approver for UiApprover {
    fn approve(&self, call: ToolCallInfo) -> BoxFuture<'static, Approval> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.approvals.lock().unwrap().insert(call.id.clone(), tx);
        let cancelled = self.cancel.load(Ordering::SeqCst);
        Box::pin(async move {
            if cancelled {
                return Approval::Deny("run cancelled".into());
            }
            rx.await.unwrap_or(Approval::Deny("run cancelled".into()))
        })
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderView {
    #[serde(flatten)]
    doc: Doc<LlmProvider>,
    /// Whether a keychain key is stored (the key itself is never sent to the UI).
    has_key: bool,
    suggested_models: Vec<String>,
}

fn view(state: &AppState, p: Doc<LlmProvider>) -> ProviderView {
    let has_key = match &p.key_source {
        KeySource::Keychain => state.engine.secrets().get(p.id()).is_some(),
        KeySource::None => true,
        KeySource::Env { var } => std::env::var(var).is_ok(),
        KeySource::Template { .. } => true,
    };
    let suggested_models = parse_kind(&p.kind)
        .suggested_models()
        .iter()
        .map(|s| s.to_string())
        .collect();
    ProviderView {
        doc: p,
        has_key,
        suggested_models,
    }
}

#[tauri::command]
pub fn llm_provider_list(state: State<'_, AppState>) -> CmdResult<Vec<ProviderView>> {
    Ok(state
        .engine
        .store
        .all_of::<LlmProvider>()
        .map_err(e)?
        .into_iter()
        .map(|p| view(&state, p))
        .collect())
}

#[tauri::command]
pub fn llm_provider_create(
    state: State<'_, AppState>,
    name: String,
    kind: String,
) -> CmdResult<ProviderView> {
    let k = parse_kind(&kind);
    let p = LlmProvider {
        name,
        kind: kind.clone(),
        base_url: String::new(),
        key_source: if k == lsock_llm::ProviderKind::Ollama {
            KeySource::None
        } else {
            KeySource::Keychain
        },
        default_model: k
            .suggested_models()
            .first()
            .copied()
            .unwrap_or("")
            .to_string(),
        headers: vec![],
    };
    let doc = state.engine.store.insert(None, p).map_err(e)?;
    Ok(view(&state, doc))
}

#[tauri::command]
pub fn llm_provider_update(
    state: State<'_, AppState>,
    doc: Doc<LlmProvider>,
) -> CmdResult<ProviderView> {
    let d = state.engine.store.update(&doc).map_err(e)?;
    Ok(view(&state, d))
}

#[tauri::command]
pub fn llm_provider_delete(state: State<'_, AppState>, id: String) -> CmdResult<()> {
    let _ = state.engine.secrets().delete(&id);
    state.engine.store.delete(&id).map(|_| ()).map_err(e)
}

/// Store (or clear, with an empty string) a provider's API key in the OS keychain.
#[tauri::command]
pub fn llm_provider_set_key(state: State<'_, AppState>, id: String, key: String) -> CmdResult<()> {
    let key = key.trim();
    if key.is_empty() {
        state.engine.secrets().delete(&id)
    } else {
        state.engine.secrets().set(&id, key)
    }
}

#[tauri::command]
pub async fn llm_models(state: State<'_, AppState>, provider_id: String) -> CmdResult<Vec<String>> {
    let (_, cfg) = state
        .engine
        .provider_config(&provider_id, None)
        .map_err(e)?;
    lsock_llm::list_models(&cfg).await.map_err(e)
}

#[tauri::command]
pub fn llm_request_create(
    state: State<'_, AppState>,
    parent_id: String,
    name: String,
) -> CmdResult<Doc<LlmRequest>> {
    let provider_id = state
        .engine
        .store
        .all_of::<LlmProvider>()
        .map_err(e)?
        .into_iter()
        .next()
        .map(|p| p.meta.id);
    state
        .engine
        .store
        .insert(
            Some(&parent_id),
            LlmRequest {
                name,
                provider_id,
                ..Default::default()
            },
        )
        .map_err(e)
}

#[tauri::command]
pub fn llm_request_update(
    state: State<'_, AppState>,
    doc: Doc<LlmRequest>,
) -> CmdResult<Doc<LlmRequest>> {
    state.engine.store.update(&doc).map_err(e)
}

#[tauri::command]
pub fn llm_runs(state: State<'_, AppState>, request_id: String) -> CmdResult<Vec<Doc<LlmRun>>> {
    state.engine.llm_runs(&request_id).map_err(e)
}

/// Start a run; events arrive as `llm-event` `{ runId, event }`. The UI picks `run_id`.
#[tauri::command]
pub async fn llm_run_start<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    state: State<'_, AppState>,
    run_id: String,
    request_id: String,
) -> CmdResult<()> {
    let prepared = state.engine.llm_prepare(&request_id).map_err(e)?;
    // Reuse MCP connections the user already opened in the inspector; connect the rest for this run.
    let mut sources = vec![];
    let mut temporary = vec![];
    for id in &prepared.mcp_server_ids {
        let existing = state.mcp.lock().await.get(id).cloned();
        match existing {
            Some(client) => {
                let tools = client
                    .list_tools()
                    .await
                    .map(|l| l.items)
                    .unwrap_or_default();
                let name = state
                    .engine
                    .store
                    .get::<lsock_core::McpServer>(id)
                    .map(|s| s.body.name)
                    .unwrap_or_default();
                let actions = state.engine.mcp_source_actions(id, &tools).await;
                sources.push(ToolSource {
                    server_id: id.clone(),
                    server_name: name,
                    client,
                    actions,
                    tools,
                });
            }
            None => temporary.push(id.clone()),
        }
    }
    let connected = state
        .engine
        .connect_tool_sources(&temporary)
        .await
        .map_err(e)?;
    let temp_clients: Vec<Arc<lsock_mcp::Client>> =
        connected.iter().map(|s| s.client.clone()).collect();
    sources.extend(connected);

    let slot = AiRunSlot::default();
    let approver = UiApprover {
        approvals: slot.approvals.clone(),
        cancel: slot.cancel.clone(),
    };
    let cancel = slot.cancel.clone();
    let mut prepared = prepared;
    prepared.options.cancel = Some(cancel);
    state.ai_runs.lock().unwrap().insert(run_id.clone(), slot);
    let engine = state.engine.clone();
    tauri::async_runtime::spawn(async move {
        let emit = |event: Value| {
            let _ = app.emit("llm-event", json!({ "runId": run_id, "event": event }));
        };
        let result = engine
            .llm_execute(&request_id, prepared, &sources, &approver, &mut |ev| {
                emit(serde_json::to_value(&ev).unwrap_or(Value::Null))
            })
            .await;
        match result {
            Ok(run) => emit(json!({ "type": "saved", "run": run })),
            Err(err) => emit(json!({ "type": "error", "message": err.to_string() })),
        }
        for c in temp_clients {
            c.close().await;
        }
        if let Some(st) = app.try_state::<AppState>() {
            st.ai_runs.lock().unwrap().remove(&run_id);
        }
    });
    Ok(())
}

#[tauri::command]
pub fn llm_approve(
    state: State<'_, AppState>,
    run_id: String,
    call_id: String,
    allow: bool,
    reason: Option<String>,
    // "Always allow this tool": saved on the AI request so later runs skip the prompt too.
    always: Option<(String, String)>,
) -> CmdResult<()> {
    if allow && let Some((request_id, allow_key)) = &always {
        state
            .engine
            .llm_allow_tool(request_id, allow_key)
            .map_err(e)?;
    }
    let runs = state.ai_runs.lock().unwrap();
    let slot = runs.get(&run_id).ok_or("This run has finished")?;
    let tx = slot
        .approvals
        .lock()
        .unwrap()
        .remove(&call_id)
        .ok_or("No pending approval for this call")?;
    let _ = tx.send(if allow && always.is_some() {
        Approval::AllowAlways
    } else if allow {
        Approval::Allow
    } else {
        Approval::Deny(reason.unwrap_or_default())
    });
    Ok(())
}

#[tauri::command]
pub fn llm_cancel(state: State<'_, AppState>, run_id: String) {
    if let Some(slot) = state.ai_runs.lock().unwrap().get(&run_id) {
        slot.cancel.store(true, Ordering::SeqCst);
        for (_, tx) in slot.approvals.lock().unwrap().drain() {
            let _ = tx.send(Approval::Deny("run cancelled".into()));
        }
    }
}
