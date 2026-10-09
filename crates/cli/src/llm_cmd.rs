//! `lsock llm ...` — AI providers, chat with MCP tools, saved AI requests.

use std::io::{IsTerminal, Write};

use anyhow::{Context as _, Result, anyhow, bail};
use clap::{Args, Subcommand};
use futures::future::BoxFuture;
use lsock_core::{KeySource, LlmPromptMessage, LlmProvider, LlmRequest, McpServer, Workspace};
use lsock_engine::Engine;
use lsock_engine::llm::{LlmPrepared, parse_kind};
use lsock_llm::agent::{
    AgentEvent, AgentOptions, AllowAll, Approval, Approver, AutoApprove, ToolCallInfo, ToolSource,
};
use lsock_llm::{ChatRequest, Message, StreamEvent};

use crate::out::*;

#[derive(Subcommand, Debug)]
pub enum LlmCmd {
    /// Manage AI providers
    #[command(subcommand)]
    Provider(ProviderCmd),
    /// List models available to a provider's key
    Models { provider: String },
    /// Ask a model something; optionally give it MCP servers as tools
    Chat(ChatArgs),
    /// Saved AI requests
    #[command(subcommand)]
    Request(RequestCmd),
    /// Run a scripted mock LLM (Anthropic + OpenAI formats, key `test-key`) for offline demos
    MockServer {
        #[arg(long, default_value_t = 8787)]
        port: u16,
    },
}

#[derive(Subcommand, Debug)]
pub enum ProviderCmd {
    /// Add a provider. The key goes to the OS keychain (or use --key-env / --key-template / --no-key)
    Add {
        name: String,
        /// anthropic | openai | ollama | openai-compatible
        #[arg(long, default_value = "anthropic")]
        kind: String,
        /// Default model id
        #[arg(long)]
        model: Option<String>,
        #[arg(long)]
        base_url: Option<String>,
        /// Read the API key from this environment variable at call time
        #[arg(long, conflicts_with_all = ["key_template", "no_key"])]
        key_env: Option<String>,
        /// Render the key from the workspace environment, e.g. '{{ _.openai_key }}'
        #[arg(long, conflicts_with = "no_key")]
        key_template: Option<String>,
        /// Provider needs no key (local Ollama)
        #[arg(long)]
        no_key: bool,
    },
    List,
    Remove {
        provider: String,
    },
}

#[derive(Args, Debug)]
pub struct ChatArgs {
    pub prompt: String,
    /// Provider name or id (default: the first one)
    #[arg(short, long)]
    pub provider: Option<String>,
    #[arg(short, long)]
    pub model: Option<String>,
    #[arg(short, long)]
    pub system: Option<String>,
    /// Saved MCP server (name or id) to use as tools; repeatable
    #[arg(long = "mcp")]
    pub mcp: Vec<String>,
    /// Ad-hoc MCP server URL (Streamable HTTP); repeatable
    #[arg(long = "mcp-url")]
    pub mcp_urls: Vec<String>,
    /// Ad-hoc stdio MCP server command; repeatable
    #[arg(long = "mcp-stdio")]
    pub mcp_stdio: Vec<String>,
    /// Run every tool call without asking
    #[arg(short, long)]
    pub yes: bool,
    #[arg(long, default_value_t = 8)]
    pub max_turns: u32,
    #[arg(long, default_value_t = 4096)]
    pub max_tokens: u32,
    /// Print the transcript as JSON at the end
    #[arg(long)]
    pub json: bool,
}

#[derive(Subcommand, Debug)]
pub enum RequestCmd {
    /// Save an AI request in a workspace
    Add {
        workspace: String,
        name: String,
        #[arg(long)]
        prompt: String,
        #[arg(long)]
        system: Option<String>,
        #[arg(short, long)]
        provider: Option<String>,
        #[arg(short, long)]
        model: Option<String>,
        #[arg(long = "mcp")]
        mcp: Vec<String>,
        /// none | read-only | all
        #[arg(long, default_value = "read-only")]
        auto_approve: String,
    },
    /// Run a saved AI request (history is stored, like HTTP responses)
    Run {
        request: String,
        #[arg(short, long)]
        yes: bool,
    },
}

/// Asks on the terminal; denies when there is no terminal.
struct TerminalApprover;

impl Approver for TerminalApprover {
    fn approve(&self, call: ToolCallInfo) -> BoxFuture<'static, Approval> {
        Box::pin(async move {
            if !std::io::stdin().is_terminal() {
                return Approval::Deny("no terminal to confirm (use --yes)".into());
            }
            tokio::task::spawn_blocking(move || {
                let kind = match call.hints {
                    Some(h) if h.destructive => red("destructive"),
                    Some(h) if h.read_only => green("read-only"),
                    _ => yellow("may modify state"),
                };
                eprint!(
                    "\n  {} run {}.{} ({kind}) with {}? [y/N] ",
                    yellow("?"),
                    call.server,
                    bold(&call.tool),
                    call.input
                );
                let _ = std::io::stderr().flush();
                let mut line = String::new();
                let _ = std::io::stdin().read_line(&mut line);
                if matches!(line.trim().to_lowercase().as_str(), "y" | "yes") {
                    Approval::Allow
                } else {
                    Approval::Deny(String::new())
                }
            })
            .await
            .unwrap_or(Approval::Deny("prompt failed".into()))
        })
    }
}

fn find_provider(engine: &Engine, needle: Option<&str>) -> Result<lsock_core::Doc<LlmProvider>> {
    match needle {
        Some(n) => crate::find::<LlmProvider>(engine, n),
        None => engine
            .store
            .all_of::<LlmProvider>()?
            .into_iter()
            .next()
            .ok_or_else(|| anyhow!("no AI provider yet — add one: lsock llm provider add Claude --kind anthropic --model claude-sonnet-5-5")),
    }
}

/// Live terminal rendering of agent events.
fn printer() -> impl FnMut(AgentEvent) + Send {
    let mut in_text = false;
    move |ev| match ev {
        AgentEvent::Stream {
            event: StreamEvent::TextDelta { text },
            ..
        } => {
            print!("{text}");
            let _ = std::io::stdout().flush();
            in_text = true;
        }
        AgentEvent::ToolCall { call, .. } => {
            if in_text {
                println!();
                in_text = false;
            }
            println!(
                "\n  {} {}{} {}",
                cyan("⚙"),
                dim(&format!("{}.", call.server)),
                bold(&call.tool),
                dim(&call.input.to_string())
            );
        }
        AgentEvent::ToolResult { result, .. } => {
            let first = result
                .text
                .lines()
                .next()
                .unwrap_or("")
                .chars()
                .take(160)
                .collect::<String>();
            let mark = if result.denied {
                yellow("⊘ denied")
            } else if result.is_error {
                red("✗")
            } else {
                green("✓")
            };
            println!(
                "    {mark} {} {}\n",
                first,
                dim(&format!("({:.0} ms)", result.latency_ms))
            );
        }
        AgentEvent::AssistantMessage { .. } => {
            if in_text {
                println!();
                in_text = false;
            }
        }
        AgentEvent::Error { message } => eprintln!("{} {message}", red("error:")),
        _ => {}
    }
}

fn usage_line(
    model: &str,
    input: u64,
    output: u64,
    turns: u32,
    ms: f64,
    ttft: Option<f64>,
) -> String {
    let ttft = ttft
        .map(|t| format!(" · first token {t:.0} ms"))
        .unwrap_or_default();
    dim(&format!(
        "{model} · {input} in / {output} out tokens · {turns} turn{} · {ms:.0} ms{ttft}",
        if turns == 1 { "" } else { "s" }
    ))
}

pub async fn run(engine: &Engine, cmd: LlmCmd) -> Result<()> {
    match cmd {
        LlmCmd::MockServer { port } => {
            let url = lsock_llm::mock::spawn(Default::default(), port).await?;
            eprintln!(
                "mock LLM listening on {url} (API key: {})",
                lsock_llm::mock::MOCK_KEY
            );
            eprintln!(
                "  lsock llm provider add Mock --kind anthropic --base-url {url} --model mock-large"
            );
            tokio::signal::ctrl_c().await?;
        }
        LlmCmd::Provider(ProviderCmd::Add {
            name,
            kind,
            model,
            base_url,
            key_env,
            key_template,
            no_key,
        }) => {
            let k = parse_kind(&kind);
            if !["anthropic", "openai", "ollama", "openai-compatible"].contains(&kind.as_str()) {
                bail!("unknown kind '{kind}' (anthropic, openai, ollama, openai-compatible)");
            }
            let key_source = if no_key
                || (k == lsock_llm::ProviderKind::Ollama
                    && key_env.is_none()
                    && key_template.is_none())
            {
                KeySource::None
            } else if let Some(var) = key_env {
                KeySource::Env { var }
            } else if let Some(template) = key_template {
                KeySource::Template { template }
            } else {
                KeySource::Keychain
            };
            let p = engine.store.insert(
                None,
                LlmProvider {
                    name: name.clone(),
                    kind: kind.clone(),
                    base_url: base_url.unwrap_or_default(),
                    key_source: key_source.clone(),
                    default_model: model.unwrap_or_else(|| {
                        k.suggested_models()
                            .first()
                            .copied()
                            .unwrap_or("")
                            .to_string()
                    }),
                    headers: vec![],
                },
            )?;
            if key_source == KeySource::Keychain {
                eprint!("API key for {name} (stored in the OS keychain): ");
                let _ = std::io::stderr().flush();
                let mut key = String::new();
                std::io::stdin().read_line(&mut key)?;
                let key = key.trim();
                if key.is_empty() {
                    engine.store.delete(p.id())?;
                    bail!("no key entered; provider not added");
                }
                engine
                    .secrets()
                    .set(p.id(), key)
                    .map_err(|e| anyhow!("keychain: {e}"))?;
            }
            println!(
                "{} {} {} {}",
                green("✓"),
                bold(&p.name),
                dim(&format!("{kind} · {}", p.default_model)),
                dim(p.id())
            );
        }
        LlmCmd::Provider(ProviderCmd::List) => {
            let rows: Vec<Vec<String>> = engine
                .store
                .all_of::<LlmProvider>()?
                .into_iter()
                .map(|p| {
                    let key = match &p.key_source {
                        KeySource::None => "none".into(),
                        KeySource::Keychain => {
                            if engine.secrets().get(p.id()).is_some() {
                                "keychain ✓".into()
                            } else {
                                "keychain (missing)".into()
                            }
                        }
                        KeySource::Env { var } => format!("${var}"),
                        KeySource::Template { template } => template.clone(),
                    };
                    vec![
                        bold(&p.name),
                        p.kind.clone(),
                        p.default_model.clone(),
                        key,
                        dim(p.id()),
                    ]
                })
                .collect();
            print!(
                "{}",
                table(&["NAME", "KIND", "DEFAULT MODEL", "KEY", "ID"], &rows, 0)
            );
        }
        LlmCmd::Provider(ProviderCmd::Remove { provider }) => {
            let p = crate::find::<LlmProvider>(engine, &provider)?;
            let _ = engine.secrets().delete(p.id());
            engine.store.delete(p.id())?;
            println!("{} removed {}", green("✓"), p.name);
        }
        LlmCmd::Models { provider } => {
            let p = crate::find::<LlmProvider>(engine, &provider)?;
            let (_, cfg) = engine.provider_config(p.id(), None)?;
            for m in lsock_llm::list_models(&cfg).await? {
                let mark = if m == p.default_model {
                    green(" (default)")
                } else {
                    String::new()
                };
                println!("{m}{mark}");
            }
        }
        LlmCmd::Chat(a) => {
            let p = find_provider(engine, a.provider.as_deref())?;
            let (provider, config) = engine.provider_config(p.id(), None)?;
            let model = a.model.clone().unwrap_or(provider.default_model.clone());
            let mut ids = vec![];
            for m in &a.mcp {
                ids.push(crate::find::<McpServer>(engine, m)?.meta.id);
            }
            let mut sources: Vec<ToolSource> = engine.connect_tool_sources(&ids).await?;
            for (i, url) in a.mcp_urls.iter().enumerate() {
                let c = lsock_mcp::Client::connect(lsock_mcp::ConnectOptions::new(
                    lsock_mcp::TransportConfig::Http {
                        url: url.clone(),
                        headers: vec![],
                        validate_certificates: true,
                    },
                ))
                .await
                .with_context(|| format!("connecting to {url}"))?;
                let tools = c.list_tools().await.map(|l| l.items).unwrap_or_default();
                sources.push(ToolSource {
                    server_id: url.clone(),
                    server_name: format!("mcp{}", i + 1),
                    client: std::sync::Arc::new(c),
                    actions: lsock_mcp::action::classify_all(&tools),
                    tools,
                });
            }
            for cmd in &a.mcp_stdio {
                let (command, args) = crate::mcp_cmd::split_command(cmd)?;
                let c = lsock_mcp::Client::connect(lsock_mcp::ConnectOptions::new(
                    lsock_mcp::TransportConfig::Stdio {
                        command: command.clone(),
                        args,
                        env: Default::default(),
                        cwd: None,
                    },
                ))
                .await
                .with_context(|| format!("starting {cmd}"))?;
                let tools = c.list_tools().await.map(|l| l.items).unwrap_or_default();
                let name = std::path::Path::new(&command)
                    .file_name()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or(command);
                sources.push(ToolSource {
                    server_id: cmd.clone(),
                    server_name: name,
                    client: std::sync::Arc::new(c),
                    actions: lsock_mcp::action::classify_all(&tools),
                    tools,
                });
            }
            if !sources.is_empty() {
                let n: usize = sources.iter().map(|s| s.tools.len()).sum();
                eprintln!(
                    "{}",
                    dim(&format!("{n} tools from {} MCP server(s)", sources.len()))
                );
            }
            let prepared = LlmPrepared {
                provider,
                config,
                chat: ChatRequest {
                    model,
                    system: a.system.clone(),
                    messages: vec![Message::user(&a.prompt)],
                    tools: vec![],
                    max_tokens: a.max_tokens,
                    temperature: None,
                },
                options: AgentOptions {
                    max_turns: a.max_turns,
                    auto_approve: if a.yes {
                        AutoApprove::All
                    } else {
                        AutoApprove::ReadOnly
                    },
                    ..Default::default()
                },
                mcp_server_ids: vec![],
            };
            let model = prepared.chat.model.clone();
            let approver: Box<dyn Approver> = if a.yes {
                Box::new(AllowAll)
            } else {
                Box::new(TerminalApprover)
            };
            let mut print = printer();
            let out = lsock_llm::agent::run(
                &prepared.config,
                prepared.chat,
                &sources,
                &prepared.options,
                approver.as_ref(),
                &mut |e| print(e),
            )
            .await;
            for s in &sources {
                s.client.close().await;
            }
            println!();
            eprintln!(
                "{}",
                usage_line(
                    &model,
                    out.usage.input_tokens,
                    out.usage.output_tokens,
                    out.turns,
                    out.total_ms,
                    None
                )
            );
            if a.json {
                println!("{}", serde_json::to_string_pretty(&out.messages)?);
            }
            if let Some(e) = out.error {
                bail!(e);
            }
        }
        LlmCmd::Request(RequestCmd::Add {
            workspace,
            name,
            prompt,
            system,
            provider,
            model,
            mcp,
            auto_approve,
        }) => {
            let ws = crate::find::<Workspace>(engine, &workspace)?;
            let provider_id = match provider {
                Some(p) => Some(crate::find::<LlmProvider>(engine, &p)?.meta.id),
                None => None,
            };
            let mut ids = vec![];
            for m in &mcp {
                ids.push(crate::find::<McpServer>(engine, m)?.meta.id);
            }
            let r = engine.store.insert(
                Some(ws.id()),
                LlmRequest {
                    name,
                    provider_id,
                    model: model.unwrap_or_default(),
                    system: system.unwrap_or_default(),
                    messages: vec![LlmPromptMessage {
                        role: "user".into(),
                        text: prompt,
                    }],
                    mcp_server_ids: ids,
                    auto_approve,
                    ..Default::default()
                },
            )?;
            println!("{} {} {}", green("✓"), bold(&r.name), dim(r.id()));
        }
        LlmCmd::Request(RequestCmd::Run { request, yes }) => {
            let r = crate::find::<LlmRequest>(engine, &request)?;
            let approver: Box<dyn Approver> = if yes {
                Box::new(AllowAll)
            } else {
                Box::new(TerminalApprover)
            };
            let mut print = printer();
            let run = engine
                .llm_run(r.id(), approver.as_ref(), &mut |e| print(e))
                .await?;
            println!();
            eprintln!(
                "{}",
                usage_line(
                    &run.model,
                    run.input_tokens,
                    run.output_tokens,
                    run.turns,
                    run.total_ms,
                    run.ttft_ms
                )
            );
            if let Some(e) = &run.error {
                bail!("{e}");
            }
        }
    }
    Ok(())
}
