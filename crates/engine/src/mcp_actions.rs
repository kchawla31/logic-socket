//! Read / Create / Update / Delete for each tool of a saved MCP server: the server's
//! hints, then an AI reading of the description (only when the server has an AI
//! provider set, cached per tool), then the tool's name. See [`lsock_mcp::action`].

use std::collections::HashMap;

use lsock_core::{CachedToolAction, Doc, McpServer, McpToolActions};
use lsock_llm::{Block, ChatRequest, Message};
use lsock_mcp::Tool;
use lsock_mcp::action::{self, Classified, ToolAction};
use sha2::Digest as _;

use crate::{Engine, EngineError, Result};

/// Tools per AI request; keeps prompts and answers small.
const BATCH: usize = 40;

fn digest(t: &Tool) -> String {
    let h = sha2::Sha256::digest(
        format!("{}\n{}", t.name, t.description.as_deref().unwrap_or("")).as_bytes(),
    );
    h.iter().take(8).map(|b| format!("{b:02x}")).collect()
}

impl Engine {
    fn tool_actions_cache(&self, server_id: &str) -> Result<Option<Doc<McpToolActions>>> {
        Ok(self
            .store
            .children::<McpToolActions>(server_id)?
            .into_iter()
            .next())
    }

    /// AI answers cached for these tools by the server's current provider, for tools
    /// whose name and description are unchanged.
    fn cached_ai_actions(
        &self,
        server: &Doc<McpServer>,
        tools: &[Tool],
    ) -> Result<HashMap<String, ToolAction>> {
        let Some(pid) = &server.action_provider_id else {
            return Ok(HashMap::new());
        };
        let Some(cache) = self
            .tool_actions_cache(server.id())?
            .filter(|c| &c.provider_id == pid)
        else {
            return Ok(HashMap::new());
        };
        Ok(tools
            .iter()
            .filter_map(|t| {
                let c = cache
                    .actions
                    .get(&t.name)
                    .filter(|c| c.digest == digest(t))?;
                Some((t.name.clone(), ToolAction::parse(&c.action)?))
            })
            .collect())
    }

    /// The action for each tool from what is known now (no network). Unknown tools are absent.
    pub fn mcp_tool_actions(
        &self,
        server_id: &str,
        tools: &[Tool],
    ) -> Result<HashMap<String, Classified>> {
        let server = self.store.get::<McpServer>(server_id)?;
        let ai = self.cached_ai_actions(&server, tools)?;
        Ok(tools
            .iter()
            .filter_map(|t| {
                action::classify(t, ai.get(&t.name).copied()).map(|c| (t.name.clone(), c))
            })
            .collect())
    }

    /// Ask the server's AI provider to classify tools that have no cached answer, and
    /// cache the result. Returns how many tools were sent; 0 when no provider is set
    /// or everything is cached. Only names and descriptions are sent.
    pub async fn mcp_classify_tools(&self, server_id: &str, tools: &[Tool]) -> Result<usize> {
        let server = self.store.get::<McpServer>(server_id)?;
        let Some(pid) = server.action_provider_id.clone() else {
            return Ok(0);
        };
        let cached = self.cached_ai_actions(&server, tools)?;
        let todo: Vec<&Tool> = tools
            .iter()
            .filter(|t| !cached.contains_key(&t.name))
            .collect();
        if todo.is_empty() {
            return Ok(0);
        }
        let (provider, cfg) = self.provider_config(&pid, Some(server_id))?;
        let mut answers = HashMap::new();
        for chunk in todo.chunks(BATCH) {
            let req = ChatRequest {
                model: provider.default_model.clone(),
                system: Some("You label API tools by what they do. Reply with JSON only.".into()),
                messages: vec![Message::user(action::classify_prompt(chunk))],
                tools: vec![],
                max_tokens: 2000,
                temperature: Some(0.0),
            };
            let resp = lsock_llm::stream_chat(&cfg, &req, &mut |_| {})
                .await
                .map_err(|e| {
                    EngineError::Llm(format!("classifying tools with '{}': {e}", provider.name))
                })?;
            let text: String = resp
                .content
                .iter()
                .filter_map(|b| match b {
                    Block::Text { text } => Some(text.as_str()),
                    _ => None,
                })
                .collect();
            answers.extend(action::parse_classification(&text));
        }

        let mut cache = match self.tool_actions_cache(server_id)? {
            Some(c) if c.provider_id == pid => c,
            Some(mut c) => {
                c.body = McpToolActions {
                    provider_id: pid.clone(),
                    ..Default::default()
                };
                c
            }
            None => self.store.insert(
                Some(server_id),
                McpToolActions {
                    provider_id: pid.clone(),
                    ..Default::default()
                },
            )?,
        };
        for t in &todo {
            if let Some(a) = answers.get(&t.name) {
                cache.body.actions.insert(
                    t.name.clone(),
                    CachedToolAction {
                        action: a.as_str().to_string(),
                        digest: digest(t),
                    },
                );
            }
        }
        self.store.update(&cache)?;
        Ok(todo.len())
    }
}
