use std::collections::HashMap;
use std::time::Duration;

use lsock_mcp::mock::{HttpState, spawn_http};
use lsock_mcp::{Client, ConnectOptions, Direction, FrameKind, McpError, TransportConfig, schema};
use serde_json::json;

fn http(url: &str, headers: Vec<(String, String)>) -> ConnectOptions {
    ConnectOptions::new(TransportConfig::Http {
        url: url.into(),
        headers,
        validate_certificates: true,
    })
}

fn stdio() -> ConnectOptions {
    ConnectOptions::new(TransportConfig::Stdio {
        command: env!("CARGO_BIN_EXE_lsock-mock-mcp").into(),
        args: vec![],
        env: HashMap::new(),
        cwd: None,
    })
}

async fn exercise(client: &Client) {
    assert_eq!(client.server().server_info.name, "lsock-mock-mcp");
    assert!(client.server().instructions.is_some());

    let tools = client.list_tools().await.unwrap();
    assert_eq!((tools.pages, tools.items.len()), (2, 10));
    let create = tools
        .items
        .iter()
        .find(|t| t.name == "create_issue")
        .unwrap();
    let rows = schema::param_rows(&create.input_schema);
    assert!(
        rows.iter()
            .any(|r| r.path == "assignee.login" && r.required && r.depth == 1)
    );
    let del = tools
        .items
        .iter()
        .find(|t| t.name == "delete_repo")
        .unwrap();
    assert_eq!(del.display_name(), "Delete repository");
    let del_ann = del.annotations.as_ref().unwrap();
    assert_eq!(
        (del_ann.destructive_hint, del_ann.idempotent_hint),
        (Some(true), Some(true))
    );
    assert_eq!(
        tools
            .items
            .iter()
            .find(|t| t.name == "get_weather")
            .unwrap()
            .annotations
            .as_ref()
            .unwrap()
            .read_only_hint,
        Some(true)
    );

    let echo = client
        .call_tool("echo", json!({"message": "hi"}))
        .await
        .unwrap();
    assert_eq!(
        echo.structured_content,
        Some(json!({"echo": {"message": "hi"}}))
    );
    assert!(!echo.is_error);
    assert!(client.call_tool("fail", json!({})).await.unwrap().is_error);
    match client.call_tool("nope", json!({})).await {
        Err(McpError::Rpc { code, message, .. }) => {
            assert_eq!((code, message.as_str()), (-32602, "Unknown tool: nope"))
        }
        other => panic!("{other:?}"),
    }

    client.call_tool("notify", json!({})).await.unwrap();
    let notes = client.notifications();
    assert_eq!(notes.len(), 1);
    assert_eq!(notes[0].params["data"], "working on it");

    assert_eq!(client.list_resources().await.unwrap().items.len(), 2);
    assert_eq!(
        client.list_resource_templates().await.unwrap().items[0].uri_template,
        "repo://{owner}/{name}/issues"
    );
    let prompts = client.list_prompts().await.unwrap();
    assert!(prompts.items[0].arguments[0].required);
    let read = client.read_resource("mem://config").await.unwrap();
    assert_eq!(read["contents"][0]["text"], "contents of mem://config");
    let got = client
        .get_prompt("review_code", json!({"code": "fn x(){}"}))
        .await
        .unwrap();
    assert!(
        got["messages"][0]["content"]["text"]
            .as_str()
            .unwrap()
            .contains("fn x(){}")
    );
    assert!(client.ping().await.unwrap() >= 0.0);

    // The log pairs every response with its request method and latency.
    let log = client.log().entries();
    let responses: Vec<_> = log
        .iter()
        .filter(|e| e.direction == Direction::In && e.kind == FrameKind::Response)
        .collect();
    assert!(
        responses
            .iter()
            .all(|e| e.latency_ms.is_some() && e.method.is_some())
    );
    assert!(
        responses
            .iter()
            .any(|e| e.method.as_deref() == Some("tools/list"))
    );
    assert!(
        log.iter()
            .any(|e| e.method.as_deref() == Some("notifications/initialized")
                && e.direction == Direction::Out)
    );
    let seqs: Vec<u64> = log.iter().map(|e| e.seq).collect();
    assert!(seqs.windows(2).all(|w| w[0] < w[1]));
}

#[tokio::test]
async fn streamable_http_end_to_end() {
    let url = spawn_http(HttpState::default(), 0).await.unwrap();
    let client = Client::connect(http(&url, vec![])).await.unwrap();
    assert!(client.session_id().unwrap().starts_with("sess-"));
    exercise(&client).await;
    // notification arrived through the SSE stream of tools/call, before the result
    let log = client.log().entries();
    let n = log
        .iter()
        .position(|e| e.method.as_deref() == Some("notifications/message"))
        .unwrap();
    let r = log
        .iter()
        .rposition(|e| e.kind == FrameKind::Response && e.method.as_deref() == Some("tools/call"))
        .unwrap();
    assert!(n < log.len() && r < log.len());

    client.close().await;
    // session was deleted server-side: a new request fails with a readable hint
    let err = client.list_tools().await.unwrap_err();
    assert!(err.to_string().contains("session expired"), "{err}");
}

#[tokio::test]
async fn http_auth_errors_are_readable() {
    let url = spawn_http(HttpState::with_token("s3cret"), 0)
        .await
        .unwrap();
    let err = Client::connect(http(&url, vec![])).await.err().unwrap();
    assert!(
        err.to_string().contains("401") && err.to_string().contains("Authorization"),
        "{err}"
    );
    let ok = Client::connect(http(
        &url,
        vec![("Authorization".into(), "Bearer s3cret".into())],
    ))
    .await;
    assert!(ok.is_ok());
}

#[tokio::test]
async fn stdio_end_to_end_with_roots_and_stderr() {
    let mut opts = stdio();
    opts.root_uris = vec![("file:///work".into(), Some("work".into()))];
    let client = Client::connect(opts).await.unwrap();
    exercise(&client).await;
    let roots = client.call_tool("ask_roots", json!({})).await.unwrap();
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&roots.text()).unwrap(),
        json!([{"name": "work", "uri": "file:///work"}])
    );
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(
        client
            .log()
            .entries()
            .iter()
            .any(|e| e.direction == Direction::Stderr
                && e.message["text"] == "lsock-mock-mcp ready on stdio")
    );
    client.close().await;
}

#[tokio::test]
async fn timeouts_and_bad_commands() {
    let mut opts = stdio();
    // generous enough for the initialize handshake on a loaded machine
    opts.request_timeout = Duration::from_millis(1500);
    let client = Client::connect(opts).await.unwrap();
    let err = client
        .call_tool("slow", json!({"ms": 6000}))
        .await
        .unwrap_err();
    assert_eq!(
        err,
        McpError::Timeout {
            method: "tools/call".into(),
            ms: 1500
        }
    );
    assert!(
        client
            .log()
            .entries()
            .iter()
            .any(|e| e.method.as_deref() == Some("notifications/cancelled"))
    );
    client.close().await;

    let bad = ConnectOptions::new(TransportConfig::Stdio {
        command: "/definitely/not/here".into(),
        args: vec![],
        env: HashMap::new(),
        cwd: None,
    });
    assert!(
        matches!(Client::connect(bad).await, Err(McpError::Transport(m)) if m.contains("failed to start"))
    );
}
