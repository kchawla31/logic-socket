use std::time::Duration;

use serde_json::json;

use super::*;

async fn wait_done(c: &StreamCall) -> Vec<irs_realtime::RtEvent> {
    for _ in 0..200 {
        if c.is_done() {
            return c.log.entries();
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("stream did not finish: {:#?}", c.log.entries());
}

fn ins(e: &[irs_realtime::RtEvent]) -> Vec<serde_json::Value> {
    e.iter()
        .filter(|x| x.direction == Direction::In)
        .map(|x| serde_json::from_str(&x.data).unwrap())
        .collect()
}

#[test]
fn schema_from_protos_lists_methods_with_examples() {
    let s = demo::schema();
    let svcs = s.services();
    assert_eq!(svcs.len(), 1);
    let m: Vec<(&str, bool, bool)> = svcs[0]
        .methods
        .iter()
        .map(|m| (m.name.as_str(), m.client_streaming, m.server_streaming))
        .collect();
    assert_eq!(
        m,
        [
            ("SayHello", false, false),
            ("CountUp", false, true),
            ("Sum", true, false),
            ("Chat", true, true)
        ]
    );
    assert_eq!(svcs[0].methods[0].path, "/demo.Greeter/SayHello");
    assert_eq!(
        svcs[0].methods[0].example,
        json!({"name": "", "tags": [], "mood": "MOOD_UNSPECIFIED"})
    );
    let err = Schema::from_protos(&[(
        "bad.proto".into(),
        "syntax = \"proto3\"; message X { string a = }".into(),
    )])
    .unwrap_err();
    assert_eq!(
        err.to_string(),
        "proto error: bad.proto:1:43: expected an integer, but found '}'"
    );
}

#[tokio::test]
async fn unary_success_errors_and_metadata() {
    let url = demo::spawn(0, false).await.unwrap();
    let ch = connect(&url).await.unwrap();
    let s = demo::schema();
    let m = s.method("/demo.Greeter/SayHello").unwrap();
    let r = unary(
        ch.clone(),
        &m,
        r#"{"name":"Ada","mood":"HAPPY","tags":["x"]}"#,
        &[],
        Duration::from_secs(5),
    )
    .await
    .unwrap();
    assert_eq!(r.status, StatusInfo::ok());
    let resp = r.response.unwrap();
    assert_eq!(
        (resp["message"].clone(), resp["length"].clone()),
        (json!("Hello, Ada!"), json!(11))
    );
    assert!(
        resp["at"].as_str().unwrap().ends_with('Z'),
        "Timestamp as RFC 3339: {resp}"
    );
    assert!(
        r.headers
            .iter()
            .any(|(k, v)| k == "x-served-by" && v == "irs-demo")
    );

    let r = unary(ch.clone(), &m, "{}", &[], Duration::from_secs(5))
        .await
        .unwrap();
    assert_eq!(
        (
            r.status.code,
            r.status.code_name.as_str(),
            r.status.message.as_str()
        ),
        (3, "InvalidArgument", "name is required")
    );
    let r = unary(
        ch.clone(),
        &m,
        r#"{"name":"x"}"#,
        &[("X-Fail".into(), "1".into())],
        Duration::from_secs(5),
    )
    .await
    .unwrap();
    assert_eq!(r.status.code_name, "PermissionDenied");
    let err = unary(
        ch.clone(),
        &m,
        r#"{"nmae":"typo"}"#,
        &[],
        Duration::from_secs(5),
    )
    .await
    .unwrap_err();
    assert!(
        err.to_string().contains("demo.HelloRequest") && err.to_string().contains("nmae"),
        "{err}"
    );
    assert!(matches!(
        s.method("/demo.Greeter/Nope"),
        Err(GrpcError::UnknownMethod(_))
    ));
}

#[tokio::test]
async fn server_client_and_bidi_streaming() {
    let url = demo::spawn(0, false).await.unwrap();
    let ch = connect(&url).await.unwrap();
    let s = demo::schema();

    let c = start_stream(
        ch.clone(),
        &s.method("demo.Greeter/CountUp").unwrap(),
        Some(r#"{"to":3}"#),
        &[],
    )
    .await
    .unwrap();
    let e = wait_done(&c).await;
    assert_eq!(
        ins(&e),
        vec![
            json!({"value": 1}),
            json!({"value": 2}),
            json!({"value": 3})
        ]
    );
    assert!(
        e.iter()
            .any(|x| x.kind == "close" && x.data.starts_with("Status OK"))
    );

    let mut c = start_stream(
        ch.clone(),
        &s.method("demo.Greeter/Sum").unwrap(),
        None,
        &[],
    )
    .await
    .unwrap();
    for v in [1, 2, 39] {
        c.send(&format!(r#"{{"value":{v}}}"#)).unwrap();
    }
    assert!(
        c.send(r#"{"value":"x"}"#).is_err(),
        "invalid message rejected locally"
    );
    c.commit();
    let e = wait_done(&c).await;
    assert_eq!(ins(&e), vec![json!({"value": 42})]);

    let mut c = start_stream(
        ch.clone(),
        &s.method("demo.Greeter/Chat").unwrap(),
        None,
        &[],
    )
    .await
    .unwrap();
    c.send(r#"{"user":"ada","text":"hi"}"#).unwrap();
    c.send(r#"{"user":"ada","text":"bye"}"#).unwrap();
    for _ in 0..100 {
        if ins(&c.log.entries()).len() == 2 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    c.commit();
    let e = wait_done(&c).await;
    assert_eq!(
        ins(&e),
        vec![
            json!({"user": "bot", "text": "you said: hi"}),
            json!({"user": "bot", "text": "you said: bye"})
        ]
    );
}

#[tokio::test]
async fn reflection_builds_schema_and_errors_are_readable() {
    let url = demo::spawn(0, true).await.unwrap();
    let schema = reflect(connect(&url).await.unwrap()).await.unwrap();
    let svcs = schema.services();
    assert_eq!(
        svcs.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(),
        ["demo.Greeter"]
    );
    assert_eq!(svcs[0].methods.len(), 4);
    assert!(
        schema
            .pool
            .get_message_by_name("google.protobuf.Timestamp")
            .is_some(),
        "imports resolved"
    );

    let no_refl = demo::spawn(0, false).await.unwrap();
    let err = reflect(connect(&no_refl).await.unwrap()).await.unwrap_err();
    assert!(err.to_string().contains("Unimplemented"), "{err}");
    let err = connect("grpc://127.0.0.1:1").await.unwrap_err();
    assert!(matches!(err, GrpcError::Connect(..)));
}
