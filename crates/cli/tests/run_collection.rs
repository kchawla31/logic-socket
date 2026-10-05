//! End-to-end: seed the Phase 2 fixture with the real `irs` binary, run it
//! against an in-process mock MCP server, check reporters and exit codes.

use std::path::{Path, PathBuf};
use std::process::Command;

fn irs(data: &Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_irs"))
        .args(args)
        .env("IRS_DATA_DIR", data)
        .env("NO_COLOR", "1")
        .output()
        .unwrap()
}

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/phase2")
}

fn seed(data: &Path, base_url: &str) {
    let f = fixtures();
    let f = |n: &str| f.join(n).to_string_lossy().into_owned();
    let steps: Vec<Vec<String>> = vec![
        vec!["workspace", "create", "Smoke"],
        vec!["env", "set", "Smoke", "base_url", &format!("\"{base_url}\"")],
        vec!["request", "folder", "Smoke", "RPC"],
        vec!["request", "script", "RPC", "--pre", &f("folder-pre.js")],
        vec!["request", "add", "RPC", "Initialize", "POST", "{{ _.base_url }}/mcp", "--json",
             r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25"}}"#],
        vec!["request", "script", "Initialize", "--after", &f("initialize-after.js")],
        vec!["request", "add", "RPC", "Weather", "POST", "{{ _.base_url }}/mcp", "--json",
             r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"get_weather","arguments":{"city":"{{ city }}"}}}"#],
        vec!["request", "script", "Weather", "--after", &f("weather-after.js")],
    ]
    .into_iter()
    .map(|v| v.into_iter().map(String::from).collect())
    .collect();
    for s in steps {
        let args: Vec<&str> = s.iter().map(String::as_str).collect();
        let o = irs(data, &args);
        assert!(
            o.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&o.stderr)
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn run_collection_reporters_and_exit_codes() {
    let url = irs_mcp::mock::spawn_http(Default::default(), 0)
        .await
        .unwrap();
    let base = url.trim_end_matches("/mcp").to_string();
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path();
    seed(data, &base);
    let csv = fixtures().join("cities.csv");
    let csv = csv.to_str().unwrap();

    // exit 0 + spec output
    let o = tokio::task::spawn_blocking({
        let data = data.to_path_buf();
        let csv = csv.to_string();
        move || irs(&data, &["run", "collection", "Smoke", "-d", &csv])
    })
    .await
    .unwrap();
    let out = String::from_utf8_lossy(&o.stdout);
    assert_eq!(
        o.status.code(),
        Some(0),
        "{out}\n{}",
        String::from_utf8_lossy(&o.stderr)
    );
    assert!(
        out.contains("Iteration 3/3") && out.contains("✓ weather for Tokyo"),
        "{out}"
    );

    // JUnit to a file
    let report = data.join("report.xml");
    let o = tokio::task::spawn_blocking({
        let data = data.to_path_buf();
        let csv = csv.to_string();
        let report = report.clone();
        move || {
            irs(
                &data,
                &[
                    "run",
                    "collection",
                    "Smoke",
                    "-d",
                    &csv,
                    "-r",
                    "junit",
                    "-o",
                    report.to_str().unwrap(),
                ],
            )
        }
    })
    .await
    .unwrap();
    assert_eq!(o.status.code(), Some(0));
    let xml = std::fs::read_to_string(&report).unwrap();
    assert!(
        xml.starts_with("<?xml") && xml.contains("tests=\"15\"") && xml.contains("failures=\"0\""),
        "{xml}"
    );

    // exit 1: no data file → unresolved {{ city }} fails the Weather request
    let o = tokio::task::spawn_blocking({
        let data = data.to_path_buf();
        move || irs(&data, &["run", "collection", "Smoke", "-r", "json"])
    })
    .await
    .unwrap();
    assert_eq!(o.status.code(), Some(1));
    let json: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
    assert_eq!(json["requestsFailed"], 1);

    // exit 2: usage errors
    assert_eq!(
        irs(data, &["run", "collection", "Nope"]).status.code(),
        Some(2)
    );
    assert_eq!(
        irs(data, &["run", "collection", "Smoke", "-r", "xml"])
            .status
            .code(),
        Some(2)
    );
}
