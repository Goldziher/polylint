//! A long-lived `poly mcp` against a repository that changes beside it, and against requests it
//! must refuse: what the server reads once must not outlive the call that read it, and a
//! read-only tool must leave the disk and the network alone.
//!
//! Driven through the shipped binary, like the parity tests: the resolver, the workspace guard
//! and enforced trust are wired in `poly mcp`, not in the server crate.
#![cfg(all(unix, feature = "bundled-mcp"))]

mod mcp_child;

use std::collections::BTreeSet;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::Command;

use mcp_child::{McpChild, POLY};
use serde_json::{Value, json};

/// The tool result of `response`, which must not be a protocol error.
fn result(response: &Value) -> &Value {
    let result = &response["result"];
    assert!(result.is_object(), "a tool result, not a protocol error: {response}");
    result
}

fn is_error(response: &Value) -> bool {
    result(response)["isError"] == json!(true)
}

fn document(response: &Value) -> &Value {
    &result(response)["structuredContent"]
}

fn write(path: &Path, body: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, body).unwrap();
}

/// Every path under `root`, relative to it.
fn tree(root: &Path) -> BTreeSet<PathBuf> {
    let mut found = BTreeSet::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(dir) = pending.pop() {
        for entry in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
            let path = entry.path();
            found.insert(path.strip_prefix(root).unwrap().to_path_buf());
            if path.is_dir() {
                pending.push(path);
            }
        }
    }
    found
}

fn git(args: &[&str], cwd: &Path) {
    let status = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .status()
        .expect("run git");
    assert!(status.success(), "git {args:?} failed");
}

/// A git repository holding `config` as its `poly.toml`: its URL and the commit.
fn git_base(config: &str) -> (tempfile::TempDir, String, String) {
    let repo = tempfile::tempdir().unwrap();
    git(&["init", "--quiet"], repo.path());
    write(&repo.path().join("poly.toml"), config);
    git(&["add", "poly.toml"], repo.path());
    git(
        &[
            "-c",
            "user.name=Poly Test",
            "-c",
            "user.email=poly@example.invalid",
            "-c",
            "commit.gpgSign=false",
            "commit",
            "--quiet",
            "-m",
            "base config",
        ],
        repo.path(),
    );
    let head = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(repo.path())
        .output()
        .expect("rev-parse");
    let oid = String::from_utf8(head.stdout).unwrap().trim().to_owned();
    let url = repo.path().to_string_lossy().into_owned();
    (repo, url, oid)
}

/// The server builds its `extends` resolver before the handshake. A `poly.lock` that does not
/// parse (a merge conflict, say) used to end the process there; it is the run's `config` failure.
#[test]
fn a_malformed_poly_lock_fails_the_run_not_the_server() {
    let dir = tempfile::tempdir().unwrap();
    let cache = tempfile::tempdir().unwrap();
    write(&dir.path().join("a.py"), "x = 1\n");
    write(&dir.path().join("poly.lock"), "<<<<<<< HEAD\nversion = 1\n");
    let mut server = McpChild::start(dir.path(), cache.path());
    let response = server.call("check", json!({ "paths": ["a.py"] }));
    assert!(is_error(&response), "{response}");
    assert_eq!(document(&response)["error"]["kind"], "config", "{response}");
    assert_eq!(document(&response)["summary"], Value::Null, "{response}");
}

/// `info` is annotated read-only and closed-world. With a remote `extends` base the source cache
/// does not hold, it reports that rather than fetching the base from a URL the repository chose.
/// The server runs online here: offline, a fetch would fail for a different reason.
#[test]
fn info_never_fetches_a_remote_extends_base() {
    let (_base, url, oid) = git_base("[defaults]\nline_length = 77\n");
    let dir = tempfile::tempdir().unwrap();
    let cache = tempfile::tempdir().unwrap();
    write(&dir.path().join("a.py"), "x = 1\n");
    write(
        &dir.path().join("poly.toml"),
        &format!("extends = [{{ git = \"{url}\", revision = \"{oid}\", file = \"poly.toml\" }}]\n"),
    );
    let mut server = McpChild::start_with(dir.path(), cache.path(), &[]);
    let response = server.call("info", json!({ "topics": ["components", "config"], "paths": ["a.py"] }));
    assert!(is_error(&response), "{response}");
    for topic in ["components", "config"] {
        let message = document(&response)[topic]["error"]["message"]
            .as_str()
            .unwrap_or_default();
        assert!(message.contains("does not fetch"), "{topic}: {response}");
    }
    let fetched: Vec<_> = tree(cache.path())
        .into_iter()
        .filter(|path| path.components().any(|part| part.as_os_str() == "checkouts"))
        .collect();
    assert!(fetched.is_empty(), "the base was fetched: {fetched:?}");
}

/// The cache topic reads a footprint. It must not create the cache it measures: with a
/// repository-set `[cache] dir` that would be a write wherever the repository points.
#[test]
fn info_cache_creates_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let cache = tempfile::tempdir().unwrap();
    write(&dir.path().join("a.py"), "x = 1\n");
    let mut server = McpChild::start(dir.path(), cache.path());
    let before = (tree(cache.path()), tree(dir.path()));
    let response = server.call("info", json!({ "topics": ["cache"] }));
    assert!(!is_error(&response), "{response}");
    assert_eq!(document(&response)["cache"]["total_entries"], 0, "{response}");
    assert_eq!((tree(cache.path()), tree(dir.path())), before, "info wrote to disk");
}

/// A repository whose `[hooks]` run a whole-project job that touches `marker`.
fn hooks_repo(marker: &Path) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join(".git")).unwrap();
    write(&dir.path().join("a.py"), "x = 1\n");
    write(
        &dir.path().join("poly.toml"),
        &format!(
            "[hooks]\nstages = [\"pre-commit\"]\n\n[hooks.builtin]\ncargo = false\n\n\
             [hooks.pre-commit.commands.marker]\nrun = \"touch {}\"\nworkspace = true\n",
            marker.display()
        ),
    );
    dir
}

/// The server always enforces trust. Asked for the whole-project phase in an untrusted
/// repository it ran nothing, which is an error, not a pass with no tools; and `poly trust` run
/// beside the server takes effect on its next call, without a restart.
#[test]
fn an_untrusted_whole_project_phase_is_an_error_until_poly_trust_runs() {
    let markers = tempfile::tempdir().unwrap();
    let marker = markers.path().join("job-ran");
    let config_home = tempfile::tempdir().unwrap();
    let cache = tempfile::tempdir().unwrap();
    let dir = hooks_repo(&marker);
    let env: [(&str, &OsStr); 2] = [
        ("POLY_OFFLINE", "1".as_ref()),
        ("POLY_CONFIG_HOME", config_home.path().as_os_str()),
    ];
    let mut server = McpChild::start_with(dir.path(), cache.path(), &env);

    let phase = server.call("check", json!({ "scope": "workspace" }));
    assert!(is_error(&phase), "nothing ran, so nothing was verified: {phase}");
    assert_eq!(document(&phase)["error"]["kind"], "untrusted", "{phase}");
    assert_eq!(document(&phase)["summary"], Value::Null, "{phase}");
    let both = server.call("check", json!({ "scope": "all", "paths": ["a.py"] }));
    assert!(is_error(&both), "half of what was asked did not run: {both}");
    assert!(
        document(&both)["summary"].is_object(),
        "the per-file tier still reports: {both}"
    );
    assert!(document(&both)["workspace"]["untrusted"].is_string(), "{both}");
    let files = server.call("check", json!({ "paths": ["a.py"] }));
    assert_eq!(document(&files)["configs"][0]["withheld"], json!(["hooks"]), "{files}");
    assert!(!marker.exists(), "an untrusted job ran");

    let trusted = Command::new(POLY)
        .args(["trust", "."])
        .current_dir(dir.path())
        .envs(env)
        .env("POLY_CACHE_HOME", cache.path())
        .output()
        .expect("run poly trust");
    assert!(trusted.status.success(), "{}", String::from_utf8_lossy(&trusted.stderr));

    let files = server.call("check", json!({ "paths": ["a.py"] }));
    assert_eq!(document(&files)["configs"][0].get("withheld"), None, "{files}");
    let phase = server.call("check", json!({ "scope": "workspace" }));
    assert!(!is_error(&phase), "{phase}");
    assert_eq!(document(&phase)["passed"], json!(true), "{phase}");
    assert_eq!(document(&phase)["tools"].as_array().map(Vec::len), Some(1), "{phase}");
    assert!(marker.exists(), "the trusted job ran");

    // `scope: all` is plain `poly lint`: one per-file document on both surfaces, and the
    // whole-project report the CLI prints to stderr under `workspace`.
    std::fs::remove_file(&marker).unwrap();
    let cli = Command::new(POLY)
        // A lockless test build cannot lint the TOML config itself.
        .args(["lint", "--format", "json", "--exclude", "poly.toml"])
        .current_dir(dir.path())
        .envs(env)
        .env("POLY_CACHE_HOME", cache.path())
        .output()
        .expect("run poly lint");
    assert_eq!(cli.status.code(), Some(0), "{}", String::from_utf8_lossy(&cli.stderr));
    assert!(marker.exists(), "plain `poly lint` runs the whole-project phase");
    let cli_document: Value = serde_json::from_slice(&cli.stdout).expect("the CLI document");
    assert!(cli_document["summary"].is_object(), "{cli_document}");
    let both = server.call("check", json!({ "scope": "all", "exclude": ["poly.toml"] }));
    assert!(!is_error(&both), "{both}");
    let mut mcp_document = document(&both).clone();
    let map = mcp_document.as_object_mut().expect("a document");
    map.remove("poly");
    let workspace = map.remove("workspace").expect("the whole-project report");
    assert_eq!(mcp_document, cli_document, "`scope: all` is `poly lint --format json`");
    assert_eq!(workspace["passed"], json!(true), "{workspace}");
    assert_eq!(workspace["tools"][0]["id"], "marker", "{workspace}");
}
