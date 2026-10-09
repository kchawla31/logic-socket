//! Sandboxed pre-request / after-response scripts (QuickJS via rquickjs).
//!
//! The `ls.*` object model lives in `prelude.js`; this crate provides
//! the host functions (HTTP, templating, timers, logging, vendored modules),
//! resource limits, and conversion between `lsock_core::Request` and the
//! script-facing request shape.

mod convert;

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use futures::future::BoxFuture;
use lsock_core::{Cookie, VarMap};
use lsock_templating::{Context, Layer, Mode, Renderer};
use rquickjs::context::EvalOptions;
use rquickjs::loader::{ImportAttributes, Loader, Resolver};
use rquickjs::prelude::{Async, Func};
use rquickjs::{
    AsyncContext, AsyncRuntime, CatchResultExt, CaughtError, Ctx, Function, Module, Object,
    Promise, Value,
};
use serde::{Deserialize, Serialize};
use serde_json::Value as Json;

pub use convert::{apply_script_request, to_script_request};

const PRELUDE: &str = include_str!("prelude.js");

fn vendored(name: &str) -> Option<&'static str> {
    Some(match name {
        "chai" => include_str!("../vendor/chai.js"),
        "lodash" => include_str!("../vendor/lodash.min.js"),
        "crypto-js" => include_str!("../vendor/crypto-js.js"),
        "moment" => include_str!("../vendor/moment.min.js"),
        "tv4" => include_str!("../vendor/tv4.js"),
        "ajv" => include_str!("../vendor/ajv.bundle.js"),
        _ => return None,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Event {
    #[serde(rename = "prerequest")]
    PreRequest,
    #[serde(rename = "afterResponse")]
    AfterResponse,
}

/// Request as seen by scripts (Postman-style shape).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ScriptRequest {
    pub id: String,
    pub name: String,
    pub method: String,
    pub url: String,
    pub query: Vec<ScriptKv>,
    pub headers: Vec<ScriptKv>,
    pub body: Json,
    pub auth: Json,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct ScriptKv {
    pub key: String,
    #[serde(default)]
    pub value: String,
    #[serde(default)]
    pub disabled: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScriptResponseData {
    pub code: u16,
    pub status: String,
    pub headers: Vec<ScriptKv>,
    pub body: String,
    pub response_time: f64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct NamedVars {
    pub name: String,
    pub data: VarMap,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FolderVars {
    pub name: String,
    pub environment: VarMap,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Info {
    pub event_name: String,
    pub iteration: u32,
    pub iteration_count: u32,
    pub request_name: String,
    pub request_id: String,
}

/// Everything a script can read. Mutations come back in [`ScriptOutput`].
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScriptInput {
    #[serde(skip)]
    pub script: String,
    #[serde(skip)]
    pub event: Option<Event>,
    pub request: ScriptRequest,
    pub response: Option<ScriptResponseData>,
    pub environment: NamedVars,
    pub base_environment: NamedVars,
    pub globals: NamedVars,
    pub iteration_data: VarMap,
    pub local_variables: VarMap,
    /// Outermost first.
    pub folders: Vec<FolderVars>,
    pub cookies: Vec<Cookie>,
    pub info: Info,
    pub location: Vec<String>,
}

#[derive(Debug, Clone, Copy)]
pub struct Limits {
    pub timeout: Duration,
    pub memory_bytes: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(5),
            memory_bytes: 64 * 1024 * 1024,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScriptTest {
    pub name: String,
    /// `passed` | `failed` | `skipped`
    pub status: String,
    pub error: Option<String>,
    pub duration_ms: f64,
    pub category: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConsoleLine {
    pub level: String,
    pub text: String,
    pub timestamp_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScriptError {
    pub message: String,
    /// 1-based line/column in the user's script, when known.
    pub line: Option<u32>,
    pub column: Option<u32>,
}

impl std::fmt::Display for ScriptError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match (self.line, self.column) {
            (Some(l), Some(c)) => write!(f, "{} (line {l}, column {c})", self.message),
            (Some(l), None) => write!(f, "{} (line {l})", self.message),
            _ => write!(f, "{}", self.message),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Execution {
    pub skip_request: bool,
    /// `Some("__stop__")` when `setNextRequest(null)` was called.
    pub next_request: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ScriptOutput {
    pub request: Option<ScriptRequest>,
    pub environment: VarMap,
    pub base_environment: VarMap,
    pub globals: VarMap,
    pub local_variables: VarMap,
    /// `Some` only when the script changed the cookie jar.
    pub cookies: Option<Vec<Cookie>>,
    pub tests: Vec<ScriptTest>,
    pub console: Vec<ConsoleLine>,
    pub execution: Execution,
    pub error: Option<ScriptError>,
    pub duration_ms: f64,
}

/// Outgoing request from `ls.sendRequest`.
#[derive(Debug, Clone, Deserialize)]
pub struct HostRequest {
    pub method: String,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Option<String>,
}

/// Capabilities the sandbox borrows from the embedding engine.
pub trait Host: Send + Sync + 'static {
    /// Perform an HTTP request for `ls.sendRequest`.
    fn send(&self, req: HostRequest) -> BoxFuture<'static, Result<ScriptResponseData, String>>;
}

/// A host that refuses network access (tests, previews).
pub struct NoNetwork;

impl Host for NoNetwork {
    fn send(&self, _req: HostRequest) -> BoxFuture<'static, Result<ScriptResponseData, String>> {
        Box::pin(async { Err("network access is disabled in this context".to_string()) })
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawOutput {
    request: ScriptRequest,
    environment: VarMap,
    base_environment: VarMap,
    globals: VarMap,
    local_variables: VarMap,
    cookies: Option<Vec<Cookie>>,
    tests: Vec<ScriptTest>,
    execution: Execution,
    error: Option<RawError>,
}

#[derive(Deserialize)]
struct RawError {
    message: String,
    stack: String,
}

const SCRIPT_FILE: &str = "script.js";

/// `import('curl')` and the other request clients. Anything else is rejected,
/// including files, Node built-ins, and QuickJS `std` / `os`.
fn request_module_source(name: &str) -> Option<String> {
    match name {
        "curl" => Some(
            "const m = globalThis.__requestClients.curl;\nexport default m;\nexport const Curl = m.Curl;\nexport const option = m.option;\n"
                .to_string(),
        ),
        "axios" | "got" | "request" | "postman-request" | "superagent" | "node-fetch"
        | "cross-fetch" | "undici" => {
            Some(format!("export default globalThis.__requestClients[{name:?}];\n"))
        }
        _ => None,
    }
}

struct ClientResolver;

impl Resolver for ClientResolver {
    fn resolve<'js>(
        &mut self,
        _ctx: &Ctx<'js>,
        base: &str,
        name: &str,
        _attributes: Option<ImportAttributes<'js>>,
    ) -> rquickjs::Result<String> {
        if request_module_source(name).is_some() {
            Ok(name.to_string())
        } else {
            Err(rquickjs::Error::new_resolving_message(
                base,
                name,
                "module is not available in the logic-socket sandbox",
            ))
        }
    }
}

struct ClientLoader;

impl Loader for ClientLoader {
    fn load<'js>(
        &mut self,
        ctx: &Ctx<'js>,
        name: &str,
        _attributes: Option<ImportAttributes<'js>>,
    ) -> rquickjs::Result<Module<'js>> {
        match request_module_source(name) {
            Some(source) => Module::declare(ctx.clone(), name, source),
            None => Err(rquickjs::Error::new_loading_message(
                name,
                "module is not available in the logic-socket sandbox",
            )),
        }
    }
}

/// Find `script.js:LINE:COL` in a QuickJS stack trace.
fn location(stack: &str) -> (Option<u32>, Option<u32>) {
    let Some(i) = stack.find(SCRIPT_FILE) else {
        return (None, None);
    };
    let mut parts = stack[i + SCRIPT_FILE.len()..]
        .trim_start_matches(':')
        .split(|c: char| !c.is_ascii_digit());
    let line = parts.next().and_then(|l| l.parse().ok());
    let col = parts.next().and_then(|c| c.parse().ok());
    (line, col)
}

fn caught_to_error(e: CaughtError<'_>) -> ScriptError {
    match e {
        CaughtError::Exception(ex) => {
            let message = ex.message().unwrap_or_else(|| "script error".into());
            let (line, column) = location(&ex.stack().unwrap_or_default());
            ScriptError {
                message,
                line,
                column,
            }
        }
        CaughtError::Value(v) => ScriptError {
            message: v
                .as_string()
                .and_then(|s| s.to_string().ok())
                .unwrap_or_else(|| format!("{v:?}")),
            line: None,
            column: None,
        },
        CaughtError::Error(e) => ScriptError {
            message: e.to_string(),
            line: None,
            column: None,
        },
    }
}

/// Run one script. Never panics on script errors: they are reported in
/// [`ScriptOutput::error`] together with any tests/logs produced before the failure.
pub async fn run(
    input: ScriptInput,
    limits: Limits,
    host: Arc<dyn Host>,
    renderer: Arc<Renderer>,
) -> ScriptOutput {
    // QuickJS parses/evaluates on the native stack; large vendored libraries
    // need more than a default 2 MB worker stack. Each script gets its own
    // thread + current-thread runtime, which also isolates it from app workers.
    let (tx, rx) = tokio::sync::oneshot::channel();
    let spawned = std::thread::Builder::new()
        .name("lsock-script".into())
        .stack_size(32 * 1024 * 1024)
        .spawn(move || {
            let out = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(rt) => rt.block_on(run_on_this_thread(input, limits, host, renderer)),
                Err(e) => ScriptOutput {
                    error: Some(ScriptError {
                        message: format!("could not start script runtime: {e}"),
                        line: None,
                        column: None,
                    }),
                    ..Default::default()
                },
            };
            let _ = tx.send(out);
        });
    if let Err(e) = spawned {
        return ScriptOutput {
            error: Some(ScriptError {
                message: format!("could not start script thread: {e}"),
                line: None,
                column: None,
            }),
            ..Default::default()
        };
    }
    rx.await.unwrap_or_else(|_| ScriptOutput {
        error: Some(ScriptError {
            message: "script runtime crashed".into(),
            line: None,
            column: None,
        }),
        ..Default::default()
    })
}

async fn run_on_this_thread(
    input: ScriptInput,
    limits: Limits,
    host: Arc<dyn Host>,
    renderer: Arc<Renderer>,
) -> ScriptOutput {
    let started = Instant::now();
    let console: Arc<Mutex<Vec<ConsoleLine>>> = Arc::default();
    let result = tokio::time::timeout(
        limits.timeout + Duration::from_millis(250),
        execute(&input, limits, host, renderer, console.clone()),
    )
    .await
    .unwrap_or_else(|_| {
        Err(ScriptError {
            message: format!("Script timed out after {} ms", limits.timeout.as_millis()),
            line: None,
            column: None,
        })
    });
    let console = console.lock().unwrap().clone();
    let duration_ms = started.elapsed().as_secs_f64() * 1000.0;
    match result {
        Ok(raw) => {
            let (line, column) = raw
                .error
                .as_ref()
                .map(|e| location(&e.stack))
                .unwrap_or((None, None));
            ScriptOutput {
                request: Some(raw.request),
                environment: raw.environment,
                base_environment: raw.base_environment,
                globals: raw.globals,
                local_variables: raw.local_variables,
                cookies: raw.cookies,
                tests: raw.tests,
                console,
                execution: raw.execution,
                error: raw.error.map(|e| ScriptError {
                    message: e.message,
                    line,
                    column,
                }),
                duration_ms,
            }
        }
        Err(error) => ScriptOutput {
            environment: input.environment.data,
            base_environment: input.base_environment.data,
            globals: input.globals.data,
            local_variables: input.local_variables,
            console,
            error: Some(error),
            duration_ms,
            ..Default::default()
        },
    }
}

async fn execute(
    input: &ScriptInput,
    limits: Limits,
    host: Arc<dyn Host>,
    renderer: Arc<Renderer>,
    console: Arc<Mutex<Vec<ConsoleLine>>>,
) -> Result<RawOutput, ScriptError> {
    let fail = |m: String| ScriptError {
        message: m,
        line: None,
        column: None,
    };
    let rt = AsyncRuntime::new().map_err(|e| fail(e.to_string()))?;
    rt.set_memory_limit(limits.memory_bytes).await;
    rt.set_max_stack_size(16 * 1024 * 1024).await; // thread has 32 MB
    let deadline = Instant::now() + limits.timeout;
    rt.set_interrupt_handler(Some(Box::new(move || Instant::now() > deadline)))
        .await;
    rt.set_loader(ClientResolver, ClientLoader).await;
    let ctx = AsyncContext::full(&rt)
        .await
        .map_err(|e| fail(e.to_string()))?;

    let mut ctx_json = serde_json::to_value(input).map_err(|e| fail(e.to_string()))?;
    let event = input.event.unwrap_or(Event::PreRequest);
    ctx_json["info"]["eventName"] = serde_json::to_value(event).unwrap();
    let ctx_json = ctx_json.to_string();
    let script = input.script.clone();
    let timeout_ms = limits.timeout.as_millis();

    let out: Result<String, ScriptError> = ctx
        .async_with(async move |ctx| {
            install_host(&ctx, host, renderer, console).map_err(|e| fail(e.to_string()))?;
            ctx.eval::<(), _>(PRELUDE).catch(&ctx).map_err(caught_to_error)?;

            let mut opts = EvalOptions::default();
            opts.global = true;
            opts.strict = false;
            opts.filename = Some(SCRIPT_FILE.into());
            let wrapped = format!(
                "(async function (ls, $, pm, require, console, setTimeout, clearTimeout) {{{script}\n}})"
            );
            let user_fn: Function = ctx.eval_with_options(wrapped, opts).catch(&ctx).map_err(caught_to_error)?;
            let lsock: Object = ctx.globals().get("__lsock").map_err(|e| fail(e.to_string()))?;
            let run_fn: Function = lsock.get("run").map_err(|e| fail(e.to_string()))?;
            let promise: Promise = run_fn.call((ctx_json, user_fn)).catch(&ctx).map_err(caught_to_error)?;
            promise.into_future::<String>().await.catch(&ctx).map_err(|e| {
                let mut err = caught_to_error(e);
                if err.message.contains("interrupted") {
                    err.message = format!("Script timed out after {timeout_ms} ms (possible infinite loop)");
                }
                err
            })
        })
        .await;
    let out = out.map_err(|mut e| {
        if e.message.contains("out of memory") {
            e.message = format!(
                "Script exceeded the memory limit ({} MB)",
                limits.memory_bytes / 1024 / 1024
            );
        } else if e.message.contains("interrupted") {
            e.message = format!(
                "Script timed out after {} ms (possible infinite loop)",
                limits.timeout.as_millis()
            );
        }
        e
    })?;
    serde_json::from_str(&out).map_err(|e| fail(format!("internal: bad script result: {e}")))
}

/// Evaluate a vendored library with a CommonJS shim and return its exports.
fn load_module<'js>(ctx: Ctx<'js>, name: String) -> rquickjs::Result<Value<'js>> {
    let Some(src) = vendored(&name) else {
        return Err(rquickjs::Error::new_from_js_message(
            "string",
            "module",
            format!("unknown module {name}"),
        ));
    };
    let wrapped = format!(
        "(function(){{var module={{exports:{{}}}};var exports=module.exports;var define=undefined;var self=globalThis;var window=undefined;\n{src}\n;if(typeof __ajv!=='undefined'){{module.exports=__ajv;}}return module.exports;}})()"
    );
    ctx.eval::<Value, _>(wrapped).catch(&ctx).map_err(|e| {
        let msg = caught_to_error(e).message;
        rquickjs::Exception::throw_message(&ctx, &format!("failed to load module '{name}': {msg}"))
    })
}

fn install_host(
    ctx: &Ctx<'_>,
    host: Arc<dyn Host>,
    renderer: Arc<Renderer>,
    console: Arc<Mutex<Vec<ConsoleLine>>>,
) -> rquickjs::Result<()> {
    let h = Object::new(ctx.clone())?;
    h.set(
        "log",
        Func::from(move |level: String, text: String| {
            console.lock().unwrap().push(ConsoleLine {
                level,
                text,
                timestamp_ms: chrono::Utc::now().timestamp_millis(),
            });
        }),
    )?;
    h.set(
        "render",
        Func::from(move |template: String, vars: String| -> String {
            let vars: VarMap = serde_json::from_str(&vars).unwrap_or_default();
            let ctx = Context::build(&renderer, &[Layer::new("script", vars)]);
            renderer
                .render_str(&template, &ctx, Mode::Keep)
                .unwrap_or(template)
        }),
    )?;
    h.set("uuid", Func::from(|| uuid::Uuid::new_v4().to_string()))?;
    h.set(
        "sleep",
        Func::from(Async(|ms: f64| async move {
            tokio::time::sleep(Duration::from_millis(ms.clamp(0.0, 600_000.0) as u64)).await;
            rquickjs::Result::Ok(())
        })),
    )?;
    h.set(
        "send",
        Func::from(Async(move |req: String| {
            let host = host.clone();
            async move {
                let out = match serde_json::from_str::<HostRequest>(&req) {
                    Ok(r) => match host.send(r).await {
                        Ok(resp) => serde_json::to_string(&resp).unwrap_or_default(),
                        Err(e) => serde_json::json!({ "error": e }).to_string(),
                    },
                    Err(e) => {
                        serde_json::json!({ "error": format!("invalid request: {e}") }).to_string()
                    }
                };
                rquickjs::Result::Ok(out)
            }
        })),
    )?;
    h.set("load", Func::from(load_module))?;
    ctx.globals().set("__host", h)?;
    Ok(())
}

/// Merge a script's variable maps back over the originals (used by the engine).
pub fn changed(before: &VarMap, after: &VarMap) -> bool {
    before != after
}

#[cfg(test)]
mod tests;
