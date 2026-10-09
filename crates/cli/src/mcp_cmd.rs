//! `lsock mcp ...` — connect to an MCP server and print readable results.

use std::collections::HashMap;

use anyhow::{Context as _, Result, anyhow, bail};
use clap::{Args, Subcommand};
use lsock_core::{McpServer, McpTransport};
use lsock_engine::Engine;
use lsock_mcp::schema::{self, ParamRow};
use lsock_mcp::{Client, ConnectOptions, Direction, FrameKind, LogEntry, Tool, TransportConfig};
use serde_json::Value;

use crate::out::*;

#[derive(Args, Debug, Clone)]
pub struct ServerArgs {
    /// Saved MCP server (id or name). Omit when using --url or --stdio.
    pub server: Option<String>,
    /// Streamable HTTP endpoint, e.g. http://localhost:3333/mcp
    #[arg(long, conflicts_with = "stdio")]
    pub url: Option<String>,
    /// stdio command line, e.g. "npx -y @modelcontextprotocol/server-everything"
    #[arg(long)]
    pub stdio: Option<String>,
    /// Extra HTTP header "Name: value" (repeatable)
    #[arg(long = "header", short = 'H')]
    pub headers: Vec<String>,
    /// Environment variable for stdio servers "KEY=value" (repeatable)
    #[arg(long = "env", short = 'e')]
    pub env: Vec<String>,
    /// Print the JSON-RPC protocol log after the command
    #[arg(long)]
    pub log: bool,
    /// With --log: include every full JSON frame
    #[arg(long)]
    pub log_full: bool,
    /// Request timeout in seconds
    #[arg(long, default_value_t = 60)]
    pub timeout: u64,
}

#[derive(Subcommand, Debug)]
pub enum McpCmd {
    /// Connect and show server info + capabilities
    Info(ServerArgs),
    /// List tools as readable cards with parameter tables
    Tools {
        #[command(flatten)]
        server: ServerArgs,
        /// Only tools whose name/title/description contains this text
        #[arg(long, short)]
        filter: Option<String>,
        /// One line per tool
        #[arg(long)]
        compact: bool,
        /// Raw JSON (all pages merged)
        #[arg(long)]
        json: bool,
    },
    /// Call a tool. Arguments are validated against the tool's input schema first.
    Call {
        #[command(flatten)]
        server: ServerArgs,
        /// Tool name
        #[arg(long, short)]
        tool: String,
        /// JSON object of arguments (default: {})
        #[arg(long, short, default_value = "{}")]
        args: String,
        /// Skip local schema validation
        #[arg(long)]
        no_validate: bool,
        #[arg(long)]
        json: bool,
    },
    /// List resources and resource templates
    Resources(ServerArgs),
    /// List prompts and their arguments
    Prompts(ServerArgs),
    /// Save an MCP server into a workspace
    Add {
        /// Workspace id or name
        workspace: String,
        name: String,
        #[arg(long, conflicts_with = "stdio")]
        url: Option<String>,
        #[arg(long)]
        stdio: Option<String>,
        #[arg(long = "header", short = 'H')]
        headers: Vec<String>,
    },
}

pub fn split_command(cmd: &str) -> Result<(String, Vec<String>)> {
    let words = lsock_engine::curl::split_words(cmd).map_err(|e| anyhow!("{e}"))?;
    let mut it = words.into_iter();
    let program = it.next().ok_or_else(|| anyhow!("empty --stdio command"))?;
    Ok((program, it.collect()))
}

fn parse_header(h: &str) -> Result<(String, String)> {
    let (k, v) = h
        .split_once(':')
        .ok_or_else(|| anyhow!("header '{h}' must look like 'Name: value'"))?;
    Ok((k.trim().to_string(), v.trim().to_string()))
}

async fn connect_options(engine: &Engine, a: &ServerArgs) -> Result<(String, ConnectOptions)> {
    let headers: Vec<(String, String)> = a
        .headers
        .iter()
        .map(|h| parse_header(h))
        .collect::<Result<_>>()?;
    let env: HashMap<String, String> = a
        .env
        .iter()
        .map(|e| {
            e.split_once('=')
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .ok_or_else(|| anyhow!("env '{e}' must be KEY=value"))
        })
        .collect::<Result<_>>()?;
    if let Some(url) = &a.url {
        let t = TransportConfig::Http {
            url: url.clone(),
            headers,
            validate_certificates: true,
        };
        return Ok((url.clone(), ConnectOptions::new(t)));
    }
    if let Some(cmd) = &a.stdio {
        let (command, args) = split_command(cmd)?;
        let t = TransportConfig::Stdio {
            command,
            args,
            env,
            cwd: None,
        };
        return Ok((cmd.clone(), ConnectOptions::new(t)));
    }
    let Some(target) = &a.server else {
        bail!("give a saved server name/id, --url or --stdio")
    };
    let server = crate::find::<McpServer>(engine, target)?;
    let mut opts = engine.mcp_connect_options(server.id()).await?;
    // Command-line headers/env add to the saved configuration.
    match &mut opts.transport {
        TransportConfig::Http { headers: h, .. } => h.extend(headers),
        TransportConfig::Stdio { env: e, .. } => e.extend(env),
    }
    Ok((server.name.clone(), opts))
}

async fn connect(engine: &Engine, a: &ServerArgs) -> Result<(String, Client)> {
    let (label, mut opts) = connect_options(engine, a).await?;
    opts.request_timeout = std::time::Duration::from_secs(a.timeout);
    let log = lsock_mcp::ProtocolLog::default();
    match Client::connect_with_log(opts, log.clone()).await {
        Ok(c) => Ok((label, c)),
        Err(e) => {
            if a.log {
                print_log(&log.entries(), a.log_full);
            }
            Err(anyhow!("could not connect to {label}: {e}"))
        }
    }
}

pub async fn run(engine: &Engine, cmd: McpCmd) -> Result<()> {
    let (client, args) = match &cmd {
        McpCmd::Add {
            workspace,
            name,
            url,
            stdio,
            headers,
        } => {
            let ws = crate::find::<lsock_core::Workspace>(engine, workspace)?;
            let transport = match (url, stdio) {
                (Some(u), _) => McpTransport::StreamableHttp { url: u.clone() },
                (None, Some(c)) => {
                    let (command, args) = split_command(c)?;
                    McpTransport::Stdio {
                        command,
                        args,
                        cwd: None,
                    }
                }
                _ => bail!("give --url or --stdio"),
            };
            let headers = headers
                .iter()
                .map(|h| parse_header(h).map(|(k, v)| lsock_core::KeyValue::new(k, v)))
                .collect::<Result<_>>()?;
            let s = engine.store.insert(
                Some(ws.id()),
                McpServer {
                    name: name.clone(),
                    transport,
                    headers,
                    ..Default::default()
                },
            )?;
            println!("{} {} {}", green("✓"), bold(&s.name), dim(s.id()));
            return Ok(());
        }
        McpCmd::Info(a) | McpCmd::Resources(a) | McpCmd::Prompts(a) => {
            (connect(engine, a).await?, a.clone())
        }
        McpCmd::Tools { server, .. } | McpCmd::Call { server, .. } => {
            (connect(engine, server).await?, server.clone())
        }
    };
    let (_label, client) = client;
    let result = run_connected(&client, &cmd).await;
    if args.log {
        print_log(&client.log().entries(), args.log_full);
    }
    client.close().await;
    result
}

async fn run_connected(client: &Client, cmd: &McpCmd) -> Result<()> {
    let info = client.server();
    match cmd {
        McpCmd::Info(_) => {
            print_server_header(client, None);
            if let Some(i) = &info.instructions {
                println!("\n{}", bold("Instructions"));
                for l in wrap(i, term_width() - 2) {
                    println!("  {l}");
                }
            }
            println!("\n{}", bold("Capabilities"));
            match info.capabilities.as_object() {
                Some(caps) if !caps.is_empty() => {
                    for (k, v) in caps {
                        let detail = v.as_object().filter(|o| !o.is_empty()).map(|o| {
                            o.iter()
                                .map(|(k, v)| format!("{k}={v}"))
                                .collect::<Vec<_>>()
                                .join(", ")
                        });
                        println!("  {} {k} {}", green("●"), dim(&detail.unwrap_or_default()));
                    }
                }
                _ => println!("  {}", dim("(none declared)")),
            }
            if let Some(s) = client.session_id() {
                println!("\n{} {}", dim("Session:"), s);
            }
        }
        McpCmd::Tools {
            filter,
            compact,
            json,
            ..
        } => {
            let listed = client.list_tools().await?;
            let needle = filter.as_deref().map(str::to_lowercase);
            let tools: Vec<&Tool> = listed
                .items
                .iter()
                .filter(|t| {
                    needle.as_ref().is_none_or(|n| {
                        t.name.to_lowercase().contains(n)
                            || t.display_name().to_lowercase().contains(n)
                            || t.description
                                .as_deref()
                                .unwrap_or("")
                                .to_lowercase()
                                .contains(n)
                    })
                })
                .collect();
            if *json {
                println!("{}", serde_json::to_string_pretty(&tools)?);
                return Ok(());
            }
            print_server_header(
                client,
                Some(format!(
                    "{} tool{} · {} page{} · {:.1} ms",
                    tools.len(),
                    if tools.len() == 1 { "" } else { "s" },
                    listed.pages,
                    if listed.pages == 1 { "" } else { "s" },
                    listed.elapsed_ms
                )),
            );
            println!();
            if *compact {
                let rows: Vec<Vec<String>> = tools
                    .iter()
                    .map(|t| {
                        vec![
                            bold(&t.name),
                            badges(t),
                            first_line(t.description.as_deref().unwrap_or("")),
                        ]
                    })
                    .collect();
                print!("{}", table(&["TOOL", "BEHAVIOR", "DESCRIPTION"], &rows, 2));
            } else {
                for (i, t) in tools.iter().enumerate() {
                    print_tool(i + 1, t);
                }
            }
        }
        McpCmd::Call {
            tool,
            args,
            no_validate,
            json,
            ..
        } => {
            let args: Value = serde_json::from_str(args).context("--args must be a JSON object")?;
            if !no_validate {
                let tools = client.list_tools().await?;
                let t = tools
                    .items
                    .iter()
                    .find(|t| &t.name == tool)
                    .ok_or_else(|| {
                        let names: Vec<&str> =
                            tools.items.iter().map(|t| t.name.as_str()).collect();
                        anyhow!("no tool named '{tool}'. Available: {}", names.join(", "))
                    })?;
                let errs = schema::validate(&t.input_schema, &args);
                if !errs.is_empty() {
                    eprintln!(
                        "{} arguments do not match {}'s input schema:",
                        red("✗"),
                        bold(tool)
                    );
                    for e in &errs {
                        eprintln!("  • {e}");
                    }
                    eprintln!(
                        "{}",
                        dim(&format!(
                            "Example: --args '{}'",
                            schema::example_args(&t.input_schema)
                        ))
                    );
                    bail!("invalid arguments (use --no-validate to send anyway)");
                }
            }
            let started = std::time::Instant::now();
            let r = client.call_tool(tool, args).await?;
            let ms = started.elapsed().as_secs_f64() * 1000.0;
            if *json {
                println!("{}", serde_json::to_string_pretty(&r)?);
                return Ok(());
            }
            let status = if r.is_error {
                red("✗ tool reported an error")
            } else {
                green("✓ success")
            };
            println!("{status} {}", dim(&format!("· {ms:.1} ms")));
            for c in &r.content {
                match c["type"].as_str() {
                    Some("text") => {
                        let text = c["text"].as_str().unwrap_or("");
                        match serde_json::from_str::<Value>(text) {
                            Ok(v) if v.is_object() || v.is_array() => {
                                println!("{}", serde_json::to_string_pretty(&v)?)
                            }
                            _ => println!("{text}"),
                        }
                    }
                    Some(kind) => println!(
                        "{} {}",
                        cyan(&format!("[{kind}]")),
                        dim(&truncate(&c.to_string(), 200))
                    ),
                    None => println!("{c}"),
                }
            }
            if let Some(s) = &r.structured_content {
                println!(
                    "\n{}\n{}",
                    bold("Structured content"),
                    serde_json::to_string_pretty(s)?
                );
            }
            let notes = client.notifications();
            if !notes.is_empty() {
                println!("\n{}", bold("Notifications"));
                for n in notes {
                    println!(
                        "  {} {}",
                        magenta(&n.method),
                        dim(&truncate(&n.params.to_string(), 160))
                    );
                }
            }
            if r.is_error {
                std::process::exit(1);
            }
        }
        McpCmd::Resources(_) => {
            print_server_header(client, None);
            let res = client.list_resources().await?;
            println!(
                "\n{} {}",
                bold("Resources"),
                dim(&format!("({})", res.items.len()))
            );
            let rows: Vec<Vec<String>> = res
                .items
                .iter()
                .map(|r| {
                    vec![
                        cyan(&r.uri),
                        r.mime_type.clone().unwrap_or_default(),
                        r.title
                            .clone()
                            .or(r.description.clone())
                            .unwrap_or_else(|| r.name.clone()),
                    ]
                })
                .collect();
            print!("{}", table(&["URI", "MIME", "NAME"], &rows, 2));
            if let Ok(t) = client.list_resource_templates().await
                && !t.items.is_empty()
            {
                println!(
                    "\n{} {}",
                    bold("Resource templates"),
                    dim(&format!("({})", t.items.len()))
                );
                let rows: Vec<Vec<String>> = t
                    .items
                    .iter()
                    .map(|r| {
                        vec![
                            cyan(&r.uri_template),
                            r.description.clone().unwrap_or_else(|| r.name.clone()),
                        ]
                    })
                    .collect();
                print!("{}", table(&["URI TEMPLATE", "DESCRIPTION"], &rows, 2));
            }
        }
        McpCmd::Prompts(_) => {
            print_server_header(client, None);
            let p = client.list_prompts().await?;
            println!();
            for prompt in &p.items {
                println!(
                    "{} {}",
                    bold(&prompt.name),
                    dim(prompt.title.as_deref().unwrap_or(""))
                );
                if let Some(d) = &prompt.description {
                    println!("  {d}");
                }
                if !prompt.arguments.is_empty() {
                    let rows: Vec<Vec<String>> = prompt
                        .arguments
                        .iter()
                        .map(|a| {
                            vec![
                                a.name.clone(),
                                if a.required { yellow("yes") } else { dim("no") },
                                a.description.clone().unwrap_or_default(),
                            ]
                        })
                        .collect();
                    print!(
                        "{}",
                        table(&["ARGUMENT", "REQUIRED", "DESCRIPTION"], &rows, 4)
                    );
                }
                println!();
            }
        }
        McpCmd::Add { .. } => unreachable!(),
    }
    Ok(())
}

fn print_server_header(client: &Client, extra: Option<String>) {
    let info = client.server();
    let name = info
        .server_info
        .title
        .clone()
        .unwrap_or_else(|| info.server_info.name.clone());
    let mut parts = vec![bold(&name)];
    if !info.server_info.version.is_empty() {
        parts.push(dim(&format!("v{}", info.server_info.version)));
    }
    parts.push(dim(&format!("· protocol {}", info.protocol_version)));
    if let Some(e) = extra {
        parts.push(dim(&format!("· {e}")));
    }
    println!("{}", parts.join(" "));
}

pub fn badges(t: &Tool) -> String {
    let h = t.hints();
    let mut b = vec![];
    if h.read_only {
        b.push(green("read-only"));
    } else if h.destructive {
        b.push(red("destructive"));
    } else {
        b.push(yellow("writes"));
    }
    if h.idempotent {
        b.push(blue("idempotent"));
    }
    b.push(if h.open_world {
        cyan("open-world")
    } else {
        dim("closed-world")
    });
    let s = b.join(dim(" · ").as_str());
    if h.declared {
        s
    } else {
        format!("{s} {}", dim("(unannotated)"))
    }
}

fn first_line(s: &str) -> String {
    s.lines()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("")
        .to_string()
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        format!("{}…", s.chars().take(n).collect::<String>())
    }
}

fn fmt_val(v: &Value) -> String {
    match v {
        Value::String(s) => format!("\"{s}\""),
        other => other.to_string(),
    }
}

pub fn param_details(r: &ParamRow) -> String {
    let mut parts = vec![];
    if let Some(d) = &r.description {
        parts.push(d.replace('\n', " "));
    }
    parts.extend(r.constraints.iter().cloned());
    if !r.enum_values.is_empty() {
        parts.push(format!(
            "one of: {}",
            r.enum_values
                .iter()
                .map(fmt_val)
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    parts.join(" · ")
}

fn print_tool(n: usize, t: &Tool) {
    let title = if t.display_name() != t.name {
        format!(" {}", dim(&format!("\"{}\"", t.display_name())))
    } else {
        String::new()
    };
    println!(
        "{} {}{}  {}",
        dim(&format!("{n:>2}")),
        bold(&cyan(&t.name)),
        title,
        badges(t)
    );
    if let Some(d) = &t.description {
        for l in wrap(d, term_width().saturating_sub(4)) {
            println!("   {l}");
        }
    }
    let rows = schema::param_rows(&t.input_schema);
    if rows.is_empty() {
        println!("   {}", dim("No parameters"));
    } else {
        let table_rows: Vec<Vec<String>> = rows
            .iter()
            .map(|r| {
                let name = if r.depth == 0 {
                    r.name.clone()
                } else {
                    format!("{}└ {}", "  ".repeat(r.depth - 1), r.name)
                };
                vec![
                    if r.required { bold(&name) } else { name },
                    r.type_label.clone(),
                    if r.required { yellow("yes") } else { dim("no") },
                    r.default.as_ref().map(fmt_val).unwrap_or_default(),
                    param_details(r),
                ]
            })
            .collect();
        print!(
            "{}",
            table(
                &["PARAMETER", "TYPE", "REQUIRED", "DEFAULT", "DETAILS"],
                &table_rows,
                3
            )
        );
    }
    if let Some(o) = &t.output_schema {
        let fields: Vec<String> = schema::param_rows(o)
            .into_iter()
            .filter(|r| r.depth == 0)
            .map(|r| format!("{}: {}", r.name, r.type_label))
            .collect();
        println!("   {} {{ {} }}", dim("Returns"), fields.join(", "));
    }
    println!();
}

pub fn print_log(entries: &[LogEntry], full: bool) {
    println!(
        "\n{} {}",
        bold("Protocol log"),
        dim(&format!("({} frames)", entries.len()))
    );
    for e in entries {
        let time = chrono_time(e.timestamp_ms);
        let arrow = match e.direction {
            Direction::Out => blue("→"),
            Direction::In => green("←"),
            Direction::Info => dim("·"),
            Direction::Stderr => yellow("!"),
            Direction::Error => red("✗"),
        };
        let kind = match e.kind {
            FrameKind::Request => "request",
            FrameKind::Response => "response",
            FrameKind::Notification => "notification",
            FrameKind::Error => "error",
            FrameKind::Other => "",
        };
        let what = match (&e.method, e.message.get("text")) {
            (Some(m), _) => m.clone(),
            (None, Some(t)) => t.as_str().unwrap_or("").to_string(),
            _ => String::new(),
        };
        let id =
            e.id.as_ref()
                .map(|i| dim(&format!("#{i}")))
                .unwrap_or_default();
        let lat = e
            .latency_ms
            .map(|l| yellow(&format!("{l:.1} ms")))
            .unwrap_or_default();
        let kind = if e.kind == FrameKind::Error {
            red(kind)
        } else {
            dim(kind)
        };
        println!("  {} {arrow} {:<12} {what} {id} {lat}", dim(&time), kind);
        if full && e.message.get("jsonrpc").is_some() {
            for l in serde_json::to_string_pretty(&e.message)
                .unwrap_or_default()
                .lines()
            {
                println!("      {}", dim(l));
            }
        }
    }
}

fn chrono_time(ms: i64) -> String {
    chrono::DateTime::from_timestamp_millis(ms)
        .map(|t| {
            t.with_timezone(&chrono::Local)
                .format("%H:%M:%S%.3f")
                .to_string()
        })
        .unwrap_or_default()
}
