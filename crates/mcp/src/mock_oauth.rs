//! OAuth for the mock MCP server, shaped like the MCP authorization spec: a 401
//! with `WWW-Authenticate: Bearer resource_metadata=…`, protected resource metadata
//! (RFC 9728), authorization server metadata at a path-inserted well-known URL
//! (RFC 8414), dynamic client registration (RFC 7591), and the authorization code
//! grant with PKCE S256 and the `resource` parameter (RFC 8707). `/authorize`
//! approves straight away, so tests can follow the redirect without a browser.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use axum::{
    Json, Router,
    extract::{Query, State},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Redirect, Response},
    routing::{get, post},
};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use serde_json::{Value, json};
use sha2::Digest as _;

pub const SCOPE: &str = "mcp:tools";

struct Grant {
    client_id: String,
    redirect_uri: String,
    challenge: String,
    resource: String,
}

#[derive(Default)]
pub struct OAuthMock {
    /// `http://127.0.0.1:<port>`, set once the listener is bound.
    pub base: Mutex<String>,
    clients: Mutex<HashMap<String, Vec<String>>>,
    codes: Mutex<HashMap<String, Grant>>,
    tokens: Mutex<HashSet<String>>,
    refresh: Mutex<HashSet<String>>,
    /// Access tokens issued so far (tests check refresh issues a new one).
    pub issued: Mutex<u32>,
}

impl OAuthMock {
    fn base(&self) -> String {
        self.base.lock().unwrap().clone()
    }
    fn resource(&self) -> String {
        format!("{}/mcp", self.base())
    }
    fn metadata_url(&self) -> String {
        format!("{}/.well-known/oauth-protected-resource/mcp", self.base())
    }
    fn issuer(&self) -> String {
        format!("{}/auth", self.base())
    }

    /// `None` when the request carries a token this server issued; otherwise the 401.
    pub fn check(&self, headers: &HeaderMap) -> Option<Response> {
        let token = headers
            .get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "));
        if token.is_some_and(|t| self.tokens.lock().unwrap().contains(t)) {
            return None;
        }
        let challenge = format!(
            "Bearer resource_metadata=\"{}\", scope=\"{SCOPE}\"",
            self.metadata_url()
        );
        Some(
            (
                StatusCode::UNAUTHORIZED,
                [(header::WWW_AUTHENTICATE, challenge)],
                "sign-in required",
            )
                .into_response(),
        )
    }

    fn issue(&self) -> Value {
        let at = format!("at-{}", uuid::Uuid::new_v4().simple());
        let rt = format!("rt-{}", uuid::Uuid::new_v4().simple());
        self.tokens.lock().unwrap().insert(at.clone());
        self.refresh.lock().unwrap().insert(rt.clone());
        *self.issued.lock().unwrap() += 1;
        json!({"access_token": at, "token_type": "Bearer", "expires_in": 3600, "refresh_token": rt, "scope": SCOPE})
    }
}

type St = Arc<OAuthMock>;

pub fn router(st: St) -> Router {
    Router::new()
        .route(
            "/.well-known/oauth-protected-resource/mcp",
            get(resource_metadata),
        )
        .route(
            "/.well-known/oauth-authorization-server/auth",
            get(server_metadata),
        )
        .route("/auth/register", post(register))
        .route("/auth/authorize", get(authorize))
        .route("/auth/token", post(token))
        .with_state(st)
}

async fn resource_metadata(State(st): State<St>) -> Json<Value> {
    Json(json!({
        "resource": st.resource(),
        "authorization_servers": [st.issuer()],
        "scopes_supported": [SCOPE],
    }))
}

async fn server_metadata(State(st): State<St>) -> Json<Value> {
    let i = st.issuer();
    Json(json!({
        "issuer": i,
        "authorization_endpoint": format!("{i}/authorize"),
        "token_endpoint": format!("{i}/token"),
        "registration_endpoint": format!("{i}/register"),
        "response_types_supported": ["code"],
        "grant_types_supported": ["authorization_code", "refresh_token"],
        "code_challenge_methods_supported": ["S256"],
        "token_endpoint_auth_methods_supported": ["none"],
    }))
}

async fn register(State(st): State<St>, Json(body): Json<Value>) -> Response {
    let redirects: Vec<String> = body["redirect_uris"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    if redirects.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "invalid_redirect_uri"})),
        )
            .into_response();
    }
    let id = format!("client-{}", uuid::Uuid::new_v4().simple());
    st.clients
        .lock()
        .unwrap()
        .insert(id.clone(), redirects.clone());
    (
        StatusCode::CREATED,
        Json(json!({"client_id": id, "redirect_uris": redirects, "token_endpoint_auth_method": "none"})),
    )
        .into_response()
}

async fn authorize(State(st): State<St>, Query(q): Query<HashMap<String, String>>) -> Response {
    let get = |k: &str| q.get(k).cloned().unwrap_or_default();
    let (client_id, redirect_uri) = (get("client_id"), get("redirect_uri"));
    let registered = st
        .clients
        .lock()
        .unwrap()
        .get(&client_id)
        .is_some_and(|r| r.contains(&redirect_uri));
    let problem = if !registered {
        Some("unknown client or redirect_uri")
    } else if get("response_type") != "code" {
        Some("response_type must be code")
    } else if get("code_challenge_method") != "S256" || get("code_challenge").is_empty() {
        Some("PKCE S256 is required")
    } else if get("resource") != st.resource() {
        Some("resource must be the MCP server URL")
    } else {
        None
    };
    if let Some(p) = problem {
        return (StatusCode::BAD_REQUEST, p).into_response();
    }
    let code = format!("code-{}", uuid::Uuid::new_v4().simple());
    st.codes.lock().unwrap().insert(
        code.clone(),
        Grant {
            client_id,
            redirect_uri: redirect_uri.clone(),
            challenge: get("code_challenge"),
            resource: get("resource"),
        },
    );
    let mut to = url::Url::parse(&redirect_uri).expect("registered redirect is a URL");
    to.query_pairs_mut()
        .append_pair("code", &code)
        .append_pair("state", &get("state"));
    Redirect::to(to.as_str()).into_response()
}

async fn token(State(st): State<St>, body: String) -> Response {
    let form: HashMap<String, String> = url::form_urlencoded::parse(body.as_bytes())
        .into_owned()
        .collect();
    let get = |k: &str| form.get(k).cloned().unwrap_or_default();
    let bad = |e: &str| (StatusCode::BAD_REQUEST, Json(json!({"error": e}))).into_response();
    match get("grant_type").as_str() {
        "authorization_code" => {
            let Some(g) = st.codes.lock().unwrap().remove(&get("code")) else {
                return bad("invalid_grant");
            };
            let verifier_ok = URL_SAFE_NO_PAD
                .encode(sha2::Sha256::digest(get("code_verifier").as_bytes()))
                == g.challenge;
            if g.client_id != get("client_id")
                || g.redirect_uri != get("redirect_uri")
                || !verifier_ok
            {
                return bad("invalid_grant");
            }
            if get("resource") != g.resource {
                return bad("invalid_target");
            }
            Json(st.issue()).into_response()
        }
        "refresh_token" => {
            if !st.refresh.lock().unwrap().remove(&get("refresh_token")) {
                return bad("invalid_grant");
            }
            if get("resource") != st.resource() {
                return bad("invalid_target");
            }
            Json(st.issue()).into_response()
        }
        _ => bad("unsupported_grant_type"),
    }
}
