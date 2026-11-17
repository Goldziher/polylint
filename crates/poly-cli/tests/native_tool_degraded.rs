//! A default-on native tool missing from `PATH` must show in the report (ADR 0075, issue #230).
//!
//! Without `rustfmt`, a Rust file goes to the tier-2 reindenter (ADR 0014) and `poly fmt` printed
//! `All formatted.` with nothing to say that rustfmt never saw the file. These run the built binary
//! with `PATH` pointed at an empty directory, so every native tool is absent whatever the host has.
#![cfg(all(unix, feature = "all-engines"))]

use std::path::Path;
use std::process::{Command, Output};

use serde_json::Value;
use tempfile::TempDir;

const POLY: &str = env!("CARGO_BIN_EXE_poly");

/// Already in rustfmt's layout, so the verdict does not depend on which formatter ran.
const RUST: &str = "pub fn add(left: u64, right: u64) -> u64 {\n    left + right\n}\n";

struct Repo {
    dir: TempDir,
    cache: TempDir,
    empty_path: TempDir,
}

fn repo(files: &[(&str, &str)]) -> Repo {
    let dir = tempfile::tempdir().expect("tempdir");
    for (name, body) in files {
        let path = dir.path().join(name);
        std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        std::fs::write(path, body).expect("write fixture");
    }
    Repo {
        dir,
        cache: tempfile::tempdir().expect("cache dir"),
        empty_path: tempfile::tempdir().expect("empty PATH dir"),
    }
}

impl Repo {
    /// Run poly with no native tool on `PATH`.
    fn without_tools(&self, args: &[&str]) -> Output {
        self.run(args, Some(self.empty_path.path()))
    }

    /// Run poly with the host's own `PATH`.
    fn with_host_path(&self, args: &[&str]) -> Output {
        self.run(args, None)
    }

    fn run(&self, args: &[&str], path: Option<&Path>) -> Output {
        let mut command = Command::new(POLY);
        command
            .args(args)
            .current_dir(self.dir.path())
            .env("POLY_CACHE_HOME", self.cache.path());
        if let Some(path) = path {
            command.env("PATH", path);
        }
        command.output().expect("run poly")
    }
}

fn document(output: &Output) -> Value {
    let stdout = String::from_utf8_lossy(&output.stdout);
    serde_json::from_str(&stdout).unwrap_or_else(|error| {
        panic!(
            "stdout is not one JSON document ({error}):\n{stdout}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stderr)
        )
    })
}

/// The reported defect: rustfmt absent, the Rust file formatted by tier 2, and the report naming it
/// both in the document and in the pretty summary — on a cache hit as much as on a fresh run.
#[test]
fn a_missing_rustfmt_names_the_rust_files_the_generic_tier_formatted() {
    let repo = repo(&[("src/lib.rs", RUST)]);

    for pass in ["first", "cached"] {
        let output = repo.without_tools(&["fmt", "--format", "json", "src/lib.rs"]);
        assert_eq!(
            output.status.code(),
            Some(0),
            "{pass}: degradation must not change the exit code"
        );
        let json = document(&output);
        assert_eq!(
            json["degraded"],
            serde_json::json!([{
                "tool": "rustfmt",
                "language": "rust",
                "reason": "not found on PATH",
                "files": ["src/lib.rs"],
            }]),
            "{pass}: {json:#}"
        );
        assert_eq!(json["summary"]["checked"], 1, "{pass}: the file was still formatted");
        assert_eq!(json["skipped"], serde_json::json!([]), "{pass}: not a skip");
    }

    let output = repo.without_tools(&["fmt", "src/lib.rs"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("rustfmt not found on PATH: 1 Rust file formatted by the generic tier"),
        "{stdout}"
    );
    assert_eq!(output.status.code(), Some(0), "{stdout}");

    let strict = repo.without_tools(&["fmt", "--deny-skips", "src/lib.rs"]);
    assert_eq!(
        strict.status.code(),
        Some(0),
        "a degraded file is not a skip and must not trip --deny-skips:\n{}",
        String::from_utf8_lossy(&strict.stdout)
    );
}

/// shellcheck is a linter: without it nothing lints shell, and the lint report says why.
#[test]
fn a_missing_shellcheck_names_the_shell_files_linted_without_it() {
    let repo = repo(&[("run.sh", "#!/bin/sh\necho \"$1\"\n")]);

    let output = repo.without_tools(&["lint", "--workspace", "never", "--format", "json", "run.sh"]);
    let json = document(&output);
    assert_eq!(
        json["degraded"],
        serde_json::json!([{
            "tool": "shellcheck",
            "language": "shell",
            "reason": "not found on PATH",
            "files": ["run.sh"],
        }]),
        "{json:#}"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("shellcheck not found on PATH: 1 Shell file linted without it"),
        "the json run echoes the note on stderr:\n{stderr}"
    );
}

/// A run with no file for the missing tool has nothing degraded, and keeps the document it had.
#[test]
fn the_field_is_absent_when_no_file_needed_the_missing_tool() {
    let repo = repo(&[("main.py", "x = 1\n")]);

    let output = repo.without_tools(&["fmt", "--format", "json", "main.py"]);
    let json = document(&output);
    assert_eq!(json["summary"]["checked"], 1, "{json:#}");
    assert!(json.get("degraded").is_none(), "{json:#}");
}

/// A tool the config switched off is not wanted, so its absence degrades nothing.
#[test]
fn the_field_is_absent_when_rustfmt_is_switched_off() {
    let repo = repo(&[
        ("poly.toml", "[engines.rustfmt]\nenabled = false\n"),
        ("src/lib.rs", RUST),
    ]);

    let output = repo.without_tools(&["fmt", "--format", "json", "src/lib.rs"]);
    let json = document(&output);
    assert_eq!(json["summary"]["checked"], 1, "{json:#}");
    assert!(json.get("degraded").is_none(), "{json:#}");
}

/// With rustfmt on the host, nothing was degraded and the field is absent.
#[test]
fn the_field_is_absent_when_rustfmt_is_present() {
    let found = Command::new("rustfmt").arg("--version").output();
    if !found.is_ok_and(|output| output.status.success()) {
        eprintln!("rustfmt not found on PATH — skipping the_field_is_absent_when_rustfmt_is_present");
        return;
    }
    let repo = repo(&[("src/lib.rs", RUST)]);

    let output = repo.with_host_path(&["fmt", "--format", "json", "src/lib.rs"]);
    let json = document(&output);
    assert_eq!(json["summary"]["checked"], 1, "{json:#}");
    assert!(json.get("degraded").is_none(), "{json:#}");
}
