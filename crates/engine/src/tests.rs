use super::*;
use axum::{Router, body::Bytes, http::HeaderMap, http::Uri, routing::any};
use irs_core::{Body, mime};
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

fn vars(v: Value) -> irs_core::VarMap {
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
fn layers_follow_insomnia_precedence() {
    let f = fixture("http://x");
    // global env workspace
    let gws =
        f.e.store
            .insert(
                None,
                Workspace {
                    name: "Globals".into(),
                    scope: irs_core::WorkspaceScope::Environment,
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
async fn scripts_run_in_insomnia_order_and_persist_results() {
    let base = server().await;
    let f = fixture(&base);
    // outer folder (parent of `f.folder`) gets scripts too
    let outer_id = f.folder.meta.parent_id.clone().unwrap();
    let mut outer = f.e.store.get::<Folder>(&outer_id).unwrap();
    outer.pre_request_script =
        Some("console.log('outer pre'); insomnia.variables.set('trail', 'outer');".into());
    outer.after_response_script = Some("console.log('outer after');".into());
    f.e.store.update(&outer).unwrap();
    let mut inner = f.e.store.get::<Folder>(f.folder.id()).unwrap();
    inner.pre_request_script = Some("console.log('inner pre'); insomnia.variables.set('trail', insomnia.variables.get('trail') + '>inner');".into());
    inner.after_response_script = Some("console.log('inner after');".into());
    f.e.store.update(&inner).unwrap();
    let mut req = f.e.store.get::<Request>(f.req.id()).unwrap();
    req.pre_request_script = Some(
        r#"console.log('request pre');
        insomnia.environment.set('user', 'from-script');
        insomnia.request.headers.add({ key: 'X-Trail', value: '{{ trail }}>request' });"#
            .into(),
    );
    req.after_response_script = Some(
        r#"console.log('request after');
        const body = insomnia.response.json();
        insomnia.environment.set('lastPath', body.path);
        insomnia.test('status ok', () => insomnia.expect(insomnia.response.code).to.equal(200));
        insomnia.test('header came through', () => insomnia.expect(body.headers['x-trail']).to.equal('outer>inner>request'));
        insomnia.test('fails on purpose', () => insomnia.expect(1).to.equal(2));"#
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
    req.pre_request_script = Some(
        "insomnia.execution.skipRequest(); insomnia.execution.setNextRequest('Other');".into(),
    );
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
        r#"const r = await insomnia.sendRequest({{ url: '{base}/token', method: 'POST', body: {{ mode: 'raw', raw: 'x' }} }});
        insomnia.variables.set('tokenPath', r.json().path);"#
    ));
    req.headers
        .push(irs_core::KeyValue::new("X-Token-Path", "{{ tokenPath }}"));
    req.headers
        .push(irs_core::KeyValue::new("X-Row", "{{ email }}"));
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
async fn without_sub_environment_insomnia_environment_is_the_base() {
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
                    "insomnia.environment.set('token', 'abc'); insomnia.environment.unset('drop'); insomnia.baseEnvironment.set('other', true);".into(),
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
                pre_request_script: Some("insomnia.request.headers.add({ key: 'X-T', value: insomnia.environment.get('token') });".into()),
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
