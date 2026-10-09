//! Resolve a saved `McpServer` into connect options, rendering `{{ vars }}`
//! in its URL, command, headers and env against the workspace environment.

use std::collections::HashMap;

use lsock_core::{Auth, McpServer, McpTransport};
use lsock_mcp::{ConnectOptions, TransportConfig};
use lsock_templating::Mode;

use crate::{Engine, EngineError, Result};

impl Engine {
    /// One JSON-RPC message to a saved Streamable HTTP server as a plain HTTP request,
    /// with the URL, headers, and auth the client would send, for code export.
    /// `message` defaults to `initialize`. Pass the live session to make later calls work.
    pub async fn mcp_code_request(
        &self,
        server_id: &str,
        message: Option<serde_json::Value>,
        session_id: Option<&str>,
        protocol_version: Option<&str>,
    ) -> Result<(lsock_convert::codegen::CodeRequest, Vec<String>)> {
        let server = self.store.get::<McpServer>(server_id)?;
        if !matches!(server.transport, McpTransport::StreamableHttp { .. }) {
            return Err(EngineError::Message(
                "stdio servers run as a local command, so there is no HTTP request to export"
                    .into(),
            ));
        }
        let mut notes = vec![];
        let oauth = match self.mcp_oauth_header(server_id).await {
            Ok(h) => h,
            Err(_) => {
                notes.push(
                    "Not signed in: replace <access-token>, or sign in on Settings › Auth first"
                        .into(),
                );
                Some("Bearer <access-token>".into())
            }
        };
        let TransportConfig::Http {
            url, mut headers, ..
        } = self.connect_options_with(server_id, oauth)?.transport
        else {
            unreachable!("checked above");
        };
        headers.push(("Content-Type".into(), "application/json".into()));
        headers.push((
            "Accept".into(),
            "application/json, text/event-stream".into(),
        ));
        let message = message.unwrap_or_else(|| {
            serde_json::json!({
                "jsonrpc": "2.0", "id": 1, "method": "initialize",
                "params": {
                    "protocolVersion": lsock_mcp::LATEST_PROTOCOL_VERSION,
                    "capabilities": {},
                    "clientInfo": {"name": "curl", "version": "1.0"},
                },
            })
        });
        let initialize = message["method"] == "initialize";
        if let Some(sid) = session_id.filter(|_| !initialize) {
            headers.push(("Mcp-Session-Id".into(), sid.into()));
        }
        if let Some(v) = protocol_version.filter(|_| !initialize) {
            headers.push(("MCP-Protocol-Version".into(), v.into()));
        }
        if !initialize && session_id.is_none() {
            notes.push(
                "Servers that keep sessions need the Mcp-Session-Id header from the initialize response; connect first to include it".into(),
            );
        }
        Ok((
            lsock_convert::codegen::CodeRequest {
                method: "POST".into(),
                url,
                headers,
                body: lsock_convert::codegen::CodeBody::Text {
                    text: message.to_string(),
                },
            },
            notes,
        ))
    }

    /// Connect options for a saved server. A server using OAuth gets its cached token,
    /// refreshed first when it is about to expire.
    pub async fn mcp_connect_options(&self, server_id: &str) -> Result<ConnectOptions> {
        let oauth = self.mcp_oauth_header(server_id).await?;
        self.connect_options_with(server_id, oauth)
    }

    fn connect_options_with(
        &self,
        server_id: &str,
        oauth: Option<String>,
    ) -> Result<ConnectOptions> {
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
        if let Some(h) = oauth {
            headers.push(("Authorization".into(), h));
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
            opts.sampling = Some(std::sync::Arc::new(lsock_llm::agent::LlmSampler {
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
    use lsock_core::{KeyValue, Workspace};

    #[tokio::test]
    async fn renders_saved_server_config() {
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
        let o = e.mcp_connect_options(s.id()).await.unwrap();
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

    #[tokio::test]
    async fn exports_mcp_messages_as_code() {
        use lsock_convert::codegen::{Target, generate};
        let e = Engine::in_memory();
        let ws = e.store.insert(None, Workspace::default()).unwrap();
        let mut base = e.base_environment(ws.id()).unwrap();
        base.data.insert("tok".into(), "abc".into());
        e.store.update(&base).unwrap();
        let http = e
            .store
            .insert(
                Some(ws.id()),
                McpServer {
                    transport: McpTransport::StreamableHttp {
                        url: "http://localhost:9/mcp".into(),
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

        let (init, notes) = e
            .mcp_code_request(http.id(), None, None, None)
            .await
            .unwrap();
        let curl = generate(&init, Target::Curl);
        for part in [
            "--url http://localhost:9/mcp",
            "'X-A: 1'",
            "'Authorization: Bearer abc'",
            "'Accept: application/json, text/event-stream'",
            r#""method":"initialize""#,
        ] {
            assert!(curl.contains(part), "{part} missing from\n{curl}");
        }
        assert!(
            !curl.contains("Mcp-Session-Id") && notes.is_empty(),
            "{curl}\n{notes:?}"
        );

        let call = serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": "echo", "arguments": {}}});
        let (r, notes) = e
            .mcp_code_request(
                http.id(),
                Some(call.clone()),
                Some("s-1"),
                Some("2025-11-25"),
            )
            .await
            .unwrap();
        let curl = generate(&r, Target::Curl);
        assert!(
            curl.contains("'Mcp-Session-Id: s-1'")
                && curl.contains("'MCP-Protocol-Version: 2025-11-25'"),
            "{curl}"
        );
        assert!(notes.is_empty());
        let (_, notes) = e
            .mcp_code_request(http.id(), Some(call), None, None)
            .await
            .unwrap();
        assert!(
            notes.iter().any(|n| n.contains("Mcp-Session-Id")),
            "{notes:?}"
        );

        // OAuth before sign-in: a placeholder token and a note, not an error.
        let oauth = e
            .store
            .insert(
                Some(ws.id()),
                McpServer {
                    transport: McpTransport::StreamableHttp {
                        url: "http://localhost:9/mcp".into(),
                    },
                    authentication: Auth::OAuth2(lsock_core::OAuth2Config::default()),
                    ..Default::default()
                },
            )
            .unwrap();
        let (r, notes) = e
            .mcp_code_request(oauth.id(), None, None, None)
            .await
            .unwrap();
        assert!(generate(&r, Target::Curl).contains("Bearer <access-token>"));
        assert!(
            notes.iter().any(|n| n.contains("Not signed in")),
            "{notes:?}"
        );

        let stdio = e
            .store
            .insert(
                Some(ws.id()),
                McpServer {
                    transport: McpTransport::Stdio {
                        command: "npx".into(),
                        args: vec![],
                        cwd: None,
                    },
                    ..Default::default()
                },
            )
            .unwrap();
        let err = e
            .mcp_code_request(stdio.id(), None, None, None)
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("stdio"), "{err}");
    }
}
