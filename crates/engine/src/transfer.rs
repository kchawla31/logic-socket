//! Import/export between the store and other formats (see `lsock-convert`), and
//! "generate code" for a rendered request.

use std::collections::{HashMap, HashSet};

use lsock_convert::codegen::{CodeBody, CodeRequest, Part};
use lsock_convert::{Entry, Format, Imported, Item, Meta as CMeta, Node, WorkspaceBundle};
use lsock_core::{
    Auth, CookieJar, Doc, Environment, Folder, GrpcRequest, LlmRequest, McpServer, Meta, Model,
    ProtoFile, RawDoc, RealtimeRequest, Request, Tx, Workspace, mime, new_id, now_ms,
};
use serde::Serialize;

use crate::{Engine, EngineError, Result, vault};

type StoreResult<T> = std::result::Result<T, lsock_core::StoreError>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ImportMode {
    /// Fresh ids: the import becomes a new copy.
    #[default]
    Copy,
    /// Keep ids and overwrite: re-importing our own files (Git sync, backups).
    /// Items missing from the file are removed; local secret values are kept.
    Replace,
}

#[derive(Debug, Clone, Default)]
pub struct ImportOptions {
    pub mode: ImportMode,
    /// Merge into this workspace instead of creating new ones.
    pub into_workspace: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ImportSummary {
    pub format: Format,
    pub workspace_ids: Vec<String>,
    pub workspaces: Vec<String>,
    pub requests: usize,
    pub folders: usize,
    pub environments: usize,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ExportFormat {
    InsomniaV5,
    Postman,
    Har,
}

#[derive(Debug, Clone, Default)]
pub struct ExportOptions {
    /// Include sub-environments marked private.
    pub include_private: bool,
    /// Include the cookie jar's cookies (off for Git).
    pub include_cookies: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Exported {
    pub file_name: String,
    pub content: String,
    pub warnings: Vec<String>,
}

/// Kinds that belong to a workspace's content (and are replaced on re-import).
const SYNCED: &[&str] = &[
    "Folder",
    "Request",
    "RealtimeRequest",
    "GrpcRequest",
    "McpServer",
    "LlmRequest",
    "ProtoFile",
];

fn err(m: impl Into<String>) -> EngineError {
    EngineError::Message(m.into())
}

struct Writer<'a, 'b> {
    tx: &'a mut Tx<'b>,
    mode: ImportMode,
    /// old id → new id (Copy mode)
    ids: HashMap<String, String>,
    written: HashSet<String>,
    counter: f64,
}

impl Writer<'_, '_> {
    fn id_for<T: Model>(&mut self, m: &CMeta) -> String {
        match (&m.id, self.mode) {
            (Some(id), ImportMode::Replace) => id.clone(),
            (Some(old), ImportMode::Copy) => self
                .ids
                .entry(old.clone())
                .or_insert_with(|| new_id(T::PREFIX))
                .clone(),
            (None, _) => new_id(T::PREFIX),
        }
    }

    fn put<T: Model>(&mut self, m: &CMeta, parent: Option<&str>, body: T) -> StoreResult<String> {
        let id = self.id_for::<T>(m);
        self.counter += 1.0;
        let now = now_ms();
        let doc = Doc {
            meta: Meta {
                id: id.clone(),
                kind: T::TYPE.into(),
                parent_id: parent.map(str::to_string),
                sort_key: m.sort_key.unwrap_or(self.counter),
                created: m.created.unwrap_or(now),
                modified: if self.mode == ImportMode::Replace {
                    m.modified.unwrap_or(now)
                } else {
                    now
                },
            },
            body,
        };
        self.tx.put(&doc)?;
        self.written.insert(id.clone());
        Ok(id)
    }

    fn items(&mut self, items: &[Item], parent: &str, n: &mut (usize, usize)) -> StoreResult<()> {
        for it in items {
            match &it.node {
                Node::Folder(f, children) => {
                    let id = self.put(&it.meta, Some(parent), f.clone())?;
                    n.1 += 1;
                    self.items(children, &id, n)?;
                }
                Node::Request(r) => {
                    self.put(&it.meta, Some(parent), r.clone())?;
                    n.0 += 1;
                }
                Node::Realtime(r) => {
                    self.put(&it.meta, Some(parent), r.clone())?;
                    n.0 += 1;
                }
                Node::Grpc(r) => {
                    self.put(&it.meta, Some(parent), r.clone())?;
                    n.0 += 1;
                }
                Node::Mcp(r) => {
                    self.put(&it.meta, Some(parent), r.clone())?;
                    n.0 += 1;
                }
                Node::Llm(r) => {
                    let mut r = r.clone();
                    // MCP servers referenced by id were re-keyed in Copy mode
                    for s in r.mcp_server_ids.iter_mut() {
                        if let Some(new) = self.ids.get(s) {
                            *s = new.clone();
                        }
                    }
                    self.put(&it.meta, Some(parent), r)?;
                    n.0 += 1;
                }
            }
        }
        Ok(())
    }
}

/// Assign Copy-mode ids to MCP servers before AI requests are written (they may come first).
fn pre_key_mcp(items: &[Item], ids: &mut HashMap<String, String>) {
    for it in items {
        match &it.node {
            Node::Folder(_, c) => pre_key_mcp(c, ids),
            Node::Mcp(_) => {
                if let Some(old) = &it.meta.id {
                    ids.entry(old.clone())
                        .or_insert_with(|| new_id(McpServer::PREFIX));
                }
            }
            _ => {}
        }
    }
}

impl Engine {
    /// Detect the format of `text` and import it.
    pub fn import_text(&self, text: &str, opts: &ImportOptions) -> Result<ImportSummary> {
        let imported = lsock_convert::import(text).map_err(|e| err(e.to_string()))?;
        self.import_bundles(imported, opts)
    }

    pub fn import_bundles(
        &self,
        imported: Imported,
        opts: &ImportOptions,
    ) -> Result<ImportSummary> {
        let mut summary = ImportSummary {
            format: imported.format,
            workspace_ids: vec![],
            workspaces: vec![],
            requests: 0,
            folders: 0,
            environments: 0,
            warnings: imported.warnings,
        };
        for b in imported.workspaces {
            let (id, n) = self.import_bundle(b, opts)?;
            summary.requests += n.0;
            summary.folders += n.1;
            summary.environments += n.2;
            summary
                .workspaces
                .push(self.store.get::<Workspace>(&id)?.name.clone());
            summary.workspace_ids.push(id);
        }
        Ok(summary)
    }

    fn import_bundle(
        &self,
        mut b: WorkspaceBundle,
        opts: &ImportOptions,
    ) -> Result<(String, (usize, usize, usize))> {
        // secrets that arrive as plaintext (e.g. Postman "secret" variables) are sealed now
        for e in b.base_env.iter_mut().chain(b.sub_envs.iter_mut()) {
            self.seal_env(&mut e.body)?;
        }
        if let Some(target) = &opts.into_workspace {
            return self.merge_into(target, b);
        }
        let mode = opts.mode;
        let ws_id = match (&b.meta.id, mode) {
            (Some(id), ImportMode::Replace) => id.clone(),
            _ => new_id(Workspace::PREFIX),
        };
        let existing: Vec<RawDoc> = if mode == ImportMode::Replace {
            self.store.descendants(&ws_id)?
        } else {
            vec![]
        };
        let existing_ws: Option<Doc<Workspace>> = self.store.get::<Workspace>(&ws_id).ok();
        let existing_envs: HashMap<String, Environment> = existing
            .iter()
            .filter(|d| d.meta.kind == "Environment")
            .filter_map(|d| {
                d.typed::<Environment>()
                    .ok()
                    .map(|e| (e.meta.id.clone(), e.body))
            })
            .collect();
        let mut counts = (0usize, 0usize, 0usize);
        self.store.batch(|tx| {
            let mut w = Writer {
                tx,
                mode,
                ids: HashMap::new(),
                written: HashSet::new(),
                counter: 0.0,
            };
            if let Some(old) = &b.meta.id {
                w.ids.insert(old.clone(), ws_id.clone());
            }
            if mode == ImportMode::Copy {
                pre_key_mcp(&b.items, &mut w.ids);
            }
            // keep this machine's choices (active environments) when replacing
            let mut ws = b.workspace.clone();
            if let Some(old) = &existing_ws {
                ws.active_environment_id = old.active_environment_id.clone();
                ws.active_global_base_id = old.active_global_base_id.clone();
                ws.active_global_sub_id = old.active_global_sub_id.clone();
            }
            let now = now_ms();
            w.tx.put(&Doc {
                meta: Meta {
                    id: ws_id.clone(),
                    kind: Workspace::TYPE.into(),
                    parent_id: None,
                    sort_key: existing_ws
                        .as_ref()
                        .map(|d| d.meta.sort_key)
                        .unwrap_or(now as f64),
                    created: b.meta.created.unwrap_or(now),
                    // keep the file's timestamp so a re-export is byte-identical (no phantom Git diffs)
                    modified: if mode == ImportMode::Replace {
                        b.meta.modified.unwrap_or(now)
                    } else {
                        now
                    },
                },
                body: ws,
            })?;

            let base = b.base_env.clone().unwrap_or_else(|| {
                Entry::new(Environment {
                    name: "Base Environment".into(),
                    ..Default::default()
                })
            });
            let keep_local = |e: &Entry<Environment>, id: &str| -> Environment {
                let mut body = e.body.clone();
                if let Some(local) = existing_envs.get(id) {
                    // files never carry secret values: keep the ones stored here
                    for k in &body.secret_keys {
                        let incoming_empty =
                            body.data.get(k).is_none_or(|v| v.as_str() == Some(""));
                        if incoming_empty
                            && let Some(v) = local.data.get(k).filter(|v| vault::is_sealed(v))
                        {
                            body.data.insert(k.clone(), v.clone());
                        }
                    }
                }
                body
            };
            let base_id = w.id_for::<Environment>(&base.meta);
            let base_body = keep_local(&base, &base_id);
            let base_id = w.put(&base.meta, Some(&ws_id), base_body)?;
            counts.2 += 1;
            for s in &b.sub_envs {
                let sid = w.id_for::<Environment>(&s.meta);
                let body = keep_local(s, &sid);
                w.put(&s.meta, Some(&base_id), body)?;
                counts.2 += 1;
            }
            if let Some(j) = &b.cookie_jar {
                w.put(&j.meta, Some(&ws_id), j.body.clone())?;
            }
            for p in &b.protos {
                w.put(&p.meta, Some(&ws_id), p.body.clone())?;
            }
            let mut n = (0, 0);
            w.items(&b.items, &ws_id, &mut n)?;
            counts.0 = n.0;
            counts.1 = n.1;

            if mode == ImportMode::Replace {
                // remove what the file no longer has; private sub-environments and
                // local-only data (responses, runs, tokens) are kept
                let written = w.written.clone();
                for d in &existing {
                    let synced = SYNCED.contains(&d.meta.kind.as_str());
                    let env = d.meta.kind == "Environment"
                        && existing_envs.get(&d.meta.id).is_some_and(|e| !e.is_private);
                    if (synced || env) && !written.contains(&d.meta.id) {
                        w.tx.delete(&d.meta.id)?;
                    }
                }
            }
            Ok(())
        })?;
        Ok((ws_id, counts))
    }

    /// Add a bundle's contents to an existing workspace (new ids).
    fn merge_into(
        &self,
        ws_id: &str,
        b: WorkspaceBundle,
    ) -> Result<(String, (usize, usize, usize))> {
        let base = self.base_environment(ws_id)?;
        let mut jar = self.cookie_jar(ws_id)?;
        let mut counts = (0, 0, 0);
        self.store.batch(|tx| {
            let mut w = Writer {
                tx,
                mode: ImportMode::Copy,
                ids: HashMap::new(),
                written: HashSet::new(),
                counter: now_ms() as f64,
            };
            pre_key_mcp(&b.items, &mut w.ids);
            if let Some(e) = &b.base_env {
                // add variables the workspace doesn't define yet
                let mut base = base.clone();
                for (k, v) in &e.body.data {
                    if !base.data.contains_key(k) {
                        base.body.data.insert(k.clone(), v.clone());
                        if e.body.secret_keys.contains(k) {
                            base.body.secret_keys.push(k.clone());
                        }
                    }
                }
                w.tx.update(&base)?;
            }
            for s in &b.sub_envs {
                w.put(
                    &CMeta {
                        id: None,
                        ..s.meta.clone()
                    },
                    Some(base.id()),
                    s.body.clone(),
                )?;
                counts.2 += 1;
            }
            if let Some(j) = &b.cookie_jar {
                jar.body.cookies.extend(j.body.cookies.iter().cloned());
                w.tx.update(&jar)?;
            }
            for p in &b.protos {
                w.put(
                    &CMeta {
                        id: None,
                        ..p.meta.clone()
                    },
                    Some(ws_id),
                    p.body.clone(),
                )?;
            }
            let mut n = (0, 0);
            w.items(&b.items, ws_id, &mut n)?;
            counts.0 = n.0;
            counts.1 = n.1;
            Ok(())
        })?;
        Ok((ws_id.to_string(), counts))
    }

    /// Read a workspace into a bundle. Secret values are blanked; their names are kept.
    pub fn export_bundle(
        &self,
        workspace_id: &str,
        opts: &ExportOptions,
    ) -> Result<WorkspaceBundle> {
        let ws: Doc<Workspace> = self.store.get(workspace_id)?;
        let meta = |m: &Meta| CMeta {
            id: Some(m.id.clone()),
            sort_key: Some(m.sort_key),
            created: Some(m.created),
            modified: Some(m.modified),
        };
        let blank = |mut e: Environment| {
            for k in &e.secret_keys {
                if let Some(v) = e.data.get_mut(k) {
                    *v = serde_json::Value::String(String::new());
                }
            }
            e
        };
        let mut b = WorkspaceBundle {
            meta: CMeta {
                sort_key: None,
                ..meta(&ws.meta)
            },
            workspace: Workspace {
                active_environment_id: None,
                active_global_base_id: None,
                active_global_sub_id: None,
                ..ws.body.clone()
            },
            ..Default::default()
        };
        let base = self.base_environment(workspace_id)?;
        b.base_env = Some(Entry {
            meta: CMeta {
                sort_key: None,
                ..meta(&base.meta)
            },
            body: blank(base.body.clone()),
        });
        for s in self.store.children::<Environment>(base.id())? {
            if s.is_private && !opts.include_private {
                continue;
            }
            b.sub_envs.push(Entry {
                meta: meta(&s.meta),
                body: blank(s.body.clone()),
            });
        }
        if let Some(j) = self
            .store
            .children::<CookieJar>(workspace_id)?
            .into_iter()
            .next()
        {
            let mut body = j.body.clone();
            if !opts.include_cookies {
                body.cookies.clear();
            }
            b.cookie_jar = Some(Entry {
                meta: CMeta {
                    sort_key: None,
                    ..meta(&j.meta)
                },
                body,
            });
        }
        for p in self.store.children::<ProtoFile>(workspace_id)? {
            b.protos.push(Entry {
                meta: meta(&p.meta),
                body: p.body.clone(),
            });
        }
        b.items = self.export_items(workspace_id, &meta)?;
        Ok(b)
    }

    fn export_items(&self, parent: &str, meta: &dyn Fn(&Meta) -> CMeta) -> Result<Vec<Item>> {
        let mut out = vec![];
        let mut kids = self.store.all_children(parent)?;
        kids.sort_by(|a, b| a.meta.sort_key.total_cmp(&b.meta.sort_key));
        for d in kids {
            let node = match d.meta.kind.as_str() {
                "Folder" => Node::Folder(
                    d.typed::<Folder>()?.body,
                    self.export_items(&d.meta.id, meta)?,
                ),
                "Request" => Node::Request(d.typed::<Request>()?.body),
                "RealtimeRequest" => Node::Realtime(d.typed::<RealtimeRequest>()?.body),
                "GrpcRequest" => Node::Grpc(d.typed::<GrpcRequest>()?.body),
                "McpServer" => Node::Mcp(d.typed::<McpServer>()?.body),
                "LlmRequest" => Node::Llm(d.typed::<LlmRequest>()?.body),
                _ => continue,
            };
            out.push(Item {
                meta: meta(&d.meta),
                node,
            });
        }
        Ok(out)
    }

    pub fn export_workspace(
        &self,
        workspace_id: &str,
        format: ExportFormat,
        opts: &ExportOptions,
    ) -> Result<Exported> {
        let b = self.export_bundle(workspace_id, opts)?;
        let slug = lsock_convert::insomnia::file_name_for(&b.workspace.name);
        let stem = slug
            .trim_start_matches("insomnia.")
            .trim_end_matches(".yaml")
            .to_string();
        let mut warnings = vec![];
        let secrets: usize = b
            .base_env
            .iter()
            .chain(b.sub_envs.iter())
            .map(|e| e.body.secret_keys.len())
            .sum();
        if secrets > 0 {
            warnings.push(format!(
                "{secrets} secret value(s) were left out; only their names are exported"
            ));
        }
        let (file_name, content) = match format {
            ExportFormat::InsomniaV5 => (
                slug,
                lsock_convert::insomnia::export_v5(&b).map_err(|e| err(e.to_string()))?,
            ),
            ExportFormat::Postman => {
                let (json, skipped) = lsock_convert::postman::export_collection(&b);
                if skipped > 0 {
                    warnings.push(format!("{skipped} item(s) have no Postman equivalent (gRPC, WebSocket/SSE/Socket.IO, MCP, AI) and were skipped"));
                }
                (format!("{stem}.postman_collection.json"), json)
            }
            ExportFormat::Har => (format!("{stem}.har"), lsock_convert::har::export(&b)),
        };
        Ok(Exported {
            file_name,
            content,
            warnings,
        })
    }

    /// The request as it would be sent (rendered, inherited, auth applied) for code generation.
    pub fn code_request(&self, request_id: &str) -> Result<(CodeRequest, Vec<String>)> {
        let p = self.prepare(request_id, &[])?;
        let mut notes = vec![];
        let auth = match &p.auth {
            Auth::OAuth2(cfg) if !cfg.disabled => {
                let owner = p.auth_owner.as_deref().unwrap_or(&p.workspace_id);
                let token = self.oauth2_cached(owner)?.map(|t| t.body.access_token).unwrap_or_else(|| {
                    notes.push("No OAuth 2 token yet — replace <access-token> or click Get token first".into());
                    "<access-token>".into()
                });
                Auth::Bearer {
                    token,
                    prefix: Some(cfg.token_prefix.clone()).filter(|x| !x.is_empty()),
                    disabled: false,
                }
            }
            Auth::Digest {
                disabled: false, ..
            } => {
                notes.push("Digest auth needs a challenge round-trip; use your client's digest support (e.g. curl --digest)".into());
                Auth::None
            }
            Auth::OAuth1(c) if !c.disabled => {
                notes.push("OAuth 1 signatures depend on time and nonce; sign with your client's OAuth 1 library".into());
                Auth::None
            }
            Auth::Iam(c) if !c.disabled => {
                notes.push(
                    "AWS SigV4 signatures expire; sign with the AWS SDK or curl --aws-sigv4".into(),
                );
                Auth::None
            }
            other => other.clone(),
        };
        let mut url = lsock_http::build_url(&p.request, p.request.settings.encode_url)
            .map_err(|e| err(e.to_string()))?;
        let mut headers: Vec<(String, String)> = p
            .request
            .headers
            .iter()
            .filter(|h| !h.disabled && !h.name.is_empty())
            .map(|h| (h.name.clone(), h.value.clone()))
            .collect();
        let mut cookies = vec![];
        lsock_http::apply_auth(&auth, &mut headers, &mut url, &mut cookies);
        if !cookies.is_empty() {
            headers.push(("Cookie".into(), cookies.join("; ")));
        }
        let b = &p.request.body;
        let body = match b.mime_type.as_deref() {
            Some(mime::FORM) => CodeBody::Form {
                fields: b
                    .params
                    .iter()
                    .filter(|x| !x.disabled)
                    .map(|x| (x.name.clone(), x.value.clone()))
                    .collect(),
            },
            Some(mime::MULTIPART) => {
                headers.retain(|(k, _)| !k.eq_ignore_ascii_case("content-type"));
                CodeBody::Multipart {
                    parts: b
                        .params
                        .iter()
                        .filter(|x| !x.disabled)
                        .map(|x| Part {
                            name: x.name.clone(),
                            value: x.value.clone(),
                            file: (x.kind.as_deref() == Some("file"))
                                .then(|| x.file_name.clone().unwrap_or_default()),
                        })
                        .collect(),
                }
            }
            Some(mime::FILE) => CodeBody::File {
                path: b.file_name.clone().unwrap_or_default(),
            },
            _ => match &b.text {
                Some(t) if !t.is_empty() => CodeBody::Text { text: t.clone() },
                _ => CodeBody::None,
            },
        };
        if let Some(m) = b.mime_type.as_deref().filter(|m| *m != mime::MULTIPART)
            && !matches!(body, CodeBody::None)
            && !headers
                .iter()
                .any(|(k, _)| k.eq_ignore_ascii_case("content-type"))
        {
            let ct = if m == mime::GRAPHQL { mime::JSON } else { m };
            headers.push(("Content-Type".into(), ct.into()));
        }
        Ok((
            CodeRequest {
                method: p.request.method.clone(),
                url: url.to_string(),
                headers,
                body,
            },
            notes,
        ))
    }
}
