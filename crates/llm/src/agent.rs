//! LLM ↔ MCP bridge: expose MCP tools to a model, execute the calls it makes
//! (with an approval gate), feed results back, repeat until it answers.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use futures::future::BoxFuture;
use lsock_mcp::{Client, Hints, Tool};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{
    Block, ChatRequest, LlmError, Message, ProviderConfig, Role, StopReason, StreamEvent, ToolSpec,
    Usage, ms_since, sanitize_tool_name, stream_chat,
};

/// MCP tools from one connected server.
pub struct ToolSource {
    pub server_name: String,
    pub client: Arc<Client>,
    pub tools: Vec<Tool>,
}

/// Which tool calls run without asking.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum AutoApprove {
    /// Ask for every call.
    None,
    /// Run read-only tools automatically, ask for anything that may write.
    #[default]
    ReadOnly,
    /// Run everything (use only with trusted servers).
    All,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ToolCallInfo {
    pub id: String,
    /// Name the model used.
    pub name: String,
    pub server: String,
    /// Original MCP tool name.
    pub tool: String,
    pub input: Value,
    pub hints: Option<Hints>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ToolResultInfo {
    pub id: String,
    pub is_error: bool,
    pub denied: bool,
    /// What the model receives.
    pub text: String,
    pub structured: Option<Value>,
    pub latency_ms: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Approval {
    Allow,
    Deny(String),
}

pub trait Approver: Send + Sync {
    fn approve(&self, call: ToolCallInfo) -> BoxFuture<'static, Approval>;
}

/// Approves everything (CLI `--yes`, tests).
pub struct AllowAll;
impl Approver for AllowAll {
    fn approve(&self, _call: ToolCallInfo) -> BoxFuture<'static, Approval> {
        Box::pin(async { Approval::Allow })
    }
}

#[derive(Debug, Clone)]
pub struct AgentOptions {
    pub max_turns: u32,
    pub auto_approve: AutoApprove,
    pub cancel: Option<Arc<AtomicBool>>,
    /// Truncate tool output sent to the model.
    pub max_tool_result_chars: usize,
}

impl Default for AgentOptions {
    fn default() -> Self {
        Self {
            max_turns: 8,
            auto_approve: AutoApprove::ReadOnly,
            cancel: None,
            max_tool_result_chars: 100_000,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum AgentEvent {
    TurnStart {
        turn: u32,
    },
    Stream {
        turn: u32,
        event: StreamEvent,
    },
    AssistantMessage {
        turn: u32,
        message: Message,
        stop_reason: StopReason,
        usage: Usage,
        ttft_ms: Option<f64>,
        total_ms: f64,
    },
    ToolCall {
        turn: u32,
        call: ToolCallInfo,
        needs_approval: bool,
    },
    ToolResult {
        turn: u32,
        result: ToolResultInfo,
    },
    Done {
        usage: Usage,
        turns: u32,
        stop_reason: StopReason,
    },
    Error {
        message: String,
    },
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentOutcome {
    /// Full conversation including the new assistant/tool messages.
    pub messages: Vec<Message>,
    pub usage: Usage,
    pub turns: u32,
    pub stop_reason: StopReason,
    pub error: Option<String>,
    pub total_ms: f64,
    /// Request bodies sent to the provider, one per turn (for the trace view).
    pub request_bodies: Vec<Value>,
}

struct Mapped {
    source: usize,
    tool: Tool,
}

/// Build provider tool specs from MCP tools, keeping names unique and valid.
pub fn tool_specs(sources: &[ToolSource]) -> (Vec<ToolSpec>, HashMap<String, (usize, Tool)>) {
    let multi = sources.len() > 1;
    let mut specs = vec![];
    let mut map: HashMap<String, (usize, Tool)> = HashMap::new();
    for (si, s) in sources.iter().enumerate() {
        for t in &s.tools {
            let base = if multi {
                format!("{}__{}", sanitize_tool_name(&s.server_name), t.name)
            } else {
                t.name.clone()
            };
            let mut name = sanitize_tool_name(&base);
            let mut n = 2;
            while map.contains_key(&name) {
                let suffix = format!("_{n}");
                name = format!(
                    "{}{suffix}",
                    &sanitize_tool_name(&base)
                        [..sanitize_tool_name(&base).len().min(64 - suffix.len())]
                );
                n += 1;
            }
            let h = t.hints();
            let mut desc = t.description.clone().unwrap_or_default();
            let tag = if h.read_only {
                "read-only"
            } else if h.destructive {
                "destructive"
            } else {
                "modifies state"
            };
            if multi {
                desc = format!("[{} · {tag}] {desc}", s.server_name);
            } else {
                desc = format!("[{tag}] {desc}");
            }
            let mut schema = t.input_schema.clone();
            if schema.get("type").is_none() {
                schema["type"] = json!("object");
            }
            specs.push(ToolSpec {
                name: name.clone(),
                description: desc.trim().to_string(),
                input_schema: schema,
            });
            map.insert(name, (si, t.clone()));
        }
    }
    (specs, map)
}

fn result_text(r: &lsock_mcp::CallToolResult, max: usize) -> String {
    let mut parts = vec![];
    for c in &r.content {
        match c["type"].as_str() {
            Some("text") => parts.push(c["text"].as_str().unwrap_or("").to_string()),
            Some("image") | Some("audio") => parts.push(format!(
                "[{} {}]",
                c["type"].as_str().unwrap_or(""),
                c["mimeType"].as_str().unwrap_or("")
            )),
            Some("resource") => {
                let res = &c["resource"];
                parts.push(format!(
                    "[resource {}]\n{}",
                    res["uri"].as_str().unwrap_or(""),
                    res["text"].as_str().unwrap_or("")
                ));
            }
            Some("resource_link") => parts.push(format!(
                "[resource link {}]",
                c["uri"].as_str().unwrap_or("")
            )),
            _ => parts.push(c.to_string()),
        }
    }
    if parts.iter().all(|p| p.trim().is_empty())
        && let Some(s) = &r.structured_content
    {
        parts = vec![s.to_string()];
    }
    let mut text = parts.join("\n");
    if text.chars().count() > max {
        text = format!(
            "{}… [truncated]",
            text.chars().take(max).collect::<String>()
        );
    }
    text
}

fn needs_approval(policy: AutoApprove, hints: Option<Hints>) -> bool {
    match policy {
        AutoApprove::All => false,
        AutoApprove::None => true,
        AutoApprove::ReadOnly => !hints.is_some_and(|h| h.read_only),
    }
}

/// Run the agent loop. `req.tools` is replaced by the MCP tools from `sources`.
pub async fn run(
    cfg: &ProviderConfig,
    mut req: ChatRequest,
    sources: &[ToolSource],
    opts: &AgentOptions,
    approver: &dyn Approver,
    on_event: &mut (dyn FnMut(AgentEvent) + Send),
) -> AgentOutcome {
    let started = Instant::now();
    let (specs, map) = tool_specs(sources);
    let map: HashMap<String, Mapped> = map
        .into_iter()
        .map(|(k, (source, tool))| (k, Mapped { source, tool }))
        .collect();
    req.tools = specs;
    let mut out = AgentOutcome::default();
    let cancelled = || {
        opts.cancel
            .as_ref()
            .is_some_and(|c| c.load(Ordering::SeqCst))
    };

    for turn in 1..=opts.max_turns.max(1) {
        if cancelled() {
            out.error = Some(LlmError::Cancelled.to_string());
            break;
        }
        on_event(AgentEvent::TurnStart { turn });
        out.turns = turn;
        let mut forward = |e: StreamEvent| on_event(AgentEvent::Stream { turn, event: e });
        let resp = match stream_chat(cfg, &req, &mut forward).await {
            Ok(r) => r,
            Err(e) => {
                on_event(AgentEvent::Error {
                    message: e.to_string(),
                });
                out.error = Some(e.to_string());
                break;
            }
        };
        out.usage += resp.usage;
        out.stop_reason = resp.stop_reason;
        out.request_bodies.push(resp.request_body.clone());
        let assistant = Message {
            role: Role::Assistant,
            content: resp.content.clone(),
        };
        on_event(AgentEvent::AssistantMessage {
            turn,
            message: assistant.clone(),
            stop_reason: resp.stop_reason,
            usage: resp.usage,
            ttft_ms: resp.ttft_ms,
            total_ms: resp.total_ms,
        });
        req.messages.push(assistant);

        let calls: Vec<(String, String, Value)> = resp
            .content
            .iter()
            .filter_map(|b| {
                if let Block::ToolUse { id, name, input } = b {
                    Some((id.clone(), name.clone(), input.clone()))
                } else {
                    None
                }
            })
            .collect();
        if calls.is_empty() {
            break;
        }
        if turn == opts.max_turns.max(1) {
            // Out of turns: answer the calls so the transcript stays valid, then stop.
            let results = calls
                .iter()
                .map(|(id, _, _)| Block::ToolResult {
                    tool_use_id: id.clone(),
                    content: "Stopped: maximum number of turns reached.".into(),
                    is_error: true,
                })
                .collect();
            req.messages.push(Message {
                role: Role::User,
                content: results,
            });
            out.error = Some(format!(
                "stopped after {} turns with tool calls still pending",
                opts.max_turns
            ));
            break;
        }

        let mut results = vec![];
        for (id, name, input) in calls {
            let mapped = map.get(&name);
            let info = ToolCallInfo {
                id: id.clone(),
                name: name.clone(),
                server: mapped
                    .map(|m| sources[m.source].server_name.clone())
                    .unwrap_or_default(),
                tool: mapped
                    .map(|m| m.tool.name.clone())
                    .unwrap_or_else(|| name.clone()),
                input: input.clone(),
                hints: mapped.map(|m| m.tool.hints()),
            };
            let ask = mapped.is_some() && needs_approval(opts.auto_approve, info.hints);
            on_event(AgentEvent::ToolCall {
                turn,
                call: info.clone(),
                needs_approval: ask,
            });
            let t = Instant::now();
            let result = match mapped {
                None => ToolResultInfo {
                    id: id.clone(),
                    is_error: true,
                    denied: false,
                    text: format!(
                        "Unknown tool '{name}'. Available: {}",
                        map.keys().cloned().collect::<Vec<_>>().join(", ")
                    ),
                    structured: None,
                    latency_ms: 0.0,
                },
                Some(m) => {
                    let approval = if ask {
                        approver.approve(info.clone()).await
                    } else {
                        Approval::Allow
                    };
                    match approval {
                        Approval::Deny(reason) => ToolResultInfo {
                            id: id.clone(),
                            is_error: true,
                            denied: true,
                            text: if reason.is_empty() {
                                "The user declined this tool call.".into()
                            } else {
                                format!("The user declined this tool call: {reason}")
                            },
                            structured: None,
                            latency_ms: 0.0,
                        },
                        Approval::Allow => match sources[m.source]
                            .client
                            .call_tool(&m.tool.name, input)
                            .await
                        {
                            Ok(r) => ToolResultInfo {
                                id: id.clone(),
                                is_error: r.is_error,
                                denied: false,
                                text: result_text(&r, opts.max_tool_result_chars),
                                structured: r.structured_content.clone(),
                                latency_ms: ms_since(t),
                            },
                            Err(e) => ToolResultInfo {
                                id: id.clone(),
                                is_error: true,
                                denied: false,
                                text: format!("Tool call failed: {e}"),
                                structured: None,
                                latency_ms: ms_since(t),
                            },
                        },
                    }
                }
            };
            on_event(AgentEvent::ToolResult {
                turn,
                result: result.clone(),
            });
            results.push(Block::ToolResult {
                tool_use_id: id,
                content: result.text,
                is_error: result.is_error,
            });
        }
        req.messages.push(Message {
            role: Role::User,
            content: results,
        });
    }
    out.total_ms = ms_since(started);
    on_event(AgentEvent::Done {
        usage: out.usage,
        turns: out.turns,
        stop_reason: out.stop_reason,
    });
    out.messages = req.messages;
    out
}

// ---------------------------------------------------------------- MCP sampling

/// Answers MCP `sampling/createMessage` requests with an LLM.
pub struct LlmSampler {
    pub cfg: ProviderConfig,
    pub model: String,
    pub max_tokens: u32,
}

impl lsock_mcp::SamplingHandler for LlmSampler {
    fn create_message(&self, params: Value) -> BoxFuture<'static, Result<Value, String>> {
        let cfg = self.cfg.clone();
        let default_model = self.model.clone();
        let cap = self.max_tokens;
        Box::pin(async move {
            let messages: Vec<Message> = params["messages"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|m| {
                    let role = if m["role"] == "assistant" {
                        Role::Assistant
                    } else {
                        Role::User
                    };
                    let blocks: Vec<&Value> = match &m["content"] {
                        Value::Array(a) => a.iter().collect(),
                        v => vec![v],
                    };
                    let content = blocks
                        .into_iter()
                        .map(|c| match c["type"].as_str() {
                            Some("text") => Block::Text {
                                text: c["text"].as_str().unwrap_or("").into(),
                            },
                            _ => Block::Text {
                                text: format!(
                                    "[{} content omitted]",
                                    c["type"].as_str().unwrap_or("non-text")
                                ),
                            },
                        })
                        .collect();
                    Message { role, content }
                })
                .collect();
            if messages.is_empty() {
                return Err("sampling request has no messages".into());
            }
            let req = ChatRequest {
                model: default_model,
                system: params["systemPrompt"].as_str().map(str::to_string),
                messages,
                tools: vec![],
                max_tokens: params["maxTokens"]
                    .as_u64()
                    .map(|m| m as u32)
                    .unwrap_or(cap)
                    .min(cap.max(1)),
                temperature: params["temperature"].as_f64().map(|t| t as f32),
            };
            let resp = stream_chat(&cfg, &req, &mut |_| {})
                .await
                .map_err(|e| e.to_string())?;
            let text = Message {
                role: Role::Assistant,
                content: resp.content,
            }
            .text();
            Ok(json!({
                "role": "assistant",
                "content": { "type": "text", "text": text },
                "model": resp.model,
                "stopReason": match resp.stop_reason {
                    StopReason::MaxTokens => "maxTokens",
                    StopReason::StopSequence => "stopSequence",
                    _ => "endTurn",
                },
            }))
        })
    }
}
