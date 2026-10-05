//! IPC contract tests: invoke commands with exactly the JSON payloads that
//! `src/lib/api.ts` sends, through Tauri's mock runtime.

use serde_json::{Value, json};
use tauri::test::{INVOKE_KEY, get_ipc_response, mock_builder, mock_context, noop_assets};
use tauri::webview::InvokeRequest;
use tauri::{WebviewWindow, WebviewWindowBuilder};

use super::*;

struct Harness {
    _app: tauri::App<tauri::test::MockRuntime>,
    w: WebviewWindow<tauri::test::MockRuntime>,
}

impl Harness {
    fn new() -> Self {
        let app = build(mock_builder(), Engine::in_memory())
            .build(mock_context(noop_assets()))
            .unwrap();
        let w = WebviewWindowBuilder::new(&app, "main", Default::default())
            .build()
            .unwrap();
        Self { _app: app, w }
    }

    fn call(&self, cmd: &str, args: Value) -> Result<Value, Value> {
        get_ipc_response(
            &self.w,
            InvokeRequest {
                cmd: cmd.into(),
                callback: tauri::ipc::CallbackFn(0),
                error: tauri::ipc::CallbackFn(1),
                url: "tauri://localhost".parse().unwrap(),
                body: tauri::ipc::InvokeBody::Json(args),
                headers: Default::default(),
                invoke_key: INVOKE_KEY.to_string(),
            },
        )
        .map(|b| b.deserialize::<Value>().unwrap())
    }

    fn ok(&self, cmd: &str, args: Value) -> Value {
        self.call(cmd, args)
            .unwrap_or_else(|e| panic!("{cmd} failed: {e}"))
    }
}

#[test]
fn workspace_tree_and_request_round_trip() {
    let h = Harness::new();
    let ws = h.ok("workspace_create", json!({"name": "API"}));
    let ws_id = ws["id"].as_str().unwrap();
    assert_eq!(ws["type"], "Workspace");
    assert_eq!(
        h.ok("workspace_list", json!({})).as_array().unwrap().len(),
        1
    );

    let folder = h.ok("folder_create", json!({"parentId": ws_id, "name": "Users"}));
    let req = h.ok(
        "request_create",
        json!({"parentId": folder["id"], "request": null}),
    );
    assert_eq!(req["method"], "GET");
    assert_eq!(req["authentication"]["type"], "inherit");

    // The UI loads with doc_get then saves the edited object with request_update.
    let mut doc = h.ok("doc_get", json!({"id": req["id"]}));
    doc["url"] = json!("{{ _.base }}/users/:id");
    doc["pathParameters"] = json!([{"name": "id", "value": "7"}]);
    doc["headers"] = json!([{"name": "X-A", "value": "1"}]);
    doc["body"] = json!({"mimeType": "application/json", "text": "{}", "params": []});
    doc["authentication"] = json!({"type": "bearer", "token": "{{ _.tok }}"});
    let saved = h.ok("request_update", json!({"doc": doc}));
    assert_eq!(saved["url"], "{{ _.base }}/users/:id");
    assert_eq!(saved["parentId"], folder["id"]);

    let tree = h.ok("tree_get", json!({"workspaceId": ws_id}));
    assert_eq!(tree[0]["kind"], "folder");
    assert_eq!(tree[0]["children"][0]["method"], "GET");
    assert!(tree[0]["sortKey"].is_number());

    h.ok("item_rename", json!({"id": req["id"], "name": "Get user"}));
    assert_eq!(
        h.ok("doc_get", json!({"id": req["id"]}))["name"],
        "Get user"
    );
    let dup = h.ok("item_duplicate", json!({"id": folder["id"]}));
    let tree = h.ok("tree_get", json!({"workspaceId": ws_id}));
    assert_eq!(tree.as_array().unwrap().len(), 2);
    assert_eq!(tree[1]["children"][0]["name"], "Get user");
    h.ok(
        "item_move",
        json!({"id": req["id"], "parentId": ws_id, "sortKey": 99.0}),
    );
    assert!(
        h.call(
            "item_move",
            json!({"id": folder["id"], "parentId": folder["id"], "sortKey": 1.0})
        )
        .is_err()
    );
    assert_eq!(h.ok("item_delete", json!({"id": dup})), json!(2));
}

#[test]
fn environments_preview_and_curl() {
    let h = Harness::new();
    let ws = h.ok("workspace_create", json!({"name": "API"}));
    let ws_id = ws["id"].as_str().unwrap();
    let envs = h.ok("env_list", json!({"workspaceId": ws_id}));
    let mut base = envs["base"].clone();
    base["data"] = json!({"base": "http://example.test", "tok": "secret"});
    h.ok("env_update", json!({"doc": base}));
    let sub = h.ok(
        "env_create",
        json!({"workspaceId": ws_id, "name": "Staging"}),
    );
    h.ok(
        "env_set_active",
        json!({"workspaceId": ws_id, "envId": sub["id"]}),
    );
    assert_eq!(
        h.ok("env_list", json!({"workspaceId": ws_id}))["activeId"],
        sub["id"]
    );

    let p = h.ok(
        "render_preview",
        json!({"id": ws_id, "text": "{{ _.base }}/x/{{ nope }}"}),
    );
    assert_eq!(p["error"], "unresolved variable: nope");
    assert_eq!(p["refs"].as_array().unwrap().len(), 2);
    let vars = h.ok("context_vars", json!({"id": ws_id}));
    assert!(
        vars.as_array()
            .unwrap()
            .iter()
            .any(|v| v["name"] == "tok" && v["source"] == "Base Environment")
    );

    let parsed = h.ok(
        "curl_parse",
        json!({"text": "curl -X POST https://x.io/a -d '{\"k\":1}'"}),
    );
    assert_eq!(
        (
            parsed["method"].as_str(),
            parsed["body"]["mimeType"].as_str()
        ),
        (Some("POST"), Some("application/json"))
    );
    let created = h.ok(
        "request_create",
        json!({"parentId": ws_id, "request": parsed}),
    );
    assert_eq!(created["url"], "https://x.io/a");

    let mut s = h.ok("settings_get", json!({}));
    s["timeoutMs"] = json!(1234);
    assert_eq!(
        h.ok("settings_update", json!({"doc": s}))["timeoutMs"],
        1234
    );
}

#[test]
fn send_error_response_is_returned_as_view() {
    let h = Harness::new();
    let ws = h.ok("workspace_create", json!({"name": "API"}));
    let req = h.ok(
        "request_create",
        json!({"parentId": ws["id"], "request": {"url": "http://127.0.0.1:1/x", "name": "r"}}),
    );
    let resp = h.ok("request_send", json!({"requestId": req["id"]}));
    assert!(resp["error"].as_str().unwrap().contains("error"));
    assert_eq!(resp["bodyText"], "");
    let list = h.ok("response_list", json!({"requestId": req["id"]}));
    assert_eq!(list.as_array().unwrap().len(), 1);
    h.ok("response_get", json!({"id": list[0]["id"]}));
    h.ok("response_clear", json!({"requestId": req["id"]}));
    assert!(
        h.ok("response_list", json!({"requestId": req["id"]}))
            .as_array()
            .unwrap()
            .is_empty()
    );
}

#[test]
fn mcp_inspector_commands_against_mock_server() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let url = rt
        .block_on(irs_mcp::mock::spawn_http(Default::default(), 0))
        .unwrap();

    let h = Harness::new();
    let ws = h.ok("workspace_create", json!({"name": "API"}));
    let s = h.ok(
        "mcp_server_create",
        json!({"parentId": ws["id"], "name": "Mock"}),
    );
    let id = s["id"].clone();
    let mut doc = h.ok("doc_get", json!({"id": id}));
    doc["transport"] = json!({"kind": "streamable-http", "url": url});
    h.ok("mcp_server_update", json!({"doc": doc}));

    assert!(
        h.call("mcp_list", json!({"serverId": id, "kind": "tools"}))
            .unwrap_err()
            .as_str()
            .unwrap()
            .contains("Not connected")
    );
    let st = h.ok("mcp_connect", json!({"serverId": id}));
    assert_eq!(st["connected"], true);
    assert_eq!(st["server"]["serverInfo"]["name"], "irs-mock-mcp");
    assert!(st["sessionId"].as_str().unwrap().starts_with("sess-"));

    let tools = h.ok("mcp_list", json!({"serverId": id, "kind": "tools"}));
    assert_eq!(
        (
            tools["items"].as_array().unwrap().len(),
            tools["pages"].as_u64()
        ),
        (10, Some(2))
    );
    let create = tools["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["name"] == "create_issue")
        .unwrap();
    assert_eq!(create["displayName"], "create_issue");
    assert_eq!(create["hints"]["destructive"], false);
    assert!(
        create["params"]
            .as_array()
            .unwrap()
            .iter()
            .any(|p| p["path"] == "assignee.login" && p["required"] == true)
    );
    assert_eq!(create["example"]["repo"], "");
    assert!(
        create["inputSchema"]["properties"].is_object(),
        "flattened tool keeps raw schema"
    );

    let errs = h.ok(
        "mcp_validate",
        json!({"schema": create["inputSchema"], "args": {"repo": 1}}),
    );
    assert!(errs.as_array().unwrap().len() >= 2);

    let r = h.ok(
        "mcp_call_tool",
        json!({"serverId": id, "name": "echo", "args": {"message": "hi"}}),
    );
    assert_eq!(r["result"]["structuredContent"]["echo"]["message"], "hi");
    assert!(r["latencyMs"].as_f64().unwrap() >= 0.0);
    h.ok(
        "mcp_call_tool",
        json!({"serverId": id, "name": "notify", "args": {}}),
    );
    assert_eq!(
        h.ok("mcp_notifications", json!({"serverId": id}))
            .as_array()
            .unwrap()
            .len(),
        1
    );

    for kind in ["resources", "templates", "prompts"] {
        assert!(
            !h.ok("mcp_list", json!({"serverId": id, "kind": kind}))["items"]
                .as_array()
                .unwrap()
                .is_empty()
        );
    }
    h.ok(
        "mcp_read_resource",
        json!({"serverId": id, "uri": "mem://config"}),
    );
    h.ok(
        "mcp_get_prompt",
        json!({"serverId": id, "name": "review_code", "args": {"code": "x"}}),
    );
    assert!(h.ok("mcp_ping", json!({"serverId": id})).as_f64().is_some());

    let log = h.ok("mcp_log", json!({"serverId": id}));
    let entries = log.as_array().unwrap();
    assert!(entries.iter().any(|e| e["kind"] == "response"
        && e["latencyMs"].is_number()
        && e["method"] == "tools/list"));
    assert!(
        entries
            .iter()
            .all(|e| e["seq"].is_number() && e["timestampMs"].is_number())
    );

    h.ok("mcp_disconnect", json!({"serverId": id}));
    assert_eq!(
        h.ok("mcp_status", json!({"serverId": id}))["connected"],
        false
    );
    h.ok("mcp_log_clear", json!({"serverId": id}));
    assert!(
        h.ok("mcp_log", json!({"serverId": id}))
            .as_array()
            .unwrap()
            .is_empty()
    );
}

#[test]
fn runner_commands_stream_and_export() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let url = rt
        .block_on(irs_mcp::mock::spawn_http(Default::default(), 0))
        .unwrap();
    let h = Harness::new();
    let ws = h.ok("workspace_create", json!({"name": "API"}));
    let req = h.ok(
        "request_create",
        json!({"parentId": ws["id"], "request": {
            "name": "Init", "method": "POST", "url": url,
            "headers": [{"name": "Accept", "value": "application/json"}],
            "body": {"mimeType": "application/json", "text": "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":{}}", "params": []},
            "afterResponseScript": "insomnia.test('ok', () => insomnia.response.to.have.status(200)); console.log('row', insomnia.iterationData.get('n'));"
        }}),
    );
    assert!(
        h.call(
            "runner_start",
            json!({"run": {"requestIds": [], "iterations": 1, "delayMs": 0, "bail": false}})
        )
        .is_err()
    );
    let run_id = h.ok(
        "runner_start",
        json!({"run": {"requestIds": [req["id"]], "iterations": 2, "delayMs": 0, "bail": false, "dataText": "n\n1\n2\n"}}),
    );
    let run_id = run_id.as_str().unwrap().to_string();
    let out_dir = tempfile::tempdir().unwrap();
    let mut path = None;
    for _ in 0..100 {
        match h.call(
            "runner_export",
            json!({"runId": run_id, "reporter": "json", "dir": out_dir.path()}),
        ) {
            Ok(p) => {
                path = Some(p.as_str().unwrap().to_string());
                break;
            }
            Err(_) => std::thread::sleep(std::time::Duration::from_millis(50)),
        }
    }
    let report: Value =
        serde_json::from_str(&std::fs::read_to_string(path.expect("run finished")).unwrap())
            .unwrap();
    assert_eq!(
        (report["requests"].as_u64(), report["testsPassed"].as_u64()),
        (Some(2), Some(2))
    );
    assert_eq!(report["results"][1]["console"][0]["text"], "row 2");
    h.ok("runner_cancel", json!({"runId": run_id}));
}

#[test]
fn ai_providers_requests_and_runs_with_approval() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let llm_base = rt
        .block_on(irs_llm::mock::spawn(Default::default(), 0))
        .unwrap();
    let mcp_url = rt
        .block_on(irs_mcp::mock::spawn_http(Default::default(), 0))
        .unwrap();
    let h = Harness::new();
    let events: Arc<std::sync::Mutex<Vec<Value>>> = Arc::default();
    let ev2 = events.clone();
    tauri::Listener::listen_any(&h.w, "llm-event", move |ev| {
        ev2.lock()
            .unwrap()
            .push(serde_json::from_str(ev.payload()).unwrap());
    });

    let p = h.ok(
        "llm_provider_create",
        json!({"name": "Mock", "kind": "anthropic"}),
    );
    assert_eq!(p["hasKey"], false);
    assert_eq!(p["defaultModel"], "claude-opus-5-5");
    let mut doc = p.clone();
    doc["baseUrl"] = json!(llm_base);
    doc["defaultModel"] = json!("mock-large");
    h.ok("llm_provider_update", json!({"doc": doc}));
    h.ok(
        "llm_provider_set_key",
        json!({"id": p["id"], "key": irs_llm::mock::MOCK_KEY}),
    );
    let list = h.ok("llm_provider_list", json!({}));
    assert_eq!(list[0]["hasKey"], true);
    assert!(
        !list.to_string().contains(irs_llm::mock::MOCK_KEY),
        "key never sent to the UI"
    );
    assert_eq!(
        h.ok("llm_models", json!({"providerId": p["id"]})),
        json!(["mock-large", "mock-small"])
    );

    let ws = h.ok("workspace_create", json!({"name": "AI"}));
    let s = h.ok(
        "mcp_server_create",
        json!({"parentId": ws["id"], "name": "Repo"}),
    );
    let mut sdoc = h.ok("doc_get", json!({"id": s["id"]}));
    sdoc["transport"] = json!({"kind": "streamable-http", "url": mcp_url});
    h.ok("mcp_server_update", json!({"doc": sdoc}));
    let r = h.ok(
        "llm_request_create",
        json!({"parentId": ws["id"], "name": "Cleanup"}),
    );
    assert_eq!(r["providerId"], p["id"], "defaults to the first provider");
    let mut rdoc = r.clone();
    rdoc["messages"] = json!([{"role": "user", "text": "please delete the old repo"}]);
    rdoc["mcpServerIds"] = json!([s["id"]]);
    h.ok("llm_request_update", json!({"doc": rdoc}));
    assert_eq!(
        h.ok("tree_get", json!({"workspaceId": ws["id"]}))[1]["kind"],
        "llm"
    );

    h.ok(
        "llm_run_start",
        json!({"runId": "run_ui_1", "requestId": r["id"]}),
    );
    // wait for the approval request, then approve it from "the UI"
    let call_id = (0..200)
        .find_map(|_| {
            std::thread::sleep(std::time::Duration::from_millis(20));
            events.lock().unwrap().iter().find_map(|e| {
                (e["event"]["type"] == "toolCall" && e["event"]["needsApproval"] == true)
                    .then(|| e["event"]["call"]["id"].clone())
            })
        })
        .unwrap_or_else(|| panic!("approval requested; got {:?}", events.lock().unwrap()));
    assert!(
        h.call(
            "llm_approve",
            json!({"runId": "run_ui_1", "callId": "nope", "allow": true})
        )
        .is_err()
    );
    h.ok(
        "llm_approve",
        json!({"runId": "run_ui_1", "callId": call_id, "allow": true}),
    );
    let saved = (0..200)
        .find_map(|_| {
            std::thread::sleep(std::time::Duration::from_millis(20));
            events
                .lock()
                .unwrap()
                .iter()
                .find(|e| e["event"]["type"] == "saved")
                .cloned()
        })
        .expect("run saved");
    assert_eq!(
        saved["event"]["run"]["toolCalls"][0]["result"]["text"],
        "ok: {\"repo\":\"acme/old\",\"confirm\":true}"
    );
    assert!(
        events
            .lock()
            .unwrap()
            .iter()
            .all(|e| e["runId"] == "run_ui_1")
    );
    assert_eq!(
        h.ok("llm_runs", json!({"requestId": r["id"]}))
            .as_array()
            .unwrap()
            .len(),
        1
    );
    h.ok("item_duplicate", json!({"id": r["id"]}));
}

#[test]
fn realtime_commands_connect_send_and_log() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let base = rt.block_on(irs_realtime::mock::spawn(0)).unwrap();
    let h = Harness::new();
    let ws = h.ok("workspace_create", json!({"name": "RT"}));
    let r = h.ok(
        "rt_create",
        json!({"parentId": ws["id"], "kind": "websocket"}),
    );
    assert_eq!(
        h.ok("tree_get", json!({"workspaceId": ws["id"]}))[0]["kind"],
        "realtime"
    );
    let mut doc = r.clone();
    doc["url"] = json!(format!("{}/ws", base.replace("http", "ws")));
    doc["headers"] = json!([{"name": "X-Token", "value": "{{ _.who }}"}]);
    h.ok("rt_update", json!({"doc": doc}));
    let mut env = h.ok("env_list", json!({"workspaceId": ws["id"]}))["base"].clone();
    env["data"] = json!({"who": "ipc"});
    h.ok("env_update", json!({"doc": env}));

    assert!(
        h.call(
            "rt_send",
            json!({"id": r["id"], "text": "x", "event": null, "ack": false})
        )
        .is_err(),
        "not connected yet"
    );
    assert_eq!(
        h.ok("rt_connect", json!({"id": r["id"]}))["connected"],
        true
    );
    h.ok(
        "rt_send",
        json!({"id": r["id"], "text": "hi {{ _.who }}", "event": null, "ack": false}),
    );
    let log = (0..100)
        .find_map(|_| {
            std::thread::sleep(std::time::Duration::from_millis(20));
            let l = h.ok("rt_log", json!({"id": r["id"]}));
            l.as_array()
                .unwrap()
                .iter()
                .any(|e| e["data"] == "echo: hi ipc")
                .then_some(l)
        })
        .expect("echo received");
    assert!(
        log.as_array()
            .unwrap()
            .iter()
            .any(|e| e["data"] == "welcome ipc" && e["direction"] == "in")
    );
    h.ok("rt_disconnect", json!({"id": r["id"]}));
    assert_eq!(
        h.ok("rt_status", json!({"id": r["id"]}))["connected"],
        false
    );
}

#[test]
fn grpc_commands_with_protos_and_reflection() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let url = rt.block_on(irs_grpc::demo::spawn(0, true)).unwrap();
    let h = Harness::new();
    let ws = h.ok("workspace_create", json!({"name": "G"}));
    let r = h.ok("grpc_create", json!({"parentId": ws["id"]}));
    assert_eq!(
        h.ok("tree_get", json!({"workspaceId": ws["id"]}))[0]["kind"],
        "grpc"
    );
    let mut doc = r.clone();
    doc["url"] = json!(url);
    doc["method"] = json!("/demo.Greeter/SayHello");
    doc["message"] = json!(r#"{"name": "{{ _.who }}"}"#);
    h.ok("grpc_update", json!({"doc": doc.clone()}));
    let mut env = h.ok("env_list", json!({"workspaceId": ws["id"]}))["base"].clone();
    env["data"] = json!({"who": "ipc"});
    h.ok("env_update", json!({"doc": env}));

    let svcs = h.ok("grpc_methods", json!({"id": r["id"], "refresh": true}));
    assert_eq!(svcs[0]["name"], "demo.Greeter", "via reflection");
    let res = h.ok("grpc_invoke", json!({"id": r["id"]}));
    assert_eq!(res["status"]["codeName"], "OK");
    assert_eq!(res["response"]["message"], "Hello, ipc!");

    // proto files instead of reflection
    doc["schemaSource"] = json!("protos");
    h.ok("grpc_update", json!({"doc": doc.clone()}));
    assert!(
        h.call("grpc_methods", json!({"id": r["id"], "refresh": true}))
            .unwrap_err()
            .as_str()
            .unwrap()
            .contains("No proto files")
    );
    h.ok("proto_file_create", json!({"workspaceId": ws["id"], "name": "demo.proto", "contents": irs_grpc::demo::DEMO_PROTO}));
    assert_eq!(
        h.ok("grpc_methods", json!({"id": r["id"], "refresh": true}))[0]["methods"]
            .as_array()
            .unwrap()
            .len(),
        4
    );

    // bidi stream via commands
    doc["method"] = json!("/demo.Greeter/Chat");
    doc["message"] = json!(r#"{"text":"yo"}"#);
    h.ok("grpc_update", json!({"doc": doc}));
    h.ok("grpc_stream_start", json!({"id": r["id"]}));
    h.ok("grpc_send", json!({"id": r["id"]}));
    h.ok("grpc_commit", json!({"id": r["id"]}));
    let log = (0..100)
        .find_map(|_| {
            std::thread::sleep(std::time::Duration::from_millis(20));
            let l = h.ok("grpc_log", json!({"id": r["id"]}));
            l.as_array()
                .unwrap()
                .iter()
                .any(|e| e["kind"] == "close")
                .then_some(l)
        })
        .expect("stream finished");
    assert!(log.as_array().unwrap().iter().any(|e| e["direction"] == "in" && e["data"].as_str().unwrap().contains("you said: yo")));
}

#[test]
fn every_emitted_event_is_listed_in_app_events() {
    let sources = [
        include_str!("lib.rs"),
        include_str!("ai.rs"),
        include_str!("rt.rs"),
        include_str!("grpc.rs"),
        include_str!("auth.rs"),
    ];
    let mut missing = vec![];
    for src in sources {
        for part in src.split("emit(\"").skip(1) {
            let name = part.split('"').next().unwrap();
            if !crate::APP_EVENTS.contains(&name) {
                missing.push(name.to_string());
            }
        }
    }
    assert!(missing.is_empty(), "add to APP_EVENTS: {missing:?}");
}

#[test]
fn import_export_secrets_code_and_git_commands() {
    let h = Harness::new();
    let postman = json!({
        "info": {"name": "Shop", "schema": "https://schema.getpostman.com/json/collection/v2.1.0/collection.json"},
        "variable": [{"key": "base", "value": "https://shop.test"}],
        "item": [{"name": "List", "request": {"method": "GET", "url": "{{base}}/orders", "auth": {"type": "bearer", "bearer": [{"key": "token", "value": "{{token}}"}]}}}]
    })
    .to_string();

    let p = h.ok("import_preview", json!({"text": postman}));
    assert_eq!(p["formatLabel"], "Postman collection");
    assert_eq!(p["workspaces"][0]["name"], "Shop");
    assert_eq!(p["workspaces"][0]["requests"], 1);
    assert!(
        h.call("import_preview", json!({"text": "not a collection"}))
            .is_err()
    );
    let s = h.ok(
        "import_apply",
        json!({"text": postman, "intoWorkspaceId": null, "replace": false}),
    );
    let ws_id = s["workspaceIds"][0].as_str().unwrap().to_string();

    // secrets: set, masked in the env doc, revealed on demand, used by code generation
    let envs = h.ok("env_list", json!({"workspaceId": ws_id}));
    let base_id = envs["base"]["id"].as_str().unwrap();
    let env = h.ok(
        "env_set_var",
        json!({"envId": base_id, "key": "token", "value": "t-secret", "secret": true}),
    );
    assert!(
        env["data"]["token"]
            .as_str()
            .unwrap()
            .starts_with("vault:v1:")
    );
    assert_eq!(env["secretKeys"], json!(["token"]));
    assert_eq!(
        h.ok("env_reveal", json!({"envId": base_id, "key": "token"})),
        json!("t-secret")
    );
    // the JSON editor saves the doc back with a plaintext edit → re-sealed
    let mut doc = env.clone();
    doc["data"]["token"] = json!("typed");
    let saved = h.ok("env_update", json!({"doc": doc}));
    assert!(
        saved["data"]["token"]
            .as_str()
            .unwrap()
            .starts_with("vault:v1:")
    );
    assert_eq!(h.ok("vault_status", json!({}))["sealedValues"], 1);

    let tree = h.ok("tree_get", json!({"workspaceId": ws_id}));
    let req_id = tree[0]["id"].as_str().unwrap();
    let targets = h.ok("code_targets", json!({}));
    assert_eq!(targets.as_array().unwrap().len(), 6);
    let code = h.ok(
        "code_generate",
        json!({"requestId": req_id, "target": "python-requests"}),
    );
    assert!(
        code["code"]
            .as_str()
            .unwrap()
            .contains("\"Authorization\": \"Bearer typed\""),
        "{code}"
    );

    let out = h.ok("export_workspace", json!({"workspaceId": ws_id, "format": "insomnia-v5", "includePrivate": false, "includeCookies": false}));
    assert_eq!(out["fileName"], "insomnia.shop.yaml");
    assert!(!out["content"].as_str().unwrap().contains("typed"));
    let out = h.ok("export_workspace", json!({"workspaceId": ws_id, "format": "postman", "includePrivate": false, "includeCookies": false}));
    assert!(
        out["content"]
            .as_str()
            .unwrap()
            .contains("\"{{base}}/orders\"")
    );

    // git: open a repo folder, link, status, commit, log
    if std::process::Command::new("git")
        .arg("--version")
        .output()
        .is_err()
    {
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("repo").to_string_lossy().into_owned();
    let opened = h.ok("git_open", json!({"dir": dir, "name": "Team"}));
    let repo_id = opened["repo"]["id"].as_str().unwrap().to_string();
    let mut repo_doc = opened["repo"].clone();
    repo_doc["authorName"] = json!("Ana");
    repo_doc["authorEmail"] = json!("ana@example.com");
    repo_doc.as_object_mut().unwrap().remove("workspaces");
    h.ok("git_repo_update", json!({"doc": repo_doc}));
    let linked = h.ok("git_link", json!({"repoId": repo_id, "workspaceId": ws_id}));
    assert_eq!(linked["workspaces"][0]["name"], "Shop");
    let st = h.ok("git_status", json!({"repoId": repo_id}));
    assert_eq!(st["branch"], "main");
    assert_eq!(st["changes"][0]["status"], "untracked");
    assert!(
        h.ok(
            "git_diff",
            json!({"repoId": repo_id, "path": "insomnia.shop.yaml"})
        )
        .as_str()
        .unwrap()
        .contains("+name: Shop")
    );
    h.ok(
        "git_commit",
        json!({"repoId": repo_id, "message": "Add Shop", "paths": []}),
    );
    assert_eq!(
        h.ok("git_log", json!({"repoId": repo_id, "limit": 5}))[0]["message"],
        "Add Shop"
    );
    assert_eq!(
        h.ok("git_branches", json!({"repoId": repo_id}))["current"],
        "main"
    );
    assert_eq!(
        h.ok("git_repo_list", json!({})).as_array().unwrap().len(),
        1
    );
    assert!(
        h.ok("git_status", json!({"repoId": repo_id}))["changes"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}
