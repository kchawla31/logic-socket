//! Anthropic Messages API (`POST /v1/messages`, `stream: true`).

use std::collections::BTreeMap;
use std::time::Instant;

use futures::StreamExt;
use serde_json::{Value, json};

use crate::sse::SseParser;
use crate::{
    Block, ChatRequest, ChatResponse, LlmError, ProviderConfig, Role, StopReason, StreamEvent, Usage, api_error, auth,
    ms_since,
};

pub(crate) fn block_to_wire(b: &Block) -> Value {
    match b {
        Block::Text { text } => json!({ "type": "text", "text": text }),
        Block::ToolUse { id, name, input } => json!({ "type": "tool_use", "id": id, "name": name, "input": input }),
        Block::ToolResult { tool_use_id, content, is_error } => {
            json!({ "type": "tool_result", "tool_use_id": tool_use_id, "content": content, "is_error": is_error })
        }
        Block::Thinking { thinking, signature } => {
            let mut v = json!({ "type": "thinking", "thinking": thinking });
            if let Some(s) = signature {
                v["signature"] = json!(s);
            }
            v
        }
        Block::Raw { raw } => raw.clone(),
    }
}

pub(crate) fn body(req: &ChatRequest) -> Value {
    let messages: Vec<Value> = req
        .messages
        .iter()
        .filter(|m| !m.content.is_empty())
        .map(|m| {
            json!({
                "role": match m.role { Role::User => "user", Role::Assistant => "assistant" },
                "content": m.content.iter().map(block_to_wire).collect::<Vec<_>>(),
            })
        })
        .collect();
    let mut b = json!({
        "model": req.model,
        "max_tokens": req.max_tokens.max(1),
        "messages": messages,
        "stream": true,
    });
    if let Some(s) = req.system.as_ref().filter(|s| !s.trim().is_empty()) {
        b["system"] = json!(s);
    }
    if let Some(t) = req.temperature {
        b["temperature"] = json!(t);
    }
    if !req.tools.is_empty() {
        b["tools"] = Value::Array(
            req.tools
                .iter()
                .map(|t| json!({ "name": t.name, "description": t.description, "input_schema": t.input_schema }))
                .collect(),
        );
    }
    b
}

/// Block being assembled from streaming deltas.
enum Partial {
    Text(String),
    Tool { id: String, name: String, json: String },
    Thinking { text: String, signature: Option<String> },
    Raw(Value),
}

pub(crate) async fn stream(
    cfg: &ProviderConfig,
    req: &ChatRequest,
    on_event: &mut (dyn FnMut(StreamEvent) + Send),
    started: Instant,
) -> Result<ChatResponse, LlmError> {
    let request_body = body(req);
    let rb = auth(cfg, cfg.client()?.post(cfg.url("v1/messages"))).json(&request_body);
    let resp = rb.send().await.map_err(|e| LlmError::Network(e.to_string()))?;
    let status = resp.status();
    if !status.is_success() {
        let v: Value = resp.json().await.unwrap_or(Value::Null);
        return Err(api_error(cfg, status.as_u16(), &v));
    }

    let mut out = ChatResponse { model: req.model.clone(), request_body, ..Default::default() };
    let mut blocks: BTreeMap<u64, Partial> = BTreeMap::new();
    let mut parser = SseParser::default();
    let mut stream = resp.bytes_stream();
    let mut done = false;
    while !done {
        let Some(chunk) = stream.next().await else { break };
        let chunk = chunk.map_err(|e| LlmError::Stream(e.to_string()))?;
        for ev in parser.feed(&chunk) {
            let v: Value = serde_json::from_str(&ev.data).map_err(|e| LlmError::Stream(format!("bad event JSON: {e}")))?;
            let idx = v["index"].as_u64().unwrap_or(0);
            match v["type"].as_str().unwrap_or(ev.event.as_str()) {
                "message_start" => {
                    let m = &v["message"];
                    if let Some(model) = m["model"].as_str() {
                        out.model = model.to_string();
                    }
                    out.usage.input_tokens = m["usage"]["input_tokens"].as_u64().unwrap_or(0);
                    out.usage.output_tokens = m["usage"]["output_tokens"].as_u64().unwrap_or(0);
                }
                "content_block_start" => {
                    let cb = &v["content_block"];
                    let p = match cb["type"].as_str() {
                        Some("text") => Partial::Text(cb["text"].as_str().unwrap_or("").to_string()),
                        Some("tool_use") => {
                            let id = cb["id"].as_str().unwrap_or("").to_string();
                            let name = cb["name"].as_str().unwrap_or("").to_string();
                            on_event(StreamEvent::ToolUseStart { id: id.clone(), name: name.clone() });
                            Partial::Tool { id, name, json: String::new() }
                        }
                        Some("thinking") => Partial::Thinking { text: cb["thinking"].as_str().unwrap_or("").into(), signature: None },
                        _ => Partial::Raw(cb.clone()),
                    };
                    blocks.insert(idx, p);
                }
                "content_block_delta" => {
                    let d = &v["delta"];
                    if out.ttft_ms.is_none() {
                        out.ttft_ms = Some(ms_since(started));
                    }
                    match (blocks.get_mut(&idx), d["type"].as_str()) {
                        (Some(Partial::Text(t)), Some("text_delta")) => {
                            let s = d["text"].as_str().unwrap_or("");
                            t.push_str(s);
                            on_event(StreamEvent::TextDelta { text: s.to_string() });
                        }
                        (Some(Partial::Tool { id, json, .. }), Some("input_json_delta")) => {
                            let s = d["partial_json"].as_str().unwrap_or("");
                            json.push_str(s);
                            on_event(StreamEvent::ToolInputDelta { id: id.clone(), partial_json: s.to_string() });
                        }
                        (Some(Partial::Thinking { text, .. }), Some("thinking_delta")) => {
                            let s = d["thinking"].as_str().unwrap_or("");
                            text.push_str(s);
                            on_event(StreamEvent::ThinkingDelta { text: s.to_string() });
                        }
                        (Some(Partial::Thinking { signature, .. }), Some("signature_delta")) => {
                            *signature = Some(format!("{}{}", signature.take().unwrap_or_default(), d["signature"].as_str().unwrap_or("")));
                        }
                        _ => {}
                    }
                }
                "message_delta" => {
                    out.stop_reason = match v["delta"]["stop_reason"].as_str() {
                        Some("end_turn") => StopReason::EndTurn,
                        Some("tool_use") => StopReason::ToolUse,
                        Some("max_tokens") => StopReason::MaxTokens,
                        Some("stop_sequence") => StopReason::StopSequence,
                        Some("refusal") => StopReason::Refusal,
                        Some(_) => StopReason::Other,
                        None => out.stop_reason,
                    };
                    if let Some(o) = v["usage"]["output_tokens"].as_u64() {
                        out.usage = Usage { input_tokens: out.usage.input_tokens, output_tokens: o };
                    }
                }
                "message_stop" => done = true,
                "error" => {
                    return Err(LlmError::Api {
                        provider: "Anthropic".into(),
                        status: 200,
                        message: v["error"]["message"].as_str().unwrap_or("stream error").to_string(),
                    });
                }
                _ => {} // ping, content_block_stop
            }
        }
    }
    out.content = blocks
        .into_values()
        .map(|p| match p {
            Partial::Text(text) => Block::Text { text },
            Partial::Tool { id, name, json } => Block::ToolUse {
                id,
                name,
                input: if json.trim().is_empty() { json!({}) } else { serde_json::from_str(&json).unwrap_or(json!({ "_raw": json })) },
            },
            Partial::Thinking { text, signature } => Block::Thinking { thinking: text, signature },
            Partial::Raw(raw) => Block::Raw { raw },
        })
        .collect();
    out.total_ms = ms_since(started);
    Ok(out)
}
