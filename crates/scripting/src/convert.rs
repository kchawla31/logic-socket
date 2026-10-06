//! `lsock_core::Request` ⇄ script-facing request shape (Postman-style body modes).

use lsock_core::{Auth, Body, BodyParam, KeyValue, Request, mime};
use serde_json::{Value, json};

use crate::{ScriptKv, ScriptRequest};

fn kv_out(list: &[KeyValue]) -> Vec<ScriptKv> {
    list.iter()
        .map(|k| ScriptKv {
            key: k.name.clone(),
            value: k.value.clone(),
            disabled: k.disabled,
        })
        .collect()
}

fn kv_in(list: &[ScriptKv], previous: &[KeyValue]) -> Vec<KeyValue> {
    list.iter()
        .map(|k| {
            // keep ids/descriptions of rows that still exist
            let prev = previous.iter().find(|p| p.name == k.key);
            KeyValue {
                id: prev.and_then(|p| p.id.clone()),
                description: prev.and_then(|p| p.description.clone()),
                name: k.key.clone(),
                value: k.value.clone(),
                disabled: k.disabled,
            }
        })
        .collect()
}

fn language(mime_type: &str) -> &'static str {
    if mime_type.contains("json") {
        "json"
    } else if mime_type.contains("xml") {
        "xml"
    } else if mime_type.contains("html") {
        "html"
    } else {
        "text"
    }
}

pub fn body_out(b: &Body) -> Value {
    let params = |kind: &str| -> Value {
        Value::Array(
            b.params
                .iter()
                .map(|p| {
                    let mut o = json!({ "key": p.name, "value": p.value, "disabled": p.disabled });
                    if kind == "formdata" {
                        let file = p.kind.as_deref() == Some("file");
                        o["type"] = json!(if file { "file" } else { "text" });
                        if file {
                            o["src"] = json!(p.file_name.clone().unwrap_or_default());
                        }
                    }
                    o
                })
                .collect(),
        )
    };
    match b.mime_type.as_deref().filter(|m| !m.is_empty()) {
        None => match &b.text {
            Some(t) if !t.is_empty() => json!({ "mode": "raw", "raw": t }),
            _ => json!({}),
        },
        Some(mime::FORM) => json!({ "mode": "urlencoded", "urlencoded": params("urlencoded") }),
        Some(mime::MULTIPART) => json!({ "mode": "formdata", "formdata": params("formdata") }),
        Some(mime::FILE) => {
            json!({ "mode": "file", "file": { "src": b.file_name.clone().unwrap_or_default() } })
        }
        Some(mime::GRAPHQL) => {
            let gql: Value =
                serde_json::from_str(b.text.as_deref().unwrap_or("{}")).unwrap_or(json!({}));
            json!({ "mode": "graphql", "graphql": gql })
        }
        Some(m) => json!({
            "mode": "raw",
            "raw": b.text.clone().unwrap_or_default(),
            "options": { "raw": { "language": language(m) } },
        }),
    }
}

pub fn body_in(v: &Value, previous: &Body) -> Body {
    let list = |key: &str| -> Vec<BodyParam> {
        v[key]
            .as_array()
            .into_iter()
            .flatten()
            .map(|p| BodyParam {
                name: p["key"].as_str().unwrap_or("").to_string(),
                value: p["value"]
                    .as_str()
                    .map(str::to_string)
                    .unwrap_or_else(|| p["value"].to_string()),
                disabled: p["disabled"].as_bool().unwrap_or(false),
                kind: p["type"]
                    .as_str()
                    .map(str::to_string)
                    .filter(|t| t == "file"),
                file_name: p["src"].as_str().map(str::to_string),
                id: None,
            })
            .collect()
    };
    match v["mode"].as_str() {
        Some("raw") => {
            let lang = v["options"]["raw"]["language"].as_str();
            let mime_type = match (previous.mime_type.as_deref(), lang) {
                // keep the user's specific mime type unless the language changed category
                (Some(p), Some(l)) if language(p) == l => Some(p.to_string()),
                (_, Some("json")) => Some(mime::JSON.to_string()),
                (_, Some("xml")) => Some("application/xml".to_string()),
                (_, Some("html")) => Some("text/html".to_string()),
                (Some(p), None) if ![mime::FORM, mime::MULTIPART, mime::FILE].contains(&p) => {
                    Some(p.to_string())
                }
                _ => Some("text/plain".to_string()),
            };
            Body {
                mime_type,
                text: v["raw"].as_str().map(str::to_string),
                ..Default::default()
            }
        }
        Some("urlencoded") => Body {
            mime_type: Some(mime::FORM.into()),
            params: list("urlencoded"),
            ..Default::default()
        },
        Some("formdata") => Body {
            mime_type: Some(mime::MULTIPART.into()),
            params: list("formdata"),
            ..Default::default()
        },
        Some("file") => Body {
            mime_type: Some(mime::FILE.into()),
            file_name: v["file"]["src"].as_str().map(str::to_string),
            ..Default::default()
        },
        Some("graphql") => Body {
            mime_type: Some(mime::GRAPHQL.into()),
            text: Some(v["graphql"].to_string()),
            ..Default::default()
        },
        _ => Body::default(),
    }
}

pub fn to_script_request(id: &str, r: &Request) -> ScriptRequest {
    ScriptRequest {
        id: id.to_string(),
        name: r.name.clone(),
        method: r.method.clone(),
        url: r.url.clone(),
        query: kv_out(&r.parameters),
        headers: kv_out(&r.headers),
        body: body_out(&r.body),
        auth: serde_json::to_value(&r.authentication).unwrap_or(json!({ "type": "inherit" })),
    }
}

/// Apply a script's request mutations onto the stored request (in memory only).
pub fn apply_script_request(r: &mut Request, s: &ScriptRequest) {
    r.method = s.method.clone();
    r.url = s.url.clone();
    r.parameters = kv_in(&s.query, &r.parameters);
    r.headers = kv_in(&s.headers, &r.headers);
    if s.body != body_out(&r.body) {
        r.body = body_in(&s.body, &r.body);
    }
    if let Ok(a) = serde_json::from_value::<Auth>(s.auth.clone())
        && serde_json::to_value(&a).ok() != serde_json::to_value(&r.authentication).ok()
    {
        r.authentication = a;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_is_lossless() {
        let bodies = [
            Body {
                mime_type: Some(mime::JSON.into()),
                text: Some("{\"a\":1}".into()),
                ..Default::default()
            },
            Body {
                mime_type: Some("application/vnd.api+json".into()),
                text: Some("{}".into()),
                ..Default::default()
            },
            Body {
                mime_type: Some(mime::FORM.into()),
                params: vec![BodyParam {
                    name: "a".into(),
                    value: "1".into(),
                    ..Default::default()
                }],
                ..Default::default()
            },
            Body {
                mime_type: Some(mime::MULTIPART.into()),
                params: vec![BodyParam {
                    name: "f".into(),
                    kind: Some("file".into()),
                    file_name: Some("/tmp/x".into()),
                    ..Default::default()
                }],
                ..Default::default()
            },
            Body {
                mime_type: Some(mime::FILE.into()),
                file_name: Some("/tmp/b".into()),
                ..Default::default()
            },
            Body::default(),
        ];
        for b in bodies {
            let r = Request {
                body: b.clone(),
                ..Default::default()
            };
            let s = to_script_request("req_1", &r);
            let mut back = r.clone();
            apply_script_request(&mut back, &s);
            assert_eq!(back, r, "body {b:?}");
        }
    }

    #[test]
    fn non_scriptable_auth_survives_a_script_round_trip() {
        let r = Request {
            authentication: Auth::Iam(lsock_core::AwsIamConfig {
                region: "eu-west-1".into(),
                ..Default::default()
            }),
            ..Default::default()
        };
        let s = to_script_request("req_1", &r);
        let mut back = r.clone();
        apply_script_request(&mut back, &s);
        assert_eq!(back.authentication, r.authentication);
    }

    #[test]
    fn script_changes_apply() {
        let mut r = Request {
            headers: vec![KeyValue {
                id: Some("h1".into()),
                ..KeyValue::new("X-A", "1")
            }],
            ..Default::default()
        };
        let mut s = to_script_request("req_1", &r);
        s.headers[0].value = "2".into();
        s.headers.push(ScriptKv {
            key: "X-B".into(),
            value: "3".into(),
            disabled: false,
        });
        s.body =
            json!({ "mode": "raw", "raw": "{}", "options": { "raw": { "language": "json" } } });
        s.auth = json!({ "type": "bearer", "token": "t" });
        apply_script_request(&mut r, &s);
        assert_eq!(r.headers[0].id.as_deref(), Some("h1"));
        assert_eq!(r.headers[0].value, "2");
        assert_eq!(r.headers[1].name, "X-B");
        assert_eq!(r.body.mime_type.as_deref(), Some(mime::JSON));
        assert!(matches!(r.authentication, Auth::Bearer { ref token, .. } if token == "t"));
    }
}
