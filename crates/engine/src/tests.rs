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
