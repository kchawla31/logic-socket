//! Logic Socket's own file format (exports and Git sync): readable YAML that
//! mirrors the data model one-to-one, so nothing is lost and diffs stay small.
//!
//! ```yaml
//! logicSocket: 1
//! kind: collection          # collection | environments | mcp
//! id: wrk_…
//! name: Shop API
//! environments:
//!   base: { id, name, data, secretKeys, … }
//!   subs: [ … ]
//! items:
//!   - type: folder          # folder | request | realtime | grpc | mcp | llm
//!     id: fld_…
//!     name: Orders
//!     children: [ … ]
//! ```

use lsock_core::{
    CookieJar, Environment, Folder, GrpcRequest, LlmRequest, McpServer, ProtoFile, RealtimeRequest,
    Request, Workspace, WorkspaceScope,
};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Map, Value, json};

use crate::{ConvertError, Entry, Format, Imported, Item, Meta, Node, Result, WorkspaceBundle};

pub const VERSION: u64 = 1;
pub const MARKER: &str = "logicSocket";

fn err(m: impl Into<String>) -> ConvertError {
    ConvertError::invalid(Format::LogicSocket, m)
}

/// A safe file name for a workspace in a Git repo: `logic-socket.<slug>.yaml`.
pub fn file_name_for(name: &str) -> String {
    let slug: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>()
        .split('-')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    format!(
        "logic-socket.{}.yaml",
        if slug.is_empty() { "workspace" } else { &slug }
    )
}

// ------------------------------------------------------------------ export

/// `type`, `id`, the model's own fields, then sort key and timestamps.
fn entry_value<T: Serialize>(kind: Option<&str>, meta: &Meta, body: &T) -> Value {
    let mut o = Map::new();
    if let Some(k) = kind {
        o.insert("type".into(), json!(k));
    }
    if let Some(id) = &meta.id {
        o.insert("id".into(), json!(id));
    }
    if let Value::Object(fields) = serde_json::to_value(body).unwrap_or_default() {
        o.extend(fields);
    }
    if let Some(s) = meta.sort_key {
        o.insert("sortKey".into(), json!(s));
    }
    if let Some(c) = meta.created {
        o.insert("created".into(), json!(c));
    }
    if let Some(m) = meta.modified {
        o.insert("modified".into(), json!(m));
    }
    Value::Object(o)
}

fn items_value(items: &[Item]) -> Vec<Value> {
    items
        .iter()
        .map(|it| {
            let m = &it.meta;
            match &it.node {
                Node::Folder(f, children) => {
                    let mut v = entry_value(Some("folder"), m, f);
                    v.as_object_mut()
                        .unwrap()
                        .insert("children".into(), Value::Array(items_value(children)));
                    v
                }
                Node::Request(r) => entry_value(Some("request"), m, r),
                Node::Realtime(r) => entry_value(Some("realtime"), m, r),
                Node::Grpc(r) => entry_value(Some("grpc"), m, r),
                Node::Mcp(r) => entry_value(Some("mcp"), m, r),
                Node::Llm(r) => entry_value(Some("llm"), m, r),
            }
        })
        .collect()
}

/// Serialise one workspace in the Logic Socket format.
pub fn export(b: &WorkspaceBundle) -> Result<String> {
    let kind = match b.workspace.scope {
        WorkspaceScope::Environment => "environments",
        WorkspaceScope::Mcp => "mcp",
        _ => "collection",
    };
    let mut doc = Map::new();
    doc.insert(MARKER.into(), json!(VERSION));
    doc.insert("kind".into(), json!(kind));
    let ws = entry_value(
        None,
        &Meta {
            sort_key: None,
            ..b.meta.clone()
        },
        &b.workspace,
    );
    if let Value::Object(fields) = ws {
        // the scope is already `kind`; per-machine state is never written
        for (k, v) in fields {
            if !matches!(
                k.as_str(),
                "scope" | "activeEnvironmentId" | "activeGlobalBaseId" | "activeGlobalSubId"
            ) {
                doc.insert(k, v);
            }
        }
    }
    let mut envs = Map::new();
    if let Some(base) = &b.base_env {
        envs.insert(
            "base".into(),
            entry_value(
                None,
                &Meta {
                    sort_key: None,
                    ..base.meta.clone()
                },
                &base.body,
            ),
        );
    }
    if !b.sub_envs.is_empty() {
        envs.insert(
            "subs".into(),
            Value::Array(
                b.sub_envs
                    .iter()
                    .map(|e| entry_value(None, &e.meta, &e.body))
                    .collect(),
            ),
        );
    }
    if !envs.is_empty() {
        doc.insert("environments".into(), Value::Object(envs));
    }
    if let Some(j) = &b.cookie_jar {
        doc.insert(
            "cookieJar".into(),
            entry_value(
                None,
                &Meta {
                    sort_key: None,
                    ..j.meta.clone()
                },
                &j.body,
            ),
        );
    }
    if !b.protos.is_empty() {
        doc.insert(
            "protoFiles".into(),
            Value::Array(
                b.protos
                    .iter()
                    .map(|p| entry_value(None, &p.meta, &p.body))
                    .collect(),
            ),
        );
    }
    doc.insert("items".into(), Value::Array(items_value(&b.items)));
    let pruned = crate::insomnia::prune(Value::Object(doc)).unwrap_or_default();
    serde_yaml_ng::to_string(&pruned).map_err(|e| ConvertError::Yaml(e.to_string()))
}

// ------------------------------------------------------------------ import

fn meta_of(v: &Value) -> Meta {
    let i = |k: &str| {
        v.get(k)
            .and_then(|x| x.as_i64().or_else(|| x.as_f64().map(|f| f as i64)))
    };
    Meta {
        id: v.get("id").and_then(Value::as_str).map(str::to_string),
        sort_key: v.get("sortKey").and_then(Value::as_f64),
        created: i("created"),
        modified: i("modified"),
    }
}

fn entry<T: DeserializeOwned>(v: &Value, what: &str) -> Result<Entry<T>> {
    Ok(Entry {
        meta: meta_of(v),
        body: serde_json::from_value(v.clone()).map_err(|e| err(format!("{what}: {e}")))?,
    })
}

fn items_of(list: &[Value], warnings: &mut Vec<String>) -> Result<Vec<Item>> {
    let mut out = vec![];
    for v in list {
        let ty = v.get("type").and_then(Value::as_str).unwrap_or("");
        let name = v.get("name").and_then(Value::as_str).unwrap_or("item");
        let what = format!("{ty} '{name}'");
        let body = |v: &Value| -> Value {
            // `type` is ours; Realtime/Grpc models have no field called `type`
            let mut c = v.clone();
            if let Some(o) = c.as_object_mut() {
                o.remove("type");
            }
            c
        };
        let node = match ty {
            "folder" => {
                let children = v
                    .get("children")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                let f: Folder =
                    serde_json::from_value(body(v)).map_err(|e| err(format!("{what}: {e}")))?;
                Node::Folder(f, items_of(&children, warnings)?)
            }
            "request" => Node::Request(
                serde_json::from_value::<Request>(body(v))
                    .map_err(|e| err(format!("{what}: {e}")))?,
            ),
            "realtime" => Node::Realtime(
                serde_json::from_value::<RealtimeRequest>(body(v))
                    .map_err(|e| err(format!("{what}: {e}")))?,
            ),
            "grpc" => Node::Grpc(
                serde_json::from_value::<GrpcRequest>(body(v))
                    .map_err(|e| err(format!("{what}: {e}")))?,
            ),
            "mcp" => Node::Mcp(
                serde_json::from_value::<McpServer>(body(v))
                    .map_err(|e| err(format!("{what}: {e}")))?,
            ),
            "llm" => Node::Llm(
                serde_json::from_value::<LlmRequest>(body(v))
                    .map_err(|e| err(format!("{what}: {e}")))?,
            ),
            other => {
                warnings.push(format!("unknown item type '{other}' skipped"));
                continue;
            }
        };
        out.push(Item {
            meta: meta_of(v),
            node,
        });
    }
    Ok(out)
}

pub fn import(v: &Value) -> Result<Imported> {
    let version = v.get(MARKER).and_then(Value::as_u64).unwrap_or(0);
    if version > VERSION {
        return Err(err(format!(
            "this file is format version {version}; update Logic Socket to open it"
        )));
    }
    let mut warnings = vec![];
    let scope = match v
        .get("kind")
        .and_then(Value::as_str)
        .unwrap_or("collection")
    {
        "environments" => WorkspaceScope::Environment,
        "mcp" => WorkspaceScope::Mcp,
        _ => WorkspaceScope::Collection,
    };
    let mut ws: Workspace =
        serde_json::from_value(v.clone()).map_err(|e| err(format!("workspace: {e}")))?;
    ws.scope = scope;
    let envs = v.get("environments").cloned().unwrap_or(Value::Null);
    let b = WorkspaceBundle {
        meta: Meta {
            sort_key: None,
            ..meta_of(v)
        },
        workspace: ws,
        base_env: envs
            .get("base")
            .map(|e| entry::<Environment>(e, "base environment"))
            .transpose()?,
        sub_envs: envs
            .get("subs")
            .and_then(Value::as_array)
            .map(|l| {
                l.iter()
                    .map(|e| entry::<Environment>(e, "environment"))
                    .collect::<Result<Vec<_>>>()
            })
            .transpose()?
            .unwrap_or_default(),
        cookie_jar: v
            .get("cookieJar")
            .map(|j| entry::<CookieJar>(j, "cookie jar"))
            .transpose()?,
        protos: v
            .get("protoFiles")
            .and_then(Value::as_array)
            .map(|l| {
                l.iter()
                    .map(|p| entry::<ProtoFile>(p, "proto file"))
                    .collect::<Result<Vec<_>>>()
            })
            .transpose()?
            .unwrap_or_default(),
        items: items_of(
            v.get("items")
                .and_then(Value::as_array)
                .map(Vec::as_slice)
                .unwrap_or(&[]),
            &mut warnings,
        )?,
        spec: None,
    };
    Ok(Imported {
        format: Format::LogicSocket,
        workspaces: vec![b],
        warnings,
    })
}
