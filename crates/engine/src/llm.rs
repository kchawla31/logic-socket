//! AI requests: resolve provider + key, render the prompt, run the agent with
//! MCP tools, persist the run. API keys never touch the database.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use irs_core::{Doc, KeySource, LlmProvider, LlmRequest, LlmRun, McpServer};
use irs_llm::agent::{AgentEvent, AgentOptions, AgentOutcome, Approver, AutoApprove, ToolSource};
use irs_llm::{Block, ChatRequest, Message, ProviderConfig, ProviderKind, Role};
use irs_templating::Mode;
use serde_json::{Value, json};

use crate::{Engine, EngineError, Result};

const KEYCHAIN_SERVICE: &str = "insomnia-rs";

/// Where provider API keys live.
pub trait SecretStore: Send + Sync {
    fn get(&self, id: &str) -> Option<String>;
    fn set(&self, id: &str, secret: &str) -> std::result::Result<(), String>;
    fn delete(&self, id: &str) -> std::result::Result<(), String>;
}

/// OS keychain (macOS Keychain, Windows Credential Manager).
pub struct KeychainStore;

impl SecretStore for KeychainStore {
    fn get(&self, id: &str) -> Option<String> {
        keyring::Entry::new(KEYCHAIN_SERVICE, id).ok()?.get_password().ok()
    }
    fn set(&self, id: &str, secret: &str) -> std::result::Result<(), String> {
        keyring::Entry::new(KEYCHAIN_SERVICE, id).and_then(|e| e.set_password(secret)).map_err(|e| e.to_string())
    }
    fn delete(&self, id: &str) -> std::result::Result<(), String> {
        match keyring::Entry::new(KEYCHAIN_SERVICE, id).and_then(|e| e.delete_credential()) {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(e.to_string()),
        }
    }
}

/// In-memory store for tests and in-memory engines.
#[derive(Default)]
pub struct MemoryStore(Mutex<HashMap<String, String>>);

impl SecretStore for MemoryStore {
    fn get(&self, id: &str) -> Option<String> {
        self.0.lock().unwrap().get(id).cloned()
    }
    fn set(&self, id: &str, secret: &str) -> std::result::Result<(), String> {
        self.0.lock().unwrap().insert(id.to_string(), secret.to_string());
        Ok(())
    }
    fn delete(&self, id: &str) -> std::result::Result<(), String> {
        self.0.lock().unwrap().remove(id);
        Ok(())
    }
}

pub fn parse_kind(kind: &str) -> ProviderKind {
    match kind {
        "openai" => ProviderKind::Openai,
        "ollama" => ProviderKind::Ollama,
        "openai-compatible" => ProviderKind::OpenaiCompatible,
        _ => ProviderKind::Anthropic,
    }
}

fn policy(s: &str) -> AutoApprove {
    match s {
        "none" => AutoApprove::None,
        "all" => AutoApprove::All,
        _ => AutoApprove::ReadOnly,
    }
}

/// Everything needed to run an AI request.
pub struct LlmPrepared {
    pub provider: Doc<LlmProvider>,
    pub config: ProviderConfig,
    pub chat: ChatRequest,
    pub options: AgentOptions,
    pub mcp_server_ids: Vec<String>,
}

impl Engine {
    pub fn secrets(&self) -> &dyn SecretStore {
        self.secrets.as_ref()
    }

    /// Resolve a provider's connection settings, including its API key.
    /// `context_id` (a request/folder/workspace) is used for template keys.
    pub fn provider_config(&self, provider_id: &str, context_id: Option<&str>) -> Result<(Doc<LlmProvider>, ProviderConfig)> {
        let p: Doc<LlmProvider> = self.store.get(provider_id)?;
        let kind = parse_kind(&p.kind);
        let key = match &p.key_source {
            KeySource::None => None,
            KeySource::Keychain => self.secrets.get(p.id()),
            KeySource::Env { var } => std::env::var(var).ok(),
            KeySource::Template { template } => {
                let ctx = match context_id {
                    Some(id) => self.context(id)?,
                    None => Default::default(),
                };
                Some(self.renderer().render_str(template, &ctx, Mode::Throw).map_err(|source| EngineError::Render { field: "API key".into(), source })?)
            }
        }
        .filter(|k| !k.trim().is_empty());
        if kind.needs_key() && key.is_none() && kind != ProviderKind::OpenaiCompatible {
            let how = match &p.key_source {
                KeySource::Keychain => "add the key in Settings → AI providers".to_string(),
                KeySource::Env { var } => format!("set the {var} environment variable"),
                KeySource::Template { template } => format!("define the variable used in {template}"),
                KeySource::None => "choose a key source".to_string(),
            };
            return Err(EngineError::Llm(format!("No API key for provider '{}': {how}", p.name)));
        }
        let mut config = ProviderConfig::new(kind, key);
        if !p.base_url.trim().is_empty() {
            config.base_url = p.base_url.trim().to_string();
        }
        config.headers = p.headers.iter().filter(|h| !h.disabled && !h.name.is_empty()).map(|h| (h.name.clone(), h.value.clone())).collect();
        Ok((p, config))
    }

    /// Render an AI request into a provider call (templates resolved).
    pub fn llm_prepare(&self, request_id: &str) -> Result<LlmPrepared> {
        let r: Doc<LlmRequest> = self.store.get(request_id)?;
        let provider_id = match &r.provider_id {
            Some(id) => id.clone(),
            None => self
                .store
                .all_of::<LlmProvider>()?
                .into_iter()
                .next()
                .map(|p| p.meta.id)
                .ok_or_else(|| EngineError::Llm("No AI provider configured — add one in Settings → AI providers".into()))?,
        };
        let (provider, config) = self.provider_config(&provider_id, Some(request_id))?;
        let ctx = self.context(request_id)?;
        let render = |field: &str, s: &str| {
            self.renderer().render_str(s, &ctx, Mode::Throw).map_err(|source| EngineError::Render { field: field.to_string(), source })
        };
        let model = if r.model.trim().is_empty() { provider.default_model.clone() } else { render("model", &r.model)? };
        if model.trim().is_empty() {
            return Err(EngineError::Llm(format!("Pick a model (provider '{}' has no default)", provider.name)));
        }
        let mut messages = vec![];
        for (i, m) in r.messages.iter().enumerate() {
            let text = render(&format!("message {}", i + 1), &m.text)?;
            if text.trim().is_empty() {
                continue;
            }
            let role = if m.role == "assistant" { Role::Assistant } else { Role::User };
            messages.push(Message { role, content: vec![Block::Text { text }] });
        }
        if messages.is_empty() {
            return Err(EngineError::Llm("Write a prompt first".into()));
        }
        let system = render("system prompt", &r.system)?;
        Ok(LlmPrepared {
            chat: ChatRequest {
                model,
                system: Some(system).filter(|s| !s.trim().is_empty()),
                messages,
                tools: vec![],
                max_tokens: r.max_tokens.max(1),
                temperature: r.temperature,
            },
            options: AgentOptions { max_turns: r.max_turns.max(1), auto_approve: policy(&r.auto_approve), ..Default::default() },
            mcp_server_ids: r.mcp_server_ids.clone(),
            provider,
            config,
        })
    }

    /// Connect to MCP servers by id and list their tools (for one-off runs, e.g. the CLI).
    pub async fn connect_tool_sources(&self, server_ids: &[String]) -> Result<Vec<ToolSource>> {
        let mut out = vec![];
        for id in server_ids {
            let server: Doc<McpServer> = self.store.get(id)?;
            let opts = self.mcp_connect_options(id)?;
            let client = irs_mcp::Client::connect(opts).await.map_err(|e| EngineError::Llm(format!("MCP server '{}': {e}", server.name)))?;
            let tools = match client.list_tools().await {
                Ok(l) => l.items,
                Err(irs_mcp::McpError::Unsupported(_)) => vec![],
                Err(e) => return Err(EngineError::Llm(format!("MCP server '{}': {e}", server.name))),
            };
            out.push(ToolSource { server_name: server.name.clone(), client: Arc::new(client), tools });
        }
        Ok(out)
    }

    /// Run a prepared AI request with tool sources, persisting the run.
    pub async fn llm_execute(
        &self,
        request_id: &str,
        prepared: LlmPrepared,
        sources: &[ToolSource],
        approver: &dyn Approver,
        on_event: &mut (dyn FnMut(AgentEvent) + Send),
    ) -> Result<Doc<LlmRun>> {
        let mut tool_calls: Vec<Value> = vec![];
        let mut ttft = None;
        let mut capture = |e: AgentEvent| {
            match &e {
                AgentEvent::ToolCall { call, .. } => tool_calls.push(json!({ "call": call })),
                AgentEvent::ToolResult { result, .. } => {
                    if let Some(entry) = tool_calls.iter_mut().rev().find(|c| c["call"]["id"] == json!(result.id)) {
                        entry["result"] = serde_json::to_value(result).unwrap_or(Value::Null);
                    }
                }
                AgentEvent::AssistantMessage { ttft_ms, turn: 1, .. } => ttft = *ttft_ms,
                _ => {}
            }
            on_event(e);
        };
        let outcome: AgentOutcome =
            irs_llm::agent::run(&prepared.config, prepared.chat.clone(), sources, &prepared.options, approver, &mut capture).await;
        let run = LlmRun {
            provider_name: prepared.provider.name.clone(),
            model: prepared.chat.model.clone(),
            transcript: outcome.messages.iter().map(|m| serde_json::to_value(m).unwrap_or(Value::Null)).collect(),
            tool_calls,
            input_tokens: outcome.usage.input_tokens,
            output_tokens: outcome.usage.output_tokens,
            turns: outcome.turns,
            stop_reason: serde_json::to_value(outcome.stop_reason).ok().and_then(|v| v.as_str().map(str::to_string)).unwrap_or_default(),
            ttft_ms: ttft,
            total_ms: outcome.total_ms,
            error: outcome.error.clone(),
            request_bodies: outcome.request_bodies.clone(),
        };
        let max_history = self.store.settings()?.max_history_per_request.max(1);
        Ok(self.store.batch(|tx| {
            let doc = tx.insert(Some(request_id), run)?;
            let mut history = tx.children::<LlmRun>(request_id)?;
            history.sort_by_key(|r| std::cmp::Reverse(r.meta.created));
            for old in history.into_iter().skip(max_history) {
                tx.delete(old.id())?;
            }
            Ok(doc)
        })?)
    }

    /// Convenience: prepare, connect MCP servers, run and persist.
    pub async fn llm_run(
        &self,
        request_id: &str,
        approver: &dyn Approver,
        on_event: &mut (dyn FnMut(AgentEvent) + Send),
    ) -> Result<Doc<LlmRun>> {
        let prepared = self.llm_prepare(request_id)?;
        let sources = self.connect_tool_sources(&prepared.mcp_server_ids).await?;
        let run = self.llm_execute(request_id, prepared, &sources, approver, on_event).await;
        for s in &sources {
            s.client.close().await;
        }
        run
    }

    pub fn llm_runs(&self, request_id: &str) -> Result<Vec<Doc<LlmRun>>> {
        let mut r = self.store.children::<LlmRun>(request_id)?;
        r.sort_by_key(|r| std::cmp::Reverse(r.meta.created));
        Ok(r)
    }
}
