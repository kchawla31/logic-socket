//! Collection runner: iterations, data files, delays, bail and script flow
//! control (`skipRequest`, `setNextRequest`). Emits events so the CLI and the
//! desktop app share one implementation; reporters turn a [`Summary`] into
//! spec / dot / json / junit output.

pub mod report;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use lsock_core::{ConsoleEntry, Request, TestResult, VarMap};
use lsock_engine::{Engine, RunState};
use serde::Serialize;

#[derive(Debug, thiserror::Error)]
pub enum RunError {
    #[error("could not read data file {0}: {1}")]
    DataFile(String, String),
    #[error("data file must be CSV with a header row or a JSON array of objects: {0}")]
    DataFormat(String),
    #[error(transparent)]
    Engine(#[from] lsock_engine::EngineError),
}

#[derive(Debug, Clone)]
pub struct RunOptions {
    pub iterations: u32,
    pub delay_ms: u64,
    /// One row per iteration (cycled if there are more iterations than rows).
    pub data: Vec<VarMap>,
    pub bail: bool,
    /// Guard against `setNextRequest` loops.
    pub max_steps_per_iteration: usize,
    /// Set to stop the run after the current request.
    pub cancel: Option<Arc<AtomicBool>>,
}

impl Default for RunOptions {
    fn default() -> Self {
        Self {
            iterations: 1,
            delay_ms: 0,
            data: vec![],
            bail: false,
            max_steps_per_iteration: 1000,
            cancel: None,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RequestResult {
    pub iteration: u32,
    pub request_id: String,
    pub name: String,
    pub method: String,
    pub url: String,
    pub status: u16,
    pub status_message: String,
    pub duration_ms: f64,
    pub error: Option<String>,
    pub script_error: Option<String>,
    pub skipped: bool,
    pub tests: Vec<TestResult>,
    pub console: Vec<ConsoleEntry>,
    pub response_id: Option<String>,
}

impl RequestResult {
    pub fn failed(&self) -> bool {
        !self.skipped
            && (self.error.is_some()
                || self.script_error.is_some()
                || self.tests.iter().any(|t| !t.passed && !t.skipped))
    }
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Summary {
    pub iterations: u32,
    pub requests: usize,
    pub requests_failed: usize,
    pub requests_skipped: usize,
    pub tests_passed: usize,
    pub tests_failed: usize,
    pub tests_skipped: usize,
    pub duration_ms: f64,
    pub bailed: bool,
    pub cancelled: bool,
    pub started_at: String,
    pub results: Vec<RequestResult>,
}

impl Summary {
    pub fn ok(&self) -> bool {
        self.requests_failed == 0 && self.tests_failed == 0
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum RunEvent {
    RunStart {
        requests: usize,
        iterations: u32,
    },
    IterationStart {
        iteration: u32,
    },
    RequestStart {
        iteration: u32,
        request_id: String,
        name: String,
        method: String,
    },
    RequestEnd {
        result: RequestResult,
    },
    IterationEnd {
        iteration: u32,
    },
    /// A problem with the run itself (e.g. unknown `setNextRequest` target).
    Warning {
        iteration: u32,
        message: String,
    },
    Done {
        summary: Summary,
    },
}

/// Parse a CSV (header row) or JSON (array of objects) data file.
pub fn parse_data(content: &str) -> Result<Vec<VarMap>, RunError> {
    let trimmed = content.trim_start_matches('\u{feff}').trim();
    if trimmed.starts_with('[') {
        let rows: Vec<VarMap> =
            serde_json::from_str(trimmed).map_err(|e| RunError::DataFormat(e.to_string()))?;
        return Ok(rows);
    }
    let mut rdr = csv::ReaderBuilder::new()
        .trim(csv::Trim::All)
        .from_reader(trimmed.as_bytes());
    let headers = rdr
        .headers()
        .map_err(|e| RunError::DataFormat(e.to_string()))?
        .clone();
    let mut rows = vec![];
    for rec in rdr.records() {
        let rec = rec.map_err(|e| RunError::DataFormat(e.to_string()))?;
        let mut m = VarMap::new();
        for (h, v) in headers.iter().zip(rec.iter()) {
            m.insert(h.to_string(), serde_json::Value::String(v.to_string()));
        }
        rows.push(m);
    }
    Ok(rows)
}

pub fn load_data_file(path: &str) -> Result<Vec<VarMap>, RunError> {
    let content = std::fs::read_to_string(path)
        .map_err(|e| RunError::DataFile(path.to_string(), e.to_string()))?;
    parse_data(&content)
}

/// Run `request_ids` (in order) for each iteration.
pub async fn run(
    engine: &Engine,
    request_ids: &[String],
    opts: &RunOptions,
    mut on_event: impl FnMut(&RunEvent),
) -> Summary {
    let started = Instant::now();
    let iterations = opts.iterations.max(1);
    let mut summary = Summary {
        iterations,
        started_at: chrono::Utc::now().to_rfc3339(),
        ..Default::default()
    };
    on_event(&RunEvent::RunStart {
        requests: request_ids.len(),
        iterations,
    });

    // Names for setNextRequest lookups.
    let names: Vec<String> = request_ids
        .iter()
        .map(|id| {
            engine
                .store
                .get::<Request>(id)
                .map(|r| r.body.name.clone())
                .unwrap_or_default()
        })
        .collect();

    'iterations: for iteration in 1..=iterations {
        on_event(&RunEvent::IterationStart { iteration });
        let row = if opts.data.is_empty() {
            VarMap::new()
        } else {
            opts.data[(iteration as usize - 1) % opts.data.len()].clone()
        };
        let mut state = RunState {
            iteration_data: row,
            iteration,
            iteration_count: iterations,
            ..Default::default()
        };
        let mut idx = 0usize;
        let mut steps = 0usize;
        while idx < request_ids.len() {
            if opts
                .cancel
                .as_ref()
                .is_some_and(|c| c.load(Ordering::SeqCst))
            {
                summary.cancelled = true;
                on_event(&RunEvent::IterationEnd { iteration });
                break 'iterations;
            }
            steps += 1;
            if steps > opts.max_steps_per_iteration {
                on_event(&RunEvent::Warning {
                    iteration,
                    message: format!(
                        "stopped after {} requests in one iteration (setNextRequest loop?)",
                        opts.max_steps_per_iteration
                    ),
                });
                break;
            }
            let id = &request_ids[idx];
            let req = engine.store.get::<Request>(id).ok();
            on_event(&RunEvent::RequestStart {
                iteration,
                request_id: id.clone(),
                name: req.as_ref().map(|r| r.name.clone()).unwrap_or_default(),
                method: req.as_ref().map(|r| r.method.clone()).unwrap_or_default(),
            });
            let t = Instant::now();
            let (result, next) = match engine.send_with_state(id, &mut state).await {
                Ok(out) => {
                    let r = &out.response;
                    (
                        RequestResult {
                            iteration,
                            request_id: id.clone(),
                            name: req.as_ref().map(|r| r.name.clone()).unwrap_or_default(),
                            method: r.method.clone(),
                            url: r.url.clone(),
                            status: r.status_code,
                            status_message: r.status_message.clone(),
                            duration_ms: if r.timings.total_ms > 0.0 {
                                r.timings.total_ms
                            } else {
                                t.elapsed().as_secs_f64() * 1000.0
                            },
                            error: if out.skipped { None } else { r.error.clone() },
                            script_error: r.script_error.clone(),
                            skipped: out.skipped,
                            tests: r.test_results.clone(),
                            console: r.console.clone(),
                            response_id: Some(r.meta.id.clone()),
                        },
                        out.next_request,
                    )
                }
                Err(e) => (
                    RequestResult {
                        iteration,
                        request_id: id.clone(),
                        name: req.as_ref().map(|r| r.name.clone()).unwrap_or_default(),
                        method: req.as_ref().map(|r| r.method.clone()).unwrap_or_default(),
                        url: req.as_ref().map(|r| r.url.clone()).unwrap_or_default(),
                        status: 0,
                        status_message: String::new(),
                        duration_ms: t.elapsed().as_secs_f64() * 1000.0,
                        error: Some(e.to_string()),
                        script_error: None,
                        skipped: false,
                        tests: vec![],
                        console: vec![],
                        response_id: None,
                    },
                    None,
                ),
            };
            let failed = result.failed();
            summary.requests += 1;
            summary.requests_failed += failed as usize;
            summary.requests_skipped += result.skipped as usize;
            for t in &result.tests {
                if t.skipped {
                    summary.tests_skipped += 1;
                } else if t.passed {
                    summary.tests_passed += 1;
                } else {
                    summary.tests_failed += 1;
                }
            }
            on_event(&RunEvent::RequestEnd {
                result: result.clone(),
            });
            summary.results.push(result);
            if failed && opts.bail {
                summary.bailed = true;
                on_event(&RunEvent::IterationEnd { iteration });
                break 'iterations;
            }

            match next.as_deref() {
                Some("__stop__") => break,
                Some(target) => match request_ids
                    .iter()
                    .position(|r| r == target)
                    .or_else(|| names.iter().position(|n| n == target))
                    .or_else(|| names.iter().position(|n| n.eq_ignore_ascii_case(target)))
                {
                    Some(i) => idx = i,
                    None => {
                        on_event(&RunEvent::Warning {
                            iteration,
                            message: format!(
                                "setNextRequest('{target}'): no request with that name or id in this run; stopping iteration"
                            ),
                        });
                        break;
                    }
                },
                None => idx += 1,
            }
            if opts.delay_ms > 0 && idx < request_ids.len() {
                tokio::time::sleep(Duration::from_millis(opts.delay_ms)).await;
            }
        }
        on_event(&RunEvent::IterationEnd { iteration });
        if opts.delay_ms > 0 && iteration < iterations {
            tokio::time::sleep(Duration::from_millis(opts.delay_ms)).await;
        }
    }
    summary.duration_ms = started.elapsed().as_secs_f64() * 1000.0;
    on_event(&RunEvent::Done {
        summary: summary.clone(),
    });
    summary
}

#[cfg(test)]
mod tests;
