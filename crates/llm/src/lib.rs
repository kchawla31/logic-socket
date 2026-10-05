//! LLM providers (Anthropic Messages + OpenAI-compatible Chat Completions),
//! streaming, and an agent loop that lets a model call MCP tools.
//!
//! Content is kept in a provider-neutral [`Block`] form so transcripts can be
//! stored, shown in the UI, and replayed to either provider.

pub mod agent;
mod anthropic;
pub mod mock;
mod openai;
mod sse;

use std::time::Instant;

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, thiserror::Error, PartialEq)]
pub enum LlmError {
    #[error("{0}")]
    Config(String),
    #[error("network error: {0}")]
    Network(String),
    #[error("{provider} API error {status}: {message}")]
    Api {
        provider: String,
        status: u16,
        message: String,
    },
    #[error("stream error: {0}")]
    Stream(String),
    #[error("cancelled")]
    Cancelled,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "kebab-case")]
pub enum ProviderKind {
    #[default]
    Anthropic,
    Openai,
    Ollama,
    /// Any OpenAI Chat Completions–compatible endpoint (OpenRouter, Gemini, LM Studio, vLLM, ...).
    OpenaiCompatible,
}

impl ProviderKind {
    pub fn default_base_url(self) -> &'static str {
        match self {
            ProviderKind::Anthropic => "https://api.anthropic.com",
            ProviderKind::Openai => "https://api.openai.com/v1",
            ProviderKind::Ollama => "http://localhost:11434/v1",
            ProviderKind::OpenaiCompatible => "",
        }
    }

    /// Suggestions shown before the live model list loads.
    pub fn suggested_models(self) -> &'static [&'static str] {
        match self {
            ProviderKind::Anthropic => &[
                "claude-opus-5-5",
                "claude-sonnet-5-5",
                "claude-haiku-4-5-20251001",
                "claude-fable-5-1",
            ],
            ProviderKind::Openai => &["gpt-5.6", "gpt-5.6-terra", "gpt-5.6-luna"],
            ProviderKind::Ollama => &["llama3.3", "qwen2.5-coder"],
            ProviderKind::OpenaiCompatible => &[],
        }
    }

    pub fn needs_key(self) -> bool {
        !matches!(self, ProviderKind::Ollama)
    }
}

/// Connection settings for one provider, with the API key already resolved.
#[derive(Debug, Clone)]
pub struct ProviderConfig {
    pub kind: ProviderKind,
    pub base_url: String,
    pub api_key: Option<String>,
    pub headers: Vec<(String, String)>,
    pub timeout: std::time::Duration,
}

impl ProviderConfig {
    pub fn new(kind: ProviderKind, api_key: Option<String>) -> Self {
        Self {
            kind,
            base_url: kind.default_base_url().to_string(),
            api_key,
            headers: vec![],
            timeout: std::time::Duration::from_secs(300),
        }
    }

    pub(crate) fn url(&self, path: &str) -> String {
        format!(
            "{}/{}",
            self.base_url.trim_end_matches('/'),
            path.trim_start_matches('/')
        )
    }

    pub(crate) fn client(&self) -> Result<reqwest::Client, LlmError> {
        reqwest::Client::builder()
            .connect_timeout(std::time::Duration::from_secs(20))
            .timeout(self.timeout)
            .build()
            .map_err(|e| LlmError::Config(e.to_string()))
    }

    fn check(&self) -> Result<(), LlmError> {
        if self.base_url.trim().is_empty() {
            return Err(LlmError::Config("provider base URL is empty".into()));
        }
        if self.kind.needs_key()
            && self.api_key.as_deref().unwrap_or("").is_empty()
            && self.kind != ProviderKind::OpenaiCompatible
        {
            return Err(LlmError::Config(format!(
                "no API key configured for {:?}",
                self.kind
            )));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    User,
    Assistant,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Block {
    Text {
        text: String,
    },
    ToolUse {
        id: String,
        name: String,
        input: Value,
    },
    ToolResult {
        tool_use_id: String,
        content: String,
        #[serde(default)]
        is_error: bool,
    },
    /// Model reasoning (Anthropic). Kept so it can be sent back with tool results.
    Thinking {
        thinking: String,
        #[serde(default)]
        signature: Option<String>,
    },
    /// Provider-specific block preserved verbatim (e.g. redacted thinking).
    Raw {
        raw: Value,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Message {
    pub role: Role,
    pub content: Vec<Block>,
}

impl Message {
    pub fn user(text: impl Into<String>) -> Self {
        Self {
            role: Role::User,
            content: vec![Block::Text { text: text.into() }],
        }
    }
    pub fn assistant(text: impl Into<String>) -> Self {
        Self {
            role: Role::Assistant,
            content: vec![Block::Text { text: text.into() }],
        }
    }
    pub fn text(&self) -> String {
        self.content
            .iter()
            .filter_map(|b| {
                if let Block::Text { text } = b {
                    Some(text.as_str())
                } else {
                    None
                }
            })
            .collect::<Vec<_>>()
            .join("")
    }
}

/// A tool the model may call (JSON Schema input).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ChatRequest {
    pub model: String,
    pub system: Option<String>,
    pub messages: Vec<Message>,
    pub tools: Vec<ToolSpec>,
    pub max_tokens: u32,
    pub temperature: Option<f32>,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum StopReason {
    #[default]
    EndTurn,
    ToolUse,
    MaxTokens,
    StopSequence,
    Refusal,
    Other,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Usage {
    pub input_tokens: u64,
    pub output_tokens: u64,
}

impl std::ops::AddAssign for Usage {
    fn add_assign(&mut self, o: Self) {
        self.input_tokens += o.input_tokens;
        self.output_tokens += o.output_tokens;
    }
}

/// Incremental output while a response streams.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum StreamEvent {
    TextDelta { text: String },
    ThinkingDelta { text: String },
    ToolUseStart { id: String, name: String },
    ToolInputDelta { id: String, partial_json: String },
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ChatResponse {
    pub model: String,
    pub content: Vec<Block>,
    pub stop_reason: StopReason,
    pub usage: Usage,
    /// Time to first streamed token.
    pub ttft_ms: Option<f64>,
    pub total_ms: f64,
    /// Exact JSON body sent to the provider (API key never included).
    pub request_body: Value,
}

/// Send `req` and stream the answer. `on_event` sees deltas as they arrive.
pub async fn stream_chat(
    cfg: &ProviderConfig,
    req: &ChatRequest,
    on_event: &mut (dyn FnMut(StreamEvent) + Send),
) -> Result<ChatResponse, LlmError> {
    cfg.check()?;
    let started = Instant::now();
    match cfg.kind {
        ProviderKind::Anthropic => anthropic::stream(cfg, req, on_event, started).await,
        _ => openai::stream(cfg, req, on_event, started).await,
    }
}

/// Model ids available to this key (falls back to suggestions on failure).
pub async fn list_models(cfg: &ProviderConfig) -> Result<Vec<String>, LlmError> {
    cfg.check()?;
    let client = cfg.client()?;
    let mut rb = client.get(cfg.url(if cfg.kind == ProviderKind::Anthropic {
        "v1/models?limit=100"
    } else {
        "models"
    }));
    rb = auth(cfg, rb);
    let resp = rb
        .send()
        .await
        .map_err(|e| LlmError::Network(e.to_string()))?;
    let status = resp.status();
    let body: Value = resp
        .json()
        .await
        .map_err(|e| LlmError::Network(e.to_string()))?;
    if !status.is_success() {
        return Err(api_error(cfg, status.as_u16(), &body));
    }
    let mut ids: Vec<String> = body["data"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|m| m["id"].as_str().map(str::to_string))
        .collect();
    ids.sort();
    Ok(ids)
}

pub(crate) fn auth(
    cfg: &ProviderConfig,
    mut rb: reqwest::RequestBuilder,
) -> reqwest::RequestBuilder {
    let key = cfg.api_key.clone().unwrap_or_default();
    match cfg.kind {
        ProviderKind::Anthropic => {
            rb = rb
                .header("x-api-key", key)
                .header("anthropic-version", "2023-06-01");
        }
        _ if !key.is_empty() => {
            rb = rb.bearer_auth(key);
        }
        _ => {}
    }
    for (k, v) in &cfg.headers {
        rb = rb.header(k, v);
    }
    rb
}

pub(crate) fn api_error(cfg: &ProviderConfig, status: u16, body: &Value) -> LlmError {
    let message = body["error"]["message"]
        .as_str()
        .or(body["message"].as_str())
        .or(body["error"].as_str())
        .map(str::to_string)
        .unwrap_or_else(|| body.to_string());
    let hint = match status {
        401 | 403 => " — check the API key",
        404 => " — check the base URL and model name",
        429 => " — rate limited; retry later",
        _ => "",
    };
    LlmError::Api {
        provider: format!("{:?}", cfg.kind),
        status,
        message: format!("{message}{hint}"),
    }
}

pub(crate) fn ms_since(t: Instant) -> f64 {
    (t.elapsed().as_secs_f64() * 1000.0 * 100.0).round() / 100.0
}

/// Make an MCP tool name valid for providers (`^[a-zA-Z0-9_-]{1,64}$`).
pub fn sanitize_tool_name(name: &str) -> String {
    let s: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let s = if s.is_empty() { "tool".to_string() } else { s };
    s.chars().take(64).collect()
}

#[cfg(test)]
mod tests;
