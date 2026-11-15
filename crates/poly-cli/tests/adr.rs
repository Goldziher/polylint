//! The `poly adr` surface that needs no adrlint engine: the subcommands exist, arguments and settings
//! are checked before any engine is asked, and a build without the web family (every build of this
//! workspace) asks the `web` component (ADR 0061). Lock, unlock and new themselves run end to end in
//! `components/crates/poly-bundled/tests/adr.rs`, against the binary that links the engine.
#![cfg(unix)]

use std::path::Path;
use std::process::{Command, Output};

use tempfile::TempDir;

const POLY: &str = env!("CARGO_BIN_EXE_poly");

/// A project whose `poly.toml` is `config`, with `dirs` created.
fn project(config: &str, dirs: &[&str]) -> TempDir {
    let dir = TempDir::new().expect("tempdir");
    std::fs::write(dir.path().join("poly.toml"), config).expect("write poly.toml");
    for sub in dirs {
        std::fs::create_dir_all(dir.path().join(sub)).expect("mkdir");
    }
    dir
}

fn poly(cwd: &Path, args: &[&str]) -> Output {
    let home = cwd.join(".poly-test-home");
    Command::new(POLY)
        .arg("--offline")
        .args(args)
        .current_dir(cwd)
        .env("POLY_CACHE_HOME", home.join("cache"))
        .env("POLY_CONFIG_HOME", home.join("config"))
        .env("POLY_COMPONENT_CACHE", home.join("components"))
        .output()
        .expect("poly invocation")
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

#[test]
fn the_help_lists_lock_unlock_and_new() {
    let dir = project("", &[]);
    let output = poly(dir.path(), &["adr", "--help"]);
    assert!(output.status.success(), "{}", stderr(&output));
    let help = String::from_utf8_lossy(&output.stdout);
    for command in ["lock", "unlock", "new"] {
        assert!(
            help.lines().any(|line| line.trim_start().starts_with(command)),
            "`{command}` missing from:\n{help}"
        );
    }
}

#[test]
fn unlock_requires_a_reason() {
    let dir = project("", &[]);
    let missing = poly(dir.path(), &["adr", "unlock", "docs/adr/0001-x.md"]);
    assert_eq!(missing.status.code(), Some(2));
    assert!(stderr(&missing).contains("--reason"), "{}", stderr(&missing));

    let blank = poly(dir.path(), &["adr", "unlock", "docs/adr/0001-x.md", "--reason", "  "]);
    assert_eq!(blank.status.code(), Some(2));
    assert!(
        stderr(&blank).contains("poly adr: --reason must not be empty"),
        "{}",
        stderr(&blank)
    );
}

#[test]
fn lock_refuses_when_adrlint_is_not_enabled() {
    let dir = project("", &["docs/adr"]);
    let output = poly(dir.path(), &["adr", "lock"]);
    assert_eq!(output.status.code(), Some(2));
    assert!(
        stderr(&output).contains("adrlint is not enabled"),
        "{}",
        stderr(&output)
    );
    assert!(!dir.path().join("adrlint.lock").exists());
}

#[test]
fn new_reads_the_configured_directory() {
    let dir = project(
        "[lint.markdown.adrlint]\nenabled = true\ndir = \"decisions\"\n",
        &["docs/adr"],
    );
    let output = poly(dir.path(), &["adr", "new", "Use Rust"]);
    assert_eq!(output.status.code(), Some(2));
    assert!(
        stderr(&output).contains("decisions does not exist"),
        "the default docs/adr must not be used: {}",
        stderr(&output)
    );
}

#[test]
fn new_asks_which_of_several_directories() {
    let dir = project(
        "[lint.markdown.adrlint]\nenabled = true\npaths = [\"a\", \"b\"]\n",
        &["a", "b"],
    );
    let output = poly(dir.path(), &["adr", "new", "Use Rust"]);
    assert_eq!(output.status.code(), Some(2));
    assert!(stderr(&output).contains("several ADR directories are configured (a, b)"));
}

#[test]
fn a_build_without_the_web_family_asks_the_web_component() {
    let dir = project("[engines.adrlint]\nenabled = true\n", &["docs/adr"]);
    let output = poly(dir.path(), &["adr", "new", "Use Rust"]);
    let stderr = stderr(&output);
    assert_eq!(output.status.code(), Some(2), "{stderr}");
    assert!(stderr.starts_with("poly adr: "), "{stderr}");
    let reached_component = stderr.contains("component not installed: web (offline)")
        || stderr.contains("this build embeds no component lock, so `web` cannot be fetched");
    assert!(reached_component, "poly adr did not ask the web component: {stderr}");
    assert!(
        std::fs::read_dir(dir.path().join("docs/adr"))
            .expect("read docs/adr")
            .next()
            .is_none(),
        "nothing is written when the engine cannot be asked"
    );
}
