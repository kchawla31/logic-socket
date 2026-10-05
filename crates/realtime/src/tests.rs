use std::time::Duration;

use serde_json::json;

use super::*;

async fn server() -> String {
    let url = crate::mock::spawn(0).await.unwrap();
    url.trim_start_matches("http://").to_string()
}

async fn wait_for(log: &EventLog, pred: impl Fn(&[RtEvent]) -> bool) -> Vec<RtEvent> {
    for _ in 0..100 {
        let e = log.entries();
        if pred(&e) {
            return e;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("timed out; log: {:#?}", log.entries());
}

fn texts(e: &[RtEvent], dir: Direction) -> Vec<String> {
    e.iter().filter(|x| x.direction == dir && x.kind != "info").map(|x| x.data.clone()).collect()
}

#[tokio::test]
async fn websocket_send_receive_headers_subprotocol_and_server_close() {
    let host = server().await;
    let s = connect(ConnectOptions {
        url: format!("ws://{host}/ws"),
        headers: vec![("X-Token".into(), "abc".into())],
        subprotocols: vec!["chat.v1".into()],
        ..Default::default()
    })
    .await
    .unwrap();
    assert!(s.is_connected());
    s.send_text("ping me").unwrap();
    s.send_binary(vec![1, 2, 255]).unwrap();
    let e = wait_for(&s.log, |e| e.iter().filter(|x| x.direction == Direction::In).count() >= 3).await;
    assert!(e.iter().any(|x| x.kind == "open" && x.data.contains("subprotocol chat.v1")));
    assert_eq!(texts(&e, Direction::In)[..2], ["welcome abc".to_string(), "echo: ping me".to_string()]);
    assert!(e.iter().any(|x| x.direction == Direction::In && x.kind == "binary" && x.data == "01 02 ff" && x.size == 3));
    assert!(matches!(s.emit("x", vec![], false), Err(RtError::Protocol(_))));

    s.send_text("bye").unwrap();
    let e = wait_for(&s.log, |e| e.iter().any(|x| x.kind == "close")).await;
    assert!(e.iter().any(|x| x.kind == "close" && x.data.contains("1000 see you")));
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(!s.is_connected());
    assert_eq!(s.send_text("late"), Err(RtError::Closed));
}

#[tokio::test]
async fn websocket_errors_are_readable() {
    let host = server().await;
    let err = connect(ConnectOptions { url: format!("ws://{host}/events"), ..Default::default() }).await.err().unwrap();
    assert!(err.to_string().contains("instead of upgrading"), "{err}");
    let err = connect(ConnectOptions { url: "not a url".into(), ..Default::default() }).await.err().unwrap();
    assert!(matches!(err, RtError::Url(..)));
}

#[tokio::test]
async fn sse_receives_named_events_and_is_receive_only() {
    let host = server().await;
    let s = connect(ConnectOptions { kind: Kind::Sse, url: format!("http://{host}/events"), ..Default::default() }).await.unwrap();
    let e = wait_for(&s.log, |e| e.iter().any(|x| x.kind == "close")).await;
    let events: Vec<(Option<String>, String)> = e.iter().filter(|x| x.kind == "event").map(|x| (x.name.clone(), x.data.clone())).collect();
    assert_eq!(events, vec![(Some("greeting".into()), "hello".into()), (Some("message #2".into()), "{\"n\":1}\nline2".into())]);
    assert!(matches!(s.send_text("x"), Err(RtError::Protocol(_))));
    let err = connect(ConnectOptions { kind: Kind::Sse, url: format!("http://{host}/missing"), ..Default::default() }).await.err().unwrap();
    assert!(err.to_string().contains("HTTP 404"));
}

#[tokio::test]
async fn socketio_connect_emit_ack_namespaces_and_auth() {
    let host = server().await;
    let s = connect(ConnectOptions { kind: Kind::Socketio, url: format!("http://{host}"), ..Default::default() }).await.unwrap();
    s.emit("chat", vec![json!({"text": "hi"})], false).unwrap();
    let ack = s.emit("save", vec![json!(1)], true).unwrap();
    assert_eq!(ack, Some(0));
    let e = wait_for(&s.log, |e| e.iter().any(|x| x.kind == "ack")).await;
    let ins: Vec<(Option<String>, String)> = e.iter().filter(|x| x.direction == Direction::In && (x.kind == "event" || x.kind == "ack")).map(|x| (x.name.clone(), x.data.clone())).collect();
    assert_eq!(ins[0], (Some("hello".into()), r#"[{"motd":"hi"}]"#.into()));
    assert!(ins.contains(&(Some("echo".into()), r#"[{"text":"hi"}]"#.into())));
    assert!(ins.contains(&(Some("ack #0".into()), r#"["got",[1]]"#.into())));
    assert!(e.iter().any(|x| x.kind == "open" && x.data.contains("Joined namespace / (socket id sock1)")));
    assert!(matches!(s.send_text("raw"), Err(RtError::Protocol(_))));
    s.close();

    let err = connect(ConnectOptions { kind: Kind::Socketio, url: format!("http://{host}"), namespace: Some("admin".into()), ..Default::default() }).await.err().unwrap();
    assert!(err.to_string().contains("not authorized"), "{err}");
    let ok = connect(ConnectOptions { kind: Kind::Socketio, url: format!("http://{host}"), namespace: Some("/admin".into()), auth: Some(json!({"token": "s3cret"})), ..Default::default() }).await;
    assert!(ok.is_ok());
}
