//! Insomnia v5 YAML (import + export) and v4/v3 JSON/YAML exports (import).
//!
//! v5 is also the Git-sync file format. Things Insomnia doesn't model (AI requests,
//! MCP servers inside collections, proto files, WebSocket payloads, SSE) are kept
//! under `x-logic-socket` / per-item `x-lsock` keys, which Insomnia ignores.

use std::collections::HashMap;

use indexmap::IndexMap;
use lsock_core::{
    Body, BodyParam, Cookie, CookieJar, Environment, Folder, GrpcRequest, KeyValue, LlmRequest,
    McpRoot, McpServer, McpTransport, ProtoFile, RealtimeRequest, Request, RequestSettings, Toggle,
    VarMap, Workspace, WorkspaceScope,
};
use serde_json::{Map, Value, json};

use crate::{
    ConvertError, Entry, Format, Imported, Item, Meta, Node, Result, WorkspaceBundle,
    auth_from_json, auth_to_json, str_of,
};

pub const SCHEMA_VERSION: &str = "5.1";
const WS_PREFIX: &str = "ws-req_";
const SIO_PREFIX: &str = "socketio-req_";

// ------------------------------------------------------------------ helpers

/// Per-item extension block (`x-lsock`; `x-irs` in files written before the rename).
fn item_ext(v: &Value) -> Option<&Value> {
    v.get("x-lsock").or_else(|| v.get("x-irs"))
}

fn arr<'a>(v: &'a Value, k: &str) -> &'a [Value] {
    v.get(k)
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[])
}

fn kvs(v: &Value, k: &str) -> Vec<KeyValue> {
    arr(v, k)
        .iter()
        .filter_map(|x| {
            let mut kv: KeyValue = serde_json::from_value(x.clone()).ok()?;
            // tolerate numbers/bools in YAML
            kv.value = crate::scalar(x.get("value").unwrap_or(&Value::Null));
            (!kv.name.is_empty() || !kv.value.is_empty()).then_some(kv)
        })
        .collect()
}

fn var_map(v: Option<&Value>) -> VarMap {
    match v {
        Some(Value::Object(m)) => m.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
        _ => IndexMap::new(),
    }
}

fn i64_of(v: &Value, k: &str) -> Option<i64> {
    v.get(k)
        .and_then(|x| x.as_i64().or_else(|| x.as_f64().map(|f| f as i64)))
}

fn meta_v5(m: Option<&Value>) -> Meta {
    let Some(m) = m else { return Meta::default() };
    Meta {
        id: m.get("id").and_then(Value::as_str).map(str::to_string),
        sort_key: m.get("sortKey").and_then(Value::as_f64),
        created: i64_of(m, "created"),
        modified: i64_of(m, "modified"),
    }
}

fn desc_v5(item: &Value) -> String {
    item.get("meta")
        .map(|m| str_of(m, "description"))
        .unwrap_or_default()
}

fn meta_out(meta: &Meta, description: Option<&str>, private: Option<bool>) -> Value {
    let mut m = Map::new();
    if let Some(id) = &meta.id {
        m.insert("id".into(), json!(id));
    }
    if let Some(c) = meta.created {
        m.insert("created".into(), json!(c));
    }
    if let Some(c) = meta.modified {
        m.insert("modified".into(), json!(c));
    }
    if let Some(p) = private {
        m.insert("isPrivate".into(), json!(p));
    }
    if let Some(d) = description.filter(|d| !d.is_empty()) {
        m.insert("description".into(), json!(d));
    }
    if let Some(s) = meta.sort_key {
        m.insert("sortKey".into(), json!(s));
    }
    Value::Object(m)
}

fn kvs_out(list: &[KeyValue]) -> Value {
    Value::Array(
        list.iter()
            .filter(|h| !h.name.is_empty() || !h.value.is_empty())
            .map(|h| {
                let mut o = Map::new();
                o.insert("name".into(), json!(h.name));
                o.insert("value".into(), json!(h.value));
                if let Some(d) = &h.description {
                    o.insert("description".into(), json!(d));
                }
                if h.disabled {
                    o.insert("disabled".into(), json!(true));
                }
                Value::Object(o)
            })
            .collect(),
    )
}

fn toggle(s: &str) -> Toggle {
    match s {
        "on" => Toggle::On,
        "off" => Toggle::Off,
        _ => Toggle::Global,
    }
}

fn toggle_str(t: Toggle) -> &'static str {
    match t {
        Toggle::Global => "global",
        Toggle::On => "on",
        Toggle::Off => "off",
    }
}

fn body_from(v: Option<&Value>) -> Body {
    let Some(b) = v.filter(|b| b.is_object()) else {
        return Body::default();
    };
    Body {
        mime_type: b
            .get("mimeType")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_string),
        text: b.get("text").and_then(Value::as_str).map(str::to_string),
        file_name: b
            .get("fileName")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_string),
        params: arr(b, "params")
            .iter()
            .filter_map(|p| {
                let mut bp: BodyParam = serde_json::from_value(p.clone()).ok()?;
                bp.value = crate::scalar(p.get("value").unwrap_or(&Value::Null));
                Some(bp)
            })
            .collect(),
    }
}

fn body_out(b: &Body) -> Value {
    let mut o = Map::new();
    if let Some(m) = &b.mime_type {
        o.insert("mimeType".into(), json!(m));
    }
    if let Some(t) = &b.text {
        o.insert("text".into(), json!(t));
    }
    if let Some(f) = &b.file_name {
        o.insert("fileName".into(), json!(f));
    }
    if !b.params.is_empty() {
        o.insert(
            "params".into(),
            Value::Array(
                b.params
                    .iter()
                    .map(|p| {
                        let mut x = Map::new();
                        x.insert("name".into(), json!(p.name));
                        x.insert("value".into(), json!(p.value));
                        if p.disabled {
                            x.insert("disabled".into(), json!(true));
                        }
                        if let Some(k) = &p.kind {
                            x.insert("type".into(), json!(k));
                        }
                        if let Some(f) = &p.file_name {
                            x.insert("fileName".into(), json!(f));
                        }
                        Value::Object(x)
                    })
                    .collect(),
            ),
        );
    }
    Value::Object(o)
}

fn scripts_out(pre: &Option<String>, post: &Option<String>) -> Value {
    let mut o = Map::new();
    if let Some(p) = pre.as_ref().filter(|s| !s.trim().is_empty()) {
        o.insert("preRequest".into(), json!(p));
    }
    if let Some(p) = post.as_ref().filter(|s| !s.trim().is_empty()) {
        o.insert("afterResponse".into(), json!(p));
    }
    Value::Object(o)
}

fn non_empty(s: String) -> Option<String> {
    (!s.trim().is_empty()).then_some(s)
}

/// Recursively drop null, empty arrays and empty objects (like Insomnia's `removeEmptyFields`).
pub(crate) fn prune(v: Value) -> Option<Value> {
    match v {
        Value::Null => None,
        Value::Array(a) => {
            let a: Vec<Value> = a.into_iter().filter_map(prune).collect();
            (!a.is_empty()).then_some(Value::Array(a))
        }
        Value::Object(o) => {
            let o: Map<String, Value> = o
                .into_iter()
                .filter_map(|(k, v)| {
                    // environment data and folder variables keep empty values on purpose
                    if k == "data" || k == "environment" {
                        return match v {
                            Value::Object(ref m) if m.is_empty() => None,
                            v => Some((k, v)),
                        };
                    }
                    prune(v).map(|v| (k, v))
                })
                .collect();
            (!o.is_empty()).then_some(Value::Object(o))
        }
        other => Some(other),
    }
}

fn to_yaml(v: &Value) -> Result<String> {
    serde_yaml_ng::to_string(v).map_err(|e| ConvertError::Yaml(e.to_string()))
}

fn cookie_from(c: &Value) -> Cookie {
    let expires = match c.get("expires") {
        Some(Value::String(s)) if s != "Infinity" => chrono::DateTime::parse_from_rfc3339(s)
            .ok()
            .map(|d| d.timestamp_millis()),
        Some(Value::Number(n)) => n.as_i64(),
        _ => None,
    };
    Cookie {
        name: str_of(c, "key"),
        value: str_of(c, "value"),
        domain: str_of(c, "domain"),
        path: c
            .get("path")
            .and_then(Value::as_str)
            .unwrap_or("/")
            .to_string(),
        expires,
        secure: c.get("secure").and_then(Value::as_bool).unwrap_or(false),
        http_only: c.get("httpOnly").and_then(Value::as_bool).unwrap_or(false),
        host_only: c.get("hostOnly").and_then(Value::as_bool).unwrap_or(false),
    }
}

fn cookie_out(c: &Cookie) -> Value {
    let expires = c
        .expires
        .and_then(chrono::DateTime::from_timestamp_millis)
        .map(|d| json!(d.to_rfc3339_opts(chrono::SecondsFormat::Millis, true)));
    json!({
        "key": c.name, "value": c.value, "domain": c.domain, "path": c.path,
        "expires": expires, "secure": c.secure, "httpOnly": c.http_only, "hostOnly": c.host_only,
    })
}

fn secret_keys(v: &Value) -> Vec<String> {
    item_ext(v)
        .map(|x| arr(x, "secretKeys"))
        .unwrap_or(&[])
        .iter()
        .filter_map(|k| k.as_str().map(str::to_string))
        .collect()
}

fn env_from_v5(v: &Value, fallback_name: &str) -> Entry<Environment> {
    Entry {
        meta: meta_v5(v.get("meta")),
        body: Environment {
            name: v
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or(fallback_name)
                .to_string(),
            data: var_map(v.get("data")),
            color: v.get("color").and_then(Value::as_str).map(str::to_string),
            is_private: v
                .get("meta")
                .and_then(|m| m.get("isPrivate"))
                .and_then(Value::as_bool)
                .unwrap_or(false),
            secret_keys: secret_keys(v),
        },
    }
}

fn env_out(e: &Entry<Environment>, sub: bool) -> Value {
    let mut o = Map::new();
    o.insert("name".into(), json!(e.body.name));
    let mut meta = e.meta.clone();
    if !sub {
        meta.sort_key = None;
    }
    o.insert(
        "meta".into(),
        meta_out(&meta, None, Some(e.body.is_private)),
    );
    o.insert(
        "data".into(),
        Value::Object(e.body.data.clone().into_iter().collect()),
    );
    if let Some(c) = &e.body.color {
        o.insert("color".into(), json!(c));
    }
    if !e.body.secret_keys.is_empty() {
        o.insert(
            "x-lsock".into(),
            json!({ "secretKeys": e.body.secret_keys }),
        );
    }
    Value::Object(o)
}

fn parse_grpc_url(u: &str) -> String {
    let u = u.trim();
    if u.is_empty() || u.contains("://") {
        u.to_string()
    } else {
        format!("grpc://{u}")
    }
}

fn shell_join(parts: &[String]) -> String {
    parts
        .iter()
        .map(|p| {
            if !p.is_empty()
                && p.chars()
                    .all(|c| c.is_ascii_alphanumeric() || "-_./:=@,+%".contains(c))
            {
                p.clone()
            } else {
                format!("'{}'", p.replace('\'', r"'\''"))
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn mcp_from(m: &Value) -> McpServer {
    // our own exports carry the full config (sampling, cwd, TLS) alongside Insomnia's fields
    if let Some(full) = item_ext(m).and_then(|x| x.get("server"))
        && let Ok(s) = serde_json::from_value::<McpServer>(full.clone())
    {
        return s;
    }
    let url = str_of(m, "url");
    let transport = if str_of(m, "transportType") == "stdio" {
        let mut words = crate::curl::split_words(&url).unwrap_or_default();
        let command = if words.is_empty() {
            String::new()
        } else {
            words.remove(0)
        };
        McpTransport::Stdio {
            command,
            args: words,
            cwd: None,
        }
    } else {
        McpTransport::StreamableHttp { url }
    };
    McpServer {
        name: str_of(m, "name"),
        description: desc_v5(m),
        transport,
        headers: kvs(m, "headers"),
        env: arr(m, "env")
            .iter()
            .map(|e| KeyValue {
                name: str_of(e, "name"),
                value: str_of(e, "value"),
                disabled: e.get("enabled").and_then(Value::as_bool) == Some(false),
                ..Default::default()
            })
            .collect(),
        roots: arr(m, "roots")
            .iter()
            .map(|r| McpRoot {
                uri: str_of(r, "uri"),
                name: r.get("name").and_then(Value::as_str).map(str::to_string),
            })
            .collect(),
        authentication: auth_from_json(m.get("authentication"), &mut vec![], ""),
        ..Default::default()
    }
}

fn mcp_out(s: &McpServer, meta: &Meta) -> Value {
    let (url, kind) = match &s.transport {
        McpTransport::StreamableHttp { url } => (url.clone(), "streamable-http"),
        McpTransport::Stdio { command, args, .. } => {
            let mut all = vec![command.clone()];
            all.extend(args.iter().cloned());
            (shell_join(&all), "stdio")
        }
    };
    json!({
        "name": s.name,
        "url": url,
        "transportType": kind,
        "headers": kvs_out(&s.headers),
        "authentication": auth_to_json(&s.authentication),
        "meta": meta_out(meta, Some(&s.description), None),
        "env": s.env.iter().enumerate().map(|(i, e)| json!({
            "id": e.id.clone().unwrap_or_else(|| format!("env-{i}")),
            "name": e.name, "value": e.value, "type": "str", "enabled": !e.disabled,
        })).collect::<Vec<_>>(),
        "roots": s.roots.iter().map(|r| json!({"uri": r.uri, "name": r.name})).collect::<Vec<_>>(),
        "x-lsock": { "server": s },
    })
}

// ------------------------------------------------------------------ v5 import

pub fn import_v5(v: &Value) -> Result<Imported> {
    let mut warnings = vec![];
    let ty = str_of(v, "type");
    let scope = match ty.as_str() {
        "collection.insomnia.rest/5.0" | "spec.insomnia.rest/5.0" => WorkspaceScope::Collection,
        "environment.insomnia.rest/5.0" => WorkspaceScope::Environment,
        "mcpClient.insomnia/5.0" => WorkspaceScope::Mcp,
        "mock.insomnia.rest/5.0" => {
            return Err(ConvertError::invalid(
                Format::InsomniaV5,
                "mock servers aren't supported yet",
            ));
        }
        other => {
            return Err(ConvertError::invalid(
                Format::InsomniaV5,
                format!("unknown type '{other}'"),
            ));
        }
    };
    let meta = meta_v5(v.get("meta"));
    let mut b = WorkspaceBundle {
        workspace: Workspace {
            name: v
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("Imported")
                .to_string(),
            description: desc_v5(v),
            scope,
            ..Default::default()
        },
        meta,
        ..Default::default()
    };
    if let Some(envs) = v.get("environments").filter(|e| e.is_object()) {
        b.base_env = Some(env_from_v5(envs, "Base Environment"));
        b.sub_envs = arr(envs, "subEnvironments")
            .iter()
            .enumerate()
            .map(|(i, e)| env_from_v5(e, &format!("Environment {i}")))
            .collect();
    }
    if let Some(jar) = v.get("cookieJar").filter(|j| j.is_object()) {
        b.cookie_jar = Some(Entry {
            meta: meta_v5(jar.get("meta")),
            body: CookieJar {
                name: jar
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or("Default Jar")
                    .to_string(),
                cookies: arr(jar, "cookies").iter().map(cookie_from).collect(),
            },
        });
    }
    b.items = items_v5(arr(v, "collection"), &mut warnings);
    if let Some(m) = v.get("mcpRequest").filter(|m| m.is_object()) {
        b.items.push(Item {
            meta: meta_v5(m.get("meta")),
            node: Node::Mcp(mcp_from(m)),
        });
    }
    if let Some(spec) = v
        .get("spec")
        .and_then(|s| s.get("contents"))
        .filter(|c| c.as_object().is_some_and(|o| !o.is_empty()))
    {
        b.spec = Some(serde_json::to_string_pretty(spec).unwrap_or_default());
    }
    if !arr(v, "testSuites").is_empty() {
        warnings.push("Unit test suites were skipped — use after-response scripts and the collection runner instead".into());
    }
    if !arr(v, "certificates").is_empty() {
        warnings.push("CA certificates were skipped — certificates aren't supported yet".into());
    }
    // files written before the rename used `x-insomnia-rs`
    if let Some(ext) = v.get("x-logic-socket").or_else(|| v.get("x-insomnia-rs")) {
        apply_extension(&mut b, ext, &mut warnings);
    }
    Ok(Imported {
        format: Format::InsomniaV5,
        workspaces: vec![b],
        warnings,
    })
}

fn items_v5(list: &[Value], warnings: &mut Vec<String>) -> Vec<Item> {
    list.iter().filter_map(|i| item_v5(i, warnings)).collect()
}

fn item_v5(i: &Value, warnings: &mut Vec<String>) -> Option<Item> {
    let mut meta = meta_v5(i.get("meta"));
    let ext = item_ext(i).cloned().unwrap_or(Value::Null);
    let name = str_of(i, "name");
    let at = if name.is_empty() {
        "item".to_string()
    } else {
        format!("'{name}'")
    };
    let auth = auth_from_json(i.get("authentication"), warnings, &at);
    let id = meta.id.clone().unwrap_or_default();
    let node = if i.get("method").is_some() {
        let r = request_v5(i, auth.clone());
        if str_of(&ext, "kind") == "sse" {
            Node::Realtime(RealtimeRequest {
                name: r.name,
                description: r.description,
                kind: "sse".into(),
                url: r.url,
                headers: r
                    .headers
                    .into_iter()
                    .filter(|h| {
                        !(h.name.eq_ignore_ascii_case("accept") && h.value == "text/event-stream")
                    })
                    .collect(),
                authentication: r.authentication,
                method: r.method,
                body: r.body.text.unwrap_or_default(),
                payload: String::new(),
                ..Default::default()
            })
        } else {
            Node::Request(r)
        }
    } else if i.get("reflectionApi").is_some() {
        let mut g = GrpcRequest {
            name,
            description: desc_v5(i),
            url: parse_grpc_url(&str_of(i, "url")),
            method: str_of(i, "protoMethodName"),
            message: i
                .get("body")
                .map(|b| str_of(b, "text"))
                .filter(|t| !t.is_empty())
                .unwrap_or_else(|| "{}".into()),
            metadata: kvs(i, "metadata"),
            ..Default::default()
        };
        g.schema_source = if str_of(i, "protoFileId").is_empty() {
            "reflection".into()
        } else {
            "protos".into()
        };
        if let Some(s) = ext.get("schemaSource").and_then(Value::as_str) {
            g.schema_source = s.into();
        }
        if let Some(t) = ext.get("timeoutMs").and_then(Value::as_u64) {
            g.timeout_ms = t;
        }
        Node::Grpc(g)
    } else if i.get("url").is_some() {
        let is_ws = id.starts_with("ws-req");
        let is_sio = id.starts_with("socketio-req") || i.get("eventListeners").is_some();
        if !is_ws && !is_sio {
            warnings.push(format!("{at}: unrecognised item skipped"));
            return None;
        }
        // our own ids were prefixed on export; restore them
        for p in [WS_PREFIX, SIO_PREFIX] {
            if let Some(rest) = id.strip_prefix(p).filter(|r| r.starts_with("rt_")) {
                meta.id = Some(rest.to_string());
            }
        }
        let mut r = RealtimeRequest {
            name,
            description: desc_v5(i),
            kind: if is_sio {
                "socketio".into()
            } else {
                "websocket".into()
            },
            url: str_of(i, "url"),
            headers: kvs(i, "headers"),
            authentication: auth,
            payload: String::new(),
            ..Default::default()
        };
        if is_sio {
            r.event = "message".into();
        }
        apply_rt_ext(&mut r, &ext);
        Node::Realtime(r)
    } else {
        let scripts = i.get("scripts").cloned().unwrap_or(Value::Null);
        let f = Folder {
            name,
            description: desc_v5(i),
            environment: var_map(i.get("environment")),
            headers: kvs(i, "headers"),
            authentication: auth,
            pre_request_script: non_empty(str_of(&scripts, "preRequest")),
            after_response_script: non_empty(str_of(&scripts, "afterResponse")),
        };
        Node::Folder(f, items_v5(arr(i, "children"), warnings))
    };
    Some(Item { meta, node })
}

fn apply_rt_ext(r: &mut RealtimeRequest, ext: &Value) {
    if !ext.is_object() {
        return;
    }
    let s = |k: &str| ext.get(k).and_then(Value::as_str).map(str::to_string);
    if let Some(x) = s("payload") {
        r.payload = x;
    }
    if let Some(x) = s("payloadFormat") {
        r.payload_format = x;
    }
    if let Some(x) = s("event") {
        r.event = x;
    }
    if let Some(x) = s("namespace") {
        r.namespace = x;
    }
    if let Some(x) = s("socketioAuth") {
        r.socketio_auth = x;
    }
    if let Some(a) = ext.get("subprotocols").and_then(Value::as_array) {
        r.subprotocols = a
            .iter()
            .filter_map(|x| x.as_str().map(str::to_string))
            .collect();
    }
}

fn request_v5(i: &Value, auth: lsock_core::Auth) -> Request {
    let scripts = i.get("scripts").cloned().unwrap_or(Value::Null);
    let st = i.get("settings").cloned().unwrap_or(Value::Null);
    let b = |o: &Value, k: &str, d: bool| o.get(k).and_then(Value::as_bool).unwrap_or(d);
    let cookies = st.get("cookies").cloned().unwrap_or(Value::Null);
    let ext = item_ext(i).cloned().unwrap_or(Value::Null);
    Request {
        name: str_of(i, "name"),
        description: desc_v5(i),
        method: str_of(i, "method").to_uppercase(),
        url: str_of(i, "url"),
        parameters: kvs(i, "parameters"),
        path_parameters: kvs(i, "pathParameters"),
        headers: kvs(i, "headers"),
        body: body_from(i.get("body")),
        authentication: auth,
        pre_request_script: non_empty(str_of(&scripts, "preRequest")),
        after_response_script: non_empty(str_of(&scripts, "afterResponse")),
        settings: RequestSettings {
            store_cookies: b(&cookies, "store", true),
            send_cookies: b(&cookies, "send", true),
            disable_render_body: !b(&st, "renderRequestBody", true),
            encode_url: b(&st, "encodeUrl", true),
            follow_redirects: toggle(
                st.get("followRedirects")
                    .and_then(Value::as_str)
                    .unwrap_or("global"),
            ),
            disable_user_agent: b(&ext, "disableUserAgent", false),
        },
    }
}

/// Our own top-level extension: items Insomnia has no model for, and proto files.
fn apply_extension(b: &mut WorkspaceBundle, ext: &Value, warnings: &mut Vec<String>) {
    for p in arr(ext, "protoFiles") {
        b.protos.push(Entry {
            meta: meta_v5(p.get("meta")),
            body: ProtoFile {
                name: str_of(p, "name"),
                contents: str_of(p, "contents"),
            },
        });
    }
    for x in arr(ext, "items") {
        let meta = meta_v5(x.get("meta"));
        let data = x.get("data").cloned().unwrap_or(Value::Null);
        let node = match str_of(x, "type").as_str() {
            "LlmRequest" => serde_json::from_value::<LlmRequest>(data)
                .ok()
                .map(Node::Llm),
            "McpServer" => serde_json::from_value::<McpServer>(data)
                .ok()
                .map(Node::Mcp),
            "RealtimeRequest" => serde_json::from_value::<RealtimeRequest>(data)
                .ok()
                .map(Node::Realtime),
            "GrpcRequest" => serde_json::from_value::<GrpcRequest>(data)
                .ok()
                .map(Node::Grpc),
            other => {
                warnings.push(format!("unknown extension item type '{other}' skipped"));
                None
            }
        };
        let Some(node) = node else { continue };
        let item = Item { meta, node };
        let parent = str_of(x, "parentId");
        if parent.is_empty()
            || Some(&parent) == b.meta.id.as_ref()
            || !insert_into(&mut b.items, &parent, item.clone())
        {
            b.items.push(item);
        }
    }
}

fn insert_into(items: &mut [Item], parent: &str, item: Item) -> bool {
    for i in items.iter_mut() {
        if let Node::Folder(_, children) = &mut i.node {
            if i.meta.id.as_deref() == Some(parent) {
                children.push(item);
                return true;
            }
            if insert_into(children, parent, item.clone()) {
                return true;
            }
        }
    }
    false
}

// ------------------------------------------------------------------ v5 export

/// Serialise one workspace as an Insomnia v5 YAML document.
pub fn export_v5(b: &WorkspaceBundle) -> Result<String> {
    let ws = &b.workspace;
    let mut doc = Map::new();
    let only_mcp = ws.scope == WorkspaceScope::Mcp
        && b.items.len() == 1
        && matches!(b.items[0].node, Node::Mcp(_));
    let ty = match ws.scope {
        WorkspaceScope::Environment => "environment.insomnia.rest/5.0",
        _ if only_mcp => "mcpClient.insomnia/5.0",
        _ => "collection.insomnia.rest/5.0",
    };
    doc.insert("type".into(), json!(ty));
    doc.insert("schema_version".into(), json!(SCHEMA_VERSION));
    doc.insert("name".into(), json!(ws.name));
    doc.insert(
        "meta".into(),
        meta_out(
            &Meta {
                sort_key: None,
                ..b.meta.clone()
            },
            Some(&ws.description),
            None,
        ),
    );

    let mut ext_items = vec![];
    let ws_id = b.meta.id.clone().unwrap_or_default();
    if only_mcp {
        if let Node::Mcp(s) = &b.items[0].node {
            doc.insert("mcpRequest".into(), mcp_out(s, &b.items[0].meta));
        }
    } else if ws.scope != WorkspaceScope::Environment {
        let coll = collection_out(&b.items, &ws_id, &mut ext_items);
        doc.insert("collection".into(), Value::Array(coll));
        if let Some(j) = &b.cookie_jar {
            doc.insert(
                "cookieJar".into(),
                json!({
                    "name": j.body.name,
                    "meta": meta_out(&Meta { sort_key: None, ..j.meta.clone() }, None, None),
                    "cookies": j.body.cookies.iter().map(cookie_out).collect::<Vec<_>>(),
                }),
            );
        }
    }
    if let Some(base) = &b.base_env {
        let mut e = env_out(base, false);
        if !b.sub_envs.is_empty() {
            e.as_object_mut().unwrap().insert(
                "subEnvironments".into(),
                Value::Array(b.sub_envs.iter().map(|s| env_out(s, true)).collect()),
            );
        }
        doc.insert("environments".into(), e);
    }
    if !ext_items.is_empty() || !b.protos.is_empty() {
        doc.insert(
            "x-logic-socket".into(),
            json!({
                "version": 1,
                "protoFiles": b.protos.iter().map(|p| json!({
                    "name": p.body.name, "contents": p.body.contents,
                    "meta": meta_out(&p.meta, None, None),
                })).collect::<Vec<_>>(),
                "items": ext_items,
            }),
        );
    }
    let pruned = prune(Value::Object(doc)).unwrap_or_default();
    to_yaml(&pruned)
}

fn collection_out(items: &[Item], parent: &str, ext: &mut Vec<Value>) -> Vec<Value> {
    let mut out = vec![];
    for it in items {
        let m = &it.meta;
        match &it.node {
            Node::Folder(f, children) => {
                let id = m.id.clone().unwrap_or_default();
                out.push(json!({
                    "name": f.name,
                    "meta": meta_out(m, Some(&f.description), Some(false)),
                    "children": collection_out(children, &id, ext),
                    "scripts": scripts_out(&f.pre_request_script, &f.after_response_script),
                    "authentication": auth_to_json(&f.authentication),
                    "environment": Value::Object(f.environment.clone().into_iter().collect()),
                    "headers": kvs_out(&f.headers),
                }));
            }
            Node::Request(r) => out.push(request_out(r, m)),
            Node::Realtime(r) if r.kind == "sse" => {
                let mut headers = r.headers.clone();
                if !headers
                    .iter()
                    .any(|h| h.name.eq_ignore_ascii_case("accept"))
                {
                    headers.push(KeyValue::new("Accept", "text/event-stream"));
                }
                let req = Request {
                    name: r.name.clone(),
                    description: r.description.clone(),
                    method: r.method.clone(),
                    url: r.url.clone(),
                    headers,
                    body: if r.body.is_empty() {
                        Body::default()
                    } else {
                        Body {
                            text: Some(r.body.clone()),
                            ..Default::default()
                        }
                    },
                    authentication: r.authentication.clone(),
                    ..Default::default()
                };
                let mut v = request_out(&req, m);
                v.as_object_mut()
                    .unwrap()
                    .insert("x-lsock".into(), json!({ "kind": "sse" }));
                out.push(v);
            }
            Node::Realtime(r) => {
                let sio = r.kind == "socketio";
                let mut meta = m.clone();
                let prefix = if sio { SIO_PREFIX } else { WS_PREFIX };
                if let Some(id) = &m.id
                    && !id.starts_with(prefix.trim_end_matches('_'))
                {
                    meta.id = Some(format!("{prefix}{id}"));
                }
                let mut v = json!({
                    "url": r.url,
                    "name": r.name,
                    "meta": meta_out(&meta, Some(&r.description), Some(false)),
                    "settings": if sio {
                        json!({"encodeUrl": true, "cookies": {"send": true, "store": true}})
                    } else {
                        json!({"encodeUrl": true, "followRedirects": "global", "cookies": {"send": true, "store": true}})
                    },
                    "authentication": auth_to_json(&r.authentication),
                    "headers": kvs_out(&r.headers),
                    "x-lsock": {
                        "payload": r.payload, "payloadFormat": r.payload_format,
                        "subprotocols": r.subprotocols,
                        "event": if sio { Value::from(r.event.clone()) } else { Value::Null },
                        "namespace": if sio { Value::from(r.namespace.clone()) } else { Value::Null },
                        "socketioAuth": if sio { Value::from(r.socketio_auth.clone()) } else { Value::Null },
                    },
                });
                if sio {
                    v.as_object_mut()
                        .unwrap()
                        .insert("eventListeners".into(), json!([]));
                }
                out.push(v);
            }
            Node::Grpc(g) => {
                let url = g.url.strip_prefix("grpc://").unwrap_or(&g.url);
                out.push(json!({
                    "url": url,
                    "name": g.name,
                    "meta": meta_out(m, Some(&g.description), Some(false)),
                    "body": { "text": g.message },
                    "metadata": kvs_out(&g.metadata),
                    "protoFileId": "",
                    "protoMethodName": g.method,
                    "reflectionApi": { "enabled": false, "url": "", "apiKey": "", "module": "" },
                    "x-lsock": { "schemaSource": g.schema_source, "timeoutMs": g.timeout_ms },
                }));
            }
            Node::Mcp(s) => ext.push(ext_item("McpServer", parent, m, s)),
            Node::Llm(l) => ext.push(ext_item("LlmRequest", parent, m, l)),
        }
    }
    out
}

fn ext_item<T: serde::Serialize>(ty: &str, parent: &str, meta: &Meta, body: &T) -> Value {
    json!({ "type": ty, "parentId": parent, "meta": meta_out(meta, None, None), "data": body })
}

fn request_out(r: &Request, m: &Meta) -> Value {
    let mut v = json!({
        "url": r.url,
        "name": r.name,
        "meta": meta_out(m, Some(&r.description), Some(false)),
        "method": r.method,
        "body": body_out(&r.body),
        "parameters": kvs_out(&r.parameters),
        "headers": kvs_out(&r.headers),
        "authentication": auth_to_json(&r.authentication),
        "scripts": scripts_out(&r.pre_request_script, &r.after_response_script),
        "settings": {
            "renderRequestBody": !r.settings.disable_render_body,
            "encodeUrl": r.settings.encode_url,
            "followRedirects": toggle_str(r.settings.follow_redirects),
            "cookies": { "send": r.settings.send_cookies, "store": r.settings.store_cookies },
            "rebuildPath": true,
        },
        "pathParameters": r.path_parameters.iter().map(|p| json!({"name": p.name, "value": p.value})).collect::<Vec<_>>(),
    });
    if r.settings.disable_user_agent {
        v.as_object_mut()
            .unwrap()
            .insert("x-lsock".into(), json!({ "disableUserAgent": true }));
    }
    v
}

/// A safe file name for a workspace in a Git repo: `insomnia.<slug>.yaml`.
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
        "insomnia.{}.yaml",
        if slug.is_empty() { "workspace" } else { &slug }
    )
}

// ------------------------------------------------------------------ v4 import

pub fn import_v4(v: &Value) -> Result<Imported> {
    let mut warnings = vec![];
    let resources = arr(v, "resources");
    let by_parent: HashMap<String, Vec<&Value>> =
        resources.iter().fold(HashMap::new(), |mut m, r| {
            m.entry(str_of(r, "parentId")).or_default().push(r);
            m
        });
    let ty = |r: &Value| str_of(r, "_type");
    let mut workspaces: Vec<&Value> = resources.iter().filter(|r| ty(r) == "workspace").collect();
    let synthetic;
    if workspaces.is_empty() {
        // request-only exports hang everything off __WORKSPACE_ID__
        synthetic = json!({"_id": "__WORKSPACE_ID__", "_type": "workspace", "name": "Imported"});
        workspaces.push(&synthetic);
    }
    let mut out = vec![];
    for w in workspaces {
        let wid = str_of(w, "_id");
        let scope = match str_of(w, "scope").as_str() {
            "environment" => WorkspaceScope::Environment,
            "mcp" => WorkspaceScope::Mcp,
            _ => WorkspaceScope::Collection,
        };
        let mut b = WorkspaceBundle {
            meta: meta_v4(w),
            workspace: Workspace {
                name: str_of(w, "name"),
                description: str_of(w, "description"),
                scope,
                ..Default::default()
            },
            ..Default::default()
        };
        let kids = |id: &str| -> Vec<&Value> {
            let mut k = by_parent.get(id).cloned().unwrap_or_default();
            k.sort_by(|a, b| {
                let ka = a.get("metaSortKey").and_then(Value::as_f64).unwrap_or(0.0);
                let kb = b.get("metaSortKey").and_then(Value::as_f64).unwrap_or(0.0);
                ka.total_cmp(&kb)
            });
            k
        };
        // environments: first one under the workspace is the base, the rest are subs
        let mut envs = kids(&wid).into_iter().filter(|r| ty(r) == "environment");
        if let Some(base) = envs.next() {
            b.base_env = Some(env_v4(base));
            let base_id = str_of(base, "_id");
            b.sub_envs = kids(&base_id)
                .into_iter()
                .filter(|r| ty(r) == "environment")
                .map(env_v4)
                .collect();
            b.sub_envs.extend(envs.map(env_v4));
        }
        if let Some(j) = kids(&wid).into_iter().find(|r| ty(r) == "cookie_jar") {
            b.cookie_jar = Some(Entry {
                meta: meta_v4(j),
                body: CookieJar {
                    name: str_of(j, "name"),
                    cookies: arr(j, "cookies").iter().map(cookie_from).collect(),
                },
            });
        }
        if let Some(spec) = kids(&wid).into_iter().find(|r| ty(r) == "api_spec") {
            b.spec = non_empty(str_of(spec, "contents"));
        }
        b.items = items_v4(&wid, &kids, &mut warnings);
        b.protos = protos_v4(&wid, &kids, "");
        if kids(&wid).iter().any(|r| ty(r) == "unit_test_suite") {
            warnings.push(format!(
                "'{}': unit test suites were skipped",
                b.workspace.name
            ));
        }
        out.push(b);
    }
    let known = [
        "workspace",
        "environment",
        "cookie_jar",
        "api_spec",
        "request_group",
        "request",
        "grpc_request",
        "websocket_request",
        "websocket_payload",
        "socketio_request",
        "socketio_payload",
        "proto_file",
        "proto_directory",
        "unit_test_suite",
        "unit_test",
        "mcp_request",
    ];
    let mut skipped: Vec<String> = resources
        .iter()
        .map(ty)
        .filter(|t| !known.contains(&t.as_str()))
        .collect();
    skipped.sort();
    skipped.dedup();
    if !skipped.is_empty() {
        warnings.push(format!(
            "skipped unsupported resource types: {}",
            skipped.join(", ")
        ));
    }
    Ok(Imported {
        format: Format::InsomniaV4,
        workspaces: out,
        warnings,
    })
}

fn meta_v4(r: &Value) -> Meta {
    Meta {
        id: r
            .get("_id")
            .and_then(Value::as_str)
            .filter(|s| !s.starts_with("__"))
            .map(str::to_string),
        sort_key: r.get("metaSortKey").and_then(Value::as_f64),
        created: i64_of(r, "created"),
        modified: i64_of(r, "modified"),
    }
}

fn env_v4(r: &Value) -> Entry<Environment> {
    Entry {
        meta: meta_v4(r),
        body: Environment {
            name: str_of(r, "name"),
            data: var_map(r.get("data")),
            color: r.get("color").and_then(Value::as_str).map(str::to_string),
            is_private: r.get("isPrivate").and_then(Value::as_bool).unwrap_or(false),
            secret_keys: vec![],
        },
    }
}

fn protos_v4<'a>(
    parent: &str,
    kids: &dyn Fn(&str) -> Vec<&'a Value>,
    prefix: &str,
) -> Vec<Entry<ProtoFile>> {
    let mut out = vec![];
    for r in kids(parent) {
        match str_of(r, "_type").as_str() {
            "proto_file" => out.push(Entry {
                meta: meta_v4(r),
                body: ProtoFile {
                    name: format!("{prefix}{}", str_of(r, "name")),
                    contents: str_of(r, "protoText"),
                },
            }),
            "proto_directory" => {
                let p = format!("{prefix}{}/", str_of(r, "name"));
                out.extend(protos_v4(&str_of(r, "_id"), kids, &p));
            }
            _ => {}
        }
    }
    out
}

fn items_v4<'a>(
    parent: &str,
    kids: &dyn Fn(&str) -> Vec<&'a Value>,
    warnings: &mut Vec<String>,
) -> Vec<Item> {
    let mut out = vec![];
    for r in kids(parent) {
        let name = str_of(r, "name");
        let at = format!("'{name}'");
        let id = str_of(r, "_id");
        let auth = || auth_from_json(r.get("authentication"), &mut vec![], &at);
        let node = match str_of(r, "_type").as_str() {
            "request_group" => Node::Folder(
                Folder {
                    name,
                    description: str_of(r, "description"),
                    environment: var_map(r.get("environment")),
                    headers: kvs(r, "headers"),
                    authentication: auth_from_json(r.get("authentication"), warnings, &at),
                    pre_request_script: non_empty(str_of(r, "preRequestScript")),
                    after_response_script: non_empty(str_of(r, "afterResponseScript")),
                },
                items_v4(&id, kids, warnings),
            ),
            "request" => {
                let b = |k: &str, d: bool| r.get(k).and_then(Value::as_bool).unwrap_or(d);
                Node::Request(Request {
                    name,
                    description: str_of(r, "description"),
                    method: r
                        .get("method")
                        .and_then(Value::as_str)
                        .unwrap_or("GET")
                        .to_uppercase(),
                    url: str_of(r, "url"),
                    parameters: kvs(r, "parameters"),
                    path_parameters: kvs(r, "pathParameters"),
                    headers: kvs(r, "headers"),
                    body: body_from(r.get("body")),
                    authentication: auth_from_json(r.get("authentication"), warnings, &at),
                    pre_request_script: non_empty(str_of(r, "preRequestScript")),
                    after_response_script: non_empty(str_of(r, "afterResponseScript")),
                    settings: RequestSettings {
                        store_cookies: b("settingStoreCookies", true),
                        send_cookies: b("settingSendCookies", true),
                        disable_render_body: b("settingDisableRenderRequestBody", false),
                        encode_url: b("settingEncodeUrl", true),
                        follow_redirects: toggle(
                            r.get("settingFollowRedirects")
                                .and_then(Value::as_str)
                                .unwrap_or("global"),
                        ),
                        disable_user_agent: b("disableUserAgentHeader", false),
                    },
                })
            }
            "websocket_request" => {
                let payload = kids(&id)
                    .into_iter()
                    .find(|p| str_of(p, "_type") == "websocket_payload");
                Node::Realtime(RealtimeRequest {
                    name,
                    description: str_of(r, "description"),
                    kind: "websocket".into(),
                    url: str_of(r, "url"),
                    headers: kvs(r, "headers"),
                    authentication: auth(),
                    payload: payload.map(|p| str_of(p, "value")).unwrap_or_default(),
                    payload_format: if payload.is_some_and(|p| str_of(p, "mode").contains("json")) {
                        "json".into()
                    } else {
                        "text".into()
                    },
                    ..Default::default()
                })
            }
            "socketio_request" => {
                let payload = kids(&id)
                    .into_iter()
                    .find(|p| str_of(p, "_type") == "socketio_payload");
                Node::Realtime(RealtimeRequest {
                    name,
                    description: str_of(r, "description"),
                    kind: "socketio".into(),
                    url: str_of(r, "url"),
                    headers: kvs(r, "headers"),
                    authentication: auth(),
                    payload: payload.map(|p| str_of(p, "value")).unwrap_or_default(),
                    event: payload
                        .map(|p| str_of(p, "eventName"))
                        .filter(|e| !e.is_empty())
                        .unwrap_or_else(|| "message".into()),
                    ..Default::default()
                })
            }
            "grpc_request" => Node::Grpc(GrpcRequest {
                name,
                description: str_of(r, "description"),
                url: parse_grpc_url(&str_of(r, "url")),
                schema_source: if str_of(r, "protoFileId").is_empty() {
                    "reflection".into()
                } else {
                    "protos".into()
                },
                method: str_of(r, "protoMethodName"),
                message: r
                    .get("body")
                    .map(|b| str_of(b, "text"))
                    .filter(|t| !t.is_empty())
                    .unwrap_or_else(|| "{}".into()),
                metadata: kvs(r, "metadata"),
                ..Default::default()
            }),
            "mcp_request" => Node::Mcp(McpServer {
                description: str_of(r, "description"),
                ..mcp_from(r)
            }),
            _ => continue,
        };
        out.push(Item {
            meta: meta_v4(r),
            node,
        });
    }
    out
}
