//! A long-lived `poly mcp` must see project configs as they are on disk at each call.
//!
//! The per-directory config lookups (`.oxlintrc.json`, `.oxfmtrc.json`, `go.mod`, …) are memoised
//! for the length of a run. The server starts many runs in one process, and an edit between two
//! calls used to be invisible to the second: the CLI reported the new findings, the server the
//! old ones. Driven through the bundled binary, since the oxc engine lives in this workspace.
#![cfg(unix)]

use std::collections::BTreeSet;
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{ChildStdin, ChildStdout, Command, Stdio};

use serde_json::{Value, json};

const POLY: &str = env!("CARGO_BIN_EXE_poly");

fn request(stdin: &mut ChildStdin, stdout: &mut BufReader<ChildStdout>, id: u64, method: &str, params: Value) -> Value {
    let message = json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params });
    writeln!(stdin, "{message}").unwrap();
    stdin.flush().unwrap();
    loop {
        let mut line = String::new();
        assert!(stdout.read_line(&mut line).unwrap() > 0, "poly mcp closed stdout");
        let reply: Value = serde_json::from_str(&line).unwrap();
        if reply["id"] == json!(id) {
            return reply;
        }
    }
}

/// The diagnostic codes of a `lint` result.
fn codes(reply: &Value) -> BTreeSet<String> {
    let results = reply["result"]["structuredContent"]["results"]
        .as_array()
        .unwrap_or_else(|| panic!("a lint document: {reply}"));
    results
        .iter()
        .flat_map(|result| result["diagnostics"].as_array().cloned().unwrap_or_default())
        .filter_map(|diagnostic| diagnostic["code"].as_str().map(str::to_owned))
        .collect()
}

fn write_rc(dir: &Path, rules: &str) {
    std::fs::write(dir.join(".oxlintrc.json"), format!("{{\"rules\": {rules}}}\n")).unwrap();
}

#[test]
fn an_oxlintrc_edit_between_two_calls_changes_the_findings() {
    let dir = tempfile::tempdir().unwrap();
    let cache = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.ts"), "debugger;\nif (a == 1) {}\n").unwrap();
    write_rc(dir.path(), r#"{"no-debugger": "error", "eqeqeq": "off"}"#);

    let mut child = Command::new(POLY)
        .arg("mcp")
        .current_dir(dir.path())
        .env("POLY_OFFLINE", "1")
        .env("POLY_CACHE_HOME", cache.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = BufReader::new(child.stdout.take().unwrap());
    let init = json!({
        "protocolVersion": "2025-06-18",
        "capabilities": {},
        "clientInfo": { "name": "poly-test", "version": "0" },
    });
    request(&mut stdin, &mut stdout, 1, "initialize", init);
    writeln!(
        stdin,
        "{}",
        json!({ "jsonrpc": "2.0", "method": "notifications/initialized" })
    )
    .unwrap();

    let call = json!({ "name": "lint", "arguments": { "paths": ["a.ts"] } });
    let before = codes(&request(&mut stdin, &mut stdout, 2, "tools/call", call.clone()));
    write_rc(dir.path(), r#"{"no-debugger": "off", "eqeqeq": "error"}"#);
    let after = codes(&request(&mut stdin, &mut stdout, 3, "tools/call", call));
    let _ = child.kill();
    let _ = child.wait();

    assert!(before.iter().any(|code| code.contains("no-debugger")), "{before:?}");
    assert!(
        !after.iter().any(|code| code.contains("no-debugger")),
        "the edit turned it off: {after:?}"
    );
    assert!(
        after.iter().any(|code| code.contains("eqeqeq")),
        "the edit turned it on: {after:?}"
    );
}
