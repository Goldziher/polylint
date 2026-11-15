//! Repository trust end to end (ADR 0057): a hostile `poly.toml` runs nothing
//! until the user trusts it, and changing what it runs untrusts it again.
//!
//! Every program the fixtures could make poly run is a script that appends to a
//! marker file outside the repository, so "did poly execute it" is a file check,
//! not an inference from output. Each run gets its own `POLY_CONFIG_HOME`, so no
//! test reads or writes the user's real trust store.
#![cfg(unix)]
// The helpers serve a test gated on `all-engines`, which a core build compiles out.
#![cfg_attr(not(feature = "all-engines"), allow(dead_code, unused_imports))]

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use tempfile::TempDir;

const POLY: &str = env!("CARGO_BIN_EXE_poly");

struct Sandbox {
    /// The per-test `POLY_CONFIG_HOME`.
    config_home: TempDir,
    /// Where marker files land, outside every walked tree.
    markers: TempDir,
}

impl Sandbox {
    fn new() -> Self {
        Sandbox {
            config_home: tempfile::tempdir().expect("config home"),
            markers: tempfile::tempdir().expect("markers"),
        }
    }

    fn marker(&self, name: &str) -> PathBuf {
        self.markers.path().join(name)
    }

    fn poly(&self, cwd: &Path, args: &[&str], env: &[(&str, &str)]) -> Output {
        let mut command = Command::new(POLY);
        command
            .args(args)
            .current_dir(cwd)
            .env("POLY_CONFIG_HOME", self.config_home.path())
            .env("POLY_OFFLINE", "1")
            .env("NO_COLOR", "1")
            .env_remove("POLY_TRUST");
        for (key, value) in env {
            command.env(key, value);
        }
        command.output().expect("run poly")
    }
}

fn text(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

fn write(path: &Path, body: &str) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("mkdir");
    }
    std::fs::write(path, body).expect("write fixture");
}

#[test]
fn an_extends_base_outside_the_repository_is_not_read_until_trusted() {
    let sandbox = Sandbox::new();
    let outside = tempfile::tempdir().expect("outside");
    let base = outside.path().join("base.toml");
    write(&base, "[defaults]\nline_length = 77\n");
    let repo = tempfile::tempdir().expect("repo");
    std::fs::create_dir(repo.path().join(".git")).expect(".git");
    write(
        &repo.path().join("poly.toml"),
        &format!("extends = [{:?}]\n", base.display().to_string()),
    );

    let untrusted = sandbox.poly(repo.path(), &["config", "show"], &[]);
    assert!(untrusted.status.success(), "{}", text(&untrusted));
    assert!(
        !String::from_utf8_lossy(&untrusted.stdout).contains("line_length = 77"),
        "an untrusted config read a file outside its repository:\n{}",
        text(&untrusted)
    );

    let show = sandbox.poly(repo.path(), &["trust", "--show"], &[]);
    assert!(
        text(&show).contains("extends base outside the repository"),
        "{}",
        text(&show)
    );
    let trusted = sandbox.poly(repo.path(), &["trust"], &[]);
    assert!(trusted.status.success(), "{}", text(&trusted));
    let shown = sandbox.poly(repo.path(), &["config", "show"], &[]);
    assert!(
        String::from_utf8_lossy(&shown.stdout).contains("line_length = 77"),
        "a trusted base must be read:\n{}",
        text(&shown)
    );
}

/// A `golangci-lint` stand-in that logs who ran it, with which argv and
/// `TRUST_PROBE`, then reports no issues.
#[cfg(feature = "all-engines")]
fn fake_linter(dir: &Path, name: &str, who: &str) -> PathBuf {
    let path = dir.join(name);
    write(
        &path,
        &format!(
            "#!/bin/sh\necho \"{who} probe=$TRUST_PROBE args=$*\" >> \"$TRUST_MARKER\"\nprintf '{{\"Issues\":[]}}'\n"
        ),
    );
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    path
}

/// A Go repository whose `[tools.golangci-lint]` sets an absolute `bin`, an `env`
/// and `extra_args`; a bare `golangci-lint` on `PATH` stands in for the user's own.
// The Go recipe needs engine components a lockless core build cannot fetch.
#[cfg(feature = "all-engines")]
#[test]
fn catalog_tool_keys_are_ignored_untrusted_and_honoured_trusted() {
    let sandbox = Sandbox::new();
    let tools = tempfile::tempdir().expect("tools");
    let on_path = tempfile::tempdir().expect("path dir");
    fake_linter(on_path.path(), "golangci-lint", "path");
    let evil = fake_linter(tools.path(), "evil-lint", "evil");
    let repo = tempfile::tempdir().expect("repo");
    std::fs::create_dir(repo.path().join(".git")).expect(".git");
    write(&repo.path().join("go.mod"), "module example.com/demo\n\ngo 1.22\n");
    write(&repo.path().join("main.go"), "package main\n\nfunc main() {}\n");
    write(
        &repo.path().join("poly.toml"),
        &format!(
            "[tools.golangci-lint]\nenabled = true\nbin = \"{}\"\nextra_args = [\"--hostile\"]\n\
             [tools.golangci-lint.env]\nTRUST_PROBE = \"hostile\"\n",
            evil.display()
        ),
    );
    let marker = sandbox.marker("tool-log");
    let marker_text = marker.display().to_string();
    let path = format!(
        "{}:{}",
        on_path.path().display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let lint = |env: &[(&str, &str)]| {
        let mut all = vec![("TRUST_MARKER", marker_text.as_str()), ("PATH", path.as_str())];
        all.extend_from_slice(env);
        sandbox.poly(
            repo.path(),
            &["lint", "--workspace", "never", "--no-cache", "main.go"],
            &all,
        )
    };

    let untrusted = lint(&[]);
    let log = std::fs::read_to_string(&marker).unwrap_or_default();
    assert!(
        !log.contains("evil"),
        "an untrusted absolute `bin` ran:\n{log}\n{}",
        text(&untrusted)
    );
    assert!(
        !log.contains("probe=hostile"),
        "an untrusted `env` reached the tool:\n{log}"
    );
    assert!(
        !log.contains("--hostile"),
        "untrusted `extra_args` reached the tool:\n{log}"
    );
    assert!(
        text(&untrusted).contains("tools.golangci-lint.bin"),
        "{}",
        text(&untrusted)
    );

    std::fs::remove_file(&marker).ok();
    let trusted = lint(&[("POLY_TRUST", "1")]);
    let log = std::fs::read_to_string(&marker).unwrap_or_default();
    assert!(
        log.contains("evil probe=hostile"),
        "trusted keys must apply:\n{log}\n{}",
        text(&trusted)
    );
    assert!(log.contains("--hostile"), "{log}");

    std::fs::remove_file(&marker).ok();
    let recorded = sandbox.poly(repo.path(), &["trust"], &[]);
    assert!(recorded.status.success(), "{}", text(&recorded));
    lint(&[]);
    let log = std::fs::read_to_string(&marker).unwrap_or_default();
    assert!(
        log.contains("evil probe=hostile"),
        "`poly trust` must allow them:\n{log}"
    );
}
