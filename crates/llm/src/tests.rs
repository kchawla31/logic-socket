use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use serde_json::json;

use crate::agent::{
    self, AgentEvent, AgentOptions, AllowAll, Approval, Approver, AutoApprove, ToolCallInfo,
    ToolSource,
};
use crate::mock::{MOCK_KEY, MockLlm};
use crate::*;

async fn provider(kind: ProviderKind) -> (ProviderConfig, MockLlm) {
    let mock = MockLlm::default();
    let base = mock::spawn(mock.clone(), 0).await.unwrap();
    let mut cfg = ProviderConfig::new(kind, Some(MOCK_KEY.into()));
    cfg.base_url = if kind == ProviderKind::Anthropic {
        base
    } else {
        format!("{base}/v1")
    };
    (cfg, mock)
}

fn req(text: &str) -> ChatRequest {
    ChatRequest {
        model: "mock-large".into(),
        system: Some("Be brief.".into()),
        messages: vec![Message::user(text)],
        max_tokens: 256,
        ..Default::default()
    }
}

#[tokio::test]
async fn streams_text_from_both_providers() {
    for kind in [
        ProviderKind::Anthropic,
        ProviderKind::OpenaiCompatible,
        ProviderKind::Openai,
    ] {
        let (cfg, mock) = provider(kind).await;
        let mut deltas = String::new();
        let r = stream_chat(&cfg, &req("hello there"), &mut |e| {
            if let StreamEvent::TextDelta { text } = e {
                deltas.push_str(&text)
            }
        })
        .await
        .unwrap();
        let text = Message {
            role: Role::Assistant,
            content: r.content.clone(),
        }
        .text();
        assert_eq!(text, "Echo: hello there", "{kind:?}");
        assert_eq!(deltas, text, "{kind:?} deltas add up");
        assert_eq!(r.stop_reason, StopReason::EndTurn);
        assert_eq!(
            r.usage,
            Usage {
                input_tokens: 12,
                output_tokens: 21
            }
        );
        assert!(r.ttft_ms.is_some() && r.total_ms >= r.ttft_ms.unwrap());
        let sent = mock.requests.lock().unwrap()[0].clone();
        assert_eq!(sent["stream"], true);
        match kind {
            ProviderKind::Anthropic => {
                assert_eq!(sent["system"], "Be brief.");
                assert_eq!(sent["max_tokens"], 256);
                assert!(
                    matches!(r.content[0], Block::Thinking { ref signature, .. } if signature.as_deref() == Some("sig-abc"))
                );
            }
            ProviderKind::Openai => assert_eq!(sent["max_completion_tokens"], 256),
            _ => {
                assert_eq!(sent["max_tokens"], 256);
                assert_eq!(
                    sent["messages"][0],
                    json!({"role": "system", "content": "Be brief."})
                );
            }
        }
        assert_eq!(r.request_body, sent, "trace shows the exact body");
    }
}

#[tokio::test]
async fn api_errors_are_readable_and_keys_never_sent_in_body() {
    for kind in [ProviderKind::Anthropic, ProviderKind::Openai] {
        let (mut cfg, mock) = provider(kind).await;
        cfg.api_key = Some("wrong".into());
        let err = stream_chat(&cfg, &req("hi"), &mut |_| {})
            .await
            .unwrap_err();
        match err {
            LlmError::Api {
                status: 401,
                message,
                ..
            } => assert!(message.contains("check the API key"), "{message}"),
            other => panic!("{other:?}"),
        }
        assert!(
            !mock.requests.lock().unwrap()[0]
                .to_string()
                .contains("wrong")
        );
    }
    let cfg = ProviderConfig::new(ProviderKind::Anthropic, None);
    assert!(matches!(
        stream_chat(&cfg, &req("x"), &mut |_| {}).await,
        Err(LlmError::Config(_))
    ));
    let mut cfg = ProviderConfig::new(ProviderKind::Openai, Some("k".into()));
    cfg.base_url = "http://127.0.0.1:1/v1".into();
    assert!(matches!(
        stream_chat(&cfg, &req("x"), &mut |_| {}).await,
        Err(LlmError::Network(_))
    ));
}

#[tokio::test]
async fn tool_use_round_trip_wire_format() {
    for kind in [ProviderKind::Anthropic, ProviderKind::OpenaiCompatible] {
        let (cfg, mock) = provider(kind).await;
        let mut r = req("what's the weather in Paris?");
        r.tools = vec![ToolSpec {
            name: "get_weather".into(),
            description: "Weather".into(),
            input_schema: json!({"type": "object"}),
        }];
        let resp = stream_chat(&cfg, &r, &mut |_| {}).await.unwrap();
        assert_eq!(resp.stop_reason, StopReason::ToolUse, "{kind:?}");
        let call = resp
            .content
            .iter()
            .find_map(|b| {
                if let Block::ToolUse { id, name, input } = b {
                    Some((id.clone(), name.clone(), input.clone()))
                } else {
                    None
                }
            })
            .unwrap();
        assert_eq!(
            (call.1.as_str(), call.2.clone()),
            ("get_weather", json!({"city": "Paris"}))
        );

        // send the result back; providers must see a well-formed tool turn
        r.messages.push(Message {
            role: Role::Assistant,
            content: resp.content.clone(),
        });
        r.messages.push(Message {
            role: Role::User,
            content: vec![Block::ToolResult {
                tool_use_id: call.0.clone(),
                content: "Paris: 18°C".into(),
                is_error: false,
            }],
        });
        let resp2 = stream_chat(&cfg, &r, &mut |_| {}).await.unwrap();
        assert_eq!(
            Message {
                role: Role::Assistant,
                content: resp2.content
            }
            .text(),
            "Based on the tool: Paris: 18°C"
        );
        let sent = mock.requests.lock().unwrap()[1].clone();
        if kind == ProviderKind::Anthropic {
            let asst = &sent["messages"][1]["content"];
            assert_eq!(
                asst[0],
                json!({"type": "thinking", "thinking": "Let me think.", "signature": "sig-abc"}),
                "thinking replayed with signature"
            );
            assert_eq!(asst[2]["type"], "tool_use");
            assert_eq!(sent["messages"][2]["content"][0]["type"], "tool_result");
            assert_eq!(sent["tools"][0]["input_schema"], json!({"type": "object"}));
        } else {
            let msgs = sent["messages"].as_array().unwrap();
            let asst = msgs.iter().find(|m| m["role"] == "assistant").unwrap();
            assert_eq!(
                asst["tool_calls"][0]["function"]["arguments"],
                json!(json!({"city": "Paris"}).to_string())
            );
            assert_eq!(
                msgs.last().unwrap(),
                &json!({"role": "tool", "tool_call_id": call.0, "content": "Paris: 18°C"})
            );
            assert_eq!(sent["tools"][0]["type"], "function");
        }
    }
}

#[tokio::test]
async fn lists_models() {
    let (cfg, _) = provider(ProviderKind::OpenaiCompatible).await;
    assert_eq!(
        list_models(&cfg).await.unwrap(),
        ["mock-large", "mock-small"]
    );
    let (cfg, _) = provider(ProviderKind::Anthropic).await;
    assert_eq!(list_models(&cfg).await.unwrap().len(), 2);
}

// ---------------------------------------------------------------- agent ↔ MCP

async fn mcp_source(name: &str) -> ToolSource {
    let url = lsock_mcp::mock::spawn_http(Default::default(), 0)
        .await
        .unwrap();
    let client = lsock_mcp::Client::connect(lsock_mcp::ConnectOptions::new(
        lsock_mcp::TransportConfig::Http {
            url,
            headers: vec![],
            validate_certificates: true,
        },
    ))
    .await
    .unwrap();
    let tools = client.list_tools().await.unwrap().items;
    ToolSource {
        server_name: name.into(),
        client: Arc::new(client),
        tools,
    }
}

#[tokio::test]
async fn agent_calls_mcp_tool_and_answers() {
    for kind in [ProviderKind::Anthropic, ProviderKind::OpenaiCompatible] {
        let (cfg, _) = provider(kind).await;
        let sources = vec![mcp_source("weather").await];
        let mut events = vec![];
        let out = agent::run(
            &cfg,
            req("What's the weather in Tokyo?"),
            &sources,
            &AgentOptions::default(),
            &AllowAll,
            &mut |e| events.push(e),
        )
        .await;
        assert_eq!(out.error, None, "{kind:?}");
        assert_eq!(out.turns, 2);
        let last = out.messages.last().unwrap();
        assert_eq!(last.text(), "Based on the tool: Tokyo: 21°C, clear sky");
        assert_eq!(
            out.usage,
            Usage {
                input_tokens: 24,
                output_tokens: 42
            }
        );
        let call = events
            .iter()
            .find_map(|e| {
                if let AgentEvent::ToolCall {
                    call,
                    needs_approval,
                    ..
                } = e
                {
                    Some((call.clone(), *needs_approval))
                } else {
                    None
                }
            })
            .unwrap();
        assert_eq!(
            (call.0.tool.as_str(), call.0.server.as_str(), call.1),
            ("get_weather", "weather", false),
            "read-only: no approval needed"
        );
        let result = events
            .iter()
            .find_map(|e| {
                if let AgentEvent::ToolResult { result, .. } = e {
                    Some(result.clone())
                } else {
                    None
                }
            })
            .unwrap();
        assert!(!result.is_error && result.latency_ms >= 0.0);
        assert!(matches!(
            events.last(),
            Some(AgentEvent::Done { turns: 2, .. })
        ));
        assert!(events.iter().any(|e| matches!(
            e,
            AgentEvent::Stream {
                event: StreamEvent::TextDelta { .. },
                ..
            }
        )));
        assert_eq!(out.request_bodies.len(), 2);
    }
}

struct Recorder {
    seen: Mutex<Vec<String>>,
    allow: bool,
}
impl Approver for Recorder {
    fn approve(&self, call: ToolCallInfo) -> futures::future::BoxFuture<'static, Approval> {
        self.seen.lock().unwrap().push(call.tool);
        let allow = self.allow;
        Box::pin(async move {
            if allow {
                Approval::Allow
            } else {
                Approval::Deny("not today".into())
            }
        })
    }
}

#[tokio::test]
async fn destructive_tools_need_approval_and_denial_is_reported_to_model() {
    let (cfg, _) = provider(ProviderKind::Anthropic).await;
    let sources = vec![mcp_source("github").await];
    let deny = Recorder {
        seen: Mutex::default(),
        allow: false,
    };
    let out = agent::run(
        &cfg,
        req("please delete the old repo"),
        &sources,
        &AgentOptions::default(),
        &deny,
        &mut |_| {},
    )
    .await;
    assert_eq!(*deny.seen.lock().unwrap(), ["delete_repo"]);
    assert!(
        out.messages
            .last()
            .unwrap()
            .text()
            .contains("The user declined this tool call: not today")
    );

    // AutoApprove::None asks even for read-only tools
    let allow = Recorder {
        seen: Mutex::default(),
        allow: true,
    };
    let opts = AgentOptions {
        auto_approve: AutoApprove::None,
        ..Default::default()
    };
    agent::run(
        &cfg,
        req("weather in Oslo"),
        &sources,
        &opts,
        &allow,
        &mut |_| {},
    )
    .await;
    assert_eq!(*allow.seen.lock().unwrap(), ["get_weather"]);
}

#[tokio::test]
async fn multi_server_names_are_prefixed_unique_and_valid() {
    let a = mcp_source("Weather API").await;
    let b = mcp_source("weather.api").await;
    let (specs, map) = agent::tool_specs(&[a, b]);
    let names: Vec<&str> = specs.iter().map(|s| s.name.as_str()).collect();
    assert!(names.contains(&"Weather_API__get_weather"));
    assert!(names.contains(&"weather_api__get_weather"));
    let unique: HashMap<&str, ()> = names.iter().map(|n| (*n, ())).collect();
    assert_eq!(unique.len(), names.len());
    assert!(names.iter().all(|n| {
        n.len() <= 64
            && n.chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    }));
    assert_eq!(map.len(), specs.len());
    assert!(
        specs
            .iter()
            .find(|s| s.name.ends_with("delete_repo"))
            .unwrap()
            .description
            .contains("destructive")
    );
}

#[tokio::test]
async fn max_turns_stops_cleanly() {
    let (cfg, _) = provider(ProviderKind::Anthropic).await;
    let sources = vec![mcp_source("w").await];
    let opts = AgentOptions {
        max_turns: 1,
        ..Default::default()
    };
    let out = agent::run(
        &cfg,
        req("weather in Rome"),
        &sources,
        &opts,
        &AllowAll,
        &mut |_| {},
    )
    .await;
    assert!(out.error.unwrap().contains("stopped after 1 turns"));
    assert!(matches!(
        out.messages.last().unwrap().content[0],
        Block::ToolResult { is_error: true, .. }
    ));
}

#[tokio::test]
async fn mcp_sampling_is_answered_by_the_llm() {
    let (cfg, mock) = provider(ProviderKind::Anthropic).await;
    let url = lsock_mcp::mock::spawn_http(Default::default(), 0)
        .await
        .unwrap();
    let mut opts = lsock_mcp::ConnectOptions::new(lsock_mcp::TransportConfig::Http {
        url,
        headers: vec![],
        validate_certificates: true,
    });
    opts.sampling = Some(Arc::new(agent::LlmSampler {
        cfg,
        model: "mock-large".into(),
        max_tokens: 500,
    }));
    let client = lsock_mcp::Client::connect(opts).await.unwrap();
    let r = client
        .call_tool("ask_llm", json!({"prompt": "say hi"}))
        .await
        .unwrap();
    assert_eq!(r.text(), "LLM said: Echo: say hi");
    let sent = mock.requests.lock().unwrap()[0].clone();
    assert_eq!(
        (sent["system"].as_str(), sent["max_tokens"].as_u64()),
        (Some("Answer briefly."), Some(200))
    );
    assert!(
        client
            .log()
            .entries()
            .iter()
            .any(|e| e.method.as_deref() == Some("sampling/createMessage")
                && e.direction == lsock_mcp::Direction::Out)
    );

    // without a handler the client refuses with a readable reason
    let url = lsock_mcp::mock::spawn_http(Default::default(), 0)
        .await
        .unwrap();
    let plain = lsock_mcp::Client::connect(lsock_mcp::ConnectOptions::new(
        lsock_mcp::TransportConfig::Http {
            url,
            headers: vec![],
            validate_certificates: true,
        },
    ))
    .await
    .unwrap();
    let r = plain
        .call_tool("ask_llm", json!({"prompt": "x"}))
        .await
        .unwrap();
    assert!(
        r.is_error && r.text().contains("Sampling is disabled"),
        "{}",
        r.text()
    );
}
