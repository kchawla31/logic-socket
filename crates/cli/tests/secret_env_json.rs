//! `lsock env set --secret` unquotes a JSON string argument the same way
//! plain values are parsed, and keeps any other text as the literal secret.

use std::process::Command;

fn lsock(data: &std::path::Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_lsock"))
        .args(args)
        .env("LSOCK_DATA_DIR", data)
        .env("LSOCK_SECRET_STORE", "memory")
        .env(
            "LSOCK_VAULT_KEY",
            "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=",
        )
        .env("NO_COLOR", "1")
        .output()
        .unwrap()
}

#[test]
fn secret_values_follow_the_same_json_parsing_as_plain_values() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path();
    for args in [
        ["workspace", "create", "Eval"].as_slice(),
        ["env", "set", "Eval", "plain", "\"quoted-value\""].as_slice(),
        [
            "env",
            "set",
            "Eval",
            "secret",
            "\"quoted-value\"",
            "--secret",
        ]
        .as_slice(),
        ["env", "set", "Eval", "pin", "012345", "--secret"].as_slice(),
    ] {
        let o = lsock(data, args);
        assert!(
            o.status.success(),
            "{args:?}\n{}",
            String::from_utf8_lossy(&o.stderr)
        );
    }
    let o = lsock(
        data,
        &["render", "Eval", "{{ _.plain }}|{{ _.secret }}|{{ _.pin }}"],
    );
    let out = String::from_utf8_lossy(&o.stdout);
    let err = String::from_utf8_lossy(&o.stderr);
    assert!(
        o.status.success() && out.contains("quoted-value|quoted-value|012345"),
        "secret JSON string was not parsed like a plain value\nstdout:\n{out}\nstderr:\n{err}"
    );
    assert!(
        !out.contains("quoted-value|\"quoted-value\""),
        "secret kept the JSON quotes\nstdout:\n{out}"
    );
}
