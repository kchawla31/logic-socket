use super::*;
use axum::{Router, body::Bytes, http::HeaderMap, http::Uri, routing::any};
use lsock_core::{Body, mime};
use serde_json::{Value, json};

async fn server() -> String {
    async fn echo(uri: Uri, headers: HeaderMap, body: Bytes) -> axum::response::Response {
        let h: serde_json::Map<String, Value> = headers
            .iter()
            .map(|(k, v)| (k.to_string(), json!(v.to_str().unwrap_or(""))))
            .collect();
        let j = json!({"path": uri.path(), "query": uri.query(), "headers": h, "body": String::from_utf8_lossy(&body)});
        axum::response::Response::builder()
            .header("content-type", "application/json")
            .header("set-cookie", "visits=1; Path=/")
            .body(axum::body::Body::from(j.to_string()))
            .unwrap()
    }
    let app = Router::new()
        .route("/{*p}", any(echo))
        .route("/big", any(|| async { "x".repeat(2 * 1024 * 1024) }));
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
    format!("http://{addr}")
}

fn vars(v: Value) -> lsock_core::VarMap {
    serde_json::from_value(v).unwrap()
}

struct Fixture {
    e: Engine,
    ws: Doc<Workspace>,
    folder: Doc<Folder>,
    req: Doc<Request>,
}

fn fixture(base_url: &str) -> Fixture {
    let e = Engine::in_memory();
    let mut ws = e
        .store
        .insert(
            None,
            Workspace {
                name: "API".into(),
                ..Default::default()
            },
        )
        .unwrap();
    let mut base = e.base_environment(ws.id()).unwrap();
    base.data = vars(json!({"base": base_url, "user": "base-user", "token": "base-token"}));
    e.store.update(&base).unwrap();
    let staging = e
        .store
        .insert(
            Some(base.id()),
            Environment {
                name: "Staging".into(),
                data: vars(json!({"user": "staging-user"})),
                ..Default::default()
            },
        )
        .unwrap();
    ws.active_environment_id = Some(staging.meta.id.clone());
    e.store.update(&ws).unwrap();

    let outer = e
        .store
        .insert(
            Some(ws.id()),
            Folder {
                name: "Outer".into(),
                headers: vec![
                    KeyValue::new("X-Outer", "o"),
                    KeyValue::new("X-Shared", "outer"),
                ],
                authentication: Auth::Bearer {
                    token: "{{ token }}".into(),
                    prefix: None,
                    disabled: false,
                },
                environment: vars(json!({"token": "folder-token"})),
                ..Default::default()
            },
        )
        .unwrap();
    let folder = e
        .store
        .insert(
            Some(outer.id()),
            Folder {
                name: "Inner".into(),
                headers: vec![KeyValue::new("X-Shared", "inner")],
                environment: vars(json!({"path": "users"})),
                ..Default::default()
            },
        )
        .unwrap();
    let req = e
        .store
        .insert(
            Some(folder.id()),
            Request {
                name: "Get user".into(),
                method: "post".into(),
                url: "{{ _.base }}/{{ path }}/:id".into(),
                path_parameters: vec![KeyValue::new("id", "{{ user }}")],
                parameters: vec![KeyValue::new("who", "{{ user }}")],
                headers: vec![KeyValue::new(
                    "X-Req",
                    "{% base64 'encode', 'normal', 'hi' %}",
                )],
                body: Body {
                    mime_type: Some(mime::JSON.into()),
                    text: Some(r#"{"u":"{{ user }}"}"#.into()),
                    ..Default::default()
                },
                ..Default::default()
            },
        )
        .unwrap();
    Fixture { e, ws, folder, req }
}

fn body_json(e: &Engine, r: &Response) -> Value {
    serde_json::from_slice(&e.response_body(r)).unwrap()
}

#[tokio::test]
async fn full_pipeline_renders_inherits_and_persists() {
    let base = server().await;
    let f = fixture(&base);
    let resp = f.e.send(f.req.id()).await.unwrap();
    assert_eq!(resp.error, None);
    assert_eq!(resp.status_code, 200);
    let j = body_json(&f.e, &resp);
    assert_eq!(j["path"], "/users/staging-user");
    assert_eq!(j["query"], "who=staging-user");
    assert_eq!(j["body"], r#"{"u":"staging-user"}"#);
    assert_eq!(j["headers"]["x-outer"], "o");
    assert_eq!(j["headers"]["x-shared"], "inner"); // nearest folder wins
    assert_eq!(j["headers"]["x-req"], "aGk=");
    assert_eq!(j["headers"]["authorization"], "Bearer folder-token"); // inherited auth, folder env
    assert_eq!(resp.meta.parent_id.as_deref(), Some(f.req.id()));

    // cookie jar was persisted and is sent next time
    let jar = f.e.cookie_jar(f.ws.id()).unwrap();
    assert_eq!(jar.cookies.len(), 1);
    let resp2 = f.e.send(f.req.id()).await.unwrap();
    assert_eq!(body_json(&f.e, &resp2)["headers"]["cookie"], "visits=1");
    assert_eq!(f.e.responses(f.req.id()).unwrap().len(), 2);
}

#[tokio::test]
async fn unresolved_variable_is_a_persisted_error_naming_field_and_var() {
    let f = fixture("http://127.0.0.1:1");
    let mut r = f.e.store.get::<Request>(f.req.id()).unwrap();
    r.headers.push(KeyValue::new("X-Key", "{{ api_key }}"));
    f.e.store.update(&r).unwrap();
    let resp = f.e.send(f.req.id()).await.unwrap();
    assert_eq!(
        resp.error.as_deref(),
        Some("Could not render header 'X-Key': unresolved variable: api_key")
    );
}

#[tokio::test]
async fn network_errors_are_persisted_too() {
    let f = fixture("http://127.0.0.1:1");
    let resp = f.e.send(f.req.id()).await.unwrap();
    assert!(
        resp.error.as_deref().unwrap().contains("error"),
        "{:?}",
        resp.error
    );
    assert_eq!(resp.status_code, 0);
}

#[test]
fn layers_follow_environment_precedence() {
    let f = fixture("http://x");
    // global env workspace
    let gws =
        f.e.store
            .insert(
                None,
                Workspace {
                    name: "Globals".into(),
                    scope: lsock_core::WorkspaceScope::Environment,
                    ..Default::default()
                },
            )
            .unwrap();
    let gbase = f.e.base_environment(gws.id()).unwrap();
    let mut gbase2 = gbase.clone();
    gbase2.data = vars(json!({"region": "eu", "user": "global-user"}));
    f.e.store.update(&gbase2).unwrap();
    let mut ws = f.e.store.get::<Workspace>(f.ws.id()).unwrap();
    ws.active_global_base_id = Some(gbase.meta.id.clone());
    f.e.store.update(&ws).unwrap();

    let names: Vec<String> =
        f.e.layers(f.req.id())
            .unwrap()
            .into_iter()
            .map(|l| l.name)
            .collect();
    assert_eq!(
        names,
        [
            "Global: Base Environment",
            "Base Environment",
            "Staging",
            "Folder: Outer",
            "Folder: Inner"
        ]
    );
    let ctx = f.e.context(f.req.id()).unwrap();
    assert_eq!(ctx.lookup("region"), Some(&json!("eu")));
    assert_eq!(ctx.lookup("user"), Some(&json!("staging-user")));
    assert_eq!(ctx.source_of("token"), Some("Folder: Outer"));

    let (rendered, refs) =
        f.e.preview(f.folder.id(), "{{ base }}/{{ path }}/{{ nope }}")
            .unwrap();
    assert!(rendered.is_err());
    assert_eq!(refs.iter().filter(|r| r.resolved).count(), 2);
}

#[test]
fn extra_layers_override_environment() {
    let f = fixture("http://x");
    let p =
        f.e.prepare(
            f.req.id(),
            &[Layer::new("Iteration 1", vars(json!({"user": "row-user"})))],
        )
        .unwrap();
    assert_eq!(p.request.url, "http://x/users/:id");
    assert_eq!(p.request.path_parameters[0].value, "row-user");
    assert_eq!(p.request.method, "POST");
}

#[tokio::test]
async fn send_many_keeps_order_history_prunes_and_large_bodies_go_to_disk() {
    let base = server().await;
    let dir = tempfile::tempdir().unwrap();
    let e = Engine::open(dir.path().to_path_buf()).unwrap();
    let ws = e.store.insert(None, Workspace::default()).unwrap();
    let mut s = e.store.settings().unwrap();
    s.max_history_per_request = 3;
    e.store.update(&s).unwrap();
    let ids: Vec<String> = (0..6)
        .map(|i| {
            e.store
                .insert(
                    Some(ws.id()),
                    Request {
                        url: format!("{base}/n/{i}"),
                        ..Default::default()
                    },
                )
                .unwrap()
                .meta
                .id
        })
        .collect();
    let results = e.send_many(&ids, 3).await;
    for (i, r) in results.iter().enumerate() {
        let r = r.as_ref().unwrap();
        assert_eq!(body_json(&e, r)["path"], format!("/n/{i}"));
    }
    assert_eq!(e.request_ids_in(ws.id()).unwrap(), ids);

    for _ in 0..4 {
        e.send(&ids[0]).await.unwrap();
    }
    assert_eq!(e.responses(&ids[0]).unwrap().len(), 3);

    let big = e
        .store
        .insert(
            Some(ws.id()),
            Request {
                url: format!("{base}/big"),
                ..Default::default()
            },
        )
        .unwrap();
    let r = e.send(big.id()).await.unwrap();
    assert!(r.body_b64.is_none() && r.body_path.is_some());
    assert_eq!(e.response_body(&r).len(), 2 * 1024 * 1024);
}

// ---------------------------------------------------------------- scripts in the pipeline

#[tokio::test]
async fn scripts_run_in_order_and_persist_results() {
    let base = server().await;
    let f = fixture(&base);
    // outer folder (parent of `f.folder`) gets scripts too
    let outer_id = f.folder.meta.parent_id.clone().unwrap();
    let mut outer = f.e.store.get::<Folder>(&outer_id).unwrap();
    outer.pre_request_script =
        Some("console.log('outer pre'); ls.variables.set('trail', 'outer');".into());
    outer.after_response_script = Some("console.log('outer after');".into());
    f.e.store.update(&outer).unwrap();
    let mut inner = f.e.store.get::<Folder>(f.folder.id()).unwrap();
    inner.pre_request_script = Some("console.log('inner pre'); ls.variables.set('trail', ls.variables.get('trail') + '>inner');".into());
    inner.after_response_script = Some("console.log('inner after');".into());
    f.e.store.update(&inner).unwrap();
    let mut req = f.e.store.get::<Request>(f.req.id()).unwrap();
    req.pre_request_script = Some(
        r#"console.log('request pre');
        ls.environment.set('user', 'from-script');
        ls.request.headers.add({ key: 'X-Trail', value: '{{ trail }}>request' });"#
            .into(),
    );
    req.after_response_script = Some(
        r#"console.log('request after');
        const body = ls.response.json();
        ls.environment.set('lastPath', body.path);
        ls.test('status ok', () => ls.expect(ls.response.code).to.equal(200));
        ls.test('header came through', () => ls.expect(body.headers['x-trail']).to.equal('outer>inner>request'));
        ls.test('fails on purpose', () => ls.expect(1).to.equal(2));"#
            .into(),
    );
    f.e.store.update(&req).unwrap();

    let resp = f.e.send(f.req.id()).await.unwrap();
    assert_eq!(resp.error, None);
    assert_eq!(resp.script_error, None);
    let order: Vec<&str> = resp.console.iter().map(|c| c.text.as_str()).collect();
    assert_eq!(
        order,
        [
            "outer pre",
            "inner pre",
            "request pre",
            "request after",
            "inner after",
            "outer after"
        ]
    );
    assert_eq!(resp.console[3].source, "after-response");
    // environment change applied before rendering (path param uses {{ user }})
    assert_eq!(body_json(&f.e, &resp)["path"], "/users/from-script");
    let results: Vec<(&str, bool)> = resp
        .test_results
        .iter()
        .map(|t| (t.name.as_str(), t.passed))
        .collect();
    assert_eq!(
        results,
        [
            ("status ok", true),
            ("header came through", true),
            ("fails on purpose", false)
        ]
    );
    // persisted to the active sub-environment
    let ws = f.e.store.get::<Workspace>(f.ws.id()).unwrap();
    let sub =
        f.e.store
            .get::<Environment>(ws.active_environment_id.as_ref().unwrap())
            .unwrap();
    assert_eq!(sub.data["user"], json!("from-script"));
    assert_eq!(sub.data["lastPath"], json!("/users/from-script"));
}

#[tokio::test]
async fn skip_and_script_errors_stop_the_send() {
    let f = fixture("http://127.0.0.1:1");
    let mut req = f.e.store.get::<Request>(f.req.id()).unwrap();
    req.pre_request_script =
        Some("ls.execution.skipRequest(); ls.execution.setNextRequest('Other');".into());
    f.e.store.update(&req).unwrap();
    let out =
        f.e.send_with_state(f.req.id(), &mut RunState::default())
            .await
            .unwrap();
    assert!(out.skipped);
    assert_eq!(out.next_request.as_deref(), Some("Other"));
    assert!(out.response.error.as_deref().unwrap().contains("skipped"));

    req.pre_request_script = Some("const a = 1;\nnotDefined();".into());
    f.e.store.update(&req).unwrap();
    let resp = f.e.send(f.req.id()).await.unwrap();
    let err = resp.error.clone().unwrap();
    assert!(
        err.starts_with("Pre-request script error (pre-request): "),
        "{err}"
    );
    assert!(err.contains("line 2"), "{err}");
    assert_eq!(resp.status_code, 0, "nothing was sent");
}

#[tokio::test]
async fn send_request_from_script_reaches_server_and_iteration_data_renders() {
    let base = server().await;
    let f = fixture(&base);
    let mut req = f.e.store.get::<Request>(f.req.id()).unwrap();
    req.pre_request_script = Some(format!(
        r#"const r = await ls.sendRequest({{ url: '{base}/token', method: 'POST', body: {{ mode: 'raw', raw: 'x' }} }});
        ls.variables.set('tokenPath', r.json().path);"#
    ));
    req.headers
        .push(lsock_core::KeyValue::new("X-Token-Path", "{{ tokenPath }}"));
    req.headers
        .push(lsock_core::KeyValue::new("X-Row", "{{ email }}"));
    f.e.store.update(&req).unwrap();
    let mut state = RunState {
        iteration_data: vars(json!({ "email": "a@b.c" })),
        iteration: 1,
        iteration_count: 1,
        ..Default::default()
    };
    let out = f.e.send_with_state(f.req.id(), &mut state).await.unwrap();
    let j = body_json(&f.e, &out.response);
    assert_eq!(j["headers"]["x-token-path"], "/token");
    assert_eq!(j["headers"]["x-row"], "a@b.c");
    assert_eq!(
        state.local_variables["tokenPath"],
        json!("/token"),
        "locals carry across requests in a run"
    );
}

#[tokio::test]
async fn without_sub_environment_ls_environment_is_the_base() {
    let base = server().await;
    let e = Engine::in_memory();
    let ws = e.store.insert(None, Workspace::default()).unwrap();
    let mut b = e.base_environment(ws.id()).unwrap();
    b.data = vars(json!({ "base": base, "keep": 1, "drop": 2 }));
    e.store.update(&b).unwrap();
    let first = e
        .store
        .insert(
            Some(ws.id()),
            Request {
                url: "{{ _.base }}/a".into(),
                after_response_script: Some(
                    "ls.environment.set('token', 'abc'); ls.environment.unset('drop'); ls.baseEnvironment.set('other', true);".into(),
                ),
                ..Default::default()
            },
        )
        .unwrap();
    let second = e
        .store
        .insert(
            Some(ws.id()),
            Request {
                url: "{{ _.base }}/b".into(),
                pre_request_script: Some(
                    "ls.request.headers.add({ key: 'X-T', value: ls.environment.get('token') });"
                        .into(),
                ),
                ..Default::default()
            },
        )
        .unwrap();
    e.send(first.id()).await.unwrap();
    let data = e.base_environment(ws.id()).unwrap().body.data;
    assert_eq!(
        (
            data.get("token"),
            data.get("other"),
            data.get("drop"),
            data.get("keep")
        ),
        (
            Some(&json!("abc")),
            Some(&json!(true)),
            None,
            Some(&json!(1))
        )
    );
    let r = e.send(second.id()).await.unwrap();
    assert_eq!(body_json(&e, &r)["headers"]["x-t"], "abc");
}

// ---------------------------------------------------------------- AI requests

mod ai {
    use super::*;
    use lsock_core::{
        KeySource, LlmPromptMessage, LlmProvider, LlmRequest, McpSampling, McpServer, McpTransport,
    };
    use lsock_llm::agent::{AgentEvent, AllowAll};

    async fn setup() -> (
        Engine,
        Doc<Workspace>,
        Doc<LlmProvider>,
        lsock_llm::mock::MockLlm,
    ) {
        let mock = lsock_llm::mock::MockLlm::default();
        let base = lsock_llm::mock::spawn(mock.clone(), 0).await.unwrap();
        let e = Engine::in_memory();
        let ws = e
            .store
            .insert(
                None,
                Workspace {
                    name: "AI".into(),
                    ..Default::default()
                },
            )
            .unwrap();
        let p = e
            .store
            .insert(
                None,
                LlmProvider {
                    name: "Mock Claude".into(),
                    kind: "anthropic".into(),
                    base_url: base,
                    default_model: "mock-large".into(),
                    ..Default::default()
                },
            )
            .unwrap();
        e.secrets().set(p.id(), lsock_llm::mock::MOCK_KEY).unwrap();
        (e, ws, p, mock)
    }

    #[tokio::test]
    async fn key_sources_resolve_and_missing_keys_explain_how_to_fix() {
        let (e, ws, p, _) = setup().await;
        let (_, cfg) = e.provider_config(p.id(), None).unwrap();
        assert_eq!(cfg.api_key.as_deref(), Some(lsock_llm::mock::MOCK_KEY));

        let mut tpl = p.clone();
        tpl.key_source = KeySource::Template {
            template: "{{ _.claude_key }}".into(),
        };
        e.store.update(&tpl).unwrap();
        let err = e
            .provider_config(p.id(), Some(ws.id()))
            .unwrap_err()
            .to_string();
        assert!(err.contains("claude_key"), "{err}");
        let mut env = e.base_environment(ws.id()).unwrap();
        env.data.insert("claude_key".into(), json!("from-env"));
        e.store.update(&env).unwrap();
        assert_eq!(
            e.provider_config(p.id(), Some(ws.id()))
                .unwrap()
                .1
                .api_key
                .as_deref(),
            Some("from-env")
        );

        let mut envvar = p.clone();
        envvar.key_source = KeySource::Env {
            var: "LSOCK_TEST_SURELY_UNSET_KEY".into(),
        };
        e.store.update(&envvar).unwrap();
        let err = e.provider_config(p.id(), None).unwrap_err().to_string();
        assert!(
            err.contains("set the LSOCK_TEST_SURELY_UNSET_KEY environment variable"),
            "{err}"
        );

        let ollama = e
            .store
            .insert(
                None,
                LlmProvider {
                    name: "Local".into(),
                    kind: "ollama".into(),
                    key_source: KeySource::None,
                    ..Default::default()
                },
            )
            .unwrap();
        let (_, cfg) = e.provider_config(ollama.id(), None).unwrap();
        assert_eq!(cfg.base_url, "http://localhost:11434/v1");
    }

    #[tokio::test]
    async fn ai_request_renders_runs_with_mcp_tools_and_persists() {
        let (e, ws, p, mock) = setup().await;
        let mcp_url = lsock_mcp::mock::spawn_http(Default::default(), 0)
            .await
            .unwrap();
        let mut env = e.base_environment(ws.id()).unwrap();
        env.data.insert("city".into(), json!("Lisbon"));
        env.data
            .insert("persona".into(), json!("a concise travel assistant"));
        e.store.update(&env).unwrap();
        let server = e
            .store
            .insert(
                Some(ws.id()),
                McpServer {
                    name: "Weather".into(),
                    transport: McpTransport::StreamableHttp { url: mcp_url },
                    ..Default::default()
                },
            )
            .unwrap();
        let req = e
            .store
            .insert(
                Some(ws.id()),
                LlmRequest {
                    provider_id: Some(p.meta.id.clone()),
                    system: "You are {{ persona }}.".into(),
                    messages: vec![LlmPromptMessage {
                        role: "user".into(),
                        text: "What's the weather in {{ _.city }}?".into(),
                    }],
                    mcp_server_ids: vec![server.meta.id.clone()],
                    ..Default::default()
                },
            )
            .unwrap();

        let prepared = e.llm_prepare(req.id()).unwrap();
        assert_eq!(prepared.chat.model, "mock-large", "provider default model");
        assert_eq!(
            prepared.chat.system.as_deref(),
            Some("You are a concise travel assistant.")
        );

        let mut events = vec![];
        let run = e
            .llm_run(req.id(), &AllowAll, &mut |ev| events.push(ev))
            .await
            .unwrap();
        assert_eq!(run.error, None);
        assert_eq!(run.turns, 2);
        assert_eq!((run.input_tokens, run.output_tokens), (24, 42));
        assert_eq!(run.tool_calls.len(), 1);
        assert_eq!(run.tool_calls[0]["call"]["tool"], "get_weather");
        assert_eq!(
            run.tool_calls[0]["result"]["text"],
            "Lisbon: 21°C, clear sky"
        );
        assert_eq!(
            run.transcript.last().unwrap()["content"][1]["text"],
            "Based on the tool: Lisbon: 21°C, clear sky"
        );
        assert!(run.ttft_ms.is_some());
        assert_eq!(run.request_bodies.len(), 2);
        assert!(
            !serde_json::to_string(&*run)
                .unwrap()
                .contains(lsock_llm::mock::MOCK_KEY),
            "no secrets persisted"
        );
        assert!(
            events
                .iter()
                .any(|ev| matches!(ev, AgentEvent::Done { .. }))
        );
        assert_eq!(e.llm_runs(req.id()).unwrap().len(), 1);
        assert_eq!(mock.requests.lock().unwrap().len(), 2);

        // a prompt with an unresolved variable fails before calling the provider
        let mut bad = e.store.get::<LlmRequest>(req.id()).unwrap();
        bad.messages[0].text = "{{ nope }}".into();
        e.store.update(&bad).unwrap();
        assert!(
            e.llm_prepare(req.id())
                .err()
                .unwrap()
                .to_string()
                .contains("nope")
        );
    }

    /// Actions come from hints and names; an AI reads descriptions only when the
    /// server has a provider set, and its answers are cached.
    #[tokio::test]
    async fn tool_actions_use_hints_names_and_opt_in_ai() {
        use lsock_mcp::action::{ActionSource, ToolAction};
        let (e, ws, p, mock) = setup().await;
        let mcp_url = lsock_mcp::mock::spawn_http(Default::default(), 0)
            .await
            .unwrap();
        let server = e
            .store
            .insert(
                Some(ws.id()),
                McpServer {
                    transport: McpTransport::StreamableHttp { url: mcp_url },
                    ..Default::default()
                },
            )
            .unwrap();
        let client = lsock_mcp::Client::connect(e.mcp_connect_options(server.id()).await.unwrap())
            .await
            .unwrap();
        let tools = client.list_tools().await.unwrap().items;
        let sent = || mock.requests.lock().unwrap().len();

        // No provider: hints and names only, and nothing is sent anywhere.
        let a = e.mcp_tool_actions(server.id(), &tools).unwrap();
        assert_eq!(
            (a["get_weather"].action, a["get_weather"].source),
            (ToolAction::Read, ActionSource::Server)
        );
        assert_eq!(a["delete_repo"].action, ToolAction::Delete);
        assert_eq!(
            (a["search"].action, a["search"].source),
            (ToolAction::Read, ActionSource::Name)
        );
        assert!(!a.contains_key("notify"), "no hints and no verb: unknown");
        assert_eq!(e.mcp_classify_tools(server.id(), &tools).await.unwrap(), 0);
        assert_eq!(sent(), 0);

        // Opt in: the AI reads the descriptions once.
        let mut s = e.store.get::<McpServer>(server.id()).unwrap();
        s.action_provider_id = Some(p.meta.id.clone());
        e.store.update(&s).unwrap();
        assert!(e.mcp_classify_tools(server.id(), &tools).await.unwrap() > 0);
        assert_eq!(sent(), 1);
        let a = e.mcp_tool_actions(server.id(), &tools).unwrap();
        assert_eq!(
            (a["notify"].action, a["notify"].source),
            (ToolAction::Read, ActionSource::Ai)
        );
        assert_eq!(
            a["get_weather"].source,
            ActionSource::Server,
            "server hints still win"
        );

        // Cached: nothing new to send.
        assert_eq!(e.mcp_classify_tools(server.id(), &tools).await.unwrap(), 0);
        assert_eq!(sent(), 1);
    }

    #[tokio::test]
    async fn saved_server_with_sampling_enabled_uses_the_provider() {
        let (e, ws, p, _) = setup().await;
        let mcp_url = lsock_mcp::mock::spawn_http(Default::default(), 0)
            .await
            .unwrap();
        let server = e
            .store
            .insert(
                Some(ws.id()),
                McpServer {
                    name: "Sampler".into(),
                    transport: McpTransport::StreamableHttp { url: mcp_url },
                    sampling: McpSampling {
                        enabled: true,
                        provider_id: Some(p.meta.id.clone()),
                        model: None,
                        max_tokens: 300,
                    },
                    ..Default::default()
                },
            )
            .unwrap();
        let client = lsock_mcp::Client::connect(e.mcp_connect_options(server.id()).await.unwrap())
            .await
            .unwrap();
        let r = client
            .call_tool("ask_llm", json!({"prompt": "ping"}))
            .await
            .unwrap();
        assert_eq!(r.text(), "LLM said: Echo: ping");
    }
}

#[test]
fn realtime_options_render_and_inherit() {
    let f = fixture("http://x");
    let rt =
        f.e.store
            .insert(
                Some(f.folder.id()),
                lsock_core::RealtimeRequest {
                    kind: "socketio".into(),
                    url: "{{ _.base }}/{{ path }}".into(),
                    headers: vec![KeyValue::new("X-User", "{{ user }}")],
                    socketio_auth: r#"{"token":"{{ token }}"}"#.into(),
                    namespace: "/chat".into(),
                    ..Default::default()
                },
            )
            .unwrap();
    let o = f.e.realtime_options(rt.id()).unwrap();
    assert_eq!(o.kind, lsock_realtime::Kind::Socketio);
    assert_eq!(o.url, "http://x/users");
    assert!(
        o.headers.contains(&("X-Outer".into(), "o".into())),
        "folder header inherited"
    );
    assert!(
        o.headers
            .contains(&("X-User".into(), "staging-user".into()))
    );
    assert!(
        o.headers
            .contains(&("Authorization".into(), "Bearer folder-token".into())),
        "folder auth inherited"
    );
    assert_eq!(o.auth, Some(json!({"token": "folder-token"})));
    assert_eq!(
        f.e.realtime_payload(rt.id(), "hi {{ user }}").unwrap(),
        "hi staging-user"
    );
}

#[tokio::test]
async fn graphql_query_uses_request_endpoint_headers_and_auth() {
    use axum::{Json, routing::post};
    let app = axum::Router::new().route(
        "/graphql",
        post(|headers: HeaderMap, Json(body): Json<Value>| async move {
            if headers.get("authorization").and_then(|v| v.to_str().ok())
                != Some("Bearer gql-token")
            {
                return Json(json!({"errors": [{"message": "unauthenticated"}]}));
            }
            Json(json!({"data": {"__schema": {"queryType": {"name": "Query"}}, "echo": body}}))
        }),
    );
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
    let e = Engine::in_memory();
    let ws = e.store.insert(None, Workspace::default()).unwrap();
    let mut base = e.base_environment(ws.id()).unwrap();
    base.data = vars(json!({"host": format!("http://{addr}"), "tok": "gql-token"}));
    e.store.update(&base).unwrap();
    let r = e
        .store
        .insert(
            Some(ws.id()),
            Request {
                method: "GET".into(),
                url: "{{ host }}/graphql".into(),
                authentication: Auth::Bearer {
                    token: "{{ tok }}".into(),
                    prefix: None,
                    disabled: false,
                },
                body: Body {
                    mime_type: Some(mime::GRAPHQL.into()),
                    text: Some(r#"{"query":"{ me }"}"#.into()),
                    ..Default::default()
                },
                ..Default::default()
            },
        )
        .unwrap();
    let v = e
        .graphql_query(
            r.id(),
            "query IntrospectionQuery { __schema { queryType { name } } }",
            Some(json!({"a": 1})),
        )
        .await
        .unwrap();
    assert_eq!(v["data"]["__schema"]["queryType"]["name"], "Query");
    assert_eq!(v["data"]["echo"]["variables"], json!({"a": 1}));
    assert!(
        e.responses(r.id()).unwrap().is_empty(),
        "introspection is not stored in history"
    );

    let mut no_auth = e.store.get::<Request>(r.id()).unwrap();
    no_auth.authentication = Auth::None;
    e.store.update(&no_auth).unwrap();
    let err = e
        .graphql_query(r.id(), "{ __typename }", None)
        .await
        .unwrap_err();
    assert_eq!(err.to_string(), "GraphQL errors: unauthenticated");
}

mod oauth {
    use super::*;
    use axum::{
        Form, Json,
        extract::Query,
        extract::State,
        http::StatusCode,
        response::Redirect,
        routing::{get, post},
    };
    use lsock_core::OAuth2Config;
    use sha2::Digest as _;
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};

    #[derive(Clone, Default)]
    struct St {
        grants: Arc<Mutex<Vec<String>>>,
        challenge: Arc<Mutex<Option<String>>>,
    }

    async fn authorize(State(st): State<St>, Query(q): Query<HashMap<String, String>>) -> Redirect {
        *st.challenge.lock().unwrap() = q.get("code_challenge").cloned();
        Redirect::to(&format!(
            "{}?code=the-code&state={}",
            q["redirect_uri"], q["state"]
        ))
    }

    async fn token(
        State(st): State<St>,
        headers: HeaderMap,
        Form(f): Form<HashMap<String, String>>,
    ) -> (StatusCode, Json<Value>) {
        let basic = headers
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        let ok_client = basic
            == format!(
                "Basic {}",
                base64::engine::general_purpose::STANDARD.encode("app:shh")
            )
            || f.get("client_secret").map(String::as_str) == Some("shh");
        if !ok_client {
            return (
                StatusCode::UNAUTHORIZED,
                Json(json!({"error": "invalid_client", "error_description": "bad client secret"})),
            );
        }
        let grant = f["grant_type"].clone();
        st.grants.lock().unwrap().push(grant.clone());
        let n = st.grants.lock().unwrap().len();
        match grant.as_str() {
            "authorization_code" => {
                let expected = st.challenge.lock().unwrap().clone().unwrap_or_default();
                let got =
                    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(sha2::Sha256::digest(
                        f.get("code_verifier")
                            .cloned()
                            .unwrap_or_default()
                            .as_bytes(),
                    ));
                if f["code"] != "the-code" || got != expected {
                    return (
                        StatusCode::BAD_REQUEST,
                        Json(
                            json!({"error": "invalid_grant", "error_description": "PKCE verification failed"}),
                        ),
                    );
                }
                (
                    StatusCode::OK,
                    Json(
                        json!({"access_token": "tok-code", "token_type": "Bearer", "expires_in": 3600, "refresh_token": "r1"}),
                    ),
                )
            }
            // expires immediately, so the next send must refresh
            "client_credentials" => (
                StatusCode::OK,
                Json(
                    json!({"access_token": format!("tok-cc-{n}"), "expires_in": 1, "refresh_token": "r-cc"}),
                ),
            ),
            "refresh_token" => (
                StatusCode::OK,
                Json(json!({"access_token": format!("tok-refreshed-{n}"), "expires_in": 3600})),
            ),
            "password" => (
                StatusCode::OK,
                Json(json!({"access_token": format!("tok-pw-{}", f["username"])})),
            ),
            _ => (
                StatusCode::BAD_REQUEST,
                Json(json!({"error": "unsupported_grant_type"})),
            ),
        }
    }

    async fn setup() -> (String, St) {
        let st = St::default();
        let app = axum::Router::new()
            .route("/authorize", get(authorize))
            .route("/token", post(token))
            .route(
                "/me",
                get(|h: HeaderMap| async move {
                    h.get("authorization")
                        .and_then(|v| v.to_str().ok())
                        .unwrap_or("none")
                        .to_string()
                }),
            )
            .with_state(st.clone());
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
        (format!("http://{addr}"), st)
    }

    fn cfg(base: &str, grant: &str) -> OAuth2Config {
        OAuth2Config {
            grant_type: grant.into(),
            access_token_url: format!("{base}/token"),
            authorization_url: format!("{base}/authorize"),
            client_id: "app".into(),
            client_secret: "{{ secret }}".into(),
            username: "ada".into(),
            password: "pw".into(),
            ..Default::default()
        }
    }

    fn engine_with(base: &str, auth: Auth, on_folder: bool) -> (Engine, String, String) {
        let e = Engine::in_memory();
        let ws = e.store.insert(None, Workspace::default()).unwrap();
        let mut env = e.base_environment(ws.id()).unwrap();
        env.data = vars(json!({"secret": "shh"}));
        e.store.update(&env).unwrap();
        let folder = e
            .store
            .insert(
                Some(ws.id()),
                Folder {
                    authentication: if on_folder {
                        auth.clone()
                    } else {
                        Auth::Inherit
                    },
                    ..Default::default()
                },
            )
            .unwrap();
        let r = e
            .store
            .insert(
                Some(folder.id()),
                Request {
                    url: format!("{base}/me"),
                    authentication: if on_folder { Auth::Inherit } else { auth },
                    ..Default::default()
                },
            )
            .unwrap();
        (e, r.meta.id.clone(), folder.meta.id.clone())
    }

    #[tokio::test]
    async fn client_credentials_then_refresh_and_folder_ownership() {
        let (base, st) = setup().await;
        let (e, req, folder) =
            engine_with(&base, Auth::OAuth2(cfg(&base, "client_credentials")), true);
        let r1 = e.send(&req).await.unwrap();
        assert_eq!(
            String::from_utf8(e.response_body(&r1)).unwrap(),
            "Bearer tok-cc-1"
        );
        assert!(
            e.oauth2_cached(&folder).unwrap().is_some(),
            "token cached on the folder that owns the auth"
        );
        let r2 = e.send(&req).await.unwrap();
        assert_eq!(
            String::from_utf8(e.response_body(&r2)).unwrap(),
            "Bearer tok-refreshed-2",
            "expired token refreshed"
        );
        let r3 = e.send(&req).await.unwrap();
        assert_eq!(
            String::from_utf8(e.response_body(&r3)).unwrap(),
            "Bearer tok-refreshed-2",
            "fresh token reused"
        );
        assert_eq!(
            *st.grants.lock().unwrap(),
            ["client_credentials", "refresh_token"]
        );
    }

    #[tokio::test]
    async fn password_grant_errors_and_code_flow_requires_sign_in() {
        let (base, _) = setup().await;
        let (e, req, _) = engine_with(
            &base,
            Auth::OAuth2(OAuth2Config {
                credentials_in_body: true,
                ..cfg(&base, "password")
            }),
            false,
        );
        assert_eq!(
            String::from_utf8(e.response_body(&e.send(&req).await.unwrap())).unwrap(),
            "Bearer tok-pw-ada"
        );

        let mut bad = cfg(&base, "client_credentials");
        bad.client_secret = "wrong".into();
        let (e, req, _) = engine_with(&base, Auth::OAuth2(bad), false);
        let r = e.send(&req).await.unwrap();
        assert_eq!(
            r.error.as_deref(),
            Some("OAuth 2 error invalid_client: bad client secret")
        );

        let (e, req, _) = engine_with(&base, Auth::OAuth2(cfg(&base, "authorization_code")), false);
        assert!(
            e.send(&req)
                .await
                .unwrap()
                .error
                .as_ref()
                .unwrap()
                .contains("click “Get token”")
        );
    }

    #[tokio::test]
    async fn authorization_code_with_pkce_via_local_redirect() {
        let (base, st) = setup().await;
        let mut c = cfg(&base, "authorization_code");
        c.client_secret = "shh".into();
        c.redirect_url = "http://127.0.0.1:18977/callback".into();
        let (e, req, _) = engine_with(&base, Auth::OAuth2(c.clone()), false);
        // the "browser": follow the authorization redirect to our localhost catcher
        let opener = |url: &str| {
            let url = url.to_string();
            tokio::spawn(async move {
                let body = reqwest::get(&url).await.unwrap().text().await.unwrap();
                assert!(body.contains("Signed in"), "{body}");
            });
        };
        let tok = e.oauth2_authorize(&req, &c, &opener).await.unwrap();
        assert_eq!(
            (tok.access_token.as_str(), tok.refresh_token.as_deref()),
            ("tok-code", Some("r1"))
        );
        assert!(
            st.challenge.lock().unwrap().is_some(),
            "PKCE challenge sent"
        );
        assert_eq!(
            String::from_utf8(e.response_body(&e.send(&req).await.unwrap())).unwrap(),
            "Bearer tok-code"
        );
        e.oauth2_clear(&req).unwrap();
        assert!(e.oauth2_cached(&req).unwrap().is_none());
    }
}

mod phase5 {
    use super::*;
    use crate::transfer::{ExportFormat, ExportOptions, ImportMode, ImportOptions};
    use lsock_core::{LlmRequest, McpServer, McpTransport, OAuth2Token};

    fn raw_value(e: &Engine, env_id: &str, key: &str) -> Value {
        e.store.raw(env_id).unwrap().unwrap().data["data"][key].clone()
    }

    #[tokio::test]
    async fn secrets_are_encrypted_at_rest_and_used_when_sending() {
        let base = server().await;
        let f = fixture(&base);
        let staging = f.ws.active_environment_id.clone().unwrap();
        f.e.set_env_var(&staging, "user", json!("s3cr3t-user"), true)
            .unwrap();
        let stored = raw_value(&f.e, &staging, "user");
        assert!(
            stored.as_str().unwrap().starts_with(vault::PREFIX),
            "{stored}"
        );
        assert!(!stored.to_string().contains("s3cr3t"));
        // rendering and sending see the plaintext
        let resp = f.e.send(f.req.id()).await.unwrap();
        assert_eq!(body_json(&f.e, &resp)["path"], "/users/s3cr3t-user");
        assert_eq!(f.e.reveal_secret(&staging, "user").unwrap(), "s3cr3t-user");
        // a UI save that sends plaintext for a secret key is sealed again
        let mut d: Doc<Environment> = f.e.store.get(&staging).unwrap();
        d.body.data.insert("user".into(), json!("typed-in"));
        f.e.update_environment(&d).unwrap();
        assert!(vault::is_sealed(&raw_value(&f.e, &staging, "user")));
        // unmarking decrypts in place
        f.e.set_env_var(&staging, "user", json!("typed-in"), false)
            .unwrap();
        assert_eq!(raw_value(&f.e, &staging, "user"), json!("typed-in"));
        assert_eq!(f.e.vault_status().unwrap().sealed_values, 0);
    }

    #[tokio::test]
    async fn scripts_read_and_write_secrets_through_the_vault() {
        let base = server().await;
        let f = fixture(&base);
        let staging = f.ws.active_environment_id.clone().unwrap();
        f.e.set_env_var(&staging, "user", json!("alice"), true)
            .unwrap();
        let mut r = f.e.store.get::<Request>(f.req.id()).unwrap();
        r.pre_request_script =
            Some("ls.environment.set('user', ls.environment.get('user') + '-2');".into());
        f.e.store.update(&r).unwrap();
        let resp = f.e.send(f.req.id()).await.unwrap();
        assert_eq!(body_json(&f.e, &resp)["path"], "/users/alice-2");
        assert!(
            vault::is_sealed(&raw_value(&f.e, &staging, "user")),
            "script write re-sealed"
        );
        assert_eq!(f.e.reveal_secret(&staging, "user").unwrap(), "alice-2");
    }

    #[test]
    fn vault_key_moves_between_machines_and_wrong_keys_are_refused() {
        let f = fixture("http://127.0.0.1:1");
        let staging = f.ws.active_environment_id.clone().unwrap();
        f.e.set_env_var(&staging, "user", json!("bob"), true)
            .unwrap();
        let key = f.e.vault_export_key().unwrap();
        // same database, a machine without the key
        let other =
            f.e.clone()
                .with_secrets(Arc::new(llm::MemoryStore::default()));
        let e = other.prepare(f.req.id(), &[]).unwrap_err().to_string();
        assert!(e.contains("vault key") && e.contains("'user'"), "{e}");
        let wrong = base64::engine::general_purpose::STANDARD.encode([7u8; 32]);
        assert!(
            other
                .vault_import_key(&wrong)
                .unwrap_err()
                .to_string()
                .contains("different key")
        );
        other.vault_import_key(&key).unwrap();
        assert_eq!(other.reveal_secret(&staging, "user").unwrap(), "bob");
        assert_eq!(other.vault_reset().unwrap(), 1);
        assert_eq!(raw_value(&other, &staging, "user"), json!(""));
        assert!(!other.vault_status().unwrap().has_key);
    }

    #[test]
    fn oauth2_tokens_are_sealed_in_the_database() {
        let f = fixture("http://127.0.0.1:1");
        f.e.store
            .insert(
                Some(f.folder.id()),
                OAuth2Token {
                    access_token: f.e.seal_value("tok-123").unwrap(),
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(
            f.e.oauth2_cached(f.folder.id())
                .unwrap()
                .unwrap()
                .access_token,
            "tok-123"
        );
        let raw = f.e.store.children::<OAuth2Token>(f.folder.id()).unwrap();
        assert!(raw[0].access_token.starts_with(vault::PREFIX));
    }

    fn postman() -> String {
        json!({
            "info": {"name": "Shop", "schema": "https://schema.getpostman.com/json/collection/v2.1.0/collection.json"},
            "variable": [{"key": "base", "value": "https://shop.test"}],
            "item": [
                {"name": "Orders", "item": [
                    {"name": "List", "request": {"method": "GET", "url": "{{base}}/orders?page=1", "header": []}},
                    {"name": "Create", "request": {"method": "POST", "url": "{{base}}/orders",
                        "body": {"mode": "raw", "raw": "{\"sku\":\"{{sku}}\"}", "options": {"raw": {"language": "json"}}}},
                     "event": [{"listen": "test", "script": {"exec": ["pm.test('created', () => pm.response.to.have.status(201));"]}}]}
                ]}
            ]
        })
        .to_string()
    }

    #[test]
    fn import_copy_mode_creates_independent_workspaces() {
        let e = Engine::in_memory();
        let s = e
            .import_text(&postman(), &ImportOptions::default())
            .unwrap();
        assert_eq!(
            (s.requests, s.folders, s.workspaces.clone()),
            (2, 1, vec!["Shop".to_string()])
        );
        let ws = &s.workspace_ids[0];
        assert_eq!(
            e.base_environment(ws).unwrap().data["base"],
            json!("https://shop.test")
        );
        let create = e
            .store
            .all_of::<Request>()
            .unwrap()
            .into_iter()
            .find(|r| r.name == "Create")
            .unwrap();
        assert_eq!(
            create.after_response_script.as_deref(),
            Some("ls.test('created', () => ls.response.to.have.status(201));")
        );
        let p = e
            .prepare(create.id(), &[Layer::new("x", vars(json!({"sku": "A1"})))])
            .unwrap();
        assert_eq!(p.request.url, "https://shop.test/orders");
        assert_eq!(p.request.body.text.as_deref(), Some("{\"sku\":\"A1\"}"));
        let again = e
            .import_text(&postman(), &ImportOptions::default())
            .unwrap();
        assert_ne!(again.workspace_ids, s.workspace_ids);
        assert_eq!(e.store.all_of::<Request>().unwrap().len(), 4);
    }

    #[test]
    fn replace_mode_round_trips_keeping_local_state() {
        let e = Engine::in_memory();
        let ws = e
            .import_text(&postman(), &ImportOptions::default())
            .unwrap()
            .workspace_ids
            .remove(0);
        // local-only state: a secret, a private env, an MCP server referenced by an AI request
        let base = e.base_environment(&ws).unwrap();
        e.set_env_var(base.id(), "api_key", json!("k-local"), true)
            .unwrap();
        let private = e
            .store
            .insert(
                Some(base.id()),
                Environment {
                    name: "Mine".into(),
                    is_private: true,
                    ..Default::default()
                },
            )
            .unwrap();
        let mcp = e
            .store
            .insert(
                Some(&ws),
                McpServer {
                    name: "Tools".into(),
                    transport: McpTransport::StreamableHttp {
                        url: "http://x/mcp".into(),
                    },
                    ..Default::default()
                },
            )
            .unwrap();
        e.store
            .insert(
                Some(&ws),
                LlmRequest {
                    name: "Ask".into(),
                    mcp_server_ids: vec![mcp.meta.id.clone()],
                    ..Default::default()
                },
            )
            .unwrap();
        let list = e
            .store
            .all_of::<Request>()
            .unwrap()
            .into_iter()
            .find(|r| r.name == "List")
            .unwrap();
        e.store
            .insert(
                Some(list.id()),
                Response {
                    status_code: 200,
                    ..Default::default()
                },
            )
            .unwrap();

        let out = e
            .export_workspace(&ws, ExportFormat::LogicSocket, &ExportOptions::default())
            .unwrap();
        assert_eq!(out.file_name, "logic-socket.shop.yaml");
        assert!(
            !out.content.contains("k-local"),
            "secret values never leave the machine"
        );
        assert!(out.content.contains("api_key"), "secret names do");
        assert!(
            !out.content.contains("Mine"),
            "private environments stay local"
        );
        assert_eq!(out.warnings.len(), 1);

        // edit the file like a teammate would: rename one request, drop another
        let edited = out
            .content
            .replace("name: List", "name: List orders")
            .replace("name: Create", "name: Create order");
        let mut v: serde_json::Value = serde_yaml_ng::from_str(&edited).unwrap();
        v["items"][0]["children"]
            .as_array_mut()
            .unwrap()
            .retain(|c| c["name"] != "Create order");
        let edited = serde_yaml_ng::to_string(&v).unwrap();
        let before = e.store.all_of::<Workspace>().unwrap().len();
        let s = e
            .import_text(
                &edited,
                &ImportOptions {
                    mode: ImportMode::Replace,
                    into_workspace: None,
                },
            )
            .unwrap();
        assert_eq!(
            s.workspace_ids,
            vec![ws.clone()],
            "same workspace, updated in place"
        );
        assert_eq!(e.store.all_of::<Workspace>().unwrap().len(), before);
        let names: Vec<String> = e
            .store
            .all_of::<Request>()
            .unwrap()
            .into_iter()
            .map(|r| r.name.clone())
            .collect();
        assert_eq!(names, vec!["List orders".to_string()]);
        let list2 = e.store.all_of::<Request>().unwrap().remove(0);
        assert_eq!(list2.meta.id, list.meta.id, "ids are stable");
        assert_eq!(e.responses(list.id()).unwrap().len(), 1, "history survives");
        assert_eq!(
            e.reveal_secret(base.id(), "api_key").unwrap(),
            "k-local",
            "local secret kept"
        );
        assert!(
            e.store.get::<Environment>(private.id()).is_ok(),
            "private env kept"
        );
        let ask = e.store.all_of::<LlmRequest>().unwrap().remove(0);
        assert_eq!(ask.mcp_server_ids, vec![mcp.meta.id.clone()]);
    }

    #[test]
    fn copy_mode_rekeys_mcp_references_and_merge_into_workspace() {
        let e = Engine::in_memory();
        let ws = e
            .store
            .insert(
                None,
                Workspace {
                    name: "Src".into(),
                    ..Default::default()
                },
            )
            .unwrap();
        let mcp = e
            .store
            .insert(
                Some(ws.id()),
                McpServer {
                    name: "Tools".into(),
                    ..Default::default()
                },
            )
            .unwrap();
        e.store
            .insert(
                Some(ws.id()),
                LlmRequest {
                    name: "Ask".into(),
                    mcp_server_ids: vec![mcp.meta.id.clone()],
                    ..Default::default()
                },
            )
            .unwrap();
        let out = e
            .export_workspace(
                ws.id(),
                ExportFormat::LogicSocket,
                &ExportOptions::default(),
            )
            .unwrap();
        let copy = e
            .import_text(&out.content, &ImportOptions::default())
            .unwrap()
            .workspace_ids
            .remove(0);
        let new_mcp = e.store.children::<McpServer>(&copy).unwrap().remove(0);
        let new_ask = e.store.children::<LlmRequest>(&copy).unwrap().remove(0);
        assert_ne!(new_mcp.meta.id, mcp.meta.id);
        assert_eq!(new_ask.mcp_server_ids, vec![new_mcp.meta.id.clone()]);

        // a Postman environment merged into an existing workspace becomes a sub-environment
        let env = json!({"name": "Prod", "values": [{"key": "token", "value": "t0p", "type": "secret", "enabled": true}]}).to_string();
        e.import_text(
            &env,
            &ImportOptions {
                mode: ImportMode::Copy,
                into_workspace: Some(ws.meta.id.clone()),
            },
        )
        .unwrap();
        let subs = e.sub_environments(ws.id()).unwrap();
        assert_eq!(subs[0].name, "Prod");
        assert!(
            vault::is_sealed(&subs[0].data["token"]),
            "imported secrets are sealed"
        );
    }

    #[test]
    fn postman_and_har_exports() {
        let e = Engine::in_memory();
        let ws = e
            .import_text(&postman(), &ImportOptions::default())
            .unwrap()
            .workspace_ids
            .remove(0);
        let rt = e
            .store
            .insert(Some(&ws), lsock_core::RealtimeRequest::default())
            .unwrap();
        let out = e
            .export_workspace(&ws, ExportFormat::Postman, &ExportOptions::default())
            .unwrap();
        assert_eq!(out.file_name, "shop.postman_collection.json");
        assert!(
            out.warnings.iter().any(|w| w.contains("1 item")),
            "{:?}",
            out.warnings
        );
        let v: Value = serde_json::from_str(&out.content).unwrap();
        assert_eq!(
            v["item"][0]["item"][1]["event"][0]["script"]["exec"][0],
            "pm.test('created', () => pm.response.to.have.status(201));"
        );
        let har = e
            .export_workspace(&ws, ExportFormat::Har, &ExportOptions::default())
            .unwrap();
        assert!(har.content.contains("\"entries\""));
        let _ = rt;
    }

    #[test]
    fn code_generation_uses_the_rendered_request() {
        let f = fixture("https://api.test");
        let (code, notes) = f.e.code_request(f.req.id()).unwrap();
        assert!(notes.is_empty());
        assert_eq!(code.method, "POST");
        assert_eq!(
            code.url,
            "https://api.test/users/staging-user?who=staging-user"
        );
        assert!(
            code.headers
                .contains(&("Authorization".into(), "Bearer folder-token".into()))
        );
        assert!(
            code.headers
                .contains(&("Content-Type".into(), "application/json".into()))
        );
        let curl = lsock_convert::codegen::generate(&code, lsock_convert::codegen::Target::Curl);
        assert!(
            curl.contains("--header 'Authorization: Bearer folder-token'"),
            "{curl}"
        );
        assert!(
            curl.contains(r#"--data-raw '{"u":"staging-user"}'"#),
            "{curl}"
        );
    }
}

mod git_sync {
    use super::*;
    use lsock_core::GitRepo;

    fn teammate(name: &str) -> Engine {
        let _ = name;
        Engine::in_memory().with_secrets(Arc::new(llm::MemoryStore::default()))
    }

    fn set_author(e: &Engine, repo: &Doc<GitRepo>, who: &str, remote: &str) -> Doc<GitRepo> {
        let mut r = repo.clone();
        r.body.author_name = who.into();
        r.body.author_email = format!("{}@example.com", who.to_lowercase());
        r.body.remote_url = remote.into();
        e.git_update_repo(&r).unwrap()
    }

    fn rename(e: &Engine, from: &str, to: &str) {
        let mut r = e
            .store
            .all_of::<Request>()
            .unwrap()
            .into_iter()
            .find(|r| r.name == from)
            .unwrap();
        r.body.name = to.into();
        e.store.update(&r).unwrap();
    }

    fn names(e: &Engine) -> Vec<String> {
        let mut n: Vec<String> = e
            .store
            .all_of::<Request>()
            .unwrap()
            .into_iter()
            .map(|r| r.name.clone())
            .collect();
        n.sort();
        n
    }

    #[test]
    fn two_teammates_share_a_workspace_through_git() {
        if std::process::Command::new("git")
            .arg("--version")
            .output()
            .is_err()
        {
            eprintln!("git not installed; skipping");
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let remote = tmp.path().join("remote.git");
        assert!(
            std::process::Command::new("git")
                .args(["init", "--bare", "-b", "main"])
                .arg(&remote)
                .status()
                .unwrap()
                .success()
        );
        let remote = remote.to_string_lossy().into_owned();

        // Ana creates a workspace with a secret and shares it
        let ana = teammate("Ana");
        let ws = ana
            .store
            .insert(
                None,
                Workspace {
                    name: "Shop API".into(),
                    ..Default::default()
                },
            )
            .unwrap();
        let base = ana.base_environment(ws.id()).unwrap();
        ana.set_env_var(base.id(), "host", json!("https://shop.test"), false)
            .unwrap();
        ana.set_env_var(base.id(), "token", json!("ana-secret-token"), true)
            .unwrap();
        ana.store
            .insert(
                Some(ws.id()),
                Request {
                    name: "List orders".into(),
                    url: "{{ _.host }}/orders".into(),
                    ..Default::default()
                },
            )
            .unwrap();
        ana.store
            .insert(
                Some(ws.id()),
                Request {
                    name: "Get order".into(),
                    url: "{{ _.host }}/orders/:id".into(),
                    ..Default::default()
                },
            )
            .unwrap();
        let (repo_a, _) = ana
            .git_open(&tmp.path().join("ana").to_string_lossy(), None)
            .unwrap();
        let repo_a = set_author(&ana, &repo_a, "Ana", &remote);
        let repo_a = ana.git_link(repo_a.id(), ws.id()).unwrap();
        assert_eq!(repo_a.files[0].path, "logic-socket.shop-api.yaml");
        let st = ana.git_status(repo_a.id()).unwrap();
        assert_eq!(st.branch, "main");
        assert_eq!(st.changes.len(), 1);
        assert_eq!(
            (
                st.changes[0].status.as_str(),
                st.changes[0].workspace.as_deref()
            ),
            ("untracked", Some("Shop API"))
        );
        ana.git_commit(repo_a.id(), "Add Shop API", &[]).unwrap();
        ana.git_push(repo_a.id()).unwrap();
        let file =
            std::fs::read_to_string(tmp.path().join("ana/logic-socket.shop-api.yaml")).unwrap();
        assert!(
            !file.contains("ana-secret-token") && !file.contains("vault:v1"),
            "no secret in the repo:\n{file}"
        );
        assert!(file.contains("secretKeys"));

        // Ben clones it: same ids, secret name without its value
        let ben = teammate("Ben");
        let (repo_b, res) = ben
            .git_clone(&remote, &tmp.path().join("ben").to_string_lossy(), None)
            .unwrap();
        assert_eq!(res.workspaces, vec!["Shop API".to_string()]);
        let repo_b = set_author(&ben, &repo_b, "Ben", &remote);
        assert_eq!(repo_b.files[0].workspace_id, ws.meta.id);
        assert_eq!(names(&ben), ["Get order", "List orders"]);
        let ben_base = ben.base_environment(ws.id()).unwrap();
        assert_eq!(ben_base.data["token"], json!(""));
        assert!(ben_base.secret_keys.contains(&"token".to_string()));
        assert!(
            ben.git_status(repo_b.id()).unwrap().changes.is_empty(),
            "fresh clone is clean (no phantom diffs)"
        );
        ben.set_env_var(ben_base.id(), "token", json!("ben-own-token"), true)
            .unwrap();
        assert!(
            ben.git_status(repo_b.id()).unwrap().changes.is_empty(),
            "setting a secret value changes nothing in Git"
        );

        // Ben renames a request and pushes; Ana pulls
        rename(&ben, "Get order", "Get order by id");
        let diff = ben
            .git_diff(repo_b.id(), "logic-socket.shop-api.yaml")
            .unwrap();
        assert!(
            diff.contains("+    name: Get order by id")
                || diff.contains("+  - name: Get order by id")
                || diff.contains("Get order by id"),
            "{diff}"
        );
        ben.git_commit(repo_b.id(), "Rename", &[]).unwrap();
        ben.git_push(repo_b.id()).unwrap();
        let pulled = ana.git_pull(repo_a.id()).unwrap();
        assert!(pulled.conflicts.is_empty());
        assert_eq!(names(&ana), ["Get order by id", "List orders"]);
        assert_eq!(
            ana.reveal_secret(base.id(), "token").unwrap(),
            "ana-secret-token",
            "Ana keeps her secret"
        );
        assert!(
            ana.git_status(repo_a.id()).unwrap().changes.is_empty(),
            "clean after pull"
        );
        assert_eq!(
            ana.git_log(repo_a.id(), 10)
                .unwrap()
                .iter()
                .map(|c| c.message.as_str())
                .collect::<Vec<_>>(),
            ["Rename", "Add Shop API"]
        );

        // pulling with uncommitted edits is refused
        rename(&ana, "List orders", "List all orders");
        assert!(
            ana.git_pull(repo_a.id())
                .unwrap_err()
                .to_string()
                .contains("commit or discard")
        );
        // discard brings back the committed version
        ana.git_discard(repo_a.id(), "logic-socket.shop-api.yaml")
            .unwrap();
        assert_eq!(names(&ana), ["Get order by id", "List orders"]);

        // both edit the same request: conflict, resolved by taking Ben's version
        rename(&ana, "List orders", "Orders (Ana)");
        ana.git_commit(repo_a.id(), "Ana's name", &[]).unwrap();
        ben.git_pull(repo_b.id()).unwrap();
        rename(&ben, "List orders", "Orders (Ben)");
        ben.git_commit(repo_b.id(), "Ben's name", &[]).unwrap();
        ben.git_push(repo_b.id()).unwrap();
        let res = ana.git_pull(repo_a.id()).unwrap();
        assert_eq!(
            res.conflicts,
            vec!["logic-socket.shop-api.yaml".to_string()]
        );
        assert_eq!(
            names(&ana),
            ["Get order by id", "Orders (Ana)"],
            "nothing imported while conflicted"
        );
        let res = ana
            .git_resolve(repo_a.id(), "logic-socket.shop-api.yaml", "theirs")
            .unwrap();
        assert!(res.conflicts.is_empty());
        assert_eq!(names(&ana), ["Get order by id", "Orders (Ben)"]);
        ana.git_push(repo_a.id()).unwrap();

        // branches
        let res = ana.git_checkout(repo_a.id(), "feature", true).unwrap();
        assert!(res.conflicts.is_empty());
        let (current, all) = ana.git_branches(repo_a.id()).unwrap();
        assert_eq!(current, "feature");
        assert!(all.contains(&"main".to_string()));
    }
}
