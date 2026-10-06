//! `lsock import`, `lsock export`, `lsock code`, `lsock vault`, `lsock git`.

use std::io::Read as _;

use anyhow::{Result, anyhow, bail};
use clap::{Args, Subcommand, ValueEnum};
use lsock_convert::codegen::{Target, generate};
use lsock_core::{Doc, GitRepo, Request, Workspace};
use lsock_engine::Engine;
use lsock_engine::git::GitSyncResult;
use lsock_engine::transfer::{ExportFormat, ExportOptions, ImportMode, ImportOptions};

use crate::find;
use crate::out::*;

#[derive(Args, Debug)]
pub struct ImportArgs {
    /// File to import (`-` for stdin): Insomnia v4/v5, Postman collection/environment,
    /// OpenAPI 3 / Swagger 2 (JSON or YAML), HAR, or curl commands
    pub file: String,
    /// Merge into an existing workspace instead of creating a new one
    #[arg(long)]
    pub into: Option<String>,
    /// Keep the file's ids and overwrite a workspace with the same id (re-importing a backup)
    #[arg(long, conflicts_with = "into")]
    pub replace: bool,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum ExportAs {
    /// Insomnia v5 YAML (also imported by Insomnia)
    Insomnia,
    /// Postman collection v2.1
    Postman,
    /// HTTP Archive (requests only)
    Har,
}

#[derive(Args, Debug)]
pub struct ExportArgs {
    pub workspace: String,
    #[arg(long, short, value_enum, default_value = "insomnia")]
    pub format: ExportAs,
    /// Output file (default: a file named after the workspace; `-` for stdout)
    #[arg(long, short)]
    pub output: Option<String>,
    /// Include private sub-environments
    #[arg(long)]
    pub include_private: bool,
    /// Include cookies from the cookie jar
    #[arg(long)]
    pub include_cookies: bool,
}

#[derive(Args, Debug)]
pub struct CodeArgs {
    /// Request (name or id)
    pub request: String,
    /// curl, httpie, js, python, go, rust
    #[arg(long, short, default_value = "curl")]
    pub lang: String,
}

#[derive(Subcommand, Debug)]
pub enum VaultCmd {
    /// Is there a vault key on this machine, and how many secrets are encrypted?
    Status,
    /// Print the vault key (base64) to move secrets to another machine — keep it safe
    ExportKey,
    /// Install a vault key from another machine (read from stdin when omitted)
    ImportKey { key: Option<String> },
    /// Delete the key and blank every secret value (cannot be undone)
    Reset {
        #[arg(long)]
        yes: bool,
    },
}

#[derive(Subcommand, Debug)]
pub enum GitCmd {
    /// List repositories and the workspaces synced with them
    List,
    /// Use an existing local repository, or create one in DIR
    Open {
        dir: String,
        #[arg(long)]
        name: Option<String>,
    },
    /// Clone a repository and import its workspaces
    Clone {
        url: String,
        dir: String,
        /// Read an HTTPS access token from this environment variable (stored in the keychain)
        #[arg(long)]
        token_env: Option<String>,
    },
    /// Configure a repository's remote and commit author
    Config {
        repo: String,
        #[arg(long)]
        remote: Option<String>,
        #[arg(long)]
        author: Option<String>,
        #[arg(long)]
        email: Option<String>,
        /// Read an HTTPS access token from this environment variable ("" clears it)
        #[arg(long)]
        token_env: Option<String>,
    },
    /// Sync a workspace through a repository
    Link { repo: String, workspace: String },
    /// Stop syncing a workspace
    Unlink {
        repo: String,
        workspace: String,
        /// Also delete its file from the working copy
        #[arg(long)]
        delete_file: bool,
    },
    /// Changed workspace files, branch and ahead/behind counts
    Status { repo: String },
    /// Diff of a workspace file
    Diff { repo: String, file: String },
    /// Commit changed workspace files
    Commit {
        repo: String,
        #[arg(long, short)]
        message: String,
        /// Only these files (default: all changed)
        files: Vec<String>,
    },
    /// Pull and import (local changes must be committed first)
    Pull { repo: String },
    /// Push the current branch
    Push { repo: String },
    /// Resolve a conflicted file with our version or theirs
    Resolve {
        repo: String,
        file: String,
        #[arg(long, value_parser = ["ours", "theirs"])]
        take: String,
    },
    /// Discard local edits to a workspace file
    Discard { repo: String, file: String },
    /// Recent commits
    Log {
        repo: String,
        #[arg(long, short, default_value_t = 15)]
        n: usize,
    },
    /// List branches, or switch to one (`--create` to make it)
    Branch {
        repo: String,
        name: Option<String>,
        #[arg(long)]
        create: bool,
    },
}

fn read_input(file: &str) -> Result<String> {
    if file == "-" {
        let mut s = String::new();
        std::io::stdin().read_to_string(&mut s)?;
        return Ok(s);
    }
    std::fs::read_to_string(file).map_err(|e| anyhow!("{file}: {e}"))
}

pub fn import(engine: &Engine, a: ImportArgs) -> Result<()> {
    let text = read_input(&a.file)?;
    let into = a
        .into
        .map(|w| find::<Workspace>(engine, &w).map(|d| d.meta.id))
        .transpose()?;
    let mode = if a.replace {
        ImportMode::Replace
    } else {
        ImportMode::Copy
    };
    let s = engine.import_text(
        &text,
        &ImportOptions {
            mode,
            into_workspace: into,
        },
    )?;
    println!(
        "{} Imported {} → {} {}",
        green("✓"),
        s.format,
        bold(&s.workspaces.join(", ")),
        dim(&format!(
            "({} requests, {} folders, {} environments)",
            s.requests, s.folders, s.environments
        ))
    );
    for w in &s.warnings {
        println!("  {} {w}", yellow("!"));
    }
    Ok(())
}

pub fn export(engine: &Engine, a: ExportArgs) -> Result<()> {
    let ws = find::<Workspace>(engine, &a.workspace)?;
    let format = match a.format {
        ExportAs::Insomnia => ExportFormat::InsomniaV5,
        ExportAs::Postman => ExportFormat::Postman,
        ExportAs::Har => ExportFormat::Har,
    };
    let out = engine.export_workspace(
        ws.id(),
        format,
        &ExportOptions {
            include_private: a.include_private,
            include_cookies: a.include_cookies,
        },
    )?;
    let path = a.output.unwrap_or_else(|| out.file_name.clone());
    if path == "-" {
        print!("{}", out.content);
    } else {
        std::fs::write(&path, &out.content)?;
        eprintln!("{} Wrote {}", green("✓"), bold(&path));
    }
    for w in &out.warnings {
        eprintln!("  {} {w}", yellow("!"));
    }
    Ok(())
}

pub fn code(engine: &Engine, a: CodeArgs) -> Result<()> {
    let target = Target::parse(&a.lang).ok_or_else(|| {
        anyhow!(
            "unknown language '{}' (curl, httpie, js, python, go, rust)",
            a.lang
        )
    })?;
    let req = find::<Request>(engine, &a.request)?;
    let (code, notes) = engine.code_request(req.id())?;
    println!("{}", generate(&code, target));
    for n in notes {
        eprintln!("{} {n}", yellow("!"));
    }
    Ok(())
}

pub fn vault(engine: &Engine, c: VaultCmd) -> Result<()> {
    match c {
        VaultCmd::Status => {
            let s = engine.vault_status()?;
            let key = if s.has_key {
                green("present")
            } else {
                dim("not created yet")
            };
            println!("vault key: {key}\nencrypted values: {}", s.sealed_values);
        }
        VaultCmd::ExportKey => {
            eprintln!(
                "{} anyone with this key can read your encrypted secrets",
                yellow("!")
            );
            println!("{}", engine.vault_export_key()?);
        }
        VaultCmd::ImportKey { key } => {
            let key = match key {
                Some(k) => k,
                None => read_input("-")?,
            };
            engine.vault_import_key(key.trim())?;
            println!("{} vault key installed", green("✓"));
        }
        VaultCmd::Reset { yes } => {
            if !yes {
                bail!(
                    "this deletes the key and blanks every secret value — re-run with --yes to confirm"
                );
            }
            let n = engine.vault_reset()?;
            println!("{} vault reset; {n} secret value(s) cleared", green("✓"));
        }
    }
    Ok(())
}

fn repo(engine: &Engine, needle: &str) -> Result<Doc<GitRepo>> {
    if let Ok(r) = find::<GitRepo>(engine, needle) {
        return Ok(r);
    }
    // a path or a linked workspace works too
    let canon = std::fs::canonicalize(needle)
        .ok()
        .map(|p| p.to_string_lossy().into_owned());
    for r in engine.git_repos()? {
        if Some(&r.path) == canon.as_ref() {
            return Ok(r);
        }
    }
    if let Ok(ws) = find::<Workspace>(engine, needle)
        && let Some(r) = engine.git_repo_of(ws.id())?
    {
        return Ok(r);
    }
    bail!("no repository '{needle}' (see `lsock git list`)")
}

fn token_from(var: &str) -> Result<String> {
    std::env::var(var).map_err(|_| anyhow!("environment variable {var} isn't set"))
}

fn print_sync(r: &GitSyncResult) {
    if !r.workspaces.is_empty() {
        println!("{} loaded {}", green("✓"), r.workspaces.join(", "));
    }
    if !r.conflicts.is_empty() {
        println!(
            "{} conflicts in {} — resolve with `lsock git resolve <repo> <file> --take ours|theirs`",
            red("✗"),
            r.conflicts.join(", ")
        );
    }
    for w in &r.warnings {
        println!("  {} {w}", yellow("!"));
    }
}

pub fn git(engine: &Engine, c: GitCmd) -> Result<()> {
    match c {
        GitCmd::List => {
            let repos = engine.git_repos()?;
            if repos.is_empty() {
                println!(
                    "{}",
                    dim(
                        "no repositories — `lsock git open <dir>` or `lsock git clone <url> <dir>`"
                    )
                );
            }
            for r in repos {
                println!("{} {} {}", bold(&r.name), dim(&r.path), dim(&r.remote_url));
                for f in &r.files {
                    let name = engine
                        .store
                        .get::<Workspace>(&f.workspace_id)
                        .map(|w| w.name.clone())
                        .unwrap_or_else(|_| "(deleted)".into());
                    println!("    {name} {}", dim(&f.path));
                }
            }
        }
        GitCmd::Open { dir, name } => {
            let (r, res) = engine.git_open(&dir, name.as_deref())?;
            println!("{} {} {}", green("✓"), bold(&r.name), dim(&r.path));
            print_sync(&res);
        }
        GitCmd::Clone {
            url,
            dir,
            token_env,
        } => {
            let token = token_env.map(|v| token_from(&v)).transpose()?;
            let (r, res) = engine.git_clone(&url, &dir, token.as_deref())?;
            println!("{} cloned into {}", green("✓"), bold(&r.path));
            print_sync(&res);
        }
        GitCmd::Config {
            repo: name,
            remote,
            author,
            email,
            token_env,
        } => {
            let mut r = repo(engine, &name)?;
            if let Some(x) = remote {
                r.body.remote_url = x;
            }
            if let Some(x) = author {
                r.body.author_name = x;
            }
            if let Some(x) = email {
                r.body.author_email = x;
            }
            let r = engine.git_update_repo(&r)?;
            if let Some(v) = token_env {
                let t = if v.is_empty() {
                    None
                } else {
                    Some(token_from(&v)?)
                };
                engine.git_set_token(r.id(), t.as_deref())?;
            }
            println!("{} {}", green("✓"), bold(&r.name));
        }
        GitCmd::Link {
            repo: name,
            workspace,
        } => {
            let r = repo(engine, &name)?;
            let ws = find::<Workspace>(engine, &workspace)?;
            let r = engine.git_link(r.id(), ws.id())?;
            let f = r
                .files
                .iter()
                .find(|f| f.workspace_id == ws.meta.id)
                .unwrap();
            println!("{} {} → {}", green("✓"), ws.name, bold(&f.path));
        }
        GitCmd::Unlink {
            repo: name,
            workspace,
            delete_file,
        } => {
            let r = repo(engine, &name)?;
            let ws = find::<Workspace>(engine, &workspace)?;
            engine.git_unlink(r.id(), ws.id(), delete_file)?;
            println!("{} {} is no longer synced", green("✓"), ws.name);
        }
        GitCmd::Status { repo: name } => {
            let r = repo(engine, &name)?;
            let s = engine.git_status(r.id())?;
            let up = s
                .upstream
                .as_deref()
                .map(|u| format!(" → {u}"))
                .unwrap_or_default();
            let ab = if s.ahead + s.behind > 0 {
                format!(" ({} ahead, {} behind)", s.ahead, s.behind)
            } else {
                String::new()
            };
            println!("on {}{}{}", bold(&s.branch), dim(&up), yellow(&ab));
            if s.changes.is_empty() {
                println!("{}", dim("nothing to commit"));
            }
            for ch in s.changes {
                let st = match ch.status.as_str() {
                    "conflict" => red("conflict"),
                    "untracked" | "added" => green(&ch.status),
                    "deleted" => red(&ch.status),
                    _ => yellow(&ch.status),
                };
                println!(
                    "  {st:<20} {} {}",
                    ch.path,
                    dim(&ch.workspace.unwrap_or_default())
                );
            }
        }
        GitCmd::Diff { repo: name, file } => {
            let r = repo(engine, &name)?;
            for line in engine.git_diff(r.id(), &file)?.lines() {
                let l = if line.starts_with('+') && !line.starts_with("+++") {
                    green(line)
                } else if line.starts_with('-') && !line.starts_with("---") {
                    red(line)
                } else if line.starts_with("@@") {
                    cyan(line)
                } else {
                    line.to_string()
                };
                println!("{l}");
            }
        }
        GitCmd::Commit {
            repo: name,
            message,
            files,
        } => {
            let r = repo(engine, &name)?;
            let hash = engine.git_commit(r.id(), &message, &files)?;
            println!("{} committed {}", green("✓"), bold(&hash));
        }
        GitCmd::Pull { repo: name } => {
            let r = repo(engine, &name)?;
            let res = engine.git_pull(r.id())?;
            if res.conflicts.is_empty() && res.workspaces.is_empty() && res.warnings.is_empty() {
                println!("{} up to date", green("✓"));
            }
            print_sync(&res);
        }
        GitCmd::Push { repo: name } => {
            let r = repo(engine, &name)?;
            engine.git_push(r.id())?;
            println!("{} pushed", green("✓"));
        }
        GitCmd::Resolve {
            repo: name,
            file,
            take,
        } => {
            let r = repo(engine, &name)?;
            print_sync(&engine.git_resolve(r.id(), &file, &take)?);
        }
        GitCmd::Discard { repo: name, file } => {
            let r = repo(engine, &name)?;
            print_sync(&engine.git_discard(r.id(), &file)?);
        }
        GitCmd::Log { repo: name, n } => {
            let r = repo(engine, &name)?;
            for c in engine.git_log(r.id(), n)? {
                let when = chrono::DateTime::from_timestamp_millis(c.time_ms)
                    .map(|d| d.format("%Y-%m-%d %H:%M").to_string())
                    .unwrap_or_default();
                println!(
                    "{} {} {} {}",
                    yellow(&c.hash),
                    c.message,
                    dim(&format!("— {}", c.author)),
                    dim(&when)
                );
            }
        }
        GitCmd::Branch {
            repo: name,
            name: branch,
            create,
        } => {
            let r = repo(engine, &name)?;
            match branch {
                Some(b) => {
                    print_sync(&engine.git_checkout(r.id(), &b, create)?);
                    println!("{} on {}", green("✓"), bold(&b));
                }
                None => {
                    let (current, all) = engine.git_branches(r.id())?;
                    for b in all {
                        if b == current {
                            println!("{} {}", green("●"), bold(&b));
                        } else {
                            println!("  {b}");
                        }
                    }
                }
            }
        }
    }
    Ok(())
}
