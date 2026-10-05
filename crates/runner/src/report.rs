//! Reporters for a finished run. `spec` and `dot` are human-readable; `json`
//! and `junit` are for CI.

use crate::{RequestResult, Summary};

fn ms(v: f64) -> String {
    if v < 1000.0 { format!("{v:.0}ms") } else { format!("{:.2}s", v / 1000.0) }
}

fn status_text(r: &RequestResult) -> String {
    if r.skipped {
        "skipped".into()
    } else if let Some(e) = &r.error {
        format!("error: {e}")
    } else {
        format!("{} {}", r.status, r.status_message).trim().to_string()
    }
}

fn totals(s: &Summary) -> String {
    let mut out = format!(
        "\n{} request{} ({} failed, {} skipped) · {} test{} ({} passed, {} failed, {} skipped) · {}\n",
        s.requests,
        if s.requests == 1 { "" } else { "s" },
        s.requests_failed,
        s.requests_skipped,
        s.tests_passed + s.tests_failed + s.tests_skipped,
        if s.tests_passed + s.tests_failed + s.tests_skipped == 1 { "" } else { "s" },
        s.tests_passed,
        s.tests_failed,
        s.tests_skipped,
        ms(s.duration_ms)
    );
    if s.bailed {
        out.push_str("Stopped early (--bail) after the first failure.\n");
    }
    out
}

pub fn spec(s: &Summary) -> String {
    let mut out = String::new();
    let mut current = 0;
    for r in &s.results {
        if r.iteration != current {
            current = r.iteration;
            if s.iterations > 1 {
                out.push_str(&format!("\nIteration {current}/{}\n", s.iterations));
            }
        }
        let mark = if r.skipped { "-" } else if r.failed() { "✗" } else { "✓" };
        out.push_str(&format!("  {mark} {} {} — {} ({})\n", r.method, r.name, status_text(r), ms(r.duration_ms)));
        if let Some(e) = &r.script_error {
            out.push_str(&format!("      script error: {e}\n"));
        }
        for t in &r.tests {
            let m = if t.skipped { "-" } else if t.passed { "✓" } else { "✗" };
            out.push_str(&format!("      {m} {}", t.name));
            if let Some(e) = t.error.as_ref().filter(|_| !t.passed) {
                out.push_str(&format!(" — {e}"));
            }
            out.push('\n');
        }
    }
    out.push_str(&totals(s));
    out
}

pub fn dot(s: &Summary) -> String {
    let mut out: String = s
        .results
        .iter()
        .map(|r| if r.skipped { 's' } else if r.failed() { 'F' } else { '.' })
        .collect();
    out.push('\n');
    let failures: Vec<&RequestResult> = s.results.iter().filter(|r| r.failed()).collect();
    if !failures.is_empty() {
        out.push_str("\nFailures:\n");
        for (i, r) in failures.iter().enumerate() {
            out.push_str(&format!("{}. [iteration {}] {} {} — {}\n", i + 1, r.iteration, r.method, r.name, status_text(r)));
            if let Some(e) = &r.script_error {
                out.push_str(&format!("   script error: {e}\n"));
            }
            for t in r.tests.iter().filter(|t| !t.passed && !t.skipped) {
                out.push_str(&format!("   ✗ {} — {}\n", t.name, t.error.clone().unwrap_or_default()));
            }
        }
    }
    out.push_str(&totals(s));
    out
}

pub fn json(s: &Summary) -> String {
    serde_json::to_string_pretty(s).unwrap_or_default()
}

fn esc(s: &str) -> String {
    s.chars()
        .filter(|c| !matches!(c, '\u{0}'..='\u{8}' | '\u{b}' | '\u{c}' | '\u{e}'..='\u{1f}'))
        .collect::<String>()
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

/// JUnit XML: one `<testsuite>` per request execution, one `<testcase>` per
/// script test plus a synthetic "request" case so transport errors fail CI too.
pub fn junit(s: &Summary) -> String {
    let mut suites = String::new();
    let mut total_tests = 0;
    let mut total_failures = 0;
    let mut total_skipped = 0;
    for r in &s.results {
        let classname = esc(&format!("iteration {}.{}", r.iteration, r.name));
        let mut cases = String::new();
        let mut failures = 0;
        let mut skipped = 0;
        let request_failed = !r.skipped && (r.error.is_some() || r.script_error.is_some());
        cases.push_str(&format!(
            "    <testcase name=\"{}\" classname=\"{classname}\" time=\"{:.3}\">",
            esc(&format!("{} {} responds", r.method, r.name)),
            r.duration_ms / 1000.0
        ));
        if r.skipped {
            skipped += 1;
            cases.push_str("<skipped message=\"skipped by pre-request script\"/>");
        } else if request_failed {
            failures += 1;
            let msg = r.error.clone().or(r.script_error.clone()).unwrap_or_default();
            cases.push_str(&format!("<failure message=\"{}\" type=\"RequestError\"/>", esc(&msg)));
        }
        cases.push_str("</testcase>\n");
        for t in &r.tests {
            cases.push_str(&format!(
                "    <testcase name=\"{}\" classname=\"{classname}\" time=\"{:.3}\">",
                esc(&t.name),
                t.duration_ms / 1000.0
            ));
            if t.skipped {
                skipped += 1;
                cases.push_str("<skipped/>");
            } else if !t.passed {
                failures += 1;
                let msg = t.error.clone().unwrap_or_else(|| "failed".into());
                cases.push_str(&format!("<failure message=\"{}\" type=\"AssertionError\">{}</failure>", esc(&msg), esc(&msg)));
            }
            cases.push_str("</testcase>\n");
        }
        let n = r.tests.len() + 1;
        total_tests += n;
        total_failures += failures;
        total_skipped += skipped;
        let mut system_out = String::new();
        if !r.console.is_empty() {
            let lines: Vec<String> = r.console.iter().map(|c| format!("[{}] {}", c.level, c.text)).collect();
            system_out = format!("    <system-out>{}</system-out>\n", esc(&lines.join("\n")));
        }
        suites.push_str(&format!(
            "  <testsuite name=\"{}\" tests=\"{n}\" failures=\"{failures}\" errors=\"0\" skipped=\"{skipped}\" time=\"{:.3}\" timestamp=\"{}\">\n{cases}{system_out}  </testsuite>\n",
            esc(&format!("{} {} (iteration {})", r.method, r.name, r.iteration)),
            r.duration_ms / 1000.0,
            esc(&s.started_at),
        ));
    }
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<testsuites name=\"insomnia-rs\" tests=\"{total_tests}\" failures=\"{total_failures}\" errors=\"0\" skipped=\"{total_skipped}\" time=\"{:.3}\">\n{suites}</testsuites>\n",
        s.duration_ms / 1000.0
    )
}

pub fn render(s: &Summary, reporter: &str) -> Option<String> {
    Some(match reporter {
        "spec" => spec(s),
        "dot" => dot(s),
        "json" => json(s),
        "junit" => junit(s),
        _ => return None,
    })
}
