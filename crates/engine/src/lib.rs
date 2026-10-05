//! Request pipeline shared by the CLI and the desktop app:
//! load request + ancestors + environments → render → send → persist.

pub mod curl;
pub mod llm;
pub mod mcp;
pub mod oauth2;
pub mod pipeline;
pub mod realtime;

pub use pipeline::{Outcome, RunState};

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use base64::Engine as _;
use irs_core::{
    Auth, CookieJar, Doc, Environment, Folder, KeyValue, RawDoc, Request, Response, Settings,
    Store, StoreError, Toggle, Workspace,
};
use irs_templating::{Context, Layer, Mode, RenderError, Renderer, VarRef};

#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error("could not render {field}: {source}")]
    Render { field: String, source: RenderError },
    #[error("{0} is not inside a workspace")]
    NoWorkspace(String),
    #[error("{0}")]
    Llm(String),
    #[error("{0}")]
    Message(String),
}

pub type Result<T> = std::result::Result<T, EngineError>;

/// Shared by the CLI and desktop app: `$IRS_DATA_DIR`, else the platform
/// app-data directory (`~/Library/Application Support/insomnia-rs` on macOS).
pub fn default_data_dir() -> PathBuf {
    if let Some(d) = std::env::var_os("IRS_DATA_DIR").filter(|d| !d.is_empty()) {
        return PathBuf::from(d);
    }
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_default();
    if cfg!(target_os = "macos") {
        home.join("Library/Application Support/insomnia-rs")
    } else if cfg!(windows) {
        std::env::var_os("APPDATA")
            .map(PathBuf::from)
            .unwrap_or(home)
            .join("insomnia-rs")
    } else {
        std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".local/share"))
            .join("insomnia-rs")
    }
}

/// Bodies larger than this are stored as files next to the database.
const INLINE_BODY_LIMIT: usize = 1024 * 1024;

#[derive(Clone)]
pub struct Engine {
    pub store: Store,
    pub(crate) renderer: Arc<Renderer>,
    data_dir: Option<PathBuf>,
    pub(crate) secrets: Arc<dyn llm::SecretStore>,
}

/// A request after rendering and inheritance, ready for `irs_http::send`.
#[derive(Debug, Clone)]
pub struct Prepared {
    pub workspace_id: String,
    /// Request or folder whose auth applies (OAuth 2 tokens are cached under it).
    pub auth_owner: Option<String>,
    pub request: Request,
    pub auth: Auth,
    pub options: irs_http::Options,
    pub environment_id: Option<String>,
}

impl Engine {
    /// Engines backed by a data directory keep API keys in the OS keychain
    /// (set `IRS_SECRET_STORE=memory` to disable, e.g. in CI); in-memory engines keep them in memory.
    pub fn new(store: Store, data_dir: Option<PathBuf>) -> Self {
        let use_keychain =
            data_dir.is_some() && std::env::var("IRS_SECRET_STORE").as_deref() != Ok("memory");
        let secrets: Arc<dyn llm::SecretStore> = if use_keychain {
            Arc::new(llm::KeychainStore)
        } else {
            Arc::new(llm::MemoryStore::default())
        };
        Self {
            store,
            renderer: Arc::new(Renderer::new()),
            data_dir,
            secrets,
        }
    }

    pub fn with_secrets(mut self, secrets: Arc<dyn llm::SecretStore>) -> Self {
        self.secrets = secrets;
        self
    }

    /// Open (or create) the database in `data_dir`.
    pub fn open(data_dir: PathBuf) -> Result<Self> {
        let store = Store::open(data_dir.join("insomnia.db"))?;
        Ok(Self::new(store, Some(data_dir)))
    }

    pub fn in_memory() -> Self {
        Self::new(Store::open_in_memory().expect("in-memory sqlite"), None)
    }

    pub fn renderer(&self) -> &Renderer {
        &self.renderer
    }

    pub fn workspace_of(&self, id: &str) -> Result<Doc<Workspace>> {
        if let Some(d) = self.store.raw(id)?
            && d.meta.kind == "Workspace"
        {
            return Ok(d.typed()?);
        }
        self.store
            .ancestors(id)?
            .into_iter()
            .find(|d| d.meta.kind == "Workspace")
            .ok_or_else(|| EngineError::NoWorkspace(id.to_string()))?
            .typed()
            .map_err(Into::into)
    }

    /// The workspace's base environment, created on first use.
    pub fn base_environment(&self, workspace_id: &str) -> Result<Doc<Environment>> {
        if let Some(e) = self
            .store
            .children::<Environment>(workspace_id)?
            .into_iter()
            .next()
        {
            return Ok(e);
        }
        Ok(self.store.insert(
            Some(workspace_id),
            Environment {
                name: "Base Environment".into(),
                ..Default::default()
            },
        )?)
    }

    pub fn sub_environments(&self, workspace_id: &str) -> Result<Vec<Doc<Environment>>> {
        let base = self.base_environment(workspace_id)?;
        Ok(self.store.children::<Environment>(base.id())?)
    }

    pub fn cookie_jar(&self, workspace_id: &str) -> Result<Doc<CookieJar>> {
        if let Some(j) = self
            .store
            .children::<CookieJar>(workspace_id)?
            .into_iter()
            .next()
        {
            return Ok(j);
        }
        Ok(self.store.insert(
            Some(workspace_id),
            CookieJar {
                name: "Default Jar".into(),
                ..Default::default()
            },
        )?)
    }

    /// Environment layers for `id` (a request, folder or workspace), lowest
    /// precedence first, matching Insomnia's `buildRenderContext`.
    pub fn layers(&self, id: &str) -> Result<Vec<Layer>> {
        let ws = self.workspace_of(id)?;
        let mut layers = vec![];

        // Global environments (an `environment`-scope workspace's base + sub).
        if let Some(gid) = &ws.active_global_base_id
            && let Ok(g) = self.store.get::<Environment>(gid)
        {
            layers.push(Layer::new(format!("Global: {}", g.name), g.data.clone()));
            if let Some(sid) = &ws.active_global_sub_id
                && let Ok(s) = self.store.get::<Environment>(sid)
                && s.meta.parent_id.as_deref() == Some(g.id())
            {
                layers.push(Layer::new(format!("Global: {}", s.name), s.data.clone()));
            }
        }

        let base = self.base_environment(ws.id())?;
        layers.push(Layer::new("Base Environment", base.data.clone()));
        if let Some(sid) = &ws.active_environment_id
            && let Ok(sub) = self.store.get::<Environment>(sid)
            && sub.meta.parent_id.as_deref() == Some(base.id())
        {
            layers.push(Layer::new(sub.name.clone(), sub.data.clone()));
        }

        let mut chain: Vec<RawDoc> = self.store.ancestors(id)?;
        if let Some(me) = self.store.raw(id)? {
            chain.insert(0, me);
        }
        for d in chain.iter().rev().filter(|d| d.meta.kind == "Folder") {
            let f: Doc<Folder> = d.typed()?;
            if !f.environment.is_empty() {
                layers.push(Layer::new(
                    format!("Folder: {}", f.name),
                    f.environment.clone(),
                ));
            }
        }
        Ok(layers)
    }

    pub fn context(&self, id: &str) -> Result<Context> {
        Ok(Context::build(&self.renderer, &self.layers(id)?))
    }

    /// Render arbitrary text in the context of `id` (for live previews).
    pub fn preview(
        &self,
        id: &str,
        text: &str,
    ) -> Result<(std::result::Result<String, RenderError>, Vec<VarRef>)> {
        let ctx = self.context(id)?;
        Ok((
            self.renderer.render_str(text, &ctx, Mode::Throw),
            self.renderer.references(text, &ctx),
        ))
    }

    /// Render + apply folder inheritance. `extra` layers (iteration data,
    /// script variables) are applied on top of the environment.
    pub fn prepare(&self, request_id: &str, extra: &[Layer]) -> Result<Prepared> {
        let req: Request = self.store.get::<Request>(request_id)?.body;
        self.prepare_request(request_id, &req, extra)
    }

    /// Like [`Engine::prepare`] but for an in-memory (e.g. script-mutated) request.
    pub fn prepare_request(
        &self,
        request_id: &str,
        req: &Request,
        extra: &[Layer],
    ) -> Result<Prepared> {
        let ws = self.workspace_of(request_id)?;
        let settings: Settings = self.store.settings()?.body;
        let mut layers = self.layers(request_id)?;
        layers.extend_from_slice(extra);
        let ctx = Context::build(&self.renderer, &layers);

        // Folders nearest-first.
        let folders: Vec<Doc<Folder>> = self
            .store
            .ancestors(request_id)?
            .iter()
            .filter(|d| d.meta.kind == "Folder")
            .map(RawDoc::typed)
            .collect::<std::result::Result<_, _>>()?;

        // Headers: outermost folder first, request last; later same-name headers replace earlier ones.
        let mut headers: Vec<KeyValue> = vec![];
        for f in folders.iter().rev() {
            merge_headers(&mut headers, &f.headers);
        }
        merge_headers(&mut headers, &req.headers);

        let (auth, auth_owner) = if req.authentication.is_inherit() {
            folders
                .iter()
                .find(|f| !f.authentication.is_inherit())
                .map(|f| (f.authentication.clone(), Some(f.meta.id.clone())))
                .unwrap_or((Auth::None, None))
        } else {
            (req.authentication.clone(), Some(request_id.to_string()))
        };

        let r = |field: &str, s: &str| -> Result<String> {
            self.renderer
                .render_str(s, &ctx, Mode::Throw)
                .map_err(|source| EngineError::Render {
                    field: field.to_string(),
                    source,
                })
        };
        let kv = |section: &str, list: &[KeyValue]| -> Result<Vec<KeyValue>> {
            list.iter()
                .map(|h| {
                    if h.disabled {
                        return Ok(h.clone());
                    }
                    Ok(KeyValue {
                        name: r(&format!("{section} name '{}'", h.name), &h.name)?,
                        value: r(&format!("{section} '{}'", h.name), &h.value)?,
                        ..h.clone()
                    })
                })
                .collect()
        };

        let mut out = req.clone();
        out.url = r("URL", &req.url)?;
        out.method = req.method.trim().to_uppercase();
        out.headers = kv("header", &headers)?;
        out.parameters = kv("query parameter", &req.parameters)?;
        out.path_parameters = kv("path parameter", &req.path_parameters)?;
        if !req.settings.disable_render_body {
            if let Some(t) = &req.body.text {
                out.body.text = Some(r("body", t)?);
            }
            for p in out.body.params.iter_mut().filter(|p| !p.disabled) {
                p.name = r(&format!("form field name '{}'", p.name), &p.name)?;
                p.value = r(&format!("form field '{}'", p.name), &p.value)?;
                if let Some(f) = &p.file_name {
                    p.file_name = Some(r(&format!("form file '{}'", p.name), f)?);
                }
            }
            if let Some(f) = &req.body.file_name {
                out.body.file_name = Some(r("body file", f)?);
            }
        }
        let auth = render_auth(&auth, &|field, s| r(field, s))?;

        let follow = match req.settings.follow_redirects {
            Toggle::Global => settings.follow_redirects,
            Toggle::On => true,
            Toggle::Off => false,
        };
        let options = irs_http::Options {
            timeout: Duration::from_millis(settings.timeout_ms.max(1)),
            follow_redirects: follow,
            max_redirects: settings.max_redirects,
            validate_certificates: settings.validate_certificates,
            send_cookies: req.settings.send_cookies,
            store_cookies: req.settings.store_cookies,
            ..Default::default()
        };
        Ok(Prepared {
            auth_owner,
            workspace_id: ws.meta.id.clone(),
            request: out,
            auth,
            options,
            environment_id: ws.active_environment_id.clone(),
        })
    }

    /// Run scripts, send and persist a response (errors are persisted too).
    pub async fn send(&self, request_id: &str) -> Result<Doc<Response>> {
        let out = self
            .send_with_state(request_id, &mut RunState::default())
            .await?;
        Ok(out.response)
    }

    /// Send a prepared request without persisting the response.
    pub async fn execute(&self, p: &Prepared) -> Result<(Response, Option<Doc<CookieJar>>)> {
        // Resolve auth that needs I/O first (OAuth 2 token, netrc).
        let resolved = match &p.auth {
            Auth::OAuth2(cfg) if !cfg.disabled => match self
                .oauth2_token(p.auth_owner.as_deref().unwrap_or(&p.workspace_id), cfg)
                .await
            {
                Ok(token) => Auth::Bearer {
                    token,
                    prefix: Some(cfg.token_prefix.clone()).filter(|x| !x.is_empty()),
                    disabled: false,
                },
                Err(e) => {
                    let resp = Response {
                        method: p.request.method.clone(),
                        url: p.request.url.clone(),
                        error: Some(e.to_string()),
                        ..Default::default()
                    };
                    return Ok((resp, None));
                }
            },
            Auth::Netrc { disabled: false } => {
                let host = irs_http::build_url(&p.request, true)
                    .ok()
                    .and_then(|u| u.host_str().map(str::to_string))
                    .unwrap_or_default();
                match oauth2::netrc_lookup(&host) {
                    Some((username, password)) => Auth::Basic {
                        username,
                        password,
                        disabled: false,
                    },
                    None => Auth::None,
                }
            }
            other => other.clone(),
        };
        let p = &Prepared {
            auth: resolved,
            ..p.clone()
        };
        let mut jar = self.cookie_jar(&p.workspace_id)?;
        let mut cookies = jar.cookies.clone();
        let result = irs_http::send(&p.request, &p.auth, &p.options, &mut cookies).await;
        let jar_changed = cookies != jar.cookies;
        jar.cookies = cookies;
        let resp = match result {
            Ok(r) => Response {
                environment_id: p.environment_id.clone(),
                method: r.method.clone(),
                url: r.url.clone(),
                status_code: r.status,
                status_message: r.status_text.clone(),
                http_version: r.http_version.clone(),
                headers: r.headers.clone(),
                content_type: r.content_type.clone(),
                body_b64: Some(base64::engine::general_purpose::STANDARD.encode(&r.body)),
                bytes: r.body.len() as u64,
                timings: r.timings.clone(),
                timeline: r.timeline.clone(),
                ..Default::default()
            },
            Err(e) => Response {
                environment_id: p.environment_id.clone(),
                method: p.request.method.clone(),
                url: p.request.url.clone(),
                error: Some(e.to_string()),
                ..Default::default()
            },
        };
        Ok((resp, jar_changed.then_some(jar)))
    }

    pub(crate) fn persist(
        &self,
        request_id: &str,
        mut resp: Response,
        jar: Option<Doc<CookieJar>>,
        envs: &[Doc<Environment>],
    ) -> Result<Doc<Response>> {
        let max_history = self.store.settings()?.max_history_per_request.max(1);
        let big_body = resp
            .body_b64
            .as_ref()
            .is_some_and(|b| b.len() > INLINE_BODY_LIMIT * 4 / 3);
        let mut file_to_write: Option<(PathBuf, Vec<u8>)> = None;
        if big_body && let Some(dir) = &self.data_dir {
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(resp.body_b64.take().unwrap())
                .unwrap_or_default();
            let path = dir
                .join("responses")
                .join(format!("{}.bin", irs_core::new_id("body")));
            resp.body_path = Some(path.to_string_lossy().into_owned());
            file_to_write = Some((path, bytes));
        }
        if let Some((path, bytes)) = &file_to_write {
            let _ = std::fs::create_dir_all(path.parent().unwrap());
            let _ = std::fs::write(path, bytes);
        }
        let doc = self.store.batch(|tx| {
            let doc = tx.insert(Some(request_id), resp)?;
            if let Some(j) = &jar {
                tx.update(j)?;
            }
            for e in envs {
                tx.update(e)?;
            }
            let mut history = tx.children::<Response>(request_id)?;
            history.sort_by_key(|r| std::cmp::Reverse(r.meta.created));
            for old in history.into_iter().skip(max_history) {
                if let Some(p) = &old.body_path {
                    let _ = std::fs::remove_file(p);
                }
                tx.delete(old.id())?;
            }
            Ok(doc)
        })?;
        Ok(doc)
    }

    /// Run a GraphQL query (e.g. introspection) against a request's endpoint,
    /// reusing its rendered URL, headers, auth and cookies. Nothing is persisted.
    pub async fn graphql_query(
        &self,
        request_id: &str,
        query: &str,
        variables: Option<serde_json::Value>,
    ) -> Result<serde_json::Value> {
        let mut p = self.prepare(request_id, &[])?;
        p.request.method = "POST".into();
        let mut body = serde_json::json!({ "query": query });
        if let Some(v) = variables {
            body["variables"] = v;
        }
        p.request.body = irs_core::Body {
            mime_type: Some(irs_core::mime::JSON.into()),
            text: Some(body.to_string()),
            ..Default::default()
        };
        p.request
            .headers
            .retain(|h| !h.name.eq_ignore_ascii_case("content-type"));
        let (resp, _) = self.execute(&p).await?;
        if let Some(e) = resp.error {
            return Err(EngineError::Message(e));
        }
        let bytes = self.response_body(&resp);
        let v: serde_json::Value = serde_json::from_slice(&bytes).map_err(|_| {
            EngineError::Message(format!(
                "HTTP {} {} — response is not JSON: {}",
                resp.status_code,
                resp.status_message,
                String::from_utf8_lossy(&bytes)
                    .chars()
                    .take(200)
                    .collect::<String>()
            ))
        })?;
        if v.get("data").is_none_or(|d| d.is_null())
            && let Some(errs) = v.get("errors").and_then(|e| e.as_array())
        {
            let msgs: Vec<String> = errs
                .iter()
                .filter_map(|e| e["message"].as_str().map(str::to_string))
                .collect();
            return Err(EngineError::Message(format!(
                "GraphQL errors: {}",
                msgs.join("; ")
            )));
        }
        Ok(v)
    }

    /// Response body bytes (inline or from file).
    pub fn response_body(&self, resp: &Response) -> Vec<u8> {
        if let Some(b) = &resp.body_b64 {
            return base64::engine::general_purpose::STANDARD
                .decode(b)
                .unwrap_or_default();
        }
        resp.body_path
            .as_ref()
            .and_then(|p| std::fs::read(p).ok())
            .unwrap_or_default()
    }

    pub fn responses(&self, request_id: &str) -> Result<Vec<Doc<Response>>> {
        let mut r = self.store.children::<Response>(request_id)?;
        r.sort_by_key(|r| std::cmp::Reverse(r.meta.created));
        Ok(r)
    }

    /// Send many requests concurrently (bounded). Results keep input order.
    pub async fn send_many(
        &self,
        ids: &[String],
        concurrency: usize,
    ) -> Vec<Result<Doc<Response>>> {
        let sem = Arc::new(tokio::sync::Semaphore::new(concurrency.max(1)));
        let tasks = ids.iter().map(|id| {
            let sem = sem.clone();
            let id = id.clone();
            let engine = self.clone();
            async move {
                let _permit = sem.acquire().await.expect("semaphore open");
                engine.send(&id).await
            }
        });
        futures::future::join_all(tasks).await
    }

    /// All request ids under `parent_id` in tree order (depth-first).
    pub fn request_ids_in(&self, parent_id: &str) -> Result<Vec<String>> {
        let mut out = vec![];
        for d in self.store.all_children(parent_id)? {
            match d.meta.kind.as_str() {
                "Request" => out.push(d.meta.id.clone()),
                "Folder" => out.extend(self.request_ids_in(&d.meta.id)?),
                _ => {}
            }
        }
        Ok(out)
    }
}

fn merge_headers(into: &mut Vec<KeyValue>, from: &[KeyValue]) {
    for h in from
        .iter()
        .filter(|h| !h.disabled && !h.name.trim().is_empty())
    {
        into.retain(|x| !x.name.eq_ignore_ascii_case(&h.name));
        into.push(h.clone());
    }
}

pub(crate) fn render_auth(auth: &Auth, r: &dyn Fn(&str, &str) -> Result<String>) -> Result<Auth> {
    Ok(match auth {
        Auth::Basic {
            username,
            password,
            disabled,
        } => Auth::Basic {
            username: r("basic auth username", username)?,
            password: r("basic auth password", password)?,
            disabled: *disabled,
        },
        Auth::Bearer {
            token,
            prefix,
            disabled,
        } => Auth::Bearer {
            token: r("bearer token", token)?,
            prefix: prefix.as_ref().map(|p| r("bearer prefix", p)).transpose()?,
            disabled: *disabled,
        },
        Auth::ApiKey {
            key,
            value,
            add_to,
            disabled,
        } => Auth::ApiKey {
            key: r("API key name", key)?,
            value: r("API key value", value)?,
            add_to: add_to.clone(),
            disabled: *disabled,
        },
        Auth::Digest {
            username,
            password,
            disabled,
        } => Auth::Digest {
            username: r("digest username", username)?,
            password: r("digest password", password)?,
            disabled: *disabled,
        },
        Auth::OAuth1(c) => Auth::OAuth1(irs_core::OAuth1Config {
            consumer_key: r("OAuth 1 consumer key", &c.consumer_key)?,
            consumer_secret: r("OAuth 1 consumer secret", &c.consumer_secret)?,
            token_key: r("OAuth 1 token", &c.token_key)?,
            token_secret: r("OAuth 1 token secret", &c.token_secret)?,
            realm: r("OAuth 1 realm", &c.realm)?,
            callback: r("OAuth 1 callback", &c.callback)?,
            verifier: r("OAuth 1 verifier", &c.verifier)?,
            ..c.clone()
        }),
        Auth::Iam(c) => Auth::Iam(irs_core::AwsIamConfig {
            access_key_id: r("AWS access key", &c.access_key_id)?,
            secret_access_key: r("AWS secret key", &c.secret_access_key)?,
            session_token: r("AWS session token", &c.session_token)?,
            region: r("AWS region", &c.region)?,
            service: r("AWS service", &c.service)?,
            disabled: c.disabled,
        }),
        Auth::OAuth2(c) => Auth::OAuth2(irs_core::OAuth2Config {
            access_token_url: r("OAuth 2 token URL", &c.access_token_url)?,
            authorization_url: r("OAuth 2 authorization URL", &c.authorization_url)?,
            client_id: r("OAuth 2 client id", &c.client_id)?,
            client_secret: r("OAuth 2 client secret", &c.client_secret)?,
            scope: r("OAuth 2 scope", &c.scope)?,
            audience: r("OAuth 2 audience", &c.audience)?,
            resource: r("OAuth 2 resource", &c.resource)?,
            username: r("OAuth 2 username", &c.username)?,
            password: r("OAuth 2 password", &c.password)?,
            redirect_url: r("OAuth 2 redirect URL", &c.redirect_url)?,
            ..c.clone()
        }),
        other => other.clone(),
    })
}

#[cfg(test)]
mod tests;
