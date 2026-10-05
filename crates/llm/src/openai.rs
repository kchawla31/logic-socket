//! OpenAI Chat Completions (`POST {base}/chat/completions`, `stream: true`).
//! Also used for Ollama, OpenRouter, Gemini's OpenAI endpoint, LM Studio, vLLM.

use std::collections::BTreeMap;
use std::time::Instant;

use futures::StreamExt;
use serde_json::{Value, json};

use crate::sse::SseParser;
use crate::{
    Block, ChatRequest, ChatResponse, LlmError, ProviderConfig, ProviderKind, Role, StopReason,
    StreamEvent, api_error, auth, ms_since,
};

pub(crate) fn body(kind: ProviderKind, req: &ChatRequest) -> Value {
    let mut messages: Vec<Value> = vec![];
    if let Some(s) = req.system.as_ref().filter(|s| !s.trim().is_empty()) {
        messages.push(json!({ "role": "system", "content": s }));
    }
    for m in &req.messages {
        match m.role {
            Role::Assistant => {
                let text: String = m
                    .content
                    .iter()
                    .filter_map(|b| {
                        if let Block::Text { text } = b {
                            Some(text.as_str())
                        } else {
                            None
                        }
                    })
                    .collect();
                let calls: Vec<Value> = m
                    .content
                    .iter()
                    .filter_map(|b| match b {
                        Block::ToolUse { id, name, input } => Some(json!({
                            "id": id, "type": "function",
                            "function": { "name": name, "arguments": input.to_string() },
                        })),
                        _ => None,
                    })
                    .collect();
                let mut msg = json!({ "role": "assistant", "content": if text.is_empty() && !calls.is_empty() { Value::Null } else { json!(text) } });
                if !calls.is_empty() {
                    msg["tool_calls"] = Value::Array(calls);
                }
                messages.push(msg);
            }
            Role::User => {
                // tool results become separate `tool` messages; text stays a user message
                for b in &m.content {
                    if let Block::ToolResult {
                        tool_use_id,
                        content,
                        is_error,
                    } = b
                    {
                        let content = if *is_error {
                            format!("Error: {content}")
                        } else {
                            content.clone()
                        };
                        messages.push(json!({ "role": "tool", "tool_call_id": tool_use_id, "content": content }));
                    }
                }
                let text: String = m
                    .content
                    .iter()
                    .filter_map(|b| {
                        if let Block::Text { text } = b {
                            Some(text.as_str())
                        } else {
                            None
                        }
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                if !text.is_empty() {
                    messages.push(json!({ "role": "user", "content": text }));
                }
            }
        }
    }
    let mut b = json!({ "model": req.model, "messages": messages, "stream": true });
    // Newer OpenAI models take max_completion_tokens; compatible servers expect max_tokens.
    let key = if kind == ProviderKind::Openai {
        "max_completion_tokens"
    } else {
        "max_tokens"
    };
    b[key] = json!(req.max_tokens.max(1));
    if kind != ProviderKind::Ollama {
        b["stream_options"] = json!({ "include_usage": true });
    }
    if let Some(t) = req.temperature {
        b["temperature"] = json!(t);
    }
    if !req.tools.is_empty() {
        b["tools"] = Value::Array(
            req.tools
                .iter()
                .map(|t| json!({ "type": "function", "function": { "name": t.name, "description": t.description, "parameters": t.input_schema } }))
                .collect(),
        );
    }
    b
}

pub(crate) async fn stream(
    cfg: &ProviderConfig,
    req: &ChatRequest,
    on_event: &mut (dyn FnMut(StreamEvent) + Send),
    started: Instant,
) -> Result<ChatResponse, LlmError> {
    let request_body = body(cfg.kind, req);
    let rb = auth(cfg, cfg.client()?.post(cfg.url("chat/completions"))).json(&request_body);
    let resp = rb
        .send()
        .await
        .map_err(|e| LlmError::Network(e.to_string()))?;
    let status = resp.status();
    if !status.is_success() {
        let v: Value = resp.json().await.unwrap_or(Value::Null);
        return Err(api_error(cfg, status.as_u16(), &v));
    }

    let mut out = ChatResponse {
        model: req.model.clone(),
        request_body,
        ..Default::default()
    };
    let mut text = String::new();
    // tool calls by index: (id, name, arguments)
    let mut calls: BTreeMap<u64, (String, String, String)> = BTreeMap::new();
    let mut parser = SseParser::default();
    let mut stream = resp.bytes_stream();
    'read: while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| LlmError::Stream(e.to_string()))?;
        for ev in parser.feed(&chunk) {
            if ev.data.trim() == "[DONE]" {
                break 'read;
            }
            let v: Value = serde_json::from_str(&ev.data)
                .map_err(|e| LlmError::Stream(format!("bad chunk JSON: {e}")))?;
            if let Some(err) = v.get("error") {
                return Err(LlmError::Api {
                    provider: format!("{:?}", cfg.kind),
                    status: 200,
                    message: err["message"].as_str().unwrap_or("stream error").into(),
                });
            }
            if let Some(m) = v["model"].as_str() {
                out.model = m.to_string();
            }
            if let Some(u) = v.get("usage").filter(|u| u.is_object()) {
                out.usage.input_tokens = u["prompt_tokens"]
                    .as_u64()
                    .unwrap_or(out.usage.input_tokens);
                out.usage.output_tokens = u["completion_tokens"]
                    .as_u64()
                    .unwrap_or(out.usage.output_tokens);
            }
            let Some(choice) = v["choices"].get(0) else {
                continue;
            };
            let d = &choice["delta"];
            if let Some(s) = d["content"].as_str().filter(|s| !s.is_empty()) {
                out.ttft_ms.get_or_insert(ms_since(started));
                text.push_str(s);
                on_event(StreamEvent::TextDelta {
                    text: s.to_string(),
                });
            }
            if let Some(s) = d["reasoning_content"]
                .as_str()
                .or(d["reasoning"].as_str())
                .filter(|s| !s.is_empty())
            {
                out.ttft_ms.get_or_insert(ms_since(started));
                on_event(StreamEvent::ThinkingDelta {
                    text: s.to_string(),
                });
            }
            for tc in d["tool_calls"].as_array().into_iter().flatten() {
                out.ttft_ms.get_or_insert(ms_since(started));
                let i = tc["index"].as_u64().unwrap_or(0);
                let entry = calls
                    .entry(i)
                    .or_insert_with(|| (String::new(), String::new(), String::new()));
                if let Some(id) = tc["id"].as_str() {
                    entry.0 = id.to_string();
                }
                if let Some(n) = tc["function"]["name"].as_str() {
                    entry.1.push_str(n);
                    on_event(StreamEvent::ToolUseStart {
                        id: entry.0.clone(),
                        name: entry.1.clone(),
                    });
                }
                if let Some(a) = tc["function"]["arguments"].as_str() {
                    entry.2.push_str(a);
                    on_event(StreamEvent::ToolInputDelta {
                        id: entry.0.clone(),
                        partial_json: a.to_string(),
                    });
                }
            }
            if let Some(r) = choice["finish_reason"].as_str() {
                out.stop_reason = match r {
                    "stop" => StopReason::EndTurn,
                    "tool_calls" | "function_call" => StopReason::ToolUse,
                    "length" => StopReason::MaxTokens,
                    "content_filter" => StopReason::Refusal,
                    _ => StopReason::Other,
                };
            }
        }
    }
    if !text.is_empty() {
        out.content.push(Block::Text { text });
    }
    for (i, (id, name, args)) in calls {
        let id = if id.is_empty() {
            format!("call_{i}")
        } else {
            id
        };
        let input = if args.trim().is_empty() {
            json!({})
        } else {
            serde_json::from_str(&args).unwrap_or(json!({ "_raw": args }))
        };
        out.content.push(Block::ToolUse { id, name, input });
    }
    if out
        .content
        .iter()
        .any(|b| matches!(b, Block::ToolUse { .. }))
        && out.stop_reason == StopReason::EndTurn
    {
        out.stop_reason = StopReason::ToolUse; // some servers report "stop" with tool calls
    }
    out.total_ms = ms_since(started);
    Ok(out)
}
