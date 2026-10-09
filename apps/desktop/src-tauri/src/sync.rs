//! Import/export, code generation, vault and Git sync commands.

use lsock_convert::codegen::{Target, generate};
use lsock_core::{Doc, Environment, GitRepo, Workspace};
use lsock_engine::Engine;
use lsock_engine::git::{GitCommit, GitStatus, GitSyncResult};
use lsock_engine::transfer::{
    ExportFormat, ExportOptions, Exported, ImportMode, ImportOptions, ImportSummary,
};
use lsock_engine::vault::VaultStatus;
use serde::Serialize;
use serde_json::Value;
use tauri::State;

use crate::{AppState, CmdResult, e};

/// Run blocking engine work (git, file I/O) off the async runtime.
async fn blocking<T: Send + 'static>(
    engine: &Engine,
    f: impl FnOnce(&Engine) -> lsock_engine::Result<T> + Send + 'static,
) -> CmdResult<T> {
    let engine = engine.clone();
    tauri::async_runtime::spawn_blocking(move || f(&engine))
        .await
        .map_err(e)?
        .map_err(e)
}

// ------------------------------------------------------------------ import / export

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewWorkspace {
    name: String,
    scope: lsock_core::WorkspaceScope,
    requests: usize,
    folders: usize,
    environments: usize,
    exists: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportPreview {
    format: lsock_convert::Format,
    format_label: String,
    workspaces: Vec<PreviewWorkspace>,
    warnings: Vec<String>,
}

/// What a file would import, without writing anything.
#[tauri::command]
pub fn import_preview(state: State<'_, AppState>, text: String) -> CmdResult<ImportPreview> {
    let i = lsock_convert::import(&text).map_err(e)?;
    Ok(ImportPreview {
        format: i.format,
        format_label: i.format.to_string(),
        workspaces: i
            .workspaces
            .iter()
            .map(|b| PreviewWorkspace {
                name: b.workspace.name.clone(),
                scope: b.workspace.scope,
                requests: b.request_count(),
                folders: b
                    .walk()
                    .iter()
                    .filter(|x| matches!(x.node, lsock_convert::Node::Folder(..)))
                    .count(),
                environments: b.base_env.iter().count() + b.sub_envs.len(),
                exists: b
                    .meta
                    .id
                    .as_ref()
                    .is_some_and(|id| state.engine.store.get::<Workspace>(id).is_ok()),
            })
            .collect(),
        warnings: i.warnings,
    })
}

#[tauri::command]
pub fn import_apply(
    state: State<'_, AppState>,
    text: String,
    into_workspace_id: Option<String>,
    replace: bool,
) -> CmdResult<ImportSummary> {
    let mode = if replace {
        ImportMode::Replace
    } else {
        ImportMode::Copy
    };
    state
        .engine
        .import_text(
            &text,
            &ImportOptions {
                mode,
                into_workspace: into_workspace_id,
            },
        )
        .map_err(e)
}

/// Download a spec/collection from a URL (OpenAPI links, raw GitHub files…).
#[tauri::command]
pub async fn fetch_text(url: String) -> CmdResult<String> {
    let resp = reqwest::Client::new()
        .get(&url)
        .header(
            "accept",
            "application/json, application/yaml, text/yaml, */*",
        )
        .timeout(std::time::Duration::from_secs(30))
        .send()
        .await
        .map_err(e)?;
    if !resp.status().is_success() {
        return Err(format!("{url} answered HTTP {}", resp.status()));
    }
    let bytes = resp.bytes().await.map_err(e)?;
    if bytes.len() > 25 * 1024 * 1024 {
        return Err("that file is larger than 25 MB".into());
    }
    String::from_utf8(bytes.to_vec()).map_err(|_| "that URL didn't return text".to_string())
}

#[tauri::command]
pub fn export_workspace(
    state: State<'_, AppState>,
    workspace_id: String,
    format: ExportFormat,
    include_private: bool,
    include_cookies: bool,
) -> CmdResult<Exported> {
    state
        .engine
        .export_workspace(
            &workspace_id,
            format,
            &ExportOptions {
                include_private,
                include_cookies,
            },
        )
        .map_err(e)
}

/// Save text to the Downloads folder (unique name) and return the path.
#[tauri::command]
pub fn save_to_downloads(file_name: String, content: String) -> CmdResult<String> {
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .ok_or("no home directory")?;
    let dir = std::path::PathBuf::from(home).join("Downloads");
    std::fs::create_dir_all(&dir).map_err(e)?;
    let safe: String = file_name
        .chars()
        .map(|c| if c == '/' || c == '\\' { '-' } else { c })
        .collect();
    let (stem, ext) = safe
        .rsplit_once('.')
        .map(|(s, x)| (s.to_string(), format!(".{x}")))
        .unwrap_or((safe.clone(), String::new()));
    let mut path = dir.join(&safe);
    let mut n = 2;
    while path.exists() {
        path = dir.join(format!("{stem} ({n}){ext}"));
        n += 1;
    }
    std::fs::write(&path, content).map_err(e)?;
    Ok(path.to_string_lossy().into_owned())
}

/// Show a file or folder in Finder / Explorer / the file manager.
#[tauri::command]
pub fn reveal_path(path: String) {
    #[cfg(target_os = "macos")]
    let _ = std::process::Command::new("open")
        .args(["-R", &path])
        .spawn();
    #[cfg(target_os = "windows")]
    let _ = std::process::Command::new("explorer")
        .arg(format!("/select,{path}"))
        .spawn();
    #[cfg(all(unix, not(target_os = "macos")))]
    let _ = std::process::Command::new("xdg-open")
        .arg(
            std::path::Path::new(&path)
                .parent()
                .unwrap_or(std::path::Path::new("/")),
        )
        .spawn();
}

// ------------------------------------------------------------------ code generation

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CodeTarget {
    id: Target,
    label: &'static str,
}

#[tauri::command]
pub fn code_targets() -> Vec<CodeTarget> {
    Target::ALL
        .iter()
        .map(|t| CodeTarget {
            id: *t,
            label: t.label(),
        })
        .collect()
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CodeSnippet {
    code: String,
    notes: Vec<String>,
}

#[tauri::command]
pub fn code_generate(
    state: State<'_, AppState>,
    request_id: String,
    target: Target,
) -> CmdResult<CodeSnippet> {
    let (req, notes) = state.engine.code_request(&request_id).map_err(e)?;
    Ok(CodeSnippet {
        code: generate(&req, target),
        notes,
    })
}

/// Code for one MCP message (default `initialize`) to a saved Streamable HTTP server.
#[tauri::command]
pub async fn mcp_code_generate(
    state: State<'_, AppState>,
    server_id: String,
    target: Target,
    message: Option<Value>,
    session_id: Option<String>,
    protocol_version: Option<String>,
) -> CmdResult<CodeSnippet> {
    let (req, notes) = state
        .engine
        .mcp_code_request(
            &server_id,
            message,
            session_id.as_deref(),
            protocol_version.as_deref(),
        )
        .await
        .map_err(e)?;
    Ok(CodeSnippet {
        code: generate(&req, target),
        notes,
    })
}

// ------------------------------------------------------------------ vault

#[tauri::command]
pub fn env_set_var(
    state: State<'_, AppState>,
    env_id: String,
    key: String,
    value: Value,
    secret: bool,
) -> CmdResult<Doc<Environment>> {
    state
        .engine
        .set_env_var(&env_id, &key, value, secret)
        .map_err(e)
}

#[tauri::command]
pub fn env_reveal(state: State<'_, AppState>, env_id: String, key: String) -> CmdResult<String> {
    state.engine.reveal_secret(&env_id, &key).map_err(e)
}

#[tauri::command]
pub fn vault_status(state: State<'_, AppState>) -> CmdResult<VaultStatus> {
    state.engine.vault_status().map_err(e)
}

#[tauri::command]
pub fn vault_export_key(state: State<'_, AppState>) -> CmdResult<String> {
    state.engine.vault_export_key().map_err(e)
}

#[tauri::command]
pub fn vault_import_key(state: State<'_, AppState>, key: String) -> CmdResult<()> {
    state.engine.vault_import_key(&key).map_err(e)
}

#[tauri::command]
pub fn vault_reset(state: State<'_, AppState>) -> CmdResult<usize> {
    state.engine.vault_reset().map_err(e)
}

// ------------------------------------------------------------------ git

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LinkedWorkspace {
    workspace_id: String,
    name: String,
    path: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RepoView {
    #[serde(flatten)]
    doc: Doc<GitRepo>,
    workspaces: Vec<LinkedWorkspace>,
}

fn view(engine: &Engine, r: Doc<GitRepo>) -> RepoView {
    let workspaces = r
        .files
        .iter()
        .map(|f| LinkedWorkspace {
            workspace_id: f.workspace_id.clone(),
            name: engine
                .store
                .get::<Workspace>(&f.workspace_id)
                .map(|w| w.name.clone())
                .unwrap_or_else(|_| "(deleted)".into()),
            path: f.path.clone(),
        })
        .collect();
    RepoView { doc: r, workspaces }
}

#[tauri::command]
pub fn git_repo_list(state: State<'_, AppState>) -> CmdResult<Vec<RepoView>> {
    Ok(state
        .engine
        .git_repos()
        .map_err(e)?
        .into_iter()
        .map(|r| view(&state.engine, r))
        .collect())
}

/// Suggested folder for a new repository: ~/Documents/logic-socket/<name>.
#[tauri::command]
pub fn git_default_dir(name: String) -> CmdResult<String> {
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .ok_or("no home directory")?;
    let slug: String = name
        .trim()
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect();
    Ok(std::path::PathBuf::from(home)
        .join("Documents")
        .join("logic-socket")
        .join(if slug.is_empty() { "repo".into() } else { slug })
        .to_string_lossy()
        .into_owned())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RepoAndSync {
    repo: RepoView,
    sync: GitSyncResult,
}

#[tauri::command]
pub async fn git_open(
    state: State<'_, AppState>,
    dir: String,
    name: Option<String>,
) -> CmdResult<RepoAndSync> {
    let (r, sync) = blocking(&state.engine, move |en| en.git_open(&dir, name.as_deref())).await?;
    Ok(RepoAndSync {
        repo: view(&state.engine, r),
        sync,
    })
}

#[tauri::command]
pub async fn git_clone(
    state: State<'_, AppState>,
    url: String,
    dir: String,
    token: Option<String>,
) -> CmdResult<RepoAndSync> {
    let (r, sync) = blocking(&state.engine, move |en| {
        en.git_clone(&url, &dir, token.as_deref())
    })
    .await?;
    Ok(RepoAndSync {
        repo: view(&state.engine, r),
        sync,
    })
}

#[tauri::command]
pub async fn git_repo_update(state: State<'_, AppState>, doc: Doc<GitRepo>) -> CmdResult<RepoView> {
    let r = blocking(&state.engine, move |en| en.git_update_repo(&doc)).await?;
    Ok(view(&state.engine, r))
}

#[tauri::command]
pub fn git_set_token(
    state: State<'_, AppState>,
    repo_id: String,
    token: Option<String>,
) -> CmdResult<()> {
    state
        .engine
        .git_set_token(&repo_id, token.as_deref())
        .map_err(e)
}

#[tauri::command]
pub fn git_remove(state: State<'_, AppState>, repo_id: String) -> CmdResult<()> {
    state.engine.git_remove_repo(&repo_id).map_err(e)
}

#[tauri::command]
pub async fn git_link(
    state: State<'_, AppState>,
    repo_id: String,
    workspace_id: String,
) -> CmdResult<RepoView> {
    let r = blocking(&state.engine, move |en| {
        en.git_link(&repo_id, &workspace_id)
    })
    .await?;
    Ok(view(&state.engine, r))
}

#[tauri::command]
pub async fn git_unlink(
    state: State<'_, AppState>,
    repo_id: String,
    workspace_id: String,
    delete_file: bool,
) -> CmdResult<RepoView> {
    let r = blocking(&state.engine, move |en| {
        en.git_unlink(&repo_id, &workspace_id, delete_file)
    })
    .await?;
    Ok(view(&state.engine, r))
}

#[tauri::command]
pub async fn git_status(state: State<'_, AppState>, repo_id: String) -> CmdResult<GitStatus> {
    blocking(&state.engine, move |en| en.git_status(&repo_id)).await
}

#[tauri::command]
pub async fn git_diff(
    state: State<'_, AppState>,
    repo_id: String,
    path: String,
) -> CmdResult<String> {
    blocking(&state.engine, move |en| en.git_diff(&repo_id, &path)).await
}

#[tauri::command]
pub async fn git_commit(
    state: State<'_, AppState>,
    repo_id: String,
    message: String,
    paths: Vec<String>,
) -> CmdResult<String> {
    blocking(&state.engine, move |en| {
        en.git_commit(&repo_id, &message, &paths)
    })
    .await
}

#[tauri::command]
pub async fn git_pull(state: State<'_, AppState>, repo_id: String) -> CmdResult<GitSyncResult> {
    blocking(&state.engine, move |en| en.git_pull(&repo_id)).await
}

#[tauri::command]
pub async fn git_push(state: State<'_, AppState>, repo_id: String) -> CmdResult<String> {
    blocking(&state.engine, move |en| en.git_push(&repo_id)).await
}

#[tauri::command]
pub async fn git_resolve(
    state: State<'_, AppState>,
    repo_id: String,
    path: String,
    take: String,
) -> CmdResult<GitSyncResult> {
    blocking(&state.engine, move |en| {
        en.git_resolve(&repo_id, &path, &take)
    })
    .await
}

#[tauri::command]
pub async fn git_abort_merge(state: State<'_, AppState>, repo_id: String) -> CmdResult<()> {
    blocking(&state.engine, move |en| en.git_abort_merge(&repo_id)).await
}

#[tauri::command]
pub async fn git_discard(
    state: State<'_, AppState>,
    repo_id: String,
    path: String,
) -> CmdResult<GitSyncResult> {
    blocking(&state.engine, move |en| en.git_discard(&repo_id, &path)).await
}

#[tauri::command]
pub async fn git_log(
    state: State<'_, AppState>,
    repo_id: String,
    limit: usize,
) -> CmdResult<Vec<GitCommit>> {
    blocking(&state.engine, move |en| en.git_log(&repo_id, limit)).await
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Branches {
    current: String,
    all: Vec<String>,
}

#[tauri::command]
pub async fn git_branches(state: State<'_, AppState>, repo_id: String) -> CmdResult<Branches> {
    let (current, all) = blocking(&state.engine, move |en| en.git_branches(&repo_id)).await?;
    Ok(Branches { current, all })
}

#[tauri::command]
pub async fn git_checkout(
    state: State<'_, AppState>,
    repo_id: String,
    branch: String,
    create: bool,
) -> CmdResult<GitSyncResult> {
    blocking(&state.engine, move |en| {
        en.git_checkout(&repo_id, &branch, create)
    })
    .await
}
