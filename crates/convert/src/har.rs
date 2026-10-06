//! HAR 1.2 (browser "Save all as HAR"): import requests, export requests.

use lsock_core::{Body, BodyParam, KeyValue, Request, Workspace, mime};
use serde_json::{Value, json};

use crate::{Format, Imported, Item, Node, Result, WorkspaceBundle, scalar, str_of};

fn arr<'a>(v: &'a Value, k: &str) -> &'a [Value] {
    v.get(k)
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[])
}

/// Headers browsers add on their own; importing them only adds noise.
fn skip_header(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    n.starts_with(':')
        || matches!(n.as_str(), "content-length" | "host" | "connection")
        || n.starts_with("sec-")
}

pub fn import(v: &Value) -> Result<Imported> {
    let mut items = vec![];
    // a full HAR, a bare entries list, or a single request object
    let single = [json!({"request": v.clone()})];
    let entries: &[Value] = if v.get("log").is_some() {
        arr(&v["log"], "entries")
    } else if v.get("entries").is_some() {
        arr(v, "entries")
    } else {
        &single
    };
    for e in entries {
        let r = &e["request"];
        let raw_url = str_of(r, "url");
        let parameters: Vec<KeyValue> = arr(r, "queryString")
            .iter()
            .map(|q| KeyValue::new(str_of(q, "name"), str_of(q, "value")))
            .collect();
        let url = if parameters.is_empty() {
            raw_url.clone()
        } else {
            raw_url.split('?').next().unwrap_or(&raw_url).to_string()
        };
        let method = r
            .get("method")
            .and_then(Value::as_str)
            .unwrap_or("GET")
            .to_uppercase();
        let mut body = Body::default();
        if let Some(pd) = r.get("postData").filter(|p| p.is_object()) {
            let mt = str_of(pd, "mimeType");
            let params = arr(pd, "params");
            let mut base = mt.split(';').next().unwrap_or("").trim().to_string();
            if base.is_empty() && !params.is_empty() {
                let files = params
                    .iter()
                    .any(|p| p.get("fileName").is_some_and(|f| !f.is_null()));
                base = if files {
                    mime::MULTIPART.into()
                } else {
                    mime::FORM.into()
                };
            }
            if !params.is_empty() && (base == mime::FORM || base == mime::MULTIPART) {
                body = Body {
                    mime_type: Some(base.clone()),
                    params: params
                        .iter()
                        .map(|p| {
                            let file = p.get("fileName").is_some_and(|f| !f.is_null());
                            BodyParam {
                                name: str_of(p, "name"),
                                value: str_of(p, "value"),
                                kind: (base == mime::MULTIPART)
                                    .then(|| if file { "file".into() } else { "text".into() }),
                                file_name: p.get("fileName").filter(|f| !f.is_null()).map(scalar),
                                ..Default::default()
                            }
                        })
                        .collect(),
                    ..Default::default()
                };
            } else if pd.get("text").is_some() {
                body = Body {
                    mime_type: Some(if base.is_empty() {
                        "text/plain".into()
                    } else {
                        base
                    }),
                    text: Some(str_of(pd, "text")),
                    ..Default::default()
                };
            }
        }
        let headers = arr(r, "headers")
            .iter()
            .filter(|h| !skip_header(&str_of(h, "name")))
            .filter(|h| {
                !(body.mime_type.as_deref() == Some(mime::MULTIPART)
                    && str_of(h, "name").eq_ignore_ascii_case("content-type"))
            })
            .map(|h| KeyValue::new(str_of(h, "name"), str_of(h, "value")))
            .collect();
        let name = Some(str_of(e, "comment"))
            .filter(|c| !c.is_empty())
            .unwrap_or_else(|| crate::curl::request_name(&method, &url));
        items.push(Item::new(Node::Request(Request {
            name,
            method,
            url,
            parameters,
            headers,
            body,
            ..Default::default()
        })));
    }
    let name = v["log"]
        .get("creator")
        .map(|c| format!("HAR import ({})", str_of(c, "name")))
        .unwrap_or_else(|| "HAR import".into());
    Ok(Imported {
        format: Format::Har,
        workspaces: vec![WorkspaceBundle {
            workspace: Workspace {
                name,
                ..Default::default()
            },
            items,
            ..Default::default()
        }],
        warnings: vec![],
    })
}

/// HAR with one entry per request (requests only; templates are left unrendered).
pub fn export(b: &WorkspaceBundle) -> String {
    let entries: Vec<Value> = b
        .walk()
        .into_iter()
        .filter_map(|i| match &i.node {
            Node::Request(r) => Some(r),
            _ => None,
        })
        .map(|r| {
            let mut post = Value::Null;
            if let Some(m) = &r.body.mime_type {
                post = if r.body.params.is_empty() {
                    json!({"mimeType": m, "text": r.body.text.clone().unwrap_or_default()})
                } else {
                    json!({"mimeType": m, "params": r.body.params.iter().filter(|p| !p.disabled).map(|p| json!({"name": p.name, "value": p.value, "fileName": p.file_name})).collect::<Vec<_>>()})
                };
            }
            let mut req = json!({
                "method": r.method,
                "url": r.url,
                "httpVersion": "HTTP/1.1",
                "headers": r.headers.iter().filter(|h| !h.disabled).map(|h| json!({"name": h.name, "value": h.value})).collect::<Vec<_>>(),
                "queryString": r.parameters.iter().filter(|h| !h.disabled).map(|h| json!({"name": h.name, "value": h.value})).collect::<Vec<_>>(),
                "cookies": [], "headersSize": -1, "bodySize": -1,
            });
            if !post.is_null() {
                req["postData"] = post;
            }
            json!({"comment": r.name, "startedDateTime": "1970-01-01T00:00:00.000Z", "time": 0, "request": req,
                   "response": {"status": 0, "statusText": "", "httpVersion": "HTTP/1.1", "headers": [], "cookies": [],
                                "content": {"size": 0, "mimeType": ""}, "redirectURL": "", "headersSize": -1, "bodySize": -1},
                   "cache": {}, "timings": {"send": 0, "wait": 0, "receive": 0}})
        })
        .collect();
    serde_json::to_string_pretty(&json!({"log": {"version": "1.2", "creator": {"name": "logic-socket", "version": env!("CARGO_PKG_VERSION")}, "entries": entries}})).unwrap_or_default()
}
