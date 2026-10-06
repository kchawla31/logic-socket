use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use axum::{
    Router,
    body::Bytes,
    http::{HeaderMap, Uri},
    routing::any,
};
use lsock_core::{Folder, KeyValue, Workspace};
use serde_json::{Value, json};

use super::*;

async fn server() -> String {
    let hits = Arc::new(AtomicUsize::new(0));
    let app = Router::new().route(
        "/{*p}",
        any(move |uri: Uri, headers: HeaderMap, body: Bytes| {
            let n = hits.fetch_add(1, Ordering::SeqCst) + 1;
            async move {
                let h: serde_json::Map<String, Value> =
                    headers.iter().map(|(k, v)| (k.to_string(), json!(v.to_str().unwrap_or("")))).collect();
                let status = if uri.path() == "/missing" { 404 } else { 200 };
                (
                    axum::http::StatusCode::from_u16(status).unwrap(),
                    [("content-type", "application/json")],
                    json!({ "path": uri.path(), "query": uri.query(), "headers": h, "hit": n, "body": String::from_utf8_lossy(&body) }).to_string(),
                )
            }
        }),
    );
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
    format!("http://{addr}")
}

struct Fx {
    e: Engine,
    ids: Vec<String>,
}

fn add(e: &Engine, parent: &str, name: &str, url: String, pre: &str, after: &str) -> String {
    e.store
        .insert(
            Some(parent),
            Request {
                name: name.into(),
                url,
                pre_request_script: Some(pre.into()).filter(|s: &String| !s.is_empty()),
                after_response_script: Some(after.into()).filter(|s: &String| !s.is_empty()),
                ..Default::default()
            },
        )
        .unwrap()
        .meta
        .id
}

async fn fixture() -> Fx {
    let base = server().await;
    let e = Engine::in_memory();
    let ws = e
        .store
        .insert(
            None,
            Workspace {
                name: "Shop".into(),
                ..Default::default()
            },
        )
        .unwrap();
    let mut env = e.base_environment(ws.id()).unwrap();
    env.data.insert("base".into(), json!(base));
    e.store.update(&env).unwrap();
    let folder = e
        .store
        .insert(
            Some(ws.id()),
            Folder {
                name: "Flow".into(),
                ..Default::default()
            },
        )
        .unwrap();
    let login = add(
        &e,
        folder.id(),
        "Login",
        "{{ _.base }}/login".into(),
        "",
        "ls.environment.set('token', 'tok-' + ls.info.iteration); ls.test('login ok', () => ls.response.to.have.status(200));",
    );
    let mut items_req = Request {
        name: "Get items".into(),
        url: "{{ _.base }}/items".into(),
        after_response_script: Some(
            "ls.test('has auth', () => ls.expect(ls.response.json().headers['x-token']).to.equal('tok-' + ls.info.iteration));\n\
             ls.test('row user', () => ls.expect(ls.response.json().query).to.equal('user=' + ls.iterationData.get('user')));"
                .into(),
        ),
        ..Default::default()
    };
    items_req
        .headers
        .push(KeyValue::new("X-Token", "{{ _.token }}"));
    items_req
        .parameters
        .push(KeyValue::new("user", "{{ user }}"));
    let items = e
        .store
        .insert(Some(folder.id()), items_req)
        .unwrap()
        .meta
        .id;
    Fx {
        e,
        ids: vec![login, items],
    }
}

#[tokio::test]
async fn three_iterations_with_csv_data() {
    let fx = fixture().await;
    let data = parse_data("user,role\nada,admin\nbob, viewer \n").unwrap();
    assert_eq!(data[1]["role"], json!("viewer"));
    let mut events = vec![];
    let opts = RunOptions {
        iterations: 3,
        data,
        ..Default::default()
    };
    let s = run(&fx.e, &fx.ids, &opts, |e| events.push(e.clone())).await;
    assert!(s.ok(), "{}", report::spec(&s));
    assert_eq!((s.requests, s.tests_passed, s.tests_failed), (6, 9, 0));
    // iteration 3 cycles back to row 1
    assert_eq!(s.results[5].iteration, 3);
    assert!(matches!(
        events.first(),
        Some(RunEvent::RunStart {
            requests: 2,
            iterations: 3
        })
    ));
    assert!(matches!(events.last(), Some(RunEvent::Done { .. })));
    assert_eq!(
        events
            .iter()
            .filter(|e| matches!(e, RunEvent::RequestEnd { .. }))
            .count(),
        6
    );
}

#[tokio::test]
async fn set_next_request_loops_and_stops() {
    let fx = fixture().await;
    let ws = fx.e.workspace_of(&fx.ids[0]).unwrap();
    let base = fx.e.base_environment(ws.id()).unwrap().data["base"]
        .as_str()
        .unwrap()
        .to_string();
    let poll = add(
        &fx.e,
        ws.id(),
        "Poll",
        format!("{base}/poll"),
        "",
        r#"const n = (ls.variables.get('polls') || 0) + 1;
           ls.variables.set('polls', n);
           if (n < 3) ls.execution.setNextRequest('Poll'); else ls.execution.setNextRequest(null);"#,
    );
    let never = add(&fx.e, ws.id(), "Never", format!("{base}/never"), "", "");
    let s = run(&fx.e, &[poll, never], &RunOptions::default(), |_| {}).await;
    let names: Vec<&str> = s.results.iter().map(|r| r.name.as_str()).collect();
    assert_eq!(
        names,
        ["Poll", "Poll", "Poll"],
        "loops 3 times then stops before 'Never'"
    );

    // unknown target → warning, iteration stops
    let bad = add(
        &fx.e,
        ws.id(),
        "Bad",
        format!("{base}/bad"),
        "ls.execution.setNextRequest('Nope');",
        "",
    );
    let mut warnings = vec![];
    run(&fx.e, &[bad], &RunOptions::default(), |e| {
        if let RunEvent::Warning { message, .. } = e {
            warnings.push(message.clone())
        }
    })
    .await;
    assert!(warnings[0].contains("no request with that name"));
}

#[tokio::test]
async fn bail_stops_on_first_failure_and_skip_is_reported() {
    let fx = fixture().await;
    let ws = fx.e.workspace_of(&fx.ids[0]).unwrap();
    let base = fx.e.base_environment(ws.id()).unwrap().data["base"]
        .as_str()
        .unwrap()
        .to_string();
    let skip = add(
        &fx.e,
        ws.id(),
        "Skip me",
        format!("{base}/skip"),
        "ls.execution.skipRequest();",
        "",
    );
    let fail = add(
        &fx.e,
        ws.id(),
        "Missing",
        format!("{base}/missing"),
        "",
        "ls.test('is 200', () => ls.response.to.have.status(200));",
    );
    let ids = vec![skip, fail, fx.ids[0].clone()];
    let s = run(
        &fx.e,
        &ids,
        &RunOptions {
            iterations: 2,
            bail: true,
            ..Default::default()
        },
        |_| {},
    )
    .await;
    assert!(s.bailed);
    assert_eq!(
        s.results.len(),
        2,
        "stopped right after the failing request"
    );
    assert!(s.results[0].skipped && !s.results[0].failed());
    assert!(s.results[1].failed());
    assert_eq!((s.requests_skipped, s.tests_failed), (1, 1));

    let s = run(
        &fx.e,
        &ids,
        &RunOptions {
            iterations: 2,
            ..Default::default()
        },
        |_| {},
    )
    .await;
    assert_eq!(s.results.len(), 6);
    assert!(!s.ok());
}

#[tokio::test]
async fn reporters_produce_valid_output() {
    let fx = fixture().await;
    let data = parse_data(r#"[{"user":"ada"},{"user":"<b>&'\""}]"#).unwrap();
    let s = run(
        &fx.e,
        &fx.ids,
        &RunOptions {
            iterations: 2,
            data,
            ..Default::default()
        },
        |_| {},
    )
    .await;

    let xml = report::junit(&s);
    let doc = roxmltree::Document::parse(&xml).expect("well-formed JUnit XML");
    let root = doc.root_element();
    assert_eq!(root.tag_name().name(), "testsuites");
    assert_eq!(root.attribute("tests"), Some("10"));
    assert_eq!(root.children().filter(|n| n.is_element()).count(), 4);

    let parsed: Value = serde_json::from_str(&report::json(&s)).unwrap();
    assert_eq!(parsed["requests"], 4);
    assert!(report::spec(&s).contains("✓ GET Login — 200 OK"));
    // row 2's special characters are URL-encoded in the query, so its "row user" test fails
    assert!(report::dot(&s).starts_with("...F\n"), "{}", report::dot(&s));
    assert!(report::dot(&s).contains("✗ row user"));
    assert!(report::render(&s, "nope").is_none());
}

#[test]
fn data_file_errors() {
    assert!(matches!(parse_data("[1,2]"), Err(RunError::DataFormat(_))));
    assert!(matches!(
        load_data_file("/definitely/missing.csv"),
        Err(RunError::DataFile(..))
    ));
}
