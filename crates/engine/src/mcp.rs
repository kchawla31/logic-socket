//! Resolve a saved `McpServer` into connect options, rendering `{{ vars }}`
//! in its URL, command, headers and env against the workspace environment.

use std::collections::HashMap;

use irs_core::{Auth, McpServer, McpTransport};
use irs_mcp::{ConnectOptions, TransportConfig};
use irs_templating::Mode;

use crate::{Engine, EngineError, Result};

impl Engine {
    pub fn mcp_connect_options(&self, server_id: &str) -> Result<ConnectOptions> {
        let server = self.store.get::<McpServer>(server_id)?;
        let ctx = self.context(server_id)?;
        let r = |field: &str, s: &str| {
            self.renderer()
                .render_str(s, &ctx, Mode::Throw)
                .map_err(|source| EngineError::Render {
                    field: field.to_string(),
                    source,
                })
        };
        let mut headers = vec![];
        for h in server
            .headers
            .iter()
            .filter(|h| !h.disabled && !h.name.trim().is_empty())
        {
            headers.push((
                r("header name", &h.name)?,
                r(&format!("header '{}'", h.name), &h.value)?,
            ));
        }
        match &server.authentication {
            Auth::Bearer {
                token,
                prefix,
                disabled: false,
            } if !token.is_empty() => {
                let p = prefix
                    .as_deref()
                    .filter(|p| !p.is_empty())
                    .unwrap_or("Bearer");
                headers.push((
                    "Authorization".into(),
                    format!("{p} {}", r("bearer token", token)?),
                ));
            }
            Auth::ApiKey {
                key,
                value,
                add_to: None,
                disabled: false,
            } if !key.is_empty() => {
                headers.push((r("API key name", key)?, r("API key value", value)?));
            }
            _ => {}
        }
        let mut env = HashMap::new();
        for e in server
            .env
            .iter()
            .filter(|e| !e.disabled && !e.name.trim().is_empty())
        {
            env.insert(e.name.clone(), r(&format!("env '{}'", e.name), &e.value)?);
        }
        let transport = match &server.transport {
            McpTransport::StreamableHttp { url } => TransportConfig::Http {
                url: r("URL", url)?,
                headers,
                validate_certificates: server.ssl_validation.unwrap_or(true),
            },
            McpTransport::Stdio { command, args, cwd } => TransportConfig::Stdio {
                command: r("command", command)?,
                args: args
                    .iter()
                    .map(|a| r("argument", a))
                    .collect::<Result<_>>()?,
                env,
                cwd: cwd
                    .as_deref()
                    .map(|c| r("working directory", c))
                    .transpose()?,
            },
        };
        let mut opts = ConnectOptions::new(transport);
        if server.sampling.enabled
            && let Some(pid) = &server.sampling.provider_id
        {
            let (provider, cfg) = self.provider_config(pid, Some(server_id))?;
            let model = server
                .sampling
                .model
                .clone()
                .filter(|m| !m.is_empty())
                .unwrap_or(provider.default_model.clone());
            let max_tokens = if server.sampling.max_tokens == 0 {
                1024
            } else {
                server.sampling.max_tokens
            };
            opts.sampling = Some(std::sync::Arc::new(irs_llm::agent::LlmSampler {
                cfg,
                model,
                max_tokens,
            }));
        }
        opts.root_uris = server
            .roots
            .iter()
            .map(|x| (x.uri.clone(), x.name.clone()))
            .collect();
        Ok(opts)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use irs_core::{KeyValue, Workspace};

    #[test]
    fn renders_saved_server_config() {
        let e = Engine::in_memory();
        let ws = e.store.insert(None, Workspace::default()).unwrap();
        let mut base = e.base_environment(ws.id()).unwrap();
        base.data.insert("host".into(), "localhost:9".into());
        base.data.insert("tok".into(), "abc".into());
        e.store.update(&base).unwrap();
        let s = e
            .store
            .insert(
                Some(ws.id()),
                McpServer {
                    transport: McpTransport::StreamableHttp {
                        url: "http://{{ host }}/mcp".into(),
                    },
                    headers: vec![KeyValue::new("X-A", "1")],
                    authentication: Auth::Bearer {
                        token: "{{ tok }}".into(),
                        prefix: None,
                        disabled: false,
                    },
                    ..Default::default()
                },
            )
            .unwrap();
        let o = e.mcp_connect_options(s.id()).unwrap();
        match o.transport {
            TransportConfig::Http { url, headers, .. } => {
                assert_eq!(url, "http://localhost:9/mcp");
                assert_eq!(
                    headers,
                    vec![
                        ("X-A".into(), "1".into()),
                        ("Authorization".into(), "Bearer abc".into())
                    ]
                );
            }
            other => panic!("{other:?}"),
        }
    }
}
