//! Full request pipeline with scripts, in this order:
//!
//! folder pre-request scripts (outermost first) → request pre-request script
//! → render → send → request after-response script → folder after-response
//! scripts (innermost first) → persist response + environment + cookies.

use std::sync::Arc;
use std::time::Duration;

use base64::Engine as _;
use futures::future::BoxFuture;
use lsock_core::{
    ConsoleEntry, CookieJar, Doc, Environment, Folder, RawDoc, Request, Response, TestResult,
    VarMap, Workspace,
};
use lsock_scripting::{
    Event, FolderVars, Host, HostRequest, Info, Limits, NamedVars, ScriptInput, ScriptOutput,
    ScriptResponseData,
};
use lsock_templating::Layer;

use crate::{Engine, EngineError, Result};

/// State carried across requests in a run (iteration data, script-local vars).
#[derive(Debug, Clone, Default)]
pub struct RunState {
    pub iteration_data: VarMap,
    /// `ls.variables.set` values; highest render precedence.
    pub local_variables: VarMap,
    pub iteration: u32,
    pub iteration_count: u32,
}

#[derive(Debug, Clone)]
pub struct Outcome {
    pub response: Doc<Response>,
    /// `ls.execution.skipRequest()` was called; nothing was sent.
    pub skipped: bool,
    /// From `ls.execution.setNextRequest`; `"__stop__"` for `null`.
    pub next_request: Option<String>,
}

/// `ls.sendRequest` backed by the HTTP engine.
struct EngineHost {
    options: lsock_http::Options,
}

impl Host for EngineHost {
    fn send(
        &self,
        req: HostRequest,
    ) -> BoxFuture<'static, std::result::Result<ScriptResponseData, String>> {
        let options = lsock_http::Options {
            send_cookies: false,
            store_cookies: false,
            ..self.options.clone()
        };
        Box::pin(async move {
            let mut r = Request {
                method: req.method,
                url: req.url,
                ..Default::default()
            };
            r.headers = req
                .headers
                .into_iter()
                .map(|(k, v)| lsock_core::KeyValue::new(k, v))
                .collect();
            r.body.text = req.body;
            let resp = lsock_http::send(&r, &lsock_core::Auth::None, &options, &mut vec![])
                .await
                .map_err(|e| e.to_string())?;
            Ok(ScriptResponseData {
                code: resp.status,
                status: resp.status_text.clone(),
                headers: resp
                    .headers
                    .iter()
                    .map(|h| lsock_scripting::ScriptKv {
                        key: h.name.clone(),
                        value: h.value.clone(),
                        disabled: false,
                    })
                    .collect(),
                body: resp.text(),
                response_time: resp.timings.total_ms,
            })
        })
    }
}

/// Environments a script can mutate.
struct Envs {
    base: Doc<Environment>,
    sub: Option<Doc<Environment>>,
    global: Option<Doc<Environment>>,
}

impl Envs {
    fn named(d: &Option<Doc<Environment>>) -> NamedVars {
        d.as_ref()
            .map(|e| NamedVars {
                name: e.name.clone(),
                data: e.data.clone(),
            })
            .unwrap_or_default()
    }

    /// What scripts see as `ls.environment`: the active sub-environment,
    /// or the base environment when none is active.
    fn script_env(&self) -> NamedVars {
        match &self.sub {
            Some(e) => NamedVars {
                name: e.name.clone(),
                data: e.data.clone(),
            },
            None => NamedVars {
                name: self.base.name.clone(),
                data: self.base.data.clone(),
            },
        }
    }

    /// Apply a script's results; returns the docs that changed.
    fn apply(&mut self, out: &ScriptOutput) -> Vec<Doc<Environment>> {
        let mut changed = vec![];
        match &mut self.sub {
            Some(s) => {
                if s.data != out.environment {
                    s.data = out.environment.clone();
                    changed.push(s.clone());
                }
                if self.base.data != out.base_environment {
                    self.base.data = out.base_environment.clone();
                    changed.push(self.base.clone());
                }
            }
            None => {
                // Both `environment` and `baseEnvironment` were views of the base:
                // start from baseEnvironment's result and replay environment's edits.
                let before = self.base.data.clone();
                let mut merged = out.base_environment.clone();
                for (k, v) in &out.environment {
                    if before.get(k) != Some(v) {
                        merged.insert(k.clone(), v.clone());
                    }
                }
                for k in before.keys() {
                    if !out.environment.contains_key(k) {
                        merged.shift_remove(k);
                    }
                }
                if merged != before {
                    self.base.data = merged;
                    changed.push(self.base.clone());
                }
            }
        }
        if let Some(g) = &mut self.global
            && g.data != out.globals
        {
            g.data = out.globals.clone();
            changed.push(g.clone());
        }
        changed
    }
}

fn console_entries(out: &ScriptOutput, source: &str) -> Vec<ConsoleEntry> {
    out.console
        .iter()
        .map(|c| ConsoleEntry {
            level: c.level.clone(),
            text: c.text.clone(),
            timestamp_ms: c.timestamp_ms,
            source: source.to_string(),
        })
        .collect()
}

fn test_results(out: &ScriptOutput) -> Vec<TestResult> {
    out.tests
        .iter()
        .map(|t| TestResult {
            name: t.name.clone(),
            passed: t.status == "passed",
            skipped: t.status == "skipped",
            error: t.error.clone(),
            duration_ms: t.duration_ms,
            category: t.category.clone(),
        })
        .collect()
}

impl Engine {
    /// Run the full pipeline for one request (scripts + send + persist).
    pub async fn send_with_state(&self, request_id: &str, state: &mut RunState) -> Result<Outcome> {
        let doc: Doc<Request> = self.store.get(request_id)?;
        let mut req = doc.body.clone();
        let ws: Doc<Workspace> = self.workspace_of(request_id)?;
        let settings = self.store.settings()?.body;
        let limits = Limits {
            timeout: Duration::from_millis(settings.script_timeout_ms.max(100)),
            ..Default::default()
        };
        let folders: Vec<Doc<Folder>> = self
            .store
            .ancestors(request_id)?
            .iter()
            .rev()
            .filter(|d| d.meta.kind == "Folder")
            .map(RawDoc::typed)
            .collect::<std::result::Result<_, _>>()?; // outermost first

        let base = self.base_environment(ws.id())?;
        let sub = ws
            .active_environment_id
            .as_ref()
            .and_then(|id| self.store.get::<Environment>(id).ok())
            .filter(|e| e.meta.parent_id.as_deref() == Some(base.id()));
        let global = ws
            .active_global_sub_id
            .as_ref()
            .or(ws.active_global_base_id.as_ref())
            .and_then(|id| self.store.get::<Environment>(id).ok());
        // scripts work on decrypted values; `sealed()` re-encrypts before saving
        let open = |d: Doc<Environment>| -> Result<Doc<Environment>> {
            let data = self.open_env_data(&d)?;
            Ok(Doc {
                body: Environment { data, ..d.body },
                meta: d.meta,
            })
        };
        let base = open(base)?;
        let sub = sub.map(open).transpose()?;
        let global = global.map(open).transpose()?;
        let mut envs = Envs { base, sub, global };
        let mut jar: Doc<CookieJar> = self.cookie_jar(ws.id())?;
        let mut jar_changed = false;
        let mut console = vec![];
        let mut tests = vec![];
        let mut next_request = None;
        let folder_vars: Vec<FolderVars> = folders
            .iter()
            .map(|f| FolderVars {
                name: f.name.clone(),
                environment: f.environment.clone(),
            })
            .collect();
        let mut location = vec![ws.name.clone()];
        location.extend(folders.iter().map(|f| f.name.clone()));

        let host: Arc<dyn Host> = Arc::new(EngineHost {
            options: lsock_http::Options {
                timeout: Duration::from_millis(settings.timeout_ms.max(1)),
                validate_certificates: settings.validate_certificates,
                ..Default::default()
            },
        });

        let make_input = |script: &str,
                          event: Event,
                          req: &Request,
                          envs: &Envs,
                          jar: &CookieJar,
                          state: &RunState,
                          response: Option<ScriptResponseData>| {
            ScriptInput {
                script: script.to_string(),
                event: Some(event),
                request: lsock_scripting::to_script_request(request_id, req),
                response,
                environment: envs.script_env(),
                base_environment: NamedVars {
                    name: envs.base.name.clone(),
                    data: envs.base.data.clone(),
                },
                globals: Envs::named(&envs.global),
                iteration_data: state.iteration_data.clone(),
                local_variables: state.local_variables.clone(),
                folders: folder_vars.clone(),
                cookies: jar.cookies.clone(),
                info: Info {
                    event_name: String::new(),
                    iteration: state.iteration.max(1),
                    iteration_count: state.iteration_count.max(1),
                    request_name: req.name.clone(),
                    request_id: request_id.to_string(),
                },
                location: location.clone(),
            }
        };

        // ---- pre-request scripts
        let mut pre_scripts: Vec<(String, String)> = folders
            .iter()
            .filter_map(|f| {
                f.pre_request_script
                    .as_ref()
                    .filter(|s| !s.trim().is_empty())
                    .map(|s| (format!("pre-request: folder {}", f.name), s.clone()))
            })
            .collect();
        if let Some(s) = req
            .pre_request_script
            .as_ref()
            .filter(|s| !s.trim().is_empty())
        {
            pre_scripts.push(("pre-request".into(), s.clone()));
        }
        let mut pre_env_changes: Vec<Doc<Environment>> = vec![];
        for (source, script) in &pre_scripts {
            let input = make_input(script, Event::PreRequest, &req, &envs, &jar, state, None);
            let out =
                lsock_scripting::run(input, limits, host.clone(), self.renderer.clone()).await;
            console.extend(console_entries(&out, source));
            tests.extend(test_results(&out));
            for c in envs.apply(&out) {
                pre_env_changes.retain(|x| x.meta.id != c.meta.id);
                pre_env_changes.push(c);
            }
            state.local_variables = out.local_variables.clone();
            if let Some(c) = &out.cookies {
                jar.cookies = c.clone();
                jar_changed = true;
            }
            if let Some(r) = &out.request {
                lsock_scripting::apply_script_request(&mut req, r);
            }
            if out.execution.next_request.is_some() {
                next_request = out.execution.next_request.clone();
            }
            if let Some(err) = &out.error {
                let resp = Response {
                    method: req.method.clone(),
                    url: req.url.clone(),
                    error: Some(format!("Pre-request script error ({source}): {err}")),
                    script_error: Some(format!("{source}: {err}")),
                    test_results: tests,
                    console,
                    ..Default::default()
                };
                let response = self.persist(
                    request_id,
                    resp,
                    jar_changed.then_some(jar),
                    &pre_env_changes,
                )?;
                return Ok(Outcome {
                    response,
                    skipped: false,
                    next_request,
                });
            }
            if out.execution.skip_request {
                let resp = Response {
                    method: req.method.clone(),
                    url: req.url.clone(),
                    error: Some(
                        "Request skipped by pre-request script (ls.execution.skipRequest)".into(),
                    ),
                    test_results: tests,
                    console,
                    ..Default::default()
                };
                let response = self.persist(
                    request_id,
                    resp,
                    jar_changed.then_some(jar),
                    &pre_env_changes,
                )?;
                return Ok(Outcome {
                    response,
                    skipped: true,
                    next_request,
                });
            }
        }
        // Persist environment changes before rendering so `{{ vars }}` see them.
        if !pre_env_changes.is_empty() || jar_changed {
            let sealed = self.sealed(&pre_env_changes)?;
            self.store.batch(|tx| {
                for e in &sealed {
                    tx.update(e)?;
                }
                if jar_changed {
                    tx.update(&jar)?;
                }
                Ok(())
            })?;
            jar = self.cookie_jar(ws.id())?;
            jar_changed = false;
        }

        // ---- render + send
        let mut extra = vec![];
        if !state.iteration_data.is_empty() {
            extra.push(Layer::new("Iteration data", state.iteration_data.clone()));
        }
        if !state.local_variables.is_empty() {
            extra.push(Layer::new(
                "Script variables",
                state.local_variables.clone(),
            ));
        }
        let (mut resp, new_jar) = match self.prepare_request(request_id, &req, &extra) {
            Ok(prepared) => self.execute(&prepared).await?,
            Err(EngineError::Render { field, source }) => (
                Response {
                    method: req.method.clone(),
                    url: req.url.clone(),
                    error: Some(format!("Could not render {field}: {source}")),
                    ..Default::default()
                },
                None,
            ),
            Err(e) => return Err(e),
        };
        if let Some(j) = new_jar {
            jar = j;
            jar_changed = true;
        }

        // ---- after-response scripts (request first, then folders innermost first)
        let mut post_env_changes: Vec<Doc<Environment>> = vec![];
        if resp.error.is_none() {
            let mut after_scripts: Vec<(String, String)> = vec![];
            if let Some(s) = req
                .after_response_script
                .as_ref()
                .filter(|s| !s.trim().is_empty())
            {
                after_scripts.push(("after-response".into(), s.clone()));
            }
            after_scripts.extend(folders.iter().rev().filter_map(|f| {
                f.after_response_script
                    .as_ref()
                    .filter(|s| !s.trim().is_empty())
                    .map(|s| (format!("after-response: folder {}", f.name), s.clone()))
            }));
            let body = self.response_body(&resp);
            let response_data = ScriptResponseData {
                code: resp.status_code,
                status: resp.status_message.clone(),
                headers: resp
                    .headers
                    .iter()
                    .map(|h| lsock_scripting::ScriptKv {
                        key: h.name.clone(),
                        value: h.value.clone(),
                        disabled: false,
                    })
                    .collect(),
                body: String::from_utf8_lossy(&body).into_owned(),
                response_time: resp.timings.total_ms,
            };
            for (source, script) in &after_scripts {
                let input = make_input(
                    script,
                    Event::AfterResponse,
                    &req,
                    &envs,
                    &jar,
                    state,
                    Some(response_data.clone()),
                );
                let out =
                    lsock_scripting::run(input, limits, host.clone(), self.renderer.clone()).await;
                console.extend(console_entries(&out, source));
                tests.extend(test_results(&out));
                for c in envs.apply(&out) {
                    post_env_changes.retain(|x| x.meta.id != c.meta.id);
                    post_env_changes.push(c);
                }
                state.local_variables = out.local_variables.clone();
                if let Some(c) = &out.cookies {
                    jar.cookies = c.clone();
                    jar_changed = true;
                }
                if out.execution.next_request.is_some() {
                    next_request = out.execution.next_request.clone();
                }
                if let Some(err) = &out.error {
                    resp.script_error = Some(format!("{source}: {err}"));
                    break;
                }
            }
        }
        resp.test_results = tests;
        resp.console = console;
        if resp.body_b64.is_none() && resp.body_path.is_none() && resp.error.is_none() {
            resp.body_b64 = Some(base64::engine::general_purpose::STANDARD.encode(b""));
        }
        let response = self.persist(
            request_id,
            resp,
            jar_changed.then_some(jar),
            &post_env_changes,
        )?;
        Ok(Outcome {
            response,
            skipped: false,
            next_request,
        })
    }
}
