//! OAuth 2.0 token handling (client credentials, password, authorization code
//! with PKCE + localhost redirect, refresh) and `~/.netrc` lookup.

use base64::Engine as _;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use lsock_core::{Doc, OAuth2Config, OAuth2Token, now_ms};
use serde_json::Value;
use sha2::Digest as _;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::{Engine, EngineError, Result};

fn err(m: impl Into<String>) -> EngineError {
    EngineError::Message(m.into())
}

/// PKCE verifier (43–128 chars) and its S256 challenge.
pub fn pkce_pair() -> (String, String) {
    let verifier = format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    );
    let challenge = URL_SAFE_NO_PAD.encode(sha2::Sha256::digest(verifier.as_bytes()));
    (verifier, challenge)
}

fn form_body(pairs: &[(&str, &str)]) -> String {
    let mut s = url::form_urlencoded::Serializer::new(String::new());
    for (k, v) in pairs.iter().filter(|(_, v)| !v.is_empty()) {
        s.append_pair(k, v);
    }
    s.finish()
}

impl Engine {
    /// The rendered OAuth 2 config defined on `owner_id` (a request or folder).
    pub fn oauth2_config(&self, owner_id: &str) -> Result<OAuth2Config> {
        let raw = self
            .store
            .raw(owner_id)?
            .ok_or_else(|| err("item not found"))?;
        let auth: lsock_core::Auth = serde_json::from_value(raw.data["authentication"].clone())
            .map_err(|e| err(e.to_string()))?;
        let ctx = self.context(owner_id)?;
        let r = |f: &str, s: &str| {
            self.renderer()
                .render_str(s, &ctx, lsock_templating::Mode::Throw)
                .map_err(|source| EngineError::Render {
                    field: f.to_string(),
                    source,
                })
        };
        match crate::render_auth(&auth, &r)? {
            lsock_core::Auth::OAuth2(c) => Ok(c),
            _ => Err(err("this item does not use OAuth 2")),
        }
    }

    /// The cached token (decrypted). A token this machine can't decrypt counts as missing.
    pub fn oauth2_cached(&self, owner_id: &str) -> Result<Option<Doc<OAuth2Token>>> {
        let Some(mut t) = self
            .store
            .children::<OAuth2Token>(owner_id)?
            .into_iter()
            .next()
        else {
            return Ok(None);
        };
        let open = |s: &str| self.unseal_value(s);
        let Ok(access) = open(&t.access_token) else {
            return Ok(None);
        };
        t.body.access_token = access;
        t.body.refresh_token = t
            .body
            .refresh_token
            .as_deref()
            .map(open)
            .transpose()
            .ok()
            .flatten();
        t.body.id_token = t
            .body
            .id_token
            .as_deref()
            .map(open)
            .transpose()
            .ok()
            .flatten();
        Ok(Some(t))
    }

    pub fn oauth2_clear(&self, owner_id: &str) -> Result<()> {
        let ids: Vec<String> = self
            .store
            .children::<OAuth2Token>(owner_id)?
            .into_iter()
            .map(|t| t.meta.id)
            .collect();
        Ok(self.store.batch(|tx| {
            for id in &ids {
                tx.delete(id)?;
            }
            Ok(())
        })?)
    }

    /// Store a token with its secrets encrypted by the vault; returns it decrypted.
    fn oauth2_save(&self, owner_id: &str, token: OAuth2Token) -> Result<Doc<OAuth2Token>> {
        let mut stored = token.clone();
        let seal = |s: &str| {
            if s.is_empty() {
                Ok(String::new())
            } else {
                self.seal_value(s)
            }
        };
        stored.access_token = seal(&token.access_token)?;
        stored.refresh_token = token.refresh_token.as_deref().map(seal).transpose()?;
        stored.id_token = token.id_token.as_deref().map(seal).transpose()?;
        let old: Vec<String> = self
            .store
            .children::<OAuth2Token>(owner_id)?
            .into_iter()
            .map(|t| t.meta.id)
            .collect();
        Ok(self
            .store
            .batch(|tx| {
                for id in &old {
                    tx.delete(id)?;
                }
                tx.insert(Some(owner_id), stored)
            })
            .map(|d| Doc {
                meta: d.meta,
                body: token,
            })?)
    }

    /// POST to the token endpoint and parse the answer (JSON or form-encoded).
    async fn token_request(
        &self,
        cfg: &OAuth2Config,
        mut pairs: Vec<(&str, String)>,
    ) -> Result<OAuth2Token> {
        if cfg.access_token_url.trim().is_empty() {
            return Err(err("OAuth 2: the access token URL is empty"));
        }
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .map_err(|e| err(e.to_string()))?;
        let mut rb = client
            .post(cfg.access_token_url.trim())
            .header("Accept", "application/json");
        if cfg.credentials_in_body {
            pairs.push(("client_id", cfg.client_id.clone()));
            pairs.push(("client_secret", cfg.client_secret.clone()));
        } else if !cfg.client_secret.is_empty() {
            let enc =
                |s: &str| url::form_urlencoded::byte_serialize(s.as_bytes()).collect::<String>();
            rb = rb.header(
                "Authorization",
                format!(
                    "Basic {}",
                    STANDARD.encode(format!(
                        "{}:{}",
                        enc(&cfg.client_id),
                        enc(&cfg.client_secret)
                    ))
                ),
            );
        } else {
            pairs.push(("client_id", cfg.client_id.clone()));
        }
        let borrowed: Vec<(&str, &str)> = pairs.iter().map(|(k, v)| (*k, v.as_str())).collect();
        let resp = rb
            .header("Content-Type", "application/x-www-form-urlencoded")
            .body(form_body(&borrowed))
            .send()
            .await
            .map_err(|e| err(format!("OAuth 2 token request failed: {e}")))?;
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        let v: Value = serde_json::from_str(&text).unwrap_or_else(|_| {
            // e.g. GitHub's form-encoded answers
            Value::Object(
                url::form_urlencoded::parse(text.as_bytes())
                    .map(|(k, v)| (k.into_owned(), Value::String(v.into_owned())))
                    .collect(),
            )
        });
        if let Some(e) = v["error"].as_str() {
            let desc = v["error_description"]
                .as_str()
                .map(|d| format!(": {d}"))
                .unwrap_or_default();
            return Err(err(format!("OAuth 2 error {e}{desc}")));
        }
        if !status.is_success() || v["access_token"].as_str().is_none() {
            return Err(err(format!(
                "OAuth 2 token endpoint answered HTTP {status}: {}",
                text.chars().take(300).collect::<String>()
            )));
        }
        let expires_in = v["expires_in"]
            .as_i64()
            .or_else(|| v["expires_in"].as_str().and_then(|s| s.parse().ok()));
        Ok(OAuth2Token {
            access_token: v["access_token"].as_str().unwrap_or("").to_string(),
            refresh_token: v["refresh_token"].as_str().map(str::to_string),
            token_type: v["token_type"].as_str().map(str::to_string),
            expires_at: expires_in.map(|s| now_ms() + s * 1000),
            scope: v["scope"].as_str().map(str::to_string),
            id_token: v["id_token"].as_str().map(str::to_string),
            error: None,
        })
    }

    /// A valid access token for `owner_id`'s OAuth 2 config: cached → refreshed →
    /// fetched (non-interactive grants). Authorization code needs [`Engine::oauth2_authorize`] first.
    pub async fn oauth2_token(&self, owner_id: &str, cfg: &OAuth2Config) -> Result<String> {
        if let Some(t) = self.oauth2_cached(owner_id)? {
            let fresh = t.expires_at.is_none_or(|e| e > now_ms() + 30_000);
            if fresh && !t.access_token.is_empty() {
                return Ok(t.access_token.clone());
            }
            if let Some(rt) = t.refresh_token.clone().filter(|r| !r.is_empty()) {
                let mut pairs = vec![
                    ("grant_type", "refresh_token".to_string()),
                    ("refresh_token", rt.clone()),
                ];
                if !cfg.scope.is_empty() {
                    pairs.push(("scope", cfg.scope.clone()));
                }
                // RFC 8707: name the protected resource on refresh too (the MCP spec requires it).
                if !cfg.resource.is_empty() {
                    pairs.push(("resource", cfg.resource.clone()));
                }
                if let Ok(mut nt) = self.token_request(cfg, pairs).await {
                    nt.refresh_token = nt.refresh_token.or(Some(rt));
                    let at = nt.access_token.clone();
                    self.oauth2_save(owner_id, nt)?;
                    return Ok(at);
                }
            }
        }
        let mut pairs = match cfg.grant_type.as_str() {
            "client_credentials" => vec![("grant_type", "client_credentials".to_string())],
            "password" => vec![
                ("grant_type", "password".to_string()),
                ("username", cfg.username.clone()),
                ("password", cfg.password.clone()),
            ],
            _ => {
                return Err(err(
                    "OAuth 2: no token yet — open the Auth tab and click “Get token” to sign in",
                ));
            }
        };
        for (k, v) in [
            ("scope", &cfg.scope),
            ("audience", &cfg.audience),
            ("resource", &cfg.resource),
        ] {
            if !v.is_empty() {
                pairs.push((k, v.clone()));
            }
        }
        let token = self.token_request(cfg, pairs).await?;
        let at = token.access_token.clone();
        self.oauth2_save(owner_id, token)?;
        Ok(at)
    }

    /// Authorization code flow with PKCE: opens the browser via `open_url`,
    /// waits for the redirect on the localhost `redirect_url`, exchanges the code.
    pub async fn oauth2_authorize(
        &self,
        owner_id: &str,
        cfg: &OAuth2Config,
        open_url: &(dyn Fn(&str) + Send + Sync),
    ) -> Result<Doc<OAuth2Token>> {
        if cfg.grant_type != "authorization_code" {
            self.oauth2_clear(owner_id)?; // force a fresh token
            self.oauth2_token(owner_id, cfg).await?;
            return self
                .oauth2_cached(owner_id)?
                .ok_or_else(|| err("no token returned"));
        }
        let redirect = url::Url::parse(cfg.redirect_url.trim())
            .map_err(|e| err(format!("OAuth 2: invalid redirect URL: {e}")))?;
        let host = redirect.host_str().unwrap_or("");
        if !(host == "localhost" || host == "127.0.0.1") || redirect.scheme() != "http" {
            return Err(err(
                "OAuth 2: the redirect URL must be http://localhost:<port>/… or http://127.0.0.1:<port>/… so logic-socket can catch it",
            ));
        }
        let port = redirect.port().unwrap_or(80);
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", port))
            .await
            .map_err(|e| {
                err(format!(
                    "OAuth 2: cannot listen on port {port} for the redirect: {e}"
                ))
            })?;
        let (verifier, challenge) = pkce_pair();
        let state = uuid::Uuid::new_v4().simple().to_string();
        let mut auth_url = url::Url::parse(cfg.authorization_url.trim())
            .map_err(|e| err(format!("OAuth 2: invalid authorization URL: {e}")))?;
        {
            let mut q = auth_url.query_pairs_mut();
            q.append_pair("response_type", "code")
                .append_pair("client_id", &cfg.client_id)
                .append_pair("redirect_uri", cfg.redirect_url.trim())
                .append_pair("state", &state);
            for (k, v) in [
                ("scope", &cfg.scope),
                ("audience", &cfg.audience),
                ("resource", &cfg.resource),
            ] {
                if !v.is_empty() {
                    q.append_pair(k, v);
                }
            }
            if cfg.use_pkce {
                q.append_pair("code_challenge", &challenge)
                    .append_pair("code_challenge_method", "S256");
            }
        }
        open_url(auth_url.as_str());

        let code = tokio::time::timeout(std::time::Duration::from_secs(300), async {
            loop {
                let (mut sock, _) = listener.accept().await.map_err(|e| err(e.to_string()))?;
                let mut buf = vec![0u8; 8192];
                let n = sock.read(&mut buf).await.unwrap_or(0);
                let req = String::from_utf8_lossy(&buf[..n]).to_string();
                let path = req.split_whitespace().nth(1).unwrap_or("/").to_string();
                let u = url::Url::parse(&format!("http://localhost{path}")).map_err(|e| err(e.to_string()))?;
                let get = |k: &str| u.query_pairs().find(|(q, _)| q == k).map(|(_, v)| v.into_owned());
                let (title, result) = match (get("code"), get("error")) {
                    (_, Some(e)) => ("Sign-in failed", Err(err(format!("OAuth 2: {e}{}", get("error_description").map(|d| format!(": {d}")).unwrap_or_default())))),
                    (Some(c), _) if get("state").as_deref() == Some(state.as_str()) => ("Signed in", Ok(c)),
                    (Some(_), _) => ("Sign-in failed", Err(err("OAuth 2: state mismatch (possible CSRF) — try again"))),
                    _ => continue, // favicon or unrelated request
                };
                let html = format!("<!doctype html><meta charset=utf-8><title>{title}</title><body style='font-family:system-ui;padding:3em'><h2>{title}</h2><p>You can close this window and return to logic-socket.</p></body>");
                let _ = sock
                    .write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{html}", html.len()).as_bytes())
                    .await;
                return result;
            }
        })
        .await
        .map_err(|_| err("OAuth 2: timed out waiting for the browser sign-in (5 minutes)"))??;

        let mut pairs = vec![
            ("grant_type", "authorization_code".to_string()),
            ("code", code),
            ("redirect_uri", cfg.redirect_url.trim().to_string()),
        ];
        if cfg.use_pkce {
            pairs.push(("code_verifier", verifier));
        }
        if !cfg.resource.is_empty() {
            pairs.push(("resource", cfg.resource.clone()));
        }
        let token = self.token_request(cfg, pairs).await?;
        self.oauth2_save(owner_id, token)
    }
}

/// Credentials for `host` from a netrc file (`$NETRC` or `~/.netrc`).
pub fn netrc_lookup(host: &str) -> Option<(String, String)> {
    let path = std::env::var_os("NETRC")
        .map(std::path::PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".netrc")))?;
    parse_netrc(&std::fs::read_to_string(path).ok()?, host)
}

/// netrc tokens with comments and `macdef` bodies removed. A token starting
/// with `#` comments out the rest of its line; a macro body runs from the line
/// after `macdef <name>` to the next blank line.
fn netrc_tokens(content: &str) -> Vec<&str> {
    let mut toks = vec![];
    let mut in_macro = false;
    for line in content.lines() {
        if in_macro {
            in_macro = !line.trim().is_empty();
            continue;
        }
        for tok in line.split_whitespace() {
            if tok.starts_with('#') {
                break;
            }
            toks.push(tok);
            if toks.len() >= 2 && toks[toks.len() - 2] == "macdef" {
                in_macro = true;
                break;
            }
        }
    }
    toks
}

pub fn parse_netrc(content: &str, host: &str) -> Option<(String, String)> {
    let toks = netrc_tokens(content);
    let mut i = 0;
    let mut default: Option<(String, String)> = None;
    while i < toks.len() {
        let (is_match, is_default) = match toks[i] {
            "machine" => {
                i += 1;
                (
                    toks.get(i).is_some_and(|m| m.eq_ignore_ascii_case(host)),
                    false,
                )
            }
            "default" => (false, true),
            _ => {
                i += 1;
                continue;
            }
        };
        i += 1;
        let (mut login, mut password) = (String::new(), String::new());
        while i < toks.len() && toks[i] != "machine" && toks[i] != "default" {
            match toks[i] {
                "login" => login = toks.get(i + 1).unwrap_or(&"").to_string(),
                "password" => password = toks.get(i + 1).unwrap_or(&"").to_string(),
                _ => {}
            }
            i += if matches!(toks[i], "login" | "password" | "account" | "macdef") {
                2
            } else {
                1
            };
        }
        if is_match {
            return Some((login, password));
        }
        if is_default {
            default = Some((login, password));
        }
    }
    default
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pkce_challenge_is_s256_of_verifier() {
        let (v, c) = pkce_pair();
        assert!(v.len() >= 43 && v.len() <= 128);
        assert_eq!(
            c,
            URL_SAFE_NO_PAD.encode(sha2::Sha256::digest(v.as_bytes()))
        );
    }

    #[test]
    fn netrc_parsing() {
        let n = "machine api.github.com login octo password tok1\nmachine other.io\n  login a\n  password b\ndefault login anon password x";
        assert_eq!(
            parse_netrc(n, "API.github.com"),
            Some(("octo".into(), "tok1".into()))
        );
        assert_eq!(parse_netrc(n, "other.io"), Some(("a".into(), "b".into())));
        assert_eq!(
            parse_netrc(n, "unknown.io"),
            Some(("anon".into(), "x".into()))
        );
        assert_eq!(parse_netrc("machine a login b password c", "z"), None);
    }

    /// netrc(5): a `#` starts a comment through end of line, and `macdef`
    /// consumes the following lines until a blank line. Neither is credentials.
    #[test]
    fn netrc_ignores_comments_and_macdef_bodies() {
        let commented =
            "# machine evil.com login bad password worse\nmachine good.com login u password p\n";
        let macdef = "\
machine example.com
login user
password secret
macdef init
login attacker
password stolen

machine other.com
login a
password b
";
        let mut problems = vec![];
        let from_comment = parse_netrc(commented, "evil.com");
        if from_comment.is_some() {
            problems.push(format!("commented machine was used: {from_comment:?}"));
        }
        if parse_netrc(commented, "good.com") != Some(("u".into(), "p".into())) {
            problems.push(format!(
                "real machine missing: {:?}",
                parse_netrc(commented, "good.com")
            ));
        }
        let from_macro = parse_netrc(macdef, "example.com");
        if from_macro != Some(("user".into(), "secret".into())) {
            problems.push(format!("macdef body overwrote credentials: {from_macro:?}"));
        }
        assert!(problems.is_empty(), "{problems:?}");
    }
}
