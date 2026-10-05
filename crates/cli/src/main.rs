//! `irs` — insomnia-rs command line.

mod mcp_cmd;
mod out;
mod run_cmd;

use anyhow::{Context as _, Result, anyhow, bail};
use clap::{Parser, Subcommand};
use irs_core::{Doc, Environment, Folder, McpServer, Model, Request, Workspace};
use irs_engine::Engine;
use out::*;
use serde_json::Value;

#[derive(Parser, Debug)]
#[command(
    name = "irs",
    version,
    about = "insomnia-rs: API client for HTTP and MCP",
    propagate_version = true
)]
struct Cli {
    /// Data directory (default: $IRS_DATA_DIR or the platform app-data dir; shared with the desktop app)
    #[arg(long, global = true, env = "IRS_DATA_DIR")]
    data_dir: Option<std::path::PathBuf>,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand, Debug)]
enum Cmd {
    /// Send a saved request and print the response
    Send {
        /// Request id or name
        request: String,
        /// Activate this sub-environment (name or id) for the send
        #[arg(long, short)]
        env: Option<String>,
        /// Include response headers
        #[arg(long, short)]
        include: bool,
        /// Print the full timeline (request + response, like curl -v)
        #[arg(long, short)]
        verbose: bool,
        /// Print the stored response as JSON
        #[arg(long)]
        json: bool,
    },
    /// Workspaces
    #[command(subcommand)]
    Workspace(WorkspaceCmd),
    /// Requests and folders
    #[command(subcommand)]
    Request(RequestCmd),
    /// Environments
    #[command(subcommand)]
    Env(EnvCmd),
    /// MCP servers: inspect tools, resources, prompts; call tools
    #[command(subcommand)]
    Mcp(mcp_cmd::McpCmd),
    /// Render a template string in the context of a workspace/folder/request
    Render { target: String, template: String },
    /// Run a collection or folder (scripts, tests, iterations) — like `inso run collection`
    #[command(subcommand)]
    Run(run_cmd::RunCmd),
}

#[derive(Subcommand, Debug)]
enum WorkspaceCmd {
    List,
    Create {
        name: String,
    },
    /// Print the tree of folders, requests and MCP servers
    Tree {
        workspace: String,
    },
}

#[derive(Subcommand, Debug)]
enum RequestCmd {
    /// Create a request: `irs request add <workspace> "Get users" GET https://...`
    Add {
        /// Workspace or folder (id or name)
        parent: String,
        name: String,
        method: String,
        url: String,
        #[arg(long = "header", short = 'H')]
        headers: Vec<String>,
        /// JSON body
        #[arg(long)]
        json: Option<String>,
    },
    /// Create a request from a curl command
    Curl { parent: String, command: String },
    /// Create a folder
    Folder { parent: String, name: String },
    /// Show recent responses for a request
    History { request: String },
    /// Attach pre-request / after-response scripts (from files) to a request or folder
    Script {
        /// Request or folder (name or id)
        target: String,
        #[arg(long)]
        pre: Option<std::path::PathBuf>,
        #[arg(long)]
        after: Option<std::path::PathBuf>,
    },
}

#[derive(Subcommand, Debug)]
enum EnvCmd {
    /// List base + sub-environments for a workspace
    List { workspace: String },
    /// Set a variable (in the base environment unless --env is given)
    Set {
        workspace: String,
        key: String,
        /// Value; parsed as JSON when possible, otherwise a string
        value: String,
        #[arg(long, short)]
        env: Option<String>,
    },
    /// Create a sub-environment
    Create { workspace: String, name: String },
    /// Make a sub-environment active (use "none" to clear)
    Use { workspace: String, env: String },
}

/// Find a document by exact id, or by case-insensitive name.
pub fn find<T: Model>(engine: &Engine, needle: &str) -> Result<Doc<T>> {
    if let Ok(d) = engine.store.get::<T>(needle) {
        return Ok(d);
    }
    let all = engine.store.all_of::<T>()?;
    let matches: Vec<Doc<T>> = all
        .into_iter()
        .filter(|d| {
            serde_json::to_value(&d.body)
                .ok()
                .and_then(|v| v["name"].as_str().map(|n| n.eq_ignore_ascii_case(needle)))
                .unwrap_or(false)
        })
        .collect();
    match matches.len() {
        1 => Ok(matches.into_iter().next().unwrap()),
        0 => bail!("no {} named or with id '{needle}'", T::TYPE),
        n => bail!("{n} {}s are named '{needle}' — use the id instead", T::TYPE),
    }
}

fn find_parent(engine: &Engine, needle: &str) -> Result<String> {
    if let Ok(f) = find::<Folder>(engine, needle) {
        return Ok(f.meta.id);
    }
    Ok(find::<Workspace>(engine, needle)
        .context("parent must be a workspace or folder")?
        .meta
        .id)
}

fn method_color(m: &str) -> String {
    match m {
        "GET" => green(m),
        "POST" => yellow(m),
        "PUT" | "PATCH" => blue(m),
        "DELETE" => red(m),
        _ => cyan(m),
    }
}

fn status_color(code: u16) -> String {
    let s = code.to_string();
    match code {
        200..=299 => green(&s),
        300..=399 => cyan(&s),
        400..=499 => yellow(&s),
        _ => red(&s),
    }
}

fn human_bytes(n: u64) -> String {
    match n {
        0..1024 => format!("{n} B"),
        1024..1_048_576 => format!("{:.1} KB", n as f64 / 1024.0),
        _ => format!("{:.1} MB", n as f64 / 1_048_576.0),
    }
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    if let Err(e) = run(cli).await {
        eprintln!("{} {e:#}", red("error:"));
        std::process::exit(2);
    }
}

async fn run(cli: Cli) -> Result<()> {
    let dir = cli
        .data_dir
        .clone()
        .unwrap_or_else(irs_engine::default_data_dir);
    let engine =
        Engine::open(dir.clone()).with_context(|| format!("opening data dir {}", dir.display()))?;
    match cli.cmd {
        Cmd::Mcp(c) => mcp_cmd::run(&engine, c).await,
        Cmd::Run(c) => run_cmd::run(&engine, c).await,
        Cmd::Send {
            request,
            env,
            include,
            verbose,
            json,
        } => send(&engine, &request, env, include, verbose, json).await,
        Cmd::Workspace(c) => workspace(&engine, c),
        Cmd::Request(c) => request_cmd(&engine, c),
        Cmd::Env(c) => env_cmd(&engine, c),
        Cmd::Render { target, template } => {
            let id = find_any(&engine, &target)?;
            let (r, refs) = engine.preview(&id, &template)?;
            for v in refs {
                let state = if v.resolved { green("✓") } else { red("✗") };
                println!(
                    "{state} {} {}",
                    v.name,
                    dim(&v.source.unwrap_or_else(|| "unresolved".into()))
                );
            }
            println!("{}", r.map_err(|e| anyhow!("{e}"))?);
            Ok(())
        }
    }
}

fn find_any(engine: &Engine, needle: &str) -> Result<String> {
    find::<Request>(engine, needle)
        .map(|d| d.meta.id)
        .or_else(|_| find::<Folder>(engine, needle).map(|d| d.meta.id))
        .or_else(|_| find::<Workspace>(engine, needle).map(|d| d.meta.id))
        .or_else(|_| find::<McpServer>(engine, needle).map(|d| d.meta.id))
        .map_err(|_| anyhow!("nothing named or with id '{needle}'"))
}

async fn send(
    engine: &Engine,
    request: &str,
    env: Option<String>,
    include: bool,
    verbose: bool,
    json: bool,
) -> Result<()> {
    let req = find::<Request>(engine, request)?;
    if let Some(env) = env {
        let mut ws = engine.workspace_of(req.id())?;
        let sub = engine
            .sub_environments(ws.id())?
            .into_iter()
            .find(|e| e.id() == env || e.name.eq_ignore_ascii_case(&env))
            .ok_or_else(|| anyhow!("no sub-environment '{env}' in workspace {}", ws.name))?;
        ws.active_environment_id = Some(sub.meta.id.clone());
        engine.store.update(&ws)?;
    }
    let resp = engine.send(req.id()).await?;
    if json {
        println!("{}", serde_json::to_string_pretty(&resp)?);
        return Ok(());
    }
    if verbose {
        for t in &resp.timeline {
            let line = match t.kind.as_str() {
                "header-out" => blue(&format!("> {}", t.text)),
                "data-out" => dim(&t.text),
                "header-in" => green(&format!("< {}", t.text)),
                "error" => red(&t.text),
                _ => dim(&format!("* {}", t.text)),
            };
            eprintln!("{line}");
        }
        eprintln!();
    }
    if let Some(err) = &resp.error {
        bail!("{} {} failed: {err}", resp.method, resp.url);
    }
    eprintln!(
        "{} {} {}  {}  {}  {}",
        method_color(&resp.method),
        resp.url,
        dim("→"),
        bold(&format!(
            "{} {}",
            status_color(resp.status_code),
            resp.status_message
        )),
        dim(&format!("{:.0} ms", resp.timings.total_ms)),
        dim(&human_bytes(resp.bytes)),
    );
    if include {
        for h in &resp.headers {
            eprintln!("{}: {}", cyan(&h.name), mask_header(&h.name, &h.value));
        }
        eprintln!();
    }
    let body = engine.response_body(&resp);
    match serde_json::from_slice::<Value>(&body) {
        Ok(v) if resp.content_type.contains("json") || v.is_object() || v.is_array() => {
            println!("{}", serde_json::to_string_pretty(&v)?)
        }
        _ => println!("{}", String::from_utf8_lossy(&body)),
    }
    if resp.status_code >= 400 {
        std::process::exit(1);
    }
    Ok(())
}

fn workspace(engine: &Engine, c: WorkspaceCmd) -> Result<()> {
    match c {
        WorkspaceCmd::List => {
            let rows: Vec<Vec<String>> = engine
                .store
                .all_of::<Workspace>()?
                .into_iter()
                .map(|w| {
                    let n = engine.request_ids_in(w.id()).map(|r| r.len()).unwrap_or(0);
                    vec![
                        bold(&w.name),
                        format!("{:?}", w.scope).to_lowercase(),
                        n.to_string(),
                        dim(w.id()),
                    ]
                })
                .collect();
            print!("{}", table(&["NAME", "SCOPE", "REQUESTS", "ID"], &rows, 0));
        }
        WorkspaceCmd::Create { name } => {
            let w = engine.store.insert(
                None,
                Workspace {
                    name,
                    ..Default::default()
                },
            )?;
            engine.base_environment(w.id())?;
            println!("{} {} {}", green("✓"), bold(&w.name), dim(w.id()));
        }
        WorkspaceCmd::Tree { workspace } => {
            let w = find::<Workspace>(engine, &workspace)?;
            println!("{}", bold(&w.name));
            print_tree(engine, w.id(), 1)?;
        }
    }
    Ok(())
}

fn print_tree(engine: &Engine, parent: &str, depth: usize) -> Result<()> {
    let pad = "  ".repeat(depth);
    for d in engine.store.all_children(parent)? {
        let name = d.data["name"].as_str().unwrap_or("").to_string();
        match d.meta.kind.as_str() {
            "Folder" => {
                println!("{pad}{} {}", yellow("▸"), bold(&name));
                print_tree(engine, &d.meta.id, depth + 1)?;
            }
            "Request" => {
                let m = d.data["method"].as_str().unwrap_or("GET");
                println!("{pad}{:<7} {name} {}", method_color(m), dim(&d.meta.id));
            }
            "McpServer" => println!("{pad}{:<7} {name} {}", magenta("MCP"), dim(&d.meta.id)),
            _ => {}
        }
    }
    Ok(())
}

fn request_cmd(engine: &Engine, c: RequestCmd) -> Result<()> {
    match c {
        RequestCmd::Add {
            parent,
            name,
            method,
            url,
            headers,
            json,
        } => {
            let pid = find_parent(engine, &parent)?;
            let mut r = Request {
                name,
                method: method.to_uppercase(),
                url,
                ..Default::default()
            };
            for h in headers {
                let (k, v) = h
                    .split_once(':')
                    .ok_or_else(|| anyhow!("header '{h}' must look like 'Name: value'"))?;
                r.headers.push(irs_core::KeyValue::new(k.trim(), v.trim()));
            }
            if let Some(j) = json {
                r.body = irs_core::Body {
                    mime_type: Some(irs_core::mime::JSON.into()),
                    text: Some(j),
                    ..Default::default()
                };
            }
            let d = engine.store.insert(Some(&pid), r)?;
            println!("{} {} {}", green("✓"), bold(&d.name), dim(d.id()));
        }
        RequestCmd::Curl { parent, command } => {
            let pid = find_parent(engine, &parent)?;
            let r = irs_engine::curl::parse(&command)?;
            let d = engine.store.insert(Some(&pid), r)?;
            println!(
                "{} {} {} {}",
                green("✓"),
                method_color(&d.method),
                bold(&d.name),
                dim(d.id())
            );
        }
        RequestCmd::Folder { parent, name } => {
            let pid = find_parent(engine, &parent)?;
            let d = engine.store.insert(
                Some(&pid),
                Folder {
                    name,
                    ..Default::default()
                },
            )?;
            println!("{} {} {}", green("✓"), bold(&d.name), dim(d.id()));
        }
        RequestCmd::Script { target, pre, after } => {
            let read = |p: &Option<std::path::PathBuf>| -> Result<Option<String>> {
                p.as_ref()
                    .map(|p| {
                        std::fs::read_to_string(p)
                            .with_context(|| format!("reading {}", p.display()))
                    })
                    .transpose()
            };
            let (pre, after) = (read(&pre)?, read(&after)?);
            let found = find::<Request>(engine, &target);
            if let Err(e) = &found
                && e.to_string().contains("are named")
            {
                bail!("{e}");
            }
            if let Ok(mut r) = found {
                if pre.is_some() {
                    r.pre_request_script = pre;
                }
                if after.is_some() {
                    r.after_response_script = after;
                }
                engine.store.update(&r)?;
                println!("{} scripts updated on {}", green("✓"), bold(&r.name));
            } else {
                let mut f = find::<Folder>(engine, &target)
                    .context("target must be a request or folder")?;
                if pre.is_some() {
                    f.pre_request_script = pre;
                }
                if after.is_some() {
                    f.after_response_script = after;
                }
                engine.store.update(&f)?;
                println!("{} scripts updated on folder {}", green("✓"), bold(&f.name));
            }
        }
        RequestCmd::History { request } => {
            let r = find::<Request>(engine, &request)?;
            let rows: Vec<Vec<String>> = engine
                .responses(r.id())?
                .into_iter()
                .map(|x| {
                    let status = match &x.error {
                        Some(e) => red(&format!("error: {e}")),
                        None => format!("{} {}", status_color(x.status_code), x.status_message),
                    };
                    vec![
                        dim(&x.meta.id),
                        format!("{:.0} ms", x.timings.total_ms),
                        human_bytes(x.bytes),
                        status,
                    ]
                })
                .collect();
            print!("{}", table(&["ID", "TIME", "SIZE", "STATUS"], &rows, 0));
        }
    }
    Ok(())
}

fn env_cmd(engine: &Engine, c: EnvCmd) -> Result<()> {
    match c {
        EnvCmd::List { workspace } => {
            let ws = find::<Workspace>(engine, &workspace)?;
            let base = engine.base_environment(ws.id())?;
            let print_env = |e: &Doc<Environment>, active: bool| {
                let mark = if active { green("● ") } else { "  ".into() };
                println!("{mark}{} {}", bold(&e.name), dim(e.id()));
                let rows: Vec<Vec<String>> = e
                    .data
                    .iter()
                    .map(|(k, v)| {
                        let shown = if e.secret_keys.contains(k) {
                            "••••••".to_string()
                        } else {
                            v.to_string()
                        };
                        vec![k.clone(), shown]
                    })
                    .collect();
                if !rows.is_empty() {
                    print!("{}", table(&["KEY", "VALUE"], &rows, 4));
                }
            };
            print_env(&base, false);
            for s in engine.sub_environments(ws.id())? {
                print_env(&s, ws.active_environment_id.as_deref() == Some(s.id()));
            }
        }
        EnvCmd::Set {
            workspace,
            key,
            value,
            env,
        } => {
            let ws = find::<Workspace>(engine, &workspace)?;
            let mut target = match env {
                None => engine.base_environment(ws.id())?,
                Some(n) => engine
                    .sub_environments(ws.id())?
                    .into_iter()
                    .find(|e| e.id() == n || e.name.eq_ignore_ascii_case(&n))
                    .ok_or_else(|| anyhow!("no sub-environment '{n}'"))?,
            };
            let v: Value = serde_json::from_str(&value).unwrap_or(Value::String(value));
            target.data.insert(key.clone(), v);
            engine.store.update(&target)?;
            println!("{} {} {} {}", green("✓"), key, dim("in"), target.name);
        }
        EnvCmd::Create { workspace, name } => {
            let ws = find::<Workspace>(engine, &workspace)?;
            let base = engine.base_environment(ws.id())?;
            let e = engine.store.insert(
                Some(base.id()),
                Environment {
                    name,
                    ..Default::default()
                },
            )?;
            println!("{} {} {}", green("✓"), bold(&e.name), dim(e.id()));
        }
        EnvCmd::Use { workspace, env } => {
            let mut ws = find::<Workspace>(engine, &workspace)?;
            if env == "none" {
                ws.active_environment_id = None;
            } else {
                let sub = engine
                    .sub_environments(ws.id())?
                    .into_iter()
                    .find(|e| e.id() == env || e.name.eq_ignore_ascii_case(&env))
                    .ok_or_else(|| anyhow!("no sub-environment '{env}'"))?;
                ws.active_environment_id = Some(sub.meta.id.clone());
            }
            engine.store.update(&ws)?;
            println!("{} active environment: {env}", green("✓"));
        }
    }
    Ok(())
}
