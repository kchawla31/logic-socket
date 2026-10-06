//! Git sync: each linked workspace is one Logic Socket YAML file
//! (`logic-socket.<name>.yaml`) in a repository.
//!
//! Uses the system `git`, so SSH agents, credential helpers, signing and hooks
//! work as usual. An optional HTTPS token is kept in the keychain and passed via
//! environment config (never on the command line). Secret values, private
//! environments and cookies are never written to the repository.

use std::path::{Path, PathBuf};
use std::process::Command;

use base64::Engine as _;
use lsock_core::{Doc, GitFile, GitRepo, Workspace};
use serde::Serialize;

use crate::transfer::{ExportOptions, ImportMode, ImportOptions};
use crate::{Engine, EngineError, Result};

fn err(m: impl Into<String>) -> EngineError {
    EngineError::Message(m.into())
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct GitChange {
    pub path: String,
    /// `modified` | `added` | `deleted` | `untracked` | `conflict` | `renamed`
    pub status: String,
    pub workspace_id: Option<String>,
    pub workspace: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct GitStatus {
    pub branch: String,
    pub upstream: Option<String>,
    pub ahead: u32,
    pub behind: u32,
    pub changes: Vec<GitChange>,
    pub has_remote: bool,
    pub has_token: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct GitCommit {
    pub hash: String,
    pub author: String,
    pub email: String,
    pub time_ms: i64,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct GitSyncResult {
    /// Workspaces created or updated from the repository.
    pub workspaces: Vec<String>,
    /// Files with merge conflicts; resolve with `git_resolve` and pull again.
    pub conflicts: Vec<String>,
    pub warnings: Vec<String>,
}

fn token_key(repo_id: &str) -> String {
    format!("git-token:{repo_id}")
}

impl Engine {
    fn git_raw(
        &self,
        repo: &Doc<GitRepo>,
        dir: &Path,
        args: &[&str],
    ) -> Result<std::process::Output> {
        let mut cmd = Command::new("git");
        cmd.arg("-C").arg(dir).args(args);
        cmd.env("GIT_TERMINAL_PROMPT", "0")
            .env("GIT_MERGE_AUTOEDIT", "no");
        if !repo.author_name.is_empty() {
            cmd.env("GIT_AUTHOR_NAME", &repo.author_name)
                .env("GIT_COMMITTER_NAME", &repo.author_name);
        }
        if !repo.author_email.is_empty() {
            cmd.env("GIT_AUTHOR_EMAIL", &repo.author_email)
                .env("GIT_COMMITTER_EMAIL", &repo.author_email);
        }
        if let Some(token) = self.secrets.get(&token_key(repo.id())) {
            let userpass = if token.contains(':') {
                token.clone()
            } else {
                format!("x-access-token:{token}")
            };
            let header = format!(
                "Authorization: Basic {}",
                base64::engine::general_purpose::STANDARD.encode(userpass)
            );
            cmd.env("GIT_CONFIG_COUNT", "1")
                .env("GIT_CONFIG_KEY_0", "http.extraHeader")
                .env("GIT_CONFIG_VALUE_0", header);
        }
        cmd.output()
            .map_err(|e| err(format!("couldn't run git ({e}) — is Git installed?")))
    }

    /// Run git in the repository; stdout on success, a readable error otherwise.
    fn git(&self, repo: &Doc<GitRepo>, args: &[&str]) -> Result<String> {
        let out = self.git_raw(repo, Path::new(&repo.path), args)?;
        if out.status.success() {
            return Ok(String::from_utf8_lossy(&out.stdout).into_owned());
        }
        let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
        let stdout = String::from_utf8_lossy(&out.stdout).trim().to_string();
        let msg = if stderr.is_empty() { stdout } else { stderr };
        let hint = if msg.contains("Please tell me who you are") || msg.contains("empty ident") {
            " — set an author name and email for this repository"
        } else if msg.contains("Authentication failed")
            || msg.contains("could not read Username")
            || msg.contains("Permission denied")
        {
            " — check the access token (or your SSH key / credential helper)"
        } else {
            ""
        };
        Err(err(format!(
            "git {}: {msg}{hint}",
            args.first().copied().unwrap_or("")
        )))
    }

    pub fn git_repos(&self) -> Result<Vec<Doc<GitRepo>>> {
        Ok(self.store.all_of::<GitRepo>()?)
    }

    pub fn git_repo_of(&self, workspace_id: &str) -> Result<Option<Doc<GitRepo>>> {
        Ok(self
            .git_repos()?
            .into_iter()
            .find(|r| r.files.iter().any(|f| f.workspace_id == workspace_id)))
    }

    /// Use an existing local repository, or create one (`git init`) in `dir`.
    pub fn git_open(&self, dir: &str, name: Option<&str>) -> Result<(Doc<GitRepo>, GitSyncResult)> {
        let path = PathBuf::from(dir);
        std::fs::create_dir_all(&path).map_err(|e| err(format!("{dir}: {e}")))?;
        let path = path
            .canonicalize()
            .map_err(|e| err(format!("{dir}: {e}")))?;
        if let Some(r) = self
            .git_repos()?
            .into_iter()
            .find(|r| Path::new(&r.path) == path)
        {
            return Ok((r, GitSyncResult::default()));
        }
        let tmp = Doc {
            meta: lsock_core::Meta {
                id: String::new(),
                kind: String::new(),
                parent_id: None,
                sort_key: 0.0,
                created: 0,
                modified: 0,
            },
            body: GitRepo::default(),
        };
        if !path.join(".git").exists() {
            let out = self.git_raw(&tmp, &path, &["init", "-b", "main"])?;
            if !out.status.success() {
                return Err(err(format!(
                    "git init: {}",
                    String::from_utf8_lossy(&out.stderr).trim()
                )));
            }
        }
        let remote = self
            .git_raw(&tmp, &path, &["remote", "get-url", "origin"])
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .unwrap_or_default();
        let name = name.map(str::to_string).unwrap_or_else(|| {
            path.file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| "Repository".into())
        });
        let repo = self.store.insert(
            None,
            GitRepo {
                name,
                path: path.to_string_lossy().into_owned(),
                remote_url: remote,
                ..Default::default()
            },
        )?;
        let res = self.git_import_all(repo.id())?;
        Ok((self.store.get(repo.id())?, res))
    }

    /// Clone `url` into `dir` and import every workspace file in it.
    pub fn git_clone(
        &self,
        url: &str,
        dir: &str,
        token: Option<&str>,
    ) -> Result<(Doc<GitRepo>, GitSyncResult)> {
        let path = PathBuf::from(dir);
        if path.exists()
            && std::fs::read_dir(&path)
                .map(|mut d| d.next().is_some())
                .unwrap_or(false)
        {
            return Err(err(format!("{dir} already exists and isn't empty")));
        }
        let parent = path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        std::fs::create_dir_all(parent).map_err(|e| err(e.to_string()))?;
        // a placeholder doc lets the token reach the clone command
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "Repository".into());
        let repo = self.store.insert(
            None,
            GitRepo {
                name,
                path: path.to_string_lossy().into_owned(),
                remote_url: url.to_string(),
                ..Default::default()
            },
        )?;
        if let Some(t) = token.filter(|t| !t.is_empty()) {
            self.secrets.set(&token_key(repo.id()), t).map_err(err)?;
        }
        let out = self.git_raw(&repo, parent, &["clone", url, &path.to_string_lossy()])?;
        if !out.status.success() {
            let _ = self.store.delete(repo.id());
            let _ = self.secrets.delete(&token_key(repo.id()));
            return Err(err(format!(
                "git clone: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            )));
        }
        let canonical = path.canonicalize().map_err(|e| err(e.to_string()))?;
        let mut repo = repo;
        repo.body.path = canonical.to_string_lossy().into_owned();
        let repo = self.store.update(&repo)?;
        let res = self.git_import_all(repo.id())?;
        Ok((self.store.get(repo.id())?, res))
    }

    pub fn git_update_repo(&self, repo: &Doc<GitRepo>) -> Result<Doc<GitRepo>> {
        let r = self.store.update(repo)?;
        if !r.remote_url.is_empty() {
            let current = self
                .git(&r, &["remote", "get-url", "origin"])
                .unwrap_or_default();
            if current.trim() != r.remote_url {
                let verb = if current.trim().is_empty() {
                    "add"
                } else {
                    "set-url"
                };
                self.git(&r, &["remote", verb, "origin", &r.remote_url])?;
            }
        }
        Ok(r)
    }

    /// Store (or clear) an HTTPS access token for this repository in the keychain.
    pub fn git_set_token(&self, repo_id: &str, token: Option<&str>) -> Result<()> {
        match token.filter(|t| !t.is_empty()) {
            Some(t) => self.secrets.set(&token_key(repo_id), t).map_err(err),
            None => self.secrets.delete(&token_key(repo_id)).map_err(err),
        }
    }

    /// Forget a repository (the folder on disk and the workspaces are kept).
    pub fn git_remove_repo(&self, repo_id: &str) -> Result<()> {
        let _ = self.secrets.delete(&token_key(repo_id));
        self.store.delete(repo_id)?;
        Ok(())
    }

    /// Start syncing a workspace through this repository.
    pub fn git_link(&self, repo_id: &str, workspace_id: &str) -> Result<Doc<GitRepo>> {
        if let Some(other) = self.git_repo_of(workspace_id)?
            && other.id() != repo_id
        {
            return Err(err(format!(
                "this workspace is already synced with '{}'",
                other.name
            )));
        }
        let mut repo: Doc<GitRepo> = self.store.get(repo_id)?;
        if !repo.files.iter().any(|f| f.workspace_id == workspace_id) {
            let ws: Doc<Workspace> = self.store.get(workspace_id)?;
            let base = lsock_convert::native::file_name_for(&ws.name);
            let mut path = base.clone();
            let mut n = 2;
            while repo.files.iter().any(|f| f.path == path)
                || Path::new(&repo.path).join(&path).exists()
            {
                path = base.replace(".yaml", &format!("-{n}.yaml"));
                n += 1;
            }
            repo.body.files.push(GitFile {
                workspace_id: workspace_id.into(),
                path,
            });
            repo = self.store.update(&repo)?;
        }
        self.git_write_files(&repo)?;
        Ok(repo)
    }

    /// Stop syncing a workspace (its file stays in the repository until committed as deleted).
    pub fn git_unlink(
        &self,
        repo_id: &str,
        workspace_id: &str,
        delete_file: bool,
    ) -> Result<Doc<GitRepo>> {
        let mut repo: Doc<GitRepo> = self.store.get(repo_id)?;
        if let Some(f) = repo.files.iter().find(|f| f.workspace_id == workspace_id)
            && delete_file
        {
            let _ = std::fs::remove_file(Path::new(&repo.path).join(&f.path));
        }
        repo.body.files.retain(|f| f.workspace_id != workspace_id);
        Ok(self.store.update(&repo)?)
    }

    /// Export every linked workspace into its file (only rewriting files that changed).
    pub fn git_write_files(&self, repo: &Doc<GitRepo>) -> Result<()> {
        let opts = ExportOptions {
            include_private: false,
            include_cookies: false,
        };
        for f in &repo.files {
            if self.store.get::<Workspace>(&f.workspace_id).is_err() {
                continue; // workspace deleted locally: leave the file alone
            }
            let b = self.export_bundle(&f.workspace_id, &opts)?;
            let content = lsock_convert::native::export(&b).map_err(|e| err(e.to_string()))?;
            let path = Path::new(&repo.path).join(&f.path);
            if std::fs::read_to_string(&path).ok().as_deref() != Some(content.as_str()) {
                if let Some(dir) = path.parent() {
                    std::fs::create_dir_all(dir).map_err(|e| err(e.to_string()))?;
                }
                std::fs::write(&path, content)
                    .map_err(|e| err(format!("{}: {e}", path.display())))?;
            }
        }
        Ok(())
    }

    /// Import every Logic Socket file in the working copy (keeping ids) and update links.
    pub fn git_import_all(&self, repo_id: &str) -> Result<GitSyncResult> {
        let mut repo: Doc<GitRepo> = self.store.get(repo_id)?;
        let root = PathBuf::from(&repo.path);
        let mut files = vec![];
        collect_yaml(&root, &root, 0, &mut files);
        let mut res = GitSyncResult::default();
        let mut links = vec![];
        for rel in files {
            let text = std::fs::read_to_string(root.join(&rel)).unwrap_or_default();
            if lsock_convert::detect(&text) != Some(lsock_convert::Format::LogicSocket) {
                continue;
            }
            match self.import_text(
                &text,
                &ImportOptions {
                    mode: ImportMode::Replace,
                    into_workspace: None,
                },
            ) {
                Ok(s) => {
                    res.warnings
                        .extend(s.warnings.into_iter().map(|w| format!("{rel}: {w}")));
                    for (id, name) in s.workspace_ids.into_iter().zip(s.workspaces) {
                        links.push(GitFile {
                            workspace_id: id,
                            path: rel.clone(),
                        });
                        res.workspaces.push(name);
                    }
                }
                Err(e) => res.warnings.push(format!("{rel}: {e}")),
            }
        }
        for old in &repo.files {
            if !links.iter().any(|l| l.workspace_id == old.workspace_id) {
                if root.join(&old.path).exists() {
                    links.push(old.clone()); // linked but not yet committed/parsable: keep
                } else {
                    res.warnings.push(format!("{} was removed from the repository; the workspace is kept but no longer synced", old.path));
                }
            }
        }
        repo.body.files = links;
        self.store.update(&repo)?;
        Ok(res)
    }

    pub fn git_status(&self, repo_id: &str) -> Result<GitStatus> {
        let repo: Doc<GitRepo> = self.store.get(repo_id)?;
        self.git_write_files(&repo)?;
        let out = self.git(
            &repo,
            &[
                "status",
                "--porcelain=v1",
                "--branch",
                "--untracked-files=all",
            ],
        )?;
        let mut st = GitStatus {
            branch: String::new(),
            upstream: None,
            ahead: 0,
            behind: 0,
            changes: vec![],
            has_remote: !self
                .git(&repo, &["remote"])
                .unwrap_or_default()
                .trim()
                .is_empty(),
            has_token: self.secrets.get(&token_key(repo_id)).is_some(),
        };
        for line in out.lines() {
            if let Some(b) = line.strip_prefix("## ") {
                parse_branch_line(b, &mut st);
                continue;
            }
            if line.len() < 4 {
                continue;
            }
            let (xy, path) = line.split_at(3);
            let path = path
                .rsplit(" -> ")
                .next()
                .unwrap_or(path)
                .trim_matches('"')
                .to_string();
            let xy = xy.trim();
            let status = match xy {
                "??" => "untracked",
                s if s.contains('U') || s == "AA" || s == "DD" => "conflict",
                s if s.contains('A') => "added",
                s if s.contains('D') => "deleted",
                s if s.contains('R') => "renamed",
                _ => "modified",
            };
            let link = repo.files.iter().find(|f| f.path == path);
            let workspace = link
                .and_then(|l| self.store.get::<Workspace>(&l.workspace_id).ok())
                .map(|w| w.name.clone());
            st.changes.push(GitChange {
                path,
                status: status.into(),
                workspace_id: link.map(|l| l.workspace_id.clone()),
                workspace,
            });
        }
        Ok(st)
    }

    /// Unified diff of one file against the last commit (untracked files show as added).
    pub fn git_diff(&self, repo_id: &str, path: &str) -> Result<String> {
        let repo: Doc<GitRepo> = self.store.get(repo_id)?;
        self.git_write_files(&repo)?;
        let tracked = self.git(&repo, &["ls-files", "--", path])?;
        if tracked.trim().is_empty() {
            let text =
                std::fs::read_to_string(Path::new(&repo.path).join(path)).unwrap_or_default();
            return Ok(text
                .lines()
                .map(|l| format!("+{l}"))
                .collect::<Vec<_>>()
                .join("\n"));
        }
        self.git(&repo, &["diff", "HEAD", "--", path])
            .or_else(|_| self.git(&repo, &["diff", "--", path]))
    }

    /// Commit the given files (all changed workspace files when `paths` is empty).
    pub fn git_commit(&self, repo_id: &str, message: &str, paths: &[String]) -> Result<String> {
        if message.trim().is_empty() {
            return Err(err("write a commit message"));
        }
        let repo: Doc<GitRepo> = self.store.get(repo_id)?;
        self.git_write_files(&repo)?;
        let paths: Vec<String> = if paths.is_empty() {
            self.git_status(repo_id)?
                .changes
                .into_iter()
                .map(|c| c.path)
                .collect()
        } else {
            paths.to_vec()
        };
        if paths.is_empty() {
            return Err(err("nothing to commit"));
        }
        let mut add = vec!["add", "-A", "--"];
        add.extend(paths.iter().map(String::as_str));
        self.git(&repo, &add)?;
        let mut commit = vec!["commit", "-m", message, "--"];
        commit.extend(paths.iter().map(String::as_str));
        self.git(&repo, &commit)?;
        Ok(self
            .git(&repo, &["rev-parse", "--short", "HEAD"])?
            .trim()
            .to_string())
    }

    /// Fetch and merge the upstream branch, then import the result.
    /// Local edits must be committed first; conflicts are reported, not imported.
    pub fn git_pull(&self, repo_id: &str) -> Result<GitSyncResult> {
        let repo: Doc<GitRepo> = self.store.get(repo_id)?;
        let st = self.git_status(repo_id)?;
        let dirty: Vec<&str> = st
            .changes
            .iter()
            .filter(|c| c.status != "untracked" || c.workspace_id.is_some())
            .map(|c| c.path.as_str())
            .collect();
        if !dirty.is_empty() {
            return Err(err(format!(
                "commit or discard your changes first: {}",
                dirty.join(", ")
            )));
        }
        if st.upstream.is_none() {
            let branch = st.branch.clone();
            if st.has_remote
                && self
                    .git(
                        &repo,
                        &["ls-remote", "--exit-code", "--heads", "origin", &branch],
                    )
                    .is_ok()
            {
                self.git(
                    &repo,
                    &["branch", "--set-upstream-to", &format!("origin/{branch}")],
                )?;
            } else {
                return Ok(GitSyncResult {
                    warnings: vec!["nothing to pull: this branch isn't on the remote yet".into()],
                    ..Default::default()
                });
            }
        }
        match self.git(&repo, &["pull", "--no-rebase", "--no-edit"]) {
            Ok(_) => self.git_import_all(repo_id),
            Err(e) => {
                let conflicts: Vec<String> = self
                    .git(&repo, &["diff", "--name-only", "--diff-filter=U"])
                    .unwrap_or_default()
                    .lines()
                    .map(str::to_string)
                    .collect();
                if conflicts.is_empty() {
                    return Err(e);
                }
                Ok(GitSyncResult {
                    conflicts,
                    ..Default::default()
                })
            }
        }
    }

    /// Resolve a conflicted file by taking our (`ours`) or the incoming (`theirs`) version.
    /// When no conflicts remain, the merge is committed and imported.
    pub fn git_resolve(&self, repo_id: &str, path: &str, take: &str) -> Result<GitSyncResult> {
        let repo: Doc<GitRepo> = self.store.get(repo_id)?;
        let side = match take {
            "ours" => "--ours",
            "theirs" => "--theirs",
            other => {
                return Err(err(format!(
                    "resolve with 'ours' or 'theirs', not '{other}'"
                )));
            }
        };
        self.git(&repo, &["checkout", side, "--", path])?;
        self.git(&repo, &["add", "--", path])?;
        let left: Vec<String> = self
            .git(&repo, &["diff", "--name-only", "--diff-filter=U"])?
            .lines()
            .map(str::to_string)
            .collect();
        if !left.is_empty() {
            return Ok(GitSyncResult {
                conflicts: left,
                ..Default::default()
            });
        }
        self.git(&repo, &["commit", "--no-edit"])?;
        self.git_import_all(repo_id)
    }

    /// Abort an in-progress merge.
    pub fn git_abort_merge(&self, repo_id: &str) -> Result<()> {
        let repo: Doc<GitRepo> = self.store.get(repo_id)?;
        self.git(&repo, &["merge", "--abort"])?;
        Ok(())
    }

    pub fn git_push(&self, repo_id: &str) -> Result<String> {
        let repo: Doc<GitRepo> = self.store.get(repo_id)?;
        let st = self.git_status(repo_id)?;
        if !st.has_remote {
            return Err(err("add a remote URL first"));
        }
        let out = self.git(&repo, &["push", "-u", "origin", "HEAD"])?;
        Ok(out.trim().to_string())
    }

    /// Discard local edits to one workspace file and reload it from the last commit.
    pub fn git_discard(&self, repo_id: &str, path: &str) -> Result<GitSyncResult> {
        let repo: Doc<GitRepo> = self.store.get(repo_id)?;
        if self
            .git(&repo, &["ls-files", "--", path])?
            .trim()
            .is_empty()
        {
            return Err(err(format!(
                "{path} has never been committed — unlink the workspace instead"
            )));
        }
        self.git(&repo, &["checkout", "HEAD", "--", path])?;
        self.git_import_all(repo_id)
    }

    pub fn git_log(&self, repo_id: &str, limit: usize) -> Result<Vec<GitCommit>> {
        let repo: Doc<GitRepo> = self.store.get(repo_id)?;
        let n = format!("-n{limit}");
        let out = match self.git(
            &repo,
            &["log", &n, "--format=%h%x1f%an%x1f%ae%x1f%at%x1f%s"],
        ) {
            Ok(o) => o,
            Err(e) if e.to_string().contains("does not have any commits") => return Ok(vec![]),
            Err(e) => return Err(e),
        };
        Ok(out
            .lines()
            .filter_map(|l| {
                let p: Vec<&str> = l.split('\u{1f}').collect();
                (p.len() == 5).then(|| GitCommit {
                    hash: p[0].into(),
                    author: p[1].into(),
                    email: p[2].into(),
                    time_ms: p[3].parse::<i64>().unwrap_or(0) * 1000,
                    message: p[4].into(),
                })
            })
            .collect())
    }

    /// Local and remote branches, and the current one.
    pub fn git_branches(&self, repo_id: &str) -> Result<(String, Vec<String>)> {
        let repo: Doc<GitRepo> = self.store.get(repo_id)?;
        let current = self
            .git(&repo, &["branch", "--show-current"])?
            .trim()
            .to_string();
        let mut all: Vec<String> = self
            .git(&repo, &["branch", "-a", "--format=%(refname:short)"])?
            .lines()
            .map(|b| b.trim().trim_start_matches("origin/").to_string())
            .filter(|b| !b.is_empty() && b != "HEAD" && b != "origin")
            .collect();
        if !current.is_empty() && !all.contains(&current) {
            all.push(current.clone());
        }
        all.sort();
        all.dedup();
        Ok((current, all))
    }

    /// Switch branch (creating it when `create`), then load that branch's workspaces.
    pub fn git_checkout(&self, repo_id: &str, branch: &str, create: bool) -> Result<GitSyncResult> {
        let repo: Doc<GitRepo> = self.store.get(repo_id)?;
        let st = self.git_status(repo_id)?;
        if st.changes.iter().any(|c| c.status != "untracked") {
            return Err(err(
                "commit or discard your changes before switching branches",
            ));
        }
        if create {
            self.git(&repo, &["checkout", "-b", branch])?;
        } else {
            self.git(&repo, &["checkout", branch])?;
        }
        self.git_import_all(repo_id)
    }
}

fn parse_branch_line(b: &str, st: &mut GitStatus) {
    // "main...origin/main [ahead 1, behind 2]" | "No commits yet on main" | "main"
    let b = b
        .trim_start_matches("No commits yet on ")
        .trim_start_matches("Initial commit on ");
    let (names, counts) = b
        .split_once(" [")
        .map(|(n, c)| (n, Some(c.trim_end_matches(']'))))
        .unwrap_or((b, None));
    match names.split_once("...") {
        Some((local, up)) => {
            st.branch = local.to_string();
            st.upstream = Some(up.to_string());
        }
        None => st.branch = names.to_string(),
    }
    for part in counts.unwrap_or("").split(", ") {
        if let Some(n) = part.strip_prefix("ahead ") {
            st.ahead = n.parse().unwrap_or(0);
        } else if let Some(n) = part.strip_prefix("behind ") {
            st.behind = n.parse().unwrap_or(0);
        }
    }
}

fn collect_yaml(root: &Path, dir: &Path, depth: usize, out: &mut Vec<String>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<_> = entries.flatten().collect();
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        let p = e.path();
        let name = e.file_name().to_string_lossy().into_owned();
        if p.is_dir() {
            if depth < 3 && !name.starts_with('.') && name != "node_modules" {
                collect_yaml(root, &p, depth + 1, out);
            }
        } else if (name.ends_with(".yaml") || name.ends_with(".yml"))
            && let Ok(rel) = p.strip_prefix(root)
        {
            out.push(rel.to_string_lossy().replace('\\', "/"));
        }
    }
}
