//! OAuth sign-in for MCP servers, following the MCP authorization spec: find the
//! authorization server from the server's 401 (`WWW-Authenticate: Bearer
//! resource_metadata=…`) or its well-known protected resource metadata (RFC 9728),
//! read the authorization server metadata (RFC 8414 / OpenID discovery), register a
//! client when none is configured (RFC 7591), then run the authorization code flow
//! with PKCE and the `resource` parameter (RFC 8707) through [`Engine::oauth2_authorize`].

use lsock_core::{Auth, Doc, McpServer, McpTransport, OAuth2Config, OAuth2Token};
use lsock_templating::Mode;
use serde_json::{Value, json};

use crate::{Engine, EngineError, Result};

fn err(m: impl Into<String>) -> EngineError {
    EngineError::Message(m.into())
}

/// Endpoints and parameters discovered for one MCP server.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Discovery {
    pub authorization_url: String,
    pub token_url: String,
    pub registration_url: Option<String>,
    pub resource: String,
    pub scope: String,
}

/// `key="value"` parameters of a `WWW-Authenticate: Bearer …` challenge.
fn challenge_param(header: &str, key: &str) -> Option<String> {
    let lower = header.to_ascii_lowercase();
    let mut from = 0;
    while let Some(i) = lower[from..].find(key) {
        let at = from + i;
        from = at + key.len();
        // a whole parameter name, followed by `=`
        let before_ok = at == 0 || matches!(lower.as_bytes()[at - 1], b' ' | b',');
        let rest = header[from..].trim_start();
        let Some(rest) = rest.strip_prefix('=').filter(|_| before_ok) else {
            continue;
        };
        let rest = rest.trim_start();
        return Some(match rest.strip_prefix('"') {
            Some(q) => q.split('"').next().unwrap_or("").to_string(),
            None => rest.split([',', ' ']).next().unwrap_or("").to_string(),
        });
    }
    None
}

/// The MCP server URL without query or fragment: the RFC 8707 resource when the
/// server's metadata does not name one.
fn canonical_resource(url: &url::Url) -> String {
    let mut u = url.clone();
    u.set_query(None);
    u.set_fragment(None);
    let s = u.to_string();
    match s.strip_suffix('/') {
        Some(t) if u.path() != "/" => t.to_string(),
        _ => s,
    }
}

/// `scheme://host[:port]`.
fn origin(url: &url::Url) -> String {
    url.origin().ascii_serialization()
}

/// Well-known URLs for an issuer, in the order the MCP spec lists them.
fn server_metadata_urls(issuer: &url::Url) -> Vec<String> {
    let o = origin(issuer);
    let path = issuer.path().trim_end_matches('/');
    if path.is_empty() {
        vec![
            format!("{o}/.well-known/oauth-authorization-server"),
            format!("{o}/.well-known/openid-configuration"),
        ]
    } else {
        vec![
            format!("{o}/.well-known/oauth-authorization-server{path}"),
            format!("{o}/.well-known/openid-configuration{path}"),
            format!("{o}{path}/.well-known/openid-configuration"),
        ]
    }
}

async fn get_json(http: &reqwest::Client, url: &str) -> Option<Value> {
    let resp = http
        .get(url)
        .header("Accept", "application/json")
        .send()
        .await
        .ok()?;
    if !resp.status().is_success() {
        return None;
    }
    resp.json::<Value>().await.ok().filter(Value::is_object)
}

/// Find where and how to sign in to the MCP server at `mcp_url`.
pub async fn discover(http: &reqwest::Client, mcp_url: &str) -> Result<Discovery> {
    let url = url::Url::parse(mcp_url).map_err(|e| err(format!("invalid MCP server URL: {e}")))?;

    // 1. Ask the server. A protected server answers 401 and points at its metadata.
    let probe = json!({"jsonrpc": "2.0", "id": 0, "method": "initialize", "params": {
        "protocolVersion": "2025-11-25", "capabilities": {},
        "clientInfo": {"name": "logic-socket", "version": env!("CARGO_PKG_VERSION")}
    }});
    let challenge = http
        .post(url.as_str())
        .header("Accept", "application/json, text/event-stream")
        .header("Content-Type", "application/json")
        .body(probe.to_string())
        .send()
        .await
        .map_err(|e| err(format!("could not reach the MCP server: {e}")))?
        .headers()
        .get_all(reqwest::header::WWW_AUTHENTICATE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .find(|v| v.to_ascii_lowercase().starts_with("bearer"))
        .map(str::to_string)
        .unwrap_or_default();

    // 2. Protected resource metadata: from the challenge, else the well-known locations.
    let path = url.path().trim_end_matches('/');
    let mut candidates: Vec<String> = challenge_param(&challenge, "resource_metadata")
        .into_iter()
        .collect();
    if !path.is_empty() {
        candidates.push(format!(
            "{}/.well-known/oauth-protected-resource{path}",
            origin(&url)
        ));
    }
    candidates.push(format!(
        "{}/.well-known/oauth-protected-resource",
        origin(&url)
    ));
    let mut resource_md = None;
    for c in &candidates {
        if let Some(v) = get_json(http, c)
            .await
            .filter(|v| v["authorization_servers"].is_array())
        {
            resource_md = Some(v);
            break;
        }
    }

    // 3. Authorization server metadata. Servers on the 2025-03-26 spec have no resource
    //    metadata; their own origin is the authorization server.
    let issuer = resource_md
        .as_ref()
        .and_then(|m| m["authorization_servers"][0].as_str().map(str::to_string))
        .unwrap_or_else(|| origin(&url));
    let issuer_url = url::Url::parse(&issuer)
        .map_err(|e| err(format!("invalid authorization server '{issuer}': {e}")))?;
    let mut server_md = None;
    for c in server_metadata_urls(&issuer_url) {
        if let Some(v) = get_json(http, &c)
            .await
            .filter(|v| v["authorization_endpoint"].is_string())
        {
            server_md = Some(v);
            break;
        }
    }
    let (authorization_url, token_url, registration_url) = match &server_md {
        Some(m) => (
            m["authorization_endpoint"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
            m["token_endpoint"].as_str().unwrap_or_default().to_string(),
            m["registration_endpoint"].as_str().map(str::to_string),
        ),
        // 2025-03-26 fallback: default endpoint paths on the server's origin.
        None if resource_md.is_none() => {
            let o = origin(&url);
            (
                format!("{o}/authorize"),
                format!("{o}/token"),
                Some(format!("{o}/register")),
            )
        }
        None => {
            return Err(err(format!(
                "the MCP server names '{issuer}' as its authorization server, but no OAuth metadata was found there"
            )));
        }
    };
    if token_url.is_empty() {
        return Err(err(format!(
            "the authorization server '{issuer}' does not publish a token endpoint"
        )));
    }

    let resource = resource_md
        .as_ref()
        .and_then(|m| m["resource"].as_str().map(str::to_string))
        .unwrap_or_else(|| canonical_resource(&url));
    let scope = challenge_param(&challenge, "scope")
        .or_else(|| {
            resource_md
                .as_ref()
                .and_then(|m| m["scopes_supported"].as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(Value::as_str)
                        .collect::<Vec<_>>()
                        .join(" ")
                })
        })
        .unwrap_or_default();
    Ok(Discovery {
        authorization_url,
        token_url,
        registration_url,
        resource,
        scope,
    })
}

/// Dynamic client registration (RFC 7591) as a public client with PKCE.
pub async fn register_client(
    http: &reqwest::Client,
    registration_url: &str,
    redirect_url: &str,
    scope: &str,
) -> Result<(String, String)> {
    let mut body = json!({
        "client_name": "Logic Socket",
        "redirect_uris": [redirect_url],
        "grant_types": ["authorization_code", "refresh_token"],
        "response_types": ["code"],
        "token_endpoint_auth_method": "none",
    });
    if !scope.is_empty() {
        body["scope"] = json!(scope);
    }
    let resp = http
        .post(registration_url)
        .header("Accept", "application/json")
        .json(&body)
        .send()
        .await
        .map_err(|e| err(format!("client registration failed: {e}")))?;
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    let v: Value = serde_json::from_str(&text).unwrap_or_default();
    match v["client_id"].as_str() {
        Some(id) if status.is_success() => Ok((
            id.to_string(),
            v["client_secret"].as_str().unwrap_or_default().to_string(),
        )),
        _ => Err(err(format!(
            "client registration at {registration_url} answered HTTP {status}: {}",
            text.chars().take(300).collect::<String>()
        ))),
    }
}

impl Engine {
    /// Sign in to an MCP server: discover its authorization server, register a client
    /// when the config has no client ID, save what was found on the server's auth, and
    /// run the browser sign-in. Fields already filled in on the config are kept.
    pub async fn mcp_oauth_sign_in(
        &self,
        server_id: &str,
        open_url: &(dyn Fn(&str) + Send + Sync),
    ) -> Result<Doc<OAuth2Token>> {
        let mut server = self.store.get::<McpServer>(server_id)?;
        let McpTransport::StreamableHttp { url } = &server.transport else {
            return Err(err(
                "OAuth sign-in is for Streamable HTTP servers; stdio servers authenticate through their environment",
            ));
        };
        let ctx = self.context(server_id)?;
        let url = self
            .renderer()
            .render_str(url, &ctx, Mode::Throw)
            .map_err(|source| EngineError::Render {
                field: "URL".into(),
                source,
            })?;
        let mut cfg = match &server.authentication {
            Auth::OAuth2(c) => c.clone(),
            _ => OAuth2Config::default(),
        };
        if cfg.redirect_url.trim().is_empty() {
            cfg.redirect_url = OAuth2Config::default().redirect_url;
        }

        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(20))
            .redirect(reqwest::redirect::Policy::limited(5))
            .build()
            .map_err(|e| err(e.to_string()))?;
        let found = discover(&http, &url).await?;
        let fill = |field: &mut String, v: &str| {
            if field.trim().is_empty() {
                *field = v.to_string();
            }
        };
        fill(&mut cfg.authorization_url, &found.authorization_url);
        fill(&mut cfg.access_token_url, &found.token_url);
        fill(&mut cfg.resource, &found.resource);
        fill(&mut cfg.scope, &found.scope);
        if cfg.client_id.trim().is_empty() {
            let Some(reg) = &found.registration_url else {
                return Err(err(
                    "this server's sign-in does not support automatic registration — enter the client ID it gave you on the Auth tab",
                ));
            };
            let (id, secret) =
                register_client(&http, reg, cfg.redirect_url.trim(), &cfg.scope).await?;
            cfg.client_id = id;
            cfg.client_secret = secret;
        }
        cfg.grant_type = "authorization_code".into();
        cfg.use_pkce = true;
        server.body.authentication = Auth::OAuth2(cfg);
        self.store.update(&server)?;

        let rendered = self.oauth2_config(server_id)?;
        self.oauth2_authorize(server_id, &rendered, open_url).await
    }

    /// `Authorization` header value for a server using OAuth: the cached token,
    /// refreshed when it is about to expire.
    pub(crate) async fn mcp_oauth_header(&self, server_id: &str) -> Result<Option<String>> {
        let server = self.store.get::<McpServer>(server_id)?;
        let Auth::OAuth2(c) = &server.authentication else {
            return Ok(None);
        };
        if c.disabled {
            return Ok(None);
        }
        let not_signed_in =
            || err("not signed in to this MCP server — open Settings › Auth and click Sign in");
        if c.access_token_url.trim().is_empty() {
            return Err(not_signed_in());
        }
        let cfg = self.oauth2_config(server_id)?;
        let token = self.oauth2_token(server_id, &cfg).await.map_err(|e| {
            match self.oauth2_cached(server_id) {
                Ok(Some(_)) => e,
                _ => not_signed_in(),
            }
        })?;
        let prefix = Some(cfg.token_prefix.trim())
            .filter(|p| !p.is_empty())
            .unwrap_or("Bearer");
        Ok(Some(format!("{prefix} {token}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_challenge_parameters() {
        let h = r#"Bearer error="invalid_token", resource_metadata="https://x.io/.well-known/oauth-protected-resource/mcp", scope="read write""#;
        assert_eq!(
            challenge_param(h, "resource_metadata").as_deref(),
            Some("https://x.io/.well-known/oauth-protected-resource/mcp")
        );
        assert_eq!(challenge_param(h, "scope").as_deref(), Some("read write"));
        assert_eq!(
            challenge_param("Bearer realm=x, scope=a", "scope").as_deref(),
            Some("a")
        );
        assert_eq!(
            challenge_param("Bearer realm=\"oauthscope=1\"", "scope"),
            None
        );
        assert_eq!(challenge_param("Bearer", "scope"), None);
    }

    #[test]
    fn metadata_urls_follow_the_spec_order() {
        let u = |s: &str| url::Url::parse(s).unwrap();
        assert_eq!(
            server_metadata_urls(&u("https://auth.x.io")),
            [
                "https://auth.x.io/.well-known/oauth-authorization-server",
                "https://auth.x.io/.well-known/openid-configuration"
            ]
        );
        assert_eq!(
            server_metadata_urls(&u("https://x.io/tenant1/")),
            [
                "https://x.io/.well-known/oauth-authorization-server/tenant1",
                "https://x.io/.well-known/openid-configuration/tenant1",
                "https://x.io/tenant1/.well-known/openid-configuration",
            ]
        );
        assert_eq!(
            canonical_resource(&u("https://X.io/mcp/?a=1#f")),
            "https://x.io/mcp"
        );
        assert_eq!(canonical_resource(&u("https://x.io")), "https://x.io/");
    }

    /// The whole flow against the mock server's MCP-spec OAuth: discovery from the 401,
    /// dynamic registration, PKCE sign-in with `resource`, connect, and refresh.
    #[tokio::test]
    async fn signs_in_connects_and_refreshes_against_a_spec_server() {
        use lsock_core::Workspace;
        use lsock_mcp::mock::{HttpState, spawn_http};

        let state = HttpState::with_oauth();
        let mock = state.oauth.clone().unwrap();
        let mcp_url = spawn_http(state, 0).await.unwrap();
        let e = Engine::in_memory();
        let ws = e.store.insert(None, Workspace::default()).unwrap();
        let server = e
            .store
            .insert(
                Some(ws.id()),
                McpServer {
                    transport: McpTransport::StreamableHttp {
                        url: mcp_url.clone(),
                    },
                    authentication: Auth::OAuth2(OAuth2Config {
                        redirect_url: "http://127.0.0.1:8973/callback".into(),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
            )
            .unwrap();

        // Before sign-in: a clear message, not a raw 401.
        let before = e
            .mcp_connect_options(server.id())
            .await
            .unwrap_err()
            .to_string();
        assert!(before.contains("not signed in"), "{before}");

        // The "browser" follows the authorize URL; the mock approves and redirects to the callback.
        let opened = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
        let seen = opened.clone();
        let browser = move |url: &str| {
            *seen.lock().unwrap() = url.to_string();
            let url = url.to_string();
            tokio::spawn(async move {
                let _ = reqwest::get(url).await;
            });
        };
        let token = e.mcp_oauth_sign_in(server.id(), &browser).await.unwrap();
        assert!(token.access_token.starts_with("at-"), "{:?}", token.body);
        assert!(token.refresh_token.is_some());
        let link = opened.lock().unwrap().clone();
        for part in [
            "code_challenge_method=S256",
            "resource=",
            "client_id=client-",
            "scope=mcp%3Atools",
        ] {
            assert!(link.contains(part), "{part} missing from {link}");
        }

        // What discovery found is saved on the server.
        let saved = e.store.get::<McpServer>(server.id()).unwrap();
        let Auth::OAuth2(cfg) = &saved.authentication else {
            panic!("{:?}", saved.authentication)
        };
        assert_eq!(cfg.resource, mcp_url);
        assert!(
            cfg.authorization_url.ends_with("/auth/authorize"),
            "{}",
            cfg.authorization_url
        );
        assert!(cfg.access_token_url.ends_with("/auth/token"));
        assert!(cfg.client_id.starts_with("client-"));
        assert_eq!(cfg.grant_type, "authorization_code");

        // Connect with the token.
        let client = lsock_mcp::Client::connect(e.mcp_connect_options(server.id()).await.unwrap())
            .await
            .unwrap();
        assert!(!client.list_tools().await.unwrap().items.is_empty());

        // An expired token is refreshed on the next connect.
        let mut stored = e
            .store
            .children::<OAuth2Token>(server.id())
            .unwrap()
            .remove(0);
        stored.body.expires_at = Some(lsock_core::now_ms() - 1);
        e.store.update(&stored).unwrap();
        let issued = *mock.issued.lock().unwrap();
        let client = lsock_mcp::Client::connect(e.mcp_connect_options(server.id()).await.unwrap())
            .await
            .unwrap();
        assert!(!client.list_tools().await.unwrap().items.is_empty());
        assert_eq!(
            *mock.issued.lock().unwrap(),
            issued + 1,
            "refresh should issue a new token"
        );
    }
}
