//! `[per-file-ignores]` keys and the run's project root hold whatever path the
//! run names and wherever it starts.
//!
//! A glob key is relative to the directory of the `poly.toml` declaring it, so
//! `poly lint docs/adr` must apply `"docs/adr/*.c"` exactly as `poly lint .` does,
//! and a run from a subdirectory with relative paths must resolve the `poly.toml`
//! above it. `file-too-long` is a `quality` rule that counts newlines, so these
//! need no component engine.
#![cfg(unix)]

use std::path::Path;
use std::process::Command;

use tempfile::TempDir;

const POLY: &str = env!("CARGO_BIN_EXE_poly");

const LONG: &str = "int a;\nint b;\nint c;\nint d;\nint e;\n";

/// A repo whose `poly.toml` flags any file over two lines and ignores the rule
/// under `docs/adr`, with one long file inside and one outside the ignored glob.
fn repo(extra: &str) -> TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    std::fs::create_dir_all(root.join("docs/adr")).expect("mkdir adr");
    std::fs::create_dir_all(root.join("docs/other")).expect("mkdir other");
    std::fs::write(
        root.join("poly.toml"),
        format!(
            "[workspace]\nroot = true\n\n[engines.quality]\nfile_too_long_lines = 2\n{extra}\n\
             [per-file-ignores]\n\"docs/adr/*.c\" = [\"file-too-long\"]\n"
        ),
    )
    .expect("write poly.toml");
    std::fs::write(root.join("docs/adr/x.c"), LONG).expect("write x.c");
    std::fs::write(root.join("docs/other/y.c"), LONG).expect("write y.c");
    dir
}

fn lint_json(cwd: &Path, args: &[&str]) -> serde_json::Value {
    let output = Command::new(POLY)
        .args(["lint", "--workspace", "never", "--no-cache", "--format", "json"])
        .args(args)
        .current_dir(cwd)
        .output()
        .expect("run poly");
    serde_json::from_slice(&output.stdout)
        .unwrap_or_else(|error| panic!("stdout is one JSON document ({error}): {output:?}"))
}

/// Paths of the files that still carry a `file-too-long` finding.
fn flagged(report: &serde_json::Value) -> Vec<String> {
    let mut paths: Vec<String> = report["results"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|result| {
            result["diagnostics"]
                .as_array()
                .into_iter()
                .flatten()
                .any(|d| d["rule"] == "file-too-long" || d["code"] == "file-too-long")
        })
        .filter_map(|result| result["path"].as_str().map(str::to_owned))
        .collect();
    paths.sort();
    paths
}

fn assert_ignored(cwd: &Path, args: &[&str], ignored: &str, reported: &str) {
    let report = lint_json(cwd, args);
    let paths = flagged(&report);
    assert!(
        paths.iter().any(|p| p.ends_with(reported)),
        "{args:?}: the control file {reported} must still be flagged, got {paths:?}"
    );
    assert!(
        !paths.iter().any(|p| p.ends_with(ignored)),
        "{args:?}: {ignored} must be ignored by its per-file-ignores glob, got {paths:?}"
    );
}

#[test]
fn per_file_ignores_apply_to_a_dot_run() {
    let dir = repo("");
    assert_ignored(dir.path(), &["."], "adr/x.c", "other/y.c");
}

#[test]
fn per_file_ignores_apply_to_a_dot_slash_directory() {
    let dir = repo("");
    assert_ignored(dir.path(), &["./docs/adr", "./docs/other"], "adr/x.c", "other/y.c");
}

#[test]
fn per_file_ignores_apply_to_a_named_file() {
    let dir = repo("");
    assert_ignored(dir.path(), &["docs/adr/x.c", "docs/other/y.c"], "adr/x.c", "other/y.c");
}

#[test]
fn per_file_ignores_apply_to_a_bare_directory() {
    let dir = repo("");
    assert_ignored(dir.path(), &["docs/adr", "docs/other"], "adr/x.c", "other/y.c");
}

#[test]
fn per_file_ignores_apply_to_a_bare_directory_from_a_subdirectory() {
    let dir = repo("");
    assert_ignored(&dir.path().join("docs"), &["adr", "other"], "adr/x.c", "other/y.c");
}
