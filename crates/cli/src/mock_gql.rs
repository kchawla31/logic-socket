//! `irs mock graphql`: a tiny canned GraphQL server (library schema) for demos.

use axum::{Json, Router, routing::post};
use serde_json::{Value, json};

const INTROSPECTION: &str = include_str!("fixtures/library-introspection.json");

async fn handle(Json(req): Json<Value>) -> Json<Value> {
    let q = req["query"].as_str().unwrap_or("");
    if q.contains("__schema") {
        return Json(serde_json::from_str(INTROSPECTION).unwrap_or(Value::Null));
    }
    let books = json!([
        {"id": "1", "title": "Programming Rust", "year": 2021, "tags": ["rust"], "author": {"id": "a1", "name": "Jim Blandy"}},
        {"id": "2", "title": "Designing Data-Intensive Applications", "year": 2017, "tags": ["databases"], "author": {"id": "a2", "name": "Martin Kleppmann"}}
    ]);
    if q.contains("addBook") {
        let input = &req["variables"]["input"];
        return Json(
            json!({"data": {"addBook": {"id": "3", "title": input["title"], "year": input["year"], "tags": [], "author": {"id": input["authorId"], "name": "New Author"}}}}),
        );
    }
    if q.contains("books") {
        let search = req["variables"]["search"]
            .as_str()
            .unwrap_or("")
            .to_lowercase();
        let list: Vec<Value> = books
            .as_array()
            .unwrap()
            .iter()
            .filter(|b| {
                b["title"]
                    .as_str()
                    .unwrap_or("")
                    .to_lowercase()
                    .contains(&search)
            })
            .cloned()
            .collect();
        return Json(json!({"data": {"books": list}}));
    }
    Json(
        json!({"errors": [{"message": "This mock only answers introspection, books and addBook"}]}),
    )
}

pub async fn spawn(port: u16) -> std::io::Result<String> {
    let l = tokio::net::TcpListener::bind(("127.0.0.1", port)).await?;
    let addr = l.local_addr()?;
    tokio::spawn(async move {
        let _ = axum::serve(l, Router::new().route("/graphql", post(handle))).await;
    });
    Ok(format!("http://{addr}/graphql"))
}
