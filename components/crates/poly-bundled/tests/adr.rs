//! `poly adr lock`, `unlock` and `new` end to end, through the bundled binary that links the adrlint
//! engine and plans in-process (ADR 0061). The core build reaches the same planner through the `web`
//! component; `crates/poly-cli/tests/adr.rs` covers the surface that needs no engine.
#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use adrlint::{adr, lock};
use tempfile::TempDir;

const POLY: &str = env!("CARGO_BIN_EXE_poly");

/// A record adrlint's own test suite holds valid, with its status and title filled in.
fn record(status: &str, title: &str) -> String {
    format!(
        "---
status: {status}
date: 2026-02-02
decision-makers: [Na'aman Hirschfeld]
consulted: []
informed: []
---

# {title}

## Context and Problem Statement

The linter must parse Markdown, hash documents and ship as a single binary on five platforms. Which
implementation language should the command-line interface use?

## Decision Drivers

* A single self-contained binary for every supported platform
* Access to a mature Markdown parser and formatter

## Considered Options

* Rust
* Go

## Decision Outcome

Chosen option: \"Rust\", because it produces a self-contained binary.

### Consequences

* Good, because distribution is a single binary per platform
* Bad, because contributing requires the Rust toolchain

### Confirmation

Release archives contain one executable and no runtime dependencies.

## Pros and Cons of the Options

### Rust

* Good, because the binary is self-contained
* Bad, because compile times are longer than a scripted tool

### Go

* Good, because the toolchain is simple
* Bad, because the Markdown linting ecosystem is thinner
"
    )
}

struct Project {
    _dir: TempDir,
    root: PathBuf,
    home: TempDir,
}

impl Project {
    fn new(config: &str) -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = std::fs::canonicalize(dir.path()).expect("canonicalize");
        let project = Self {
            _dir: dir,
            root,
            home: tempfile::tempdir().expect("tempdir"),
        };
        project.write("poly.toml", config);
        project
    }

    fn write(&self, name: &str, content: &str) {
        let path = self.root.join(name);
        std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        std::fs::write(path, content).expect("write");
    }

    fn read(&self, name: &str) -> String {
        std::fs::read_to_string(self.root.join(name)).unwrap_or_else(|error| panic!("reading {name}: {error}"))
    }

    fn poly(&self, args: &[&str]) -> Output {
        self.poly_in(&self.root, args)
    }

    fn poly_in(&self, cwd: &Path, args: &[&str]) -> Output {
        Command::new(POLY)
            .arg("--offline")
            .args(args)
            .env("POLY_CACHE_HOME", self.home.path().join("cache"))
            .env("POLY_CONFIG_HOME", self.home.path().join("config"))
            .current_dir(cwd)
            .output()
            .expect("run poly")
    }

    /// The keys of the written `adrlint.lock`, which must parse.
    fn locked(&self) -> Vec<String> {
        let manifest = lock::parse_manifest(&self.read("adrlint.lock")).expect("adrlint.lock parses");
        manifest.adrs.keys().cloned().collect()
    }
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

fn assert_success(output: &Output) {
    assert!(
        output.status.success(),
        "exit {:?}\nstdout: {}\nstderr: {}",
        output.status.code(),
        text(&output.stdout),
        text(&output.stderr)
    );
}

#[test]
fn lock_writes_adrlint_lock_for_the_accepted_records() {
    let project = Project::new("[engines.adrlint]\nenabled = true\n");
    let accepted = record("accepted", "Use Rust");
    project.write("docs/adr/0001-use-rust.md", &accepted);
    project.write("docs/adr/0002-use-go.md", &record("proposed", "Use Go"));

    let output = project.poly(&["adr", "lock"]);
    assert_success(&output);
    assert!(
        text(&output.stdout).contains("locked 1 ADR(s) in adrlint.lock"),
        "{}",
        text(&output.stdout)
    );
    assert_eq!(project.locked(), ["docs/adr/0001-use-rust.md"]);
    let manifest = lock::parse_manifest(&project.read("adrlint.lock")).expect("parses");
    assert_eq!(
        manifest.entry("docs/adr/0001-use-rust.md"),
        Some(&lock::entry(&adr::parse(&accepted), 1)),
        "the entry is adrlint's own hash of the record"
    );

    let check = project.poly(&["adr", "lock", "--check"]);
    assert_success(&check);
    assert!(text(&check.stdout).contains("adrlint.lock is up to date"));

    // Accepting the second record makes the manifest stale until it is locked again.
    project.write("docs/adr/0002-use-go.md", &record("accepted", "Use Go"));
    let stale = project.poly(&["adr", "lock", "--check"]);
    assert_eq!(stale.status.code(), Some(1), "{}", text(&stale.stderr));
    assert!(text(&stale.stderr).contains("would add docs/adr/0002-use-go.md"));
    assert_eq!(project.locked(), ["docs/adr/0001-use-rust.md"], "--check wrote nothing");
}

#[test]
fn lock_is_refused_while_the_log_has_an_adrlint_error() {
    let project = Project::new("[engines.adrlint]\nenabled = true\n");
    project.write("docs/adr/0001-use-rust.md", &record("accepted", "Use Rust"));
    project.write("docs/adr/0002-broken.md", "---\nstatus: accepted\n---\n\n# Broken\n");

    let output = project.poly(&["adr", "lock"]);
    assert_eq!(output.status.code(), Some(1), "{}", text(&output.stderr));
    assert!(
        text(&output.stderr).contains("refusing to lock"),
        "{}",
        text(&output.stderr)
    );
    assert!(!project.root.join("adrlint.lock").exists());
}

#[test]
fn lock_requires_adrlint_to_be_enabled() {
    let project = Project::new("[engines.adrlint]\nenabled = false\n");
    project.write("docs/adr/0001-use-rust.md", &record("accepted", "Use Rust"));
    let output = project.poly(&["adr", "lock"]);
    assert_eq!(output.status.code(), Some(2));
    assert!(text(&output.stderr).contains("adrlint is not enabled"));
}

#[test]
fn per_file_ignores_let_a_template_share_the_first_number() {
    let config = "[lint.markdown.adrlint]\nenabled = true\n";
    let project = Project::new(config);
    project.write("docs/adr/0000-template.md", &record("proposed", "Template"));
    project.write("docs/adr/0000-use-rust.md", &record("accepted", "Use Rust"));
    project.write("docs/adr/0001-use-go.md", &record("accepted", "Use Go"));

    let refused = project.poly(&["adr", "lock"]);
    assert_eq!(refused.status.code(), Some(1), "{}", text(&refused.stderr));
    assert!(text(&refused.stderr).contains("MADR43"), "{}", text(&refused.stderr));

    project.write(
        "poly.toml",
        // The glob is relative to the config's directory, so it names the ADR directory.
        &format!("{config}\n[per-file-ignores]\n\"docs/adr/0000-*.md\" = [\"MADR43\"]\n"),
    );
    let output = project.poly(&["adr", "lock"]);
    assert_success(&output);
    assert_eq!(
        project.locked(),
        ["docs/adr/0000-use-rust.md", "docs/adr/0001-use-go.md"],
        "the template is never locked"
    );
}

#[test]
fn unlock_releases_one_record_and_prints_the_reason() {
    let project = Project::new("[engines.adrlint]\nenabled = true\n");
    project.write("docs/adr/0001-use-rust.md", &record("accepted", "Use Rust"));
    project.write("docs/adr/0002-use-go.md", &record("accepted", "Use Go"));
    assert_success(&project.poly(&["adr", "lock"]));

    // A path relative to a subdirectory resolves against the working directory, not the root.
    let output = project.poly_in(
        &project.root.join("docs/adr"),
        &["adr", "unlock", "0001-use-rust.md", "--reason", "fix a typo"],
    );
    assert_success(&output);
    assert!(
        text(&output.stdout).contains("unlocked docs/adr/0001-use-rust.md: fix a typo"),
        "{}",
        text(&output.stdout)
    );
    assert_eq!(project.locked(), ["docs/adr/0002-use-go.md"]);

    let again = project.poly(&["adr", "unlock", "docs/adr/0001-use-rust.md", "--reason", "again"]);
    assert_eq!(again.status.code(), Some(2));
    assert!(text(&again.stderr).contains("no entry for docs/adr/0001-use-rust.md"));
}

#[test]
fn new_creates_the_next_record_from_the_template_in_the_configured_directory() {
    let project = Project::new("[lint.markdown.adrlint]\nenabled = true\ndir = \"decisions\"\n");
    project.write("decisions/0000-template.md", &record("accepted", "Template"));
    project.write("decisions/0000-use-rust.md", &record("accepted", "Use Rust"));
    project.write("decisions/0001-use-go.md", &record("accepted", "Use Go"));

    let output = project.poly(&["adr", "new", "Choose a database"]);
    assert_success(&output);
    assert!(text(&output.stdout).contains("created decisions/0002-choose-a-database.md"));

    let created = project.read("decisions/0002-choose-a-database.md");
    assert!(created.starts_with("---\nstatus: proposed\ndate: "), "{created}");
    assert!(created.contains("\n# Choose a database\n"), "{created}");
    assert!(!created.contains("# Template"), "{created}");
    assert!(
        created.contains("## Decision Drivers\n"),
        "the template's body is kept: {created}"
    );
}

#[test]
fn new_starts_an_empty_log_with_the_madr_template() {
    let project = Project::new("[engines.adrlint]\nenabled = true\n");
    std::fs::create_dir_all(project.root.join("docs/adr")).expect("mkdir");

    let output = project.poly(&["adr", "new", "Use Rust"]);
    assert_success(&output);
    let created = project.read("docs/adr/0001-use-rust.md");
    assert!(created.starts_with("---\nstatus: proposed\n"), "{created}");
    assert!(created.contains("\n# Use Rust\n"), "{created}");

    let second = project.poly(&["adr", "new", "Use Go"]);
    assert_success(&second);
    assert!(project.root.join("docs/adr/0002-use-go.md").is_file());
}

#[test]
fn lock_check_passes_a_log_that_has_not_opted_in() {
    let project = Project::new("[engines.adrlint]\nenabled = true\n");
    project.write("docs/adr/0001-use-go.md", &record("proposed", "Use Go"));
    let output = project.poly(&["adr", "lock", "--check"]);
    assert_success(&output);
    assert!(
        text(&output.stdout).contains("nothing to lock"),
        "{}",
        text(&output.stdout)
    );
    assert!(!project.root.join("adrlint.lock").exists());
}

/// `lock` reads the log through the project check's own walk, so a record `[discovery] exclude` keeps
/// out of `poly lint` is not locked, and the lint that follows finds no lock entry without its record
/// (`ADR51`).
#[test]
fn lock_judges_the_files_poly_lint_judges() {
    let project = Project::new(
        "[discovery]\nexclude = [\"docs/adr/archive\"]\n\n[engines.adrlint]\nenabled = true\nrecursive = true\n",
    );
    project.write("docs/adr/0001-use-rust.md", &record("accepted", "Use Rust"));
    project.write("docs/adr/archive/0001-use-go.md", &record("accepted", "Use Go"));

    assert_success(&project.poly(&["adr", "lock"]));
    assert_eq!(
        project.locked(),
        ["docs/adr/0001-use-rust.md"],
        "the excluded record is not locked"
    );

    let lint = project.poly(&["lint", "--workspace", "never", "--only", "adrlint", "."]);
    assert_eq!(
        lint.status.code(),
        Some(0),
        "stdout: {}\nstderr: {}",
        text(&lint.stdout),
        text(&lint.stderr)
    );
}
