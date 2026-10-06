//! Dev-only: serve the real Tauri command layer over HTTP so the UI can run
//! in a normal browser (UI development, screenshots, Playwright).
//!
//!   cargo run -p logic-socket-desktop --example web_bridge
//!   VITE_WEB_BRIDGE=1 npm run dev   # then open http://localhost:1420
//!
//! POST /invoke/<command> with the JSON args → JSON result (400 + JSON error).
//! GET  /events → Server-Sent Events for `db-changed` and `mcp-log`.

use std::convert::Infallible;

use axum::{
    Json, Router,
    extract::{Path, State},
    http::{HeaderValue, Method, StatusCode},
    response::sse::{Event, KeepAlive, Sse},
    routing::{get, post},
};
use futures::Stream;
use serde_json::Value;
use tauri::Listener;
use tauri::test::{
    INVOKE_KEY, MockRuntime, get_ipc_response, mock_builder, mock_context, noop_assets,
};
use tauri::webview::InvokeRequest;
use tokio::sync::broadcast;

#[derive(Clone)]
struct Bridge {
    window: tauri::WebviewWindow<MockRuntime>,
    events: broadcast::Sender<String>,
}

fn invoke_blocking(
    w: &tauri::WebviewWindow<MockRuntime>,
    cmd: String,
    args: Value,
) -> Result<Value, Value> {
    get_ipc_response(
        w,
        InvokeRequest {
            cmd,
            callback: tauri::ipc::CallbackFn(0),
            error: tauri::ipc::CallbackFn(1),
            url: "tauri://localhost".parse().unwrap(),
            body: tauri::ipc::InvokeBody::Json(args),
            headers: Default::default(),
            invoke_key: INVOKE_KEY.to_string(),
        },
    )
    .map(|b| b.deserialize::<Value>().unwrap_or(Value::Null))
}

async fn invoke(
    State(b): State<Bridge>,
    Path(cmd): Path<String>,
    Json(args): Json<Value>,
) -> (StatusCode, Json<Value>) {
    let w = b.window.clone();
    match tokio::task::spawn_blocking(move || invoke_blocking(&w, cmd, args))
        .await
        .unwrap()
    {
        Ok(v) => (StatusCode::OK, Json(v)),
        Err(e) => (StatusCode::BAD_REQUEST, Json(e)),
    }
}

async fn events(State(b): State<Bridge>) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let mut rx = b.events.subscribe();
    let stream = async_stream(move |tx| async move {
        while let Ok(msg) = rx.recv().await {
            if tx.send(Ok(Event::default().data(msg))).await.is_err() {
                break;
            }
        }
    });
    Sse::new(stream).keep_alive(KeepAlive::default())
}

/// Tiny channel-backed stream helper (avoids an extra dependency).
fn async_stream<F, Fut>(f: F) -> impl Stream<Item = Result<Event, Infallible>>
where
    F: FnOnce(tokio::sync::mpsc::Sender<Result<Event, Infallible>>) -> Fut,
    Fut: std::future::Future<Output = ()> + Send + 'static,
{
    let (tx, rx) = tokio::sync::mpsc::channel(256);
    tokio::spawn(f(tx));
    futures::stream::unfold(rx, |mut rx| async move { rx.recv().await.map(|x| (x, rx)) })
}

fn main() {
    let data_dir = std::env::var_os("LSOCK_DATA_DIR")
        .map(Into::into)
        .unwrap_or_else(lsock_engine::default_data_dir);
    eprintln!(
        "web bridge using data dir {}",
        std::path::Path::new(&data_dir).display()
    );
    let engine = lsock_engine::Engine::open(data_dir).expect("open database");
    let app = lsock_desktop::build(mock_builder(), engine)
        .build(mock_context(noop_assets()))
        .expect("build app");
    let window = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
        .build()
        .unwrap();
    let (events_tx, _) = broadcast::channel::<String>(1024);
    for &name in lsock_desktop::APP_EVENTS {
        let tx = events_tx.clone();
        app.listen_any(name, move |ev| {
            let _ = tx.send(serde_json::json!({ "event": name, "payload": serde_json::from_str::<Value>(ev.payload()).unwrap_or(Value::Null) }).to_string());
        });
    }
    let bridge = Bridge {
        window,
        events: events_tx,
    };
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async move {
        let cors = tower_http::cors::CorsLayer::new()
            .allow_origin("http://localhost:1420".parse::<HeaderValue>().unwrap())
            .allow_methods([Method::GET, Method::POST])
            .allow_headers([axum::http::header::CONTENT_TYPE]);
        let router = Router::new()
            .route("/invoke/{cmd}", post(invoke))
            .route("/events", get(events))
            .layer(cors)
            .with_state(bridge);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:1421")
            .await
            .unwrap();
        eprintln!("web bridge listening on http://127.0.0.1:1421");
        axum::serve(listener, router).await.unwrap();
    });
}
