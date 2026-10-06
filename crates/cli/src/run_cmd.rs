//! `lsock run collection` — flags mirror `inso run collection` where possible.

use anyhow::{Context as _, Result, anyhow, bail};
use clap::{Args, Subcommand};
use lsock_core::{Folder, Request, Workspace};
use lsock_engine::Engine;
use lsock_runner::{RunEvent, RunOptions, report};

use crate::out::*;

#[derive(Subcommand, Debug)]
pub enum RunCmd {
    /// Run every request in a collection or folder, in sidebar order
    Collection(CollectionArgs),
}

#[derive(Args, Debug)]
pub struct CollectionArgs {
    /// Workspace or folder (name or id)
    pub target: String,
    /// Sub-environment to activate (name or id)
    #[arg(short, long)]
    pub env: Option<String>,
    /// Only run these requests or folders (name or id); repeatable
    #[arg(short = 'i', long = "item")]
    pub items: Vec<String>,
    /// Number of iterations (default: number of data rows, or 1)
    #[arg(short = 'n', long = "iteration-count", alias = "iterations")]
    pub iterations: Option<u32>,
    /// CSV (header row) or JSON array data file; one row per iteration
    #[arg(short = 'd', long = "iteration-data", alias = "data")]
    pub data: Option<String>,
    /// Milliseconds to wait between requests
    #[arg(long = "delay-request", alias = "delay", default_value_t = 0)]
    pub delay: u64,
    /// Stop after the first failing request or test
    #[arg(short, long)]
    pub bail: bool,
    /// spec | dot | json | junit
    #[arg(short, long, default_value = "spec")]
    pub reporter: String,
    /// Write the report to a file instead of stdout
    #[arg(short, long)]
    pub output: Option<String>,
    /// Override a variable for this run: KEY=value (repeatable)
    #[arg(long = "env-var")]
    pub env_vars: Vec<String>,
}

pub async fn run(engine: &Engine, cmd: RunCmd) -> Result<()> {
    let RunCmd::Collection(a) = cmd;
    if !["spec", "dot", "json", "junit"].contains(&a.reporter.as_str()) {
        bail!(
            "unknown reporter '{}' (use spec, dot, json or junit)",
            a.reporter
        );
    }
    let target_id = crate::find::<Folder>(engine, &a.target)
        .map(|f| f.meta.id)
        .or_else(|_| crate::find::<Workspace>(engine, &a.target).map(|w| w.meta.id))
        .map_err(|_| anyhow!("no collection or folder named or with id '{}'", a.target))?;

    if let Some(env) = &a.env {
        let mut ws = engine.workspace_of(&target_id)?;
        let sub = engine
            .sub_environments(ws.id())?
            .into_iter()
            .find(|e| e.id() == env || e.name.eq_ignore_ascii_case(env))
            .ok_or_else(|| anyhow!("no sub-environment '{env}' in {}", ws.name))?;
        ws.active_environment_id = Some(sub.meta.id.clone());
        engine.store.update(&ws)?;
    }

    let mut ids = engine.request_ids_in(&target_id)?;
    if !a.items.is_empty() {
        let mut keep = vec![];
        for item in &a.items {
            if let Ok(f) = crate::find::<Folder>(engine, item) {
                keep.extend(engine.request_ids_in(f.id())?);
            } else {
                keep.push(
                    crate::find::<Request>(engine, item)
                        .with_context(|| format!("--item {item}"))?
                        .meta
                        .id,
                );
            }
        }
        ids.retain(|id| keep.contains(id));
    }
    if ids.is_empty() {
        bail!("nothing to run: no requests in '{}'", a.target);
    }

    let mut data = match &a.data {
        Some(p) => lsock_runner::load_data_file(p)?,
        None => vec![],
    };
    for kv in &a.env_vars {
        let (k, v) = kv
            .split_once('=')
            .ok_or_else(|| anyhow!("--env-var must be KEY=value"))?;
        if data.is_empty() {
            data.push(Default::default());
        }
        for row in &mut data {
            row.insert(k.to_string(), serde_json::Value::String(v.to_string()));
        }
    }
    let iterations = a.iterations.unwrap_or(if a.data.is_some() {
        data.len().max(1) as u32
    } else {
        1
    });
    let opts = RunOptions {
        iterations,
        delay_ms: a.delay,
        data,
        bail: a.bail,
        ..Default::default()
    };

    // Stream progress for the human reporters when printing to a terminal.
    let live = a.reporter == "spec" && a.output.is_none();
    let summary = lsock_runner::run(engine, &ids, &opts, |ev| {
        if !live {
            return;
        }
        match ev {
            RunEvent::IterationStart { iteration } if iterations > 1 => {
                println!("\n{}", bold(&format!("Iteration {iteration}/{iterations}")))
            }
            RunEvent::RequestEnd { result: r } => {
                let mark = if r.skipped {
                    dim("-")
                } else if r.failed() {
                    red("✗")
                } else {
                    green("✓")
                };
                let status = if r.skipped {
                    dim("skipped")
                } else if let Some(e) = &r.error {
                    red(&format!("error: {e}"))
                } else {
                    let s = format!("{} {}", r.status, r.status_message);
                    if r.status >= 400 { yellow(&s) } else { s }
                };
                println!(
                    "  {mark} {} {} {} {}",
                    bold(&r.method),
                    r.name,
                    status,
                    dim(&format!("({:.0} ms)", r.duration_ms))
                );
                if let Some(e) = &r.script_error {
                    println!("      {}", red(&format!("script error: {e}")));
                }
                for t in &r.tests {
                    if t.skipped {
                        println!("      {} {}", dim("-"), dim(&t.name));
                    } else if t.passed {
                        println!("      {} {}", green("✓"), t.name);
                    } else {
                        println!(
                            "      {} {} {}",
                            red("✗"),
                            t.name,
                            dim(&format!("— {}", t.error.clone().unwrap_or_default()))
                        );
                    }
                }
                for c in r
                    .console
                    .iter()
                    .filter(|c| c.level == "error" || c.level == "warn")
                {
                    println!("      {} {}", yellow(&format!("[{}]", c.level)), c.text);
                }
            }
            RunEvent::Warning { message, .. } => println!("  {} {message}", yellow("!")),
            _ => {}
        }
    })
    .await;

    let text = if live {
        let t = format!(
            "\n{} requests ({} failed, {} skipped) · {} tests ({} passed, {} failed, {} skipped) · {:.0} ms{}",
            summary.requests,
            summary.requests_failed,
            summary.requests_skipped,
            summary.tests_passed + summary.tests_failed + summary.tests_skipped,
            summary.tests_passed,
            summary.tests_failed,
            summary.tests_skipped,
            summary.duration_ms,
            if summary.bailed {
                "\nStopped early (--bail) after the first failure."
            } else {
                ""
            }
        );
        if summary.ok() { green(&t) } else { red(&t) }
    } else {
        report::render(&summary, &a.reporter).unwrap_or_default()
    };
    match &a.output {
        Some(path) => {
            std::fs::write(path, &text).with_context(|| format!("writing {path}"))?;
            eprintln!(
                "{} report written to {path}",
                if summary.ok() {
                    green("✓")
                } else {
                    red("✗")
                }
            );
        }
        None => println!("{text}"),
    }
    if !summary.ok() {
        std::process::exit(1);
    }
    Ok(())
}
