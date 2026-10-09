//! What a tool does, as one of Read / Create / Update / Delete, for the badge on each
//! tool and for deciding when the AI chat asks before a call.
//!
//! Decided in order: the server's own hints (when it sent any), then an AI reading of
//! the description (when enabled for the server), then the tool's name. When none of
//! these is confident the tool has no action, and callers treat it as unknown.

use serde::{Deserialize, Serialize};

use crate::types::Tool;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ToolAction {
    Read,
    Create,
    Update,
    Delete,
}

impl ToolAction {
    /// Stored and serialized spelling: `read`, `create`, `update`, `delete`.
    pub fn as_str(self) -> &'static str {
        match self {
            ToolAction::Read => "read",
            ToolAction::Create => "create",
            ToolAction::Update => "update",
            ToolAction::Delete => "delete",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            ToolAction::Read => "Read",
            ToolAction::Create => "Create",
            ToolAction::Update => "Update",
            ToolAction::Delete => "Delete",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "read" => Some(ToolAction::Read),
            "create" => Some(ToolAction::Create),
            "update" => Some(ToolAction::Update),
            "delete" => Some(ToolAction::Delete),
            _ => None,
        }
    }

    /// Whether a call changes something (the AI chat asks before these).
    pub fn writes(self) -> bool {
        self != ToolAction::Read
    }
}

/// Where an action came from, shown on hover.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ActionSource {
    /// The server's annotations.
    Server,
    /// An AI provider reading the description.
    Ai,
    /// The verb in the tool's name.
    Name,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Classified {
    pub action: ToolAction,
    pub source: ActionSource,
}

const READ: &[&str] = &[
    "get", "list", "search", "find", "read", "fetch", "lookup", "describe", "show", "check",
    "view", "query", "count", "status", "quote", "inspect", "browse", "preview", "download",
    "export", "echo", "ping", "watch", "retrieve",
];
const CREATE: &[&str] = &[
    "create", "add", "new", "insert", "post", "submit", "send", "place", "buy", "sell", "trade",
    "upload", "register", "invite", "book", "schedule", "start", "launch", "make", "issue",
    "transfer", "pay", "deposit", "withdraw", "comment", "reply", "publish", "import", "fork",
    "clone",
];
const UPDATE: &[&str] = &[
    "update",
    "edit",
    "set",
    "modify",
    "patch",
    "rename",
    "move",
    "change",
    "enable",
    "disable",
    "assign",
    "replace",
    "save",
    "write",
    "mark",
    "toggle",
    "apply",
    "approve",
    "reject",
    "merge",
    "close",
    "reopen",
    "restore",
    "configure",
    "upsert",
    "sync",
    "lock",
    "unlock",
];
const DELETE: &[&str] = &[
    "delete",
    "remove",
    "cancel",
    "destroy",
    "drop",
    "clear",
    "revoke",
    "purge",
    "erase",
    "unregister",
    "wipe",
    "archive",
];

/// Words of a tool name: `get_quote`, `placeOrder`, `orders.cancel` → `get quote`, `place order`, `orders cancel`.
fn words(name: &str) -> Vec<String> {
    let mut out = vec![];
    let mut cur = String::new();
    let mut prev_lower = false;
    for c in name.chars() {
        if !c.is_alphanumeric() {
            if !cur.is_empty() {
                out.push(std::mem::take(&mut cur));
            }
            prev_lower = false;
            continue;
        }
        if c.is_uppercase() && prev_lower && !cur.is_empty() {
            out.push(std::mem::take(&mut cur));
        }
        prev_lower = c.is_lowercase() || c.is_ascii_digit();
        cur.extend(c.to_lowercase());
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// The action named by the first verb in the tool's name, if any.
pub fn from_name(name: &str) -> Option<ToolAction> {
    words(name).iter().find_map(|w| {
        let w = w.as_str();
        if READ.contains(&w) {
            Some(ToolAction::Read)
        } else if DELETE.contains(&w) {
            Some(ToolAction::Delete)
        } else if UPDATE.contains(&w) {
            Some(ToolAction::Update)
        } else if CREATE.contains(&w) {
            Some(ToolAction::Create)
        } else {
            None
        }
    })
}

/// Classify `tool`. `ai` is an AI reading of its description, when enabled and available.
pub fn classify(tool: &Tool, ai: Option<ToolAction>) -> Option<Classified> {
    let a = tool.annotations.clone().unwrap_or_default();
    let refine = ai.or_else(|| from_name(&tool.name));
    let server = |action| {
        Some(Classified {
            action,
            source: ActionSource::Server,
        })
    };
    if a.read_only_hint == Some(true) {
        return server(ToolAction::Read);
    }
    // The hints say "destructive" or "additive" but not which kind of write; the
    // description or name can say which.
    if a.destructive_hint == Some(true) {
        return server(
            refine
                .filter(|x| matches!(x, ToolAction::Update | ToolAction::Delete))
                .unwrap_or(ToolAction::Delete),
        );
    }
    if a.destructive_hint == Some(false) {
        return server(
            refine
                .filter(|x| matches!(x, ToolAction::Create | ToolAction::Update))
                .unwrap_or(ToolAction::Create),
        );
    }
    if a.read_only_hint == Some(false) {
        return server(refine.filter(|x| x.writes()).unwrap_or(ToolAction::Update));
    }
    if let Some(action) = ai {
        return Some(Classified {
            action,
            source: ActionSource::Ai,
        });
    }
    from_name(&tool.name).map(|action| Classified {
        action,
        source: ActionSource::Name,
    })
}

/// Actions for tools of a server with no AI reading (ad-hoc connections): hints, then names.
pub fn classify_all(tools: &[Tool]) -> std::collections::HashMap<String, Classified> {
    tools
        .iter()
        .filter_map(|t| classify(t, None).map(|c| (t.name.clone(), c)))
        .collect()
}

/// The prompt asking an AI provider to classify `tools` from their descriptions.
pub fn classify_prompt(tools: &[&Tool]) -> String {
    let mut s = String::from(
        "Classify each MCP tool below by what calling it does, based on its description (use the name only as a hint).\n\
         Answer with one word per tool: read (only looks things up), create (adds or submits something new, \
         such as placing an order or sending a message), update (changes something that exists), or delete \
         (removes or cancels something).\n\
         Reply with only a JSON object mapping each tool name to its word, for example {\"get_quote\":\"read\"}.\n\nTools:\n",
    );
    for t in tools {
        let desc = t
            .description
            .as_deref()
            .unwrap_or("")
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        let desc: String = desc.chars().take(600).collect();
        s.push_str(&format!(
            "- {}: {}\n",
            t.name,
            if desc.is_empty() {
                "(no description)"
            } else {
                &desc
            }
        ));
    }
    s
}

/// Parse the AI's answer: the first JSON object in the text, `{name: word}`.
pub fn parse_classification(text: &str) -> std::collections::HashMap<String, ToolAction> {
    let (Some(start), Some(end)) = (text.find('{'), text.rfind('}')) else {
        return Default::default();
    };
    let Ok(serde_json::Value::Object(m)) =
        serde_json::from_str::<serde_json::Value>(&text[start..=end])
    else {
        return Default::default();
    };
    m.into_iter()
        .filter_map(|(k, v)| v.as_str().and_then(ToolAction::parse).map(|a| (k, a)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn tool(name: &str, annotations: serde_json::Value) -> Tool {
        serde_json::from_value(json!({ "name": name, "annotations": annotations })).unwrap()
    }

    #[test]
    fn names_with_a_verb() {
        assert_eq!(from_name("get_quote"), Some(ToolAction::Read));
        assert_eq!(from_name("placeOrder"), Some(ToolAction::Create));
        assert_eq!(from_name("orders.cancel"), Some(ToolAction::Delete));
        assert_eq!(from_name("set-watchlist"), Some(ToolAction::Update));
        assert_eq!(
            from_name("list_orders"),
            Some(ToolAction::Read),
            "the first verb wins"
        );
        assert_eq!(from_name("portfolio"), None);
        assert_eq!(from_name("ask_llm"), None);
    }

    #[test]
    fn server_hints_first_then_ai_then_name() {
        let plain = tool("portfolio", json!(null));
        assert_eq!(
            classify(&plain, None),
            None,
            "no hints, no AI, no verb: unknown"
        );
        assert_eq!(
            classify(&plain, Some(ToolAction::Read)),
            Some(Classified {
                action: ToolAction::Read,
                source: ActionSource::Ai
            })
        );
        // AI wins over the name.
        let named = tool("get_order", json!(null));
        assert_eq!(
            classify(&named, Some(ToolAction::Create)).unwrap().action,
            ToolAction::Create
        );
        assert_eq!(
            classify(&named, None),
            Some(Classified {
                action: ToolAction::Read,
                source: ActionSource::Name
            })
        );
        // Server hints win over both.
        let ro = tool("delete_everything", json!({"readOnlyHint": true}));
        assert_eq!(
            classify(&ro, Some(ToolAction::Delete)).unwrap(),
            Classified {
                action: ToolAction::Read,
                source: ActionSource::Server
            }
        );
        // Destructive is update or delete; the name or AI says which.
        assert_eq!(
            classify(&tool("set_price", json!({"destructiveHint": true})), None)
                .unwrap()
                .action,
            ToolAction::Update
        );
        assert_eq!(
            classify(&tool("thing", json!({"destructiveHint": true})), None)
                .unwrap()
                .action,
            ToolAction::Delete
        );
        assert_eq!(
            classify(
                &tool(
                    "thing",
                    json!({"readOnlyHint": false, "destructiveHint": false})
                ),
                None
            )
            .unwrap()
            .action,
            ToolAction::Create
        );
        // A title alone is not a hint.
        assert_eq!(classify(&tool("thing", json!({"title": "x"})), None), None);
    }

    #[test]
    fn parses_the_ai_answer() {
        let m = parse_classification(
            "Sure:\n```json\n{\"get_quote\": \"read\", \"place_order\": \"Create\", \"x\": \"maybe\"}\n```",
        );
        assert_eq!(m.get("get_quote"), Some(&ToolAction::Read));
        assert_eq!(m.get("place_order"), Some(&ToolAction::Create));
        assert!(!m.contains_key("x"));
        assert!(parse_classification("no json here").is_empty());
    }
}
