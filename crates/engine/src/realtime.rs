//! Render a saved realtime request into connection options and messages.

use base64::Engine as _;
use lsock_core::{Auth, Doc, Folder, RawDoc, RealtimeRequest};
use lsock_realtime::{ConnectOptions, Kind};
use lsock_templating::Mode;
use serde_json::Value;

use crate::{Engine, EngineError, Result};

pub fn parse_kind(k: &str) -> Kind {
    match k {
        "sse" => Kind::Sse,
        "socketio" => Kind::Socketio,
        _ => Kind::Websocket,
    }
}

impl Engine {
    fn render_field(&self, id: &str, field: &str, text: &str) -> Result<String> {
        let ctx = self.context(id)?;
        self.renderer()
            .render_str(text, &ctx, Mode::Throw)
            .map_err(|source| EngineError::Render {
                field: field.to_string(),
                source,
            })
    }

    /// Connection options with variables rendered and folder headers/auth inherited.
    pub fn realtime_options(&self, id: &str) -> Result<ConnectOptions> {
        let r: Doc<RealtimeRequest> = self.store.get(id)?;
        let folders: Vec<Doc<Folder>> = self
            .store
            .ancestors(id)?
            .iter()
            .filter(|d| d.meta.kind == "Folder")
            .map(RawDoc::typed)
            .collect::<std::result::Result<_, _>>()?;
        let mut headers: Vec<(String, String)> = vec![];
        for h in folders
            .iter()
            .rev()
            .flat_map(|f| f.headers.iter())
            .chain(r.headers.iter())
            .filter(|h| !h.disabled && !h.name.trim().is_empty())
        {
            let name = self.render_field(id, "header name", &h.name)?;
            let value = self.render_field(id, &format!("header '{}'", h.name), &h.value)?;
            headers.retain(|(k, _)| !k.eq_ignore_ascii_case(&name));
            headers.push((name, value));
        }
        let auth = if r.authentication.is_inherit() {
            folders
                .iter()
                .map(|f| f.authentication.clone())
                .find(|a| !a.is_inherit())
                .unwrap_or(Auth::None)
        } else {
            r.authentication.clone()
        };
        let mut url = self.render_field(id, "URL", &r.url)?;
        match auth {
            Auth::Bearer {
                token,
                prefix,
                disabled: false,
            } if !token.is_empty() => {
                let t = self.render_field(id, "bearer token", &token)?;
                headers.push((
                    "Authorization".into(),
                    format!(
                        "{} {t}",
                        prefix.filter(|p| !p.is_empty()).unwrap_or("Bearer".into())
                    ),
                ));
            }
            Auth::Basic {
                username,
                password,
                disabled: false,
            } => {
                let u = self.render_field(id, "username", &username)?;
                let p = self.render_field(id, "password", &password)?;
                headers.push((
                    "Authorization".into(),
                    format!(
                        "Basic {}",
                        base64::engine::general_purpose::STANDARD.encode(format!("{u}:{p}"))
                    ),
                ));
            }
            Auth::ApiKey {
                key,
                value,
                add_to,
                disabled: false,
            } if !key.is_empty() => {
                let k = self.render_field(id, "API key name", &key)?;
                let v = self.render_field(id, "API key", &value)?;
                if add_to.as_deref() == Some("queryParams") {
                    let sep = if url.contains('?') { '&' } else { '?' };
                    url = format!("{url}{sep}{}={}", url_encode(&k), url_encode(&v));
                } else {
                    headers.push((k, v));
                }
            }
            _ => {}
        }
        let auth_json = if r.socketio_auth.trim().is_empty() {
            None
        } else {
            let s = self.render_field(id, "Socket.IO auth", &r.socketio_auth)?;
            Some(
                serde_json::from_str::<Value>(&s)
                    .map_err(|e| EngineError::Llm(format!("Socket.IO auth must be JSON: {e}")))?,
            )
        };
        let settings = self.store.settings()?;
        Ok(ConnectOptions {
            kind: parse_kind(&r.kind),
            url,
            headers,
            subprotocols: r
                .subprotocols
                .iter()
                .filter(|s| !s.trim().is_empty())
                .cloned()
                .collect(),
            method: Some(r.method.clone()).filter(|m| !m.is_empty()),
            body: Some(self.render_field(id, "body", &r.body.body)?).filter(|b| !b.is_empty()),
            namespace: Some(r.namespace.clone()),
            auth: auth_json,
            validate_certificates: settings.validate_certificates,
        })
    }

    /// Render a message from the composer (or `text`) for sending.
    pub fn realtime_payload(&self, id: &str, text: &str) -> Result<String> {
        self.render_field(id, "message", text)
    }
}

fn url_encode(s: &str) -> String {
    url::form_urlencoded::byte_serialize(s.as_bytes()).collect()
}
