//! Typed document models. Field names serialize as camelCase so the UI and a
//! future Insomnia import/export layer see the same shapes as Insomnia.

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Common columns stored outside the JSON body.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Meta {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub parent_id: Option<String>,
    pub sort_key: f64,
    pub created: i64,
    pub modified: i64,
}

/// A stored document: metadata + typed body, flattened when serialized.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Doc<T> {
    #[serde(flatten)]
    pub meta: Meta,
    #[serde(flatten)]
    pub body: T,
}

impl<T> Doc<T> {
    pub fn id(&self) -> &str {
        &self.meta.id
    }
}

impl<T> std::ops::Deref for Doc<T> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.body
    }
}

impl<T> std::ops::DerefMut for Doc<T> {
    fn deref_mut(&mut self) -> &mut T {
        &mut self.body
    }
}

pub trait Model: Serialize + for<'de> Deserialize<'de> + Clone + Send + 'static {
    const TYPE: &'static str;
    const PREFIX: &'static str;
}

macro_rules! model {
    ($t:ty, $type:literal, $prefix:literal) => {
        impl Model for $t {
            const TYPE: &'static str = $type;
            const PREFIX: &'static str = $prefix;
        }
    };
}

pub type VarMap = IndexMap<String, Value>;

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct KeyValue {
    pub id: Option<String>,
    pub name: String,
    pub value: String,
    pub description: Option<String>,
    pub disabled: bool,
}

impl KeyValue {
    pub fn new(name: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            value: value.into(),
            ..Default::default()
        }
    }
}

// ---------------------------------------------------------------- Workspace

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum WorkspaceScope {
    #[default]
    Collection,
    Design,
    Mcp,
    /// A workspace holding only global environments.
    Environment,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Workspace {
    pub name: String,
    pub description: String,
    pub scope: WorkspaceScope,
    /// Active sub-environment (id of an Environment whose parent is the base env).
    pub active_environment_id: Option<String>,
    /// Global environment workspace + sub env to layer under this workspace.
    pub active_global_base_id: Option<String>,
    pub active_global_sub_id: Option<String>,
}
model!(Workspace, "Workspace", "wrk");

// ---------------------------------------------------------------- Auth

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(
    tag = "type",
    rename_all = "lowercase",
    rename_all_fields = "camelCase"
)]
pub enum Auth {
    /// Inherit from the nearest folder (Insomnia stores this as `{}`).
    #[default]
    Inherit,
    None,
    Basic {
        #[serde(default)]
        username: String,
        #[serde(default)]
        password: String,
        #[serde(default)]
        disabled: bool,
    },
    Bearer {
        #[serde(default)]
        token: String,
        #[serde(default)]
        prefix: Option<String>,
        #[serde(default)]
        disabled: bool,
    },
    #[serde(rename = "apikey")]
    ApiKey {
        #[serde(default)]
        key: String,
        #[serde(default)]
        value: String,
        /// `header` (default) | `queryParams` | `cookie`
        #[serde(default)]
        add_to: Option<String>,
        #[serde(default)]
        disabled: bool,
    },
    /// HTTP Digest (RFC 7616): answered after the server's 401 challenge.
    Digest {
        #[serde(default)]
        username: String,
        #[serde(default)]
        password: String,
        #[serde(default)]
        disabled: bool,
    },
    #[serde(rename = "oauth2")]
    OAuth2(OAuth2Config),
    #[serde(rename = "oauth1")]
    OAuth1(OAuth1Config),
    /// AWS Signature Version 4.
    Iam(AwsIamConfig),
    /// Credentials for the request host from `~/.netrc`.
    Netrc {
        #[serde(default)]
        disabled: bool,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct OAuth2Config {
    /// `client_credentials` | `password` | `authorization_code` | `refresh_token`
    pub grant_type: String,
    pub access_token_url: String,
    pub authorization_url: String,
    pub client_id: String,
    pub client_secret: String,
    pub scope: String,
    pub audience: String,
    pub resource: String,
    pub username: String,
    pub password: String,
    /// Must be `http://localhost:<port>/...` or `http://127.0.0.1:<port>/...` for the code flow.
    pub redirect_url: String,
    pub use_pkce: bool,
    /// Send client id/secret in the body instead of HTTP Basic.
    pub credentials_in_body: bool,
    /// Header prefix (default `Bearer`).
    pub token_prefix: String,
    pub disabled: bool,
}

impl Default for OAuth2Config {
    fn default() -> Self {
        Self {
            grant_type: "client_credentials".into(),
            access_token_url: String::new(),
            authorization_url: String::new(),
            client_id: String::new(),
            client_secret: String::new(),
            scope: String::new(),
            audience: String::new(),
            resource: String::new(),
            username: String::new(),
            password: String::new(),
            redirect_url: "http://127.0.0.1:8970/callback".into(),
            use_pkce: true,
            credentials_in_body: false,
            token_prefix: String::new(),
            disabled: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct OAuth1Config {
    pub consumer_key: String,
    pub consumer_secret: String,
    pub token_key: String,
    pub token_secret: String,
    /// `HMAC-SHA1` | `HMAC-SHA256` | `PLAINTEXT`
    pub signature_method: String,
    pub realm: String,
    pub callback: String,
    pub verifier: String,
    /// Fixed values for testing; generated when empty.
    pub nonce: String,
    pub timestamp: String,
    pub include_body_hash: bool,
    pub disabled: bool,
}

impl Default for OAuth1Config {
    fn default() -> Self {
        Self {
            consumer_key: String::new(),
            consumer_secret: String::new(),
            token_key: String::new(),
            token_secret: String::new(),
            signature_method: "HMAC-SHA1".into(),
            realm: String::new(),
            callback: String::new(),
            verifier: String::new(),
            nonce: String::new(),
            timestamp: String::new(),
            include_body_hash: false,
            disabled: false,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct AwsIamConfig {
    pub access_key_id: String,
    pub secret_access_key: String,
    pub session_token: String,
    pub region: String,
    pub service: String,
    pub disabled: bool,
}

/// Cached OAuth 2 token, stored as a child of the request/folder that owns the auth.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct OAuth2Token {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub token_type: Option<String>,
    /// Epoch ms; `None` = no expiry given.
    pub expires_at: Option<i64>,
    pub scope: Option<String>,
    pub id_token: Option<String>,
    /// Error from the last attempt, shown in the UI.
    pub error: Option<String>,
}
model!(OAuth2Token, "OAuth2Token", "oa2");

impl Auth {
    pub fn is_inherit(&self) -> bool {
        matches!(self, Auth::Inherit)
    }
}

// ---------------------------------------------------------------- Folder

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Folder {
    pub name: String,
    pub description: String,
    pub environment: VarMap,
    pub headers: Vec<KeyValue>,
    pub authentication: Auth,
    pub pre_request_script: Option<String>,
    pub after_response_script: Option<String>,
}
model!(Folder, "Folder", "fld");

// ---------------------------------------------------------------- Request

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct BodyParam {
    pub id: Option<String>,
    pub name: String,
    pub value: String,
    pub disabled: bool,
    /// `file` for multipart file parts, otherwise text.
    #[serde(rename = "type")]
    pub kind: Option<String>,
    pub file_name: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Body {
    pub mime_type: Option<String>,
    pub text: Option<String>,
    pub file_name: Option<String>,
    pub params: Vec<BodyParam>,
}

pub mod mime {
    pub const JSON: &str = "application/json";
    pub const FORM: &str = "application/x-www-form-urlencoded";
    pub const MULTIPART: &str = "multipart/form-data";
    pub const FILE: &str = "application/octet-stream";
    pub const GRAPHQL: &str = "application/graphql";
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Toggle {
    #[default]
    Global,
    On,
    Off,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct RequestSettings {
    pub store_cookies: bool,
    pub send_cookies: bool,
    pub disable_render_body: bool,
    pub encode_url: bool,
    pub follow_redirects: Toggle,
    pub disable_user_agent: bool,
}

impl Default for RequestSettings {
    fn default() -> Self {
        Self {
            store_cookies: true,
            send_cookies: true,
            disable_render_body: false,
            encode_url: true,
            follow_redirects: Toggle::Global,
            disable_user_agent: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Request {
    pub name: String,
    pub description: String,
    pub method: String,
    pub url: String,
    pub parameters: Vec<KeyValue>,
    pub path_parameters: Vec<KeyValue>,
    pub headers: Vec<KeyValue>,
    pub body: Body,
    pub authentication: Auth,
    pub pre_request_script: Option<String>,
    pub after_response_script: Option<String>,
    pub settings: RequestSettings,
}
model!(Request, "Request", "req");

impl Default for Request {
    fn default() -> Self {
        Self {
            name: "New Request".into(),
            description: String::new(),
            method: "GET".into(),
            url: String::new(),
            parameters: vec![],
            path_parameters: vec![],
            headers: vec![],
            body: Body::default(),
            authentication: Auth::Inherit,
            pre_request_script: None,
            after_response_script: None,
            settings: RequestSettings::default(),
        }
    }
}

/// Matches `/:param` segments (Insomnia's PATH_PARAMETER_REGEX `/\/:[^/?#:]+/g`).
pub fn path_params_in_url(url: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let bytes = url.as_bytes();
    let mut i = 0;
    while i + 1 < bytes.len() {
        if bytes[i] == b'/' && bytes[i + 1] == b':' {
            let start = i + 2;
            let mut end = start;
            while end < bytes.len() && !matches!(bytes[end], b'/' | b'?' | b'#' | b':') {
                end += 1;
            }
            if end > start {
                let name = url[start..end].to_string();
                if !out.contains(&name) {
                    out.push(name);
                }
            }
            i = end;
        } else {
            i += 1;
        }
    }
    out
}

// ---------------------------------------------------------------- Environment

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Environment {
    pub name: String,
    pub data: VarMap,
    pub color: Option<String>,
    pub is_private: bool,
    /// Keys whose values are secrets (masked in UI/logs; vault-encrypted in Phase 5).
    pub secret_keys: Vec<String>,
}
model!(Environment, "Environment", "env");

// ---------------------------------------------------------------- MCP server

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum McpTransport {
    StreamableHttp {
        url: String,
    },
    Stdio {
        command: String,
        #[serde(default)]
        args: Vec<String>,
        #[serde(default)]
        cwd: Option<String>,
    },
}

impl Default for McpTransport {
    fn default() -> Self {
        McpTransport::StreamableHttp { url: String::new() }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct McpServer {
    pub name: String,
    pub description: String,
    pub transport: McpTransport,
    pub headers: Vec<KeyValue>,
    /// Environment variables passed to stdio servers.
    pub env: Vec<KeyValue>,
    pub roots: Vec<McpRoot>,
    pub authentication: Auth,
    pub ssl_validation: Option<bool>,
    pub sampling: McpSampling,
}
model!(McpServer, "McpServer", "mcp");

// ---------------------------------------------------------------- Realtime

/// WebSocket, Server-Sent Events or Socket.IO connection.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct RealtimeRequest {
    pub name: String,
    pub description: String,
    /// `websocket` | `sse` | `socketio`
    pub kind: String,
    pub url: String,
    pub headers: Vec<KeyValue>,
    pub authentication: Auth,
    /// Message composer content (supports `{{ variables }}`).
    pub payload: String,
    /// `text` | `json`
    pub payload_format: String,
    pub subprotocols: Vec<String>,
    /// Socket.IO: event to emit, namespace and `auth` JSON.
    pub event: String,
    pub namespace: String,
    pub socketio_auth: String,
    /// SSE: method and body.
    pub method: String,
    pub body: String,
}
model!(RealtimeRequest, "RealtimeRequest", "rt");

impl Default for RealtimeRequest {
    fn default() -> Self {
        Self {
            name: "New WebSocket".into(),
            description: String::new(),
            kind: "websocket".into(),
            url: "wss://echo.websocket.org".into(),
            headers: vec![],
            authentication: Auth::Inherit,
            payload: String::new(),
            payload_format: "json".into(),
            subprotocols: vec![],
            event: "message".into(),
            namespace: "/".into(),
            socketio_auth: String::new(),
            method: "GET".into(),
            body: String::new(),
        }
    }
}

// ---------------------------------------------------------------- gRPC

/// A `.proto` source stored in a workspace (imports resolve among them).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct ProtoFile {
    /// File name used for imports, e.g. `shop/v1/orders.proto`.
    pub name: String,
    pub contents: String,
}
model!(ProtoFile, "ProtoFile", "pf");

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct GrpcRequest {
    pub name: String,
    pub description: String,
    /// `grpc://host:port` (plaintext) or `grpcs://host:port` (TLS).
    pub url: String,
    /// `reflection` | `protos`
    pub schema_source: String,
    /// `/package.Service/Method`
    pub method: String,
    /// JSON request message (supports `{{ variables }}`).
    pub message: String,
    pub metadata: Vec<KeyValue>,
    pub timeout_ms: u64,
}
model!(GrpcRequest, "GrpcRequest", "grpc");

impl Default for GrpcRequest {
    fn default() -> Self {
        Self {
            name: "New gRPC Request".into(),
            description: String::new(),
            url: "grpc://localhost:50051".into(),
            schema_source: "reflection".into(),
            method: String::new(),
            message: "{}".into(),
            metadata: vec![],
            timeout_ms: 30_000,
        }
    }
}

// ---------------------------------------------------------------- LLM

/// Where a provider's API key comes from. Keys are never stored in the database.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum KeySource {
    /// Not needed (e.g. local Ollama).
    None,
    /// OS keychain entry `insomnia-rs` / `<provider id>`.
    #[default]
    Keychain,
    /// Process environment variable, e.g. `ANTHROPIC_API_KEY`.
    Env { var: String },
    /// Template rendered against the workspace environment, e.g. `{{ _.openai_key }}`.
    Template { template: String },
}

/// A configured AI provider (global, not tied to a workspace).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct LlmProvider {
    pub name: String,
    /// `anthropic` | `openai` | `ollama` | `openai-compatible`
    pub kind: String,
    pub base_url: String,
    pub key_source: KeySource,
    pub default_model: String,
    pub headers: Vec<KeyValue>,
}
model!(LlmProvider, "LlmProvider", "llmp");

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct LlmPromptMessage {
    /// `user` | `assistant`
    pub role: String,
    /// Supports `{{ variables }}`.
    pub text: String,
}

/// An "AI request": a prompt, a model, and optional MCP servers as tools.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct LlmRequest {
    pub name: String,
    pub description: String,
    pub provider_id: Option<String>,
    pub model: String,
    pub system: String,
    pub messages: Vec<LlmPromptMessage>,
    pub max_tokens: u32,
    pub temperature: Option<f32>,
    /// MCP servers whose tools the model may call.
    pub mcp_server_ids: Vec<String>,
    pub max_turns: u32,
    /// `none` | `read-only` | `all`
    pub auto_approve: String,
}
model!(LlmRequest, "LlmRequest", "llm");

impl Default for LlmRequest {
    fn default() -> Self {
        Self {
            name: "New AI Request".into(),
            description: String::new(),
            provider_id: None,
            model: String::new(),
            system: String::new(),
            messages: vec![LlmPromptMessage {
                role: "user".into(),
                text: String::new(),
            }],
            max_tokens: 4096,
            temperature: None,
            mcp_server_ids: vec![],
            max_turns: 8,
            auto_approve: "read-only".into(),
        }
    }
}

/// One execution of an [`LlmRequest`] (stored as its child, like HTTP responses).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct LlmRun {
    pub provider_name: String,
    pub model: String,
    /// Full conversation (provider-neutral blocks as JSON).
    pub transcript: Vec<serde_json::Value>,
    /// Tool calls with their results, in order.
    pub tool_calls: Vec<serde_json::Value>,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub turns: u32,
    pub stop_reason: String,
    pub ttft_ms: Option<f64>,
    pub total_ms: f64,
    pub error: Option<String>,
    /// Exact JSON sent to the provider per turn (no API keys).
    pub request_bodies: Vec<serde_json::Value>,
}
model!(LlmRun, "LlmRun", "llmr");

/// Let the server ask our LLM for completions (MCP `sampling/createMessage`).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct McpSampling {
    pub enabled: bool,
    pub provider_id: Option<String>,
    pub model: Option<String>,
    pub max_tokens: u32,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct McpRoot {
    pub uri: String,
    pub name: Option<String>,
}

// ---------------------------------------------------------------- Git sync

/// A workspace file tracked in a Git repository.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct GitFile {
    pub workspace_id: String,
    /// Path relative to the repository root, e.g. `insomnia.orders-api.yaml`.
    pub path: String,
}

/// A local Git repository that workspaces are synced with (global, not in a workspace).
/// Credentials are not stored here: Git's own helpers/SSH agent are used, or a
/// token kept in the OS keychain.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct GitRepo {
    pub name: String,
    /// Absolute path of the working copy.
    pub path: String,
    pub remote_url: String,
    pub author_name: String,
    pub author_email: String,
    pub files: Vec<GitFile>,
}
model!(GitRepo, "GitRepo", "git");

// ---------------------------------------------------------------- Response

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Timings {
    pub dns_ms: Option<f64>,
    pub connect_ms: Option<f64>,
    pub tls_ms: Option<f64>,
    /// Time to first byte (request sent → headers received).
    pub ttfb_ms: f64,
    pub download_ms: f64,
    pub total_ms: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TimelineEntry {
    /// `info` | `header-out` | `data-out` | `header-in` | `data-in` | `error`
    pub kind: String,
    pub text: String,
    pub at_ms: f64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct TestResult {
    pub name: String,
    pub passed: bool,
    pub skipped: bool,
    pub error: Option<String>,
    pub duration_ms: f64,
    /// `pre-request` | `after-response`
    pub category: String,
}

/// One `console.*` line from a script.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct ConsoleEntry {
    /// `log` | `info` | `warn` | `error` | `debug`
    pub level: String,
    pub text: String,
    pub timestamp_ms: i64,
    /// Which script produced it, e.g. "pre-request: Folder Auth".
    pub source: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Response {
    pub environment_id: Option<String>,
    pub method: String,
    pub url: String,
    pub status_code: u16,
    pub status_message: String,
    pub http_version: String,
    pub headers: Vec<KeyValue>,
    pub content_type: String,
    /// Body as base64 when inline; otherwise `body_path` points to a file.
    pub body_b64: Option<String>,
    pub body_path: Option<String>,
    pub bytes: u64,
    pub timings: Timings,
    pub timeline: Vec<TimelineEntry>,
    pub error: Option<String>,
    pub test_results: Vec<TestResult>,
    pub console: Vec<ConsoleEntry>,
    /// Script failure (pre-request or after-response), with location.
    pub script_error: Option<String>,
}
model!(Response, "Response", "res");

// ---------------------------------------------------------------- Cookies

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Cookie {
    pub name: String,
    pub value: String,
    pub domain: String,
    pub path: String,
    pub expires: Option<i64>,
    pub secure: bool,
    pub http_only: bool,
    pub host_only: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct CookieJar {
    pub name: String,
    pub cookies: Vec<Cookie>,
}
model!(CookieJar, "CookieJar", "jar");

// ---------------------------------------------------------------- Settings

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    pub timeout_ms: u64,
    pub follow_redirects: bool,
    pub max_redirects: usize,
    pub validate_certificates: bool,
    pub max_history_per_request: usize,
    /// `system` | `light` | `dark`
    pub theme: String,
    pub max_concurrent_requests: usize,
    pub script_timeout_ms: u64,
    /// e.g. `http://proxy.corp:3128` (applies to HTTP and HTTPS); empty = no proxy.
    pub proxy_url: String,
    /// Comma-separated hosts/domains/CIDRs that bypass the proxy.
    pub no_proxy: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            timeout_ms: 30_000,
            follow_redirects: true,
            max_redirects: 10,
            validate_certificates: true,
            max_history_per_request: 20,
            theme: "system".into(),
            max_concurrent_requests: 8,
            script_timeout_ms: 5_000,
            proxy_url: String::new(),
            no_proxy: "localhost,127.0.0.1".into(),
        }
    }
}
model!(Settings, "Settings", "set");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_params_extracted_once_in_order() {
        assert_eq!(
            path_params_in_url("https://x.io/users/:id/posts/:postId?q=:no#:frag/:id"),
            vec!["id", "postId"]
        );
        assert!(path_params_in_url("https://x.io:8080/a").is_empty());
    }

    #[test]
    fn auth_round_trips_with_insomnia_names() {
        let a: Auth =
            serde_json::from_str(r#"{"type":"apikey","key":"X-Key","value":"v","addTo":"header"}"#)
                .unwrap();
        assert!(matches!(a, Auth::ApiKey { ref key, .. } if key == "X-Key"));
        let b: Auth = serde_json::from_str(r#"{"type":"bearer","token":"t"}"#).unwrap();
        assert_eq!(serde_json::to_value(&b).unwrap()["type"], "bearer");
    }

    #[test]
    fn request_defaults_fill_missing_fields() {
        let r: Request = serde_json::from_str(r#"{"url":"http://a"}"#).unwrap();
        assert_eq!(r.method, "GET");
        assert!(r.settings.send_cookies);
    }
}
