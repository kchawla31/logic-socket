//! Import and export between insomnia-rs and other API tools.
//!
//! Importers turn a file into [`WorkspaceBundle`]s (plain model values plus
//! optional original ids); the engine writes bundles to the store. Exporters
//! go the other way. Nothing here touches the database, the network or secrets.

pub mod codegen;
pub mod curl;
pub mod har;
pub mod insomnia;
pub mod openapi;
pub mod postman;

use irs_core::{
    CookieJar, Environment, Folder, GrpcRequest, LlmRequest, McpServer, ProtoFile, RealtimeRequest,
    Request, Workspace,
};
use serde_json::Value;

#[derive(Debug, thiserror::Error)]
pub enum ConvertError {
    #[error(
        "unrecognised file: expected an Insomnia, Postman, OpenAPI/Swagger, HAR or cURL export"
    )]
    Unknown,
    #[error("{format}: {message}")]
    Invalid { format: Format, message: String },
    #[error("YAML: {0}")]
    Yaml(String),
}

impl ConvertError {
    pub(crate) fn invalid(format: Format, message: impl Into<String>) -> Self {
        Self::Invalid {
            format,
            message: message.into(),
        }
    }
}

pub type Result<T, E = ConvertError> = std::result::Result<T, E>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Format {
    InsomniaV5,
    InsomniaV4,
    PostmanCollection,
    PostmanEnvironment,
    OpenApi3,
    Swagger2,
    Har,
    Curl,
}

impl std::fmt::Display for Format {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Format::InsomniaV5 => "Insomnia v5",
            Format::InsomniaV4 => "Insomnia v4",
            Format::PostmanCollection => "Postman collection",
            Format::PostmanEnvironment => "Postman environment",
            Format::OpenApi3 => "OpenAPI 3",
            Format::Swagger2 => "Swagger 2",
            Format::Har => "HAR",
            Format::Curl => "cURL",
        })
    }
}

/// Original identity of an imported document. Everything is optional: ids are
/// kept when re-importing our own files (Git sync), otherwise fresh ids are used.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Meta {
    pub id: Option<String>,
    pub sort_key: Option<f64>,
    pub created: Option<i64>,
    pub modified: Option<i64>,
}

impl Meta {
    pub fn with_id(id: impl Into<String>) -> Self {
        Self {
            id: Some(id.into()),
            ..Default::default()
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Entry<T> {
    pub meta: Meta,
    pub body: T,
}

impl<T> Entry<T> {
    pub fn new(body: T) -> Self {
        Self {
            meta: Meta::default(),
            body,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Node {
    Folder(Folder, Vec<Item>),
    Request(Request),
    Realtime(RealtimeRequest),
    Grpc(GrpcRequest),
    Mcp(McpServer),
    Llm(LlmRequest),
}

impl Node {
    pub fn name(&self) -> &str {
        match self {
            Node::Folder(f, _) => &f.name,
            Node::Request(r) => &r.name,
            Node::Realtime(r) => &r.name,
            Node::Grpc(r) => &r.name,
            Node::Mcp(r) => &r.name,
            Node::Llm(r) => &r.name,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Item {
    pub meta: Meta,
    pub node: Node,
}

impl Item {
    pub fn new(node: Node) -> Self {
        Self {
            meta: Meta::default(),
            node,
        }
    }
}

/// One workspace with everything under it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct WorkspaceBundle {
    pub meta: Meta,
    pub workspace: Workspace,
    pub base_env: Option<Entry<Environment>>,
    pub sub_envs: Vec<Entry<Environment>>,
    pub cookie_jar: Option<Entry<CookieJar>>,
    pub items: Vec<Item>,
    pub protos: Vec<Entry<ProtoFile>>,
    /// API description the collection was generated from (OpenAPI/Swagger), as text.
    pub spec: Option<String>,
}

impl WorkspaceBundle {
    /// Every item in tree order (depth first).
    pub fn walk(&self) -> Vec<&Item> {
        fn go<'a>(items: &'a [Item], out: &mut Vec<&'a Item>) {
            for i in items {
                out.push(i);
                if let Node::Folder(_, c) = &i.node {
                    go(c, out);
                }
            }
        }
        let mut out = vec![];
        go(&self.items, &mut out);
        out
    }

    pub fn request_count(&self) -> usize {
        self.walk()
            .iter()
            .filter(|i| !matches!(i.node, Node::Folder(..)))
            .count()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Imported {
    pub format: Format,
    pub workspaces: Vec<WorkspaceBundle>,
    /// Things that could not be carried over exactly (shown to the user).
    pub warnings: Vec<String>,
}

/// Parse JSON or YAML into a JSON value.
pub fn parse_structured(text: &str) -> Option<Value> {
    let t = text.trim_start_matches('\u{feff}').trim();
    if t.is_empty() {
        return None;
    }
    if (t.starts_with('{') || t.starts_with('['))
        && let Ok(v) = serde_json::from_str::<Value>(t)
    {
        return Some(v);
    }
    serde_yaml_ng::from_str::<Value>(t)
        .ok()
        .filter(|v| v.is_object())
}

/// Recognise the format of a file's contents.
pub fn detect(text: &str) -> Option<Format> {
    // first meaningful line of a pasted shell snippet
    let first = text
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty() && !l.starts_with('#'))
        .unwrap_or("");
    if curl::looks_like_curl(first) {
        return Some(Format::Curl);
    }
    let v = parse_structured(text)?;
    detect_value(&v)
}

fn detect_value(v: &Value) -> Option<Format> {
    let s = |k: &str| v.get(k).and_then(Value::as_str).unwrap_or("");
    if s("type").ends_with(".insomnia.rest/5.0") || s("type") == "mcpClient.insomnia/5.0" {
        return Some(Format::InsomniaV5);
    }
    if s("_type") == "export" && v.get("resources").is_some_and(Value::is_array) {
        return Some(Format::InsomniaV4);
    }
    if v.get("openapi")
        .is_some_and(|x| x.as_str().is_some_and(|x| x.starts_with('3')))
    {
        return Some(Format::OpenApi3);
    }
    if s("swagger") == "2.0" {
        return Some(Format::Swagger2);
    }
    if let Some(info) = v.get("info") {
        let schema = info.get("schema").and_then(Value::as_str).unwrap_or("");
        if schema.contains("getpostman.com") || schema.contains("schema.postman.com") {
            return Some(Format::PostmanCollection);
        }
    }
    // collections downloaded from the Postman API are wrapped in {"collection": ...}
    if v.get("collection")
        .is_some_and(|c| c.get("info").is_some() && c.get("item").is_some())
    {
        return Some(Format::PostmanCollection);
    }
    if v.get("log").is_some_and(|l| l.get("entries").is_some())
        || (v.get("method").is_some()
            && v.get("url").is_some()
            && (v.get("httpVersion").is_some() || v.get("postData").is_some()))
    {
        return Some(Format::Har);
    }
    if v.get("values").is_some_and(Value::is_array)
        && (!s("_postman_variable_scope").is_empty() || v.get("name").is_some())
        && v.get("item").is_none()
    {
        return Some(Format::PostmanEnvironment);
    }
    None
}

/// Import any supported file.
pub fn import(text: &str) -> Result<Imported> {
    let format = detect(text).ok_or(ConvertError::Unknown)?;
    if format == Format::Curl {
        return curl::import_many(text);
    }
    let v = parse_structured(text).ok_or(ConvertError::Unknown)?;
    match format {
        Format::InsomniaV5 => insomnia::import_v5(&v),
        Format::InsomniaV4 => insomnia::import_v4(&v),
        Format::PostmanCollection => postman::import_collection(v.get("collection").unwrap_or(&v)),
        Format::PostmanEnvironment => postman::import_environment(&v),
        Format::OpenApi3 | Format::Swagger2 => openapi::import(&v, text, format),
        Format::Har => har::import(&v),
        Format::Curl => unreachable!(),
    }
}

/// Insomnia stores "inherit" as `{}`; we use a tagged enum.
pub(crate) fn auth_from_json(
    v: Option<&Value>,
    warnings: &mut Vec<String>,
    at: &str,
) -> irs_core::Auth {
    use irs_core::Auth;
    let Some(v) = v.filter(|v| v.is_object() && v.get("type").is_some()) else {
        return Auth::Inherit;
    };
    match serde_json::from_value::<Auth>(v.clone()) {
        Ok(a) => a,
        Err(_) => {
            let t = v.get("type").and_then(Value::as_str).unwrap_or("?");
            warnings.push(format!(
                "{at}: {t} auth isn't supported yet; set to No auth"
            ));
            Auth::None
        }
    }
}

pub(crate) fn auth_to_json(a: &irs_core::Auth) -> Value {
    match a {
        irs_core::Auth::Inherit => Value::Object(Default::default()),
        other => serde_json::to_value(other).unwrap_or_default(),
    }
}

pub(crate) fn str_of(v: &Value, k: &str) -> String {
    match v.get(k) {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Null) | None => String::new(),
        Some(other) => other.to_string(),
    }
}

/// Turn any JSON scalar into the string Insomnia would show.
pub(crate) fn scalar(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests;
