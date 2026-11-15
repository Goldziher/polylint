//! Repository trust end to end (ADR 0057): a hostile `poly.toml` runs nothing
//! until the user trusts it, and changing what it runs untrusts it again.
//!
//! Every program the fixtures could make poly run is a script that appends to a
//! marker file outside the repository, so "did poly execute it" is a file check,
//! not an inference from output. Each run gets its own `POLY_CONFIG_HOME`, so no
//! test reads or writes the user's real trust store.
#![cfg(unix)]

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

/// A whole-project `[hooks]` job and a `skip` guard, both of which touch a
/// marker, plus a `[cache] dir` pointing outside the repository.
fn hostile_hooks(sandbox: &Sandbox) -> String {
    format!(
        r#"[lint.quality]
enabled = false

[rules]
builtin = false

[cache]
dir = "{cache}"

[hooks]
stages = ["pre-commit"]

[hooks.builtin]
cargo = false

[hooks.pre-commit.commands.hostile]
run = "touch {job}"
workspace = true
skip = [{{ run = "touch {guard}; exit 1" }}]
"#,
        cache = sandbox.marker("hostile-cache").display(),
        job = sandbox.marker("job-ran").display(),
        guard = sandbox.marker("guard-ran").display(),
    )
}

/// A repository holding one Rust file, and an out-of-tree config for it (a
/// `poly.toml` inside the walk would need the TOML component, which a lockless
/// core build cannot fetch).
struct WorkspaceFixture {
    repo: TempDir,
    config_dir: TempDir,
}

impl WorkspaceFixture {
    fn new(config: &str) -> Self {
        let repo = tempfile::tempdir().expect("repo");
        write(&repo.path().join("lib.rs"), "pub fn main() {}\n");
        let config_dir = tempfile::tempdir().expect("config dir");
        write(&config_dir.path().join("poly.toml"), config);
        WorkspaceFixture { repo, config_dir }
    }

    fn config(&self) -> String {
        self.config_dir.path().join("poly.toml").display().to_string()
    }

    fn lint(&self, sandbox: &Sandbox, env: &[(&str, &str)]) -> Output {
        let config = self.config();
        sandbox.poly(self.repo.path(), &["lint", "--config", &config, "."], env)
    }
}

#[test]
fn an_untrusted_whole_project_phase_runs_nothing_and_says_so() {
    let sandbox = Sandbox::new();
    let fixture = WorkspaceFixture::new(&hostile_hooks(&sandbox));
    let output = fixture.lint(&sandbox, &[]);
    let text = text(&output);
    assert!(!sandbox.marker("job-ran").exists(), "an untrusted job ran:\n{text}");
    assert!(!sandbox.marker("guard-ran").exists(), "an untrusted guard ran:\n{text}");
    assert!(
        !sandbox.marker("hostile-cache").exists(),
        "an untrusted [cache] dir was used:\n{text}"
    );
    assert!(text.contains("skipped (untrusted)"), "{text}");
    assert!(text.contains("is not trusted"), "{text}");
    assert!(
        text.contains("hooks") && text.contains("cache.dir"),
        "the note names the keys:\n{text}"
    );
    assert!(text.contains("poly trust"), "the note says how to allow them:\n{text}");
}

#[test]
fn poly_trust_env_honours_the_keys_for_one_run() {
    let sandbox = Sandbox::new();
    let fixture = WorkspaceFixture::new(&hostile_hooks(&sandbox));
    let output = fixture.lint(&sandbox, &[("POLY_TRUST", "1")]);
    let text = text(&output);
    assert!(sandbox.marker("job-ran").exists(), "the trusted job must run:\n{text}");
    assert!(
        sandbox.marker("guard-ran").exists(),
        "the trusted guard must run:\n{text}"
    );
    assert!(!text.contains("is not trusted"), "{text}");

    std::fs::remove_file(sandbox.marker("job-ran")).expect("reset marker");
    let without = fixture.lint(&sandbox, &[]);
    assert!(
        !sandbox.marker("job-ran").exists(),
        "POLY_TRUST must not leave a record behind:\n{}",
        text(&without)
    );
}

#[test]
fn the_trust_flag_honours_the_keys_for_one_run() {
    let sandbox = Sandbox::new();
    let fixture = WorkspaceFixture::new(&hostile_hooks(&sandbox));
    let config = fixture.config();
    let output = sandbox.poly(fixture.repo.path(), &["--trust", "lint", "--config", &config, "."], &[]);
    assert!(sandbox.marker("job-ran").exists(), "{}", text(&output));
}

#[test]
fn poly_trust_records_the_repository_until_its_keys_change() {
    let sandbox = Sandbox::new();
    let fixture = WorkspaceFixture::new(&hostile_hooks(&sandbox));
    let config_dir = fixture.config_dir.path().display().to_string();

    let show = sandbox.poly(fixture.repo.path(), &["trust", "--show", &config_dir], &[]);
    assert!(show.status.success(), "{}", text(&show));
    assert!(text(&show).contains("untrusted"), "{}", text(&show));
    assert!(
        text(&show).contains("hooks"),
        "--show names the gated keys:\n{}",
        text(&show)
    );
    assert!(!sandbox.marker("job-ran").exists(), "--show must run nothing");

    let trusted = sandbox.poly(fixture.repo.path(), &["trust", &config_dir], &[]);
    assert!(trusted.status.success(), "{}", text(&trusted));
    let store = sandbox.config_home.path().join("trust.json");
    let mode = std::fs::metadata(&store)
        .expect("trust.json written")
        .permissions()
        .mode();
    assert_eq!(mode & 0o077, 0, "the store must be owner-only: {mode:o}");

    let output = fixture.lint(&sandbox, &[]);
    assert!(
        sandbox.marker("job-ran").exists(),
        "a trusted job must run:\n{}",
        text(&output)
    );
    let show = sandbox.poly(fixture.repo.path(), &["trust", "--show", &config_dir], &[]);
    assert!(
        text(&show).contains("trusted: these keys are honoured"),
        "{}",
        text(&show)
    );

    // Any change to what the config runs untrusts it again.
    std::fs::remove_file(sandbox.marker("job-ran")).expect("reset marker");
    let changed = hostile_hooks(&sandbox).replace("job-ran", "job-ran-2");
    write(&fixture.config_dir.path().join("poly.toml"), &changed);
    let output = fixture.lint(&sandbox, &[]);
    assert!(
        !sandbox.marker("job-ran-2").exists(),
        "a changed job must not run:\n{}",
        text(&output)
    );
    assert!(text(&output).contains("skipped (untrusted)"), "{}", text(&output));
    let show = sandbox.poly(fixture.repo.path(), &["trust", "--show", &config_dir], &[]);
    assert!(text(&show).contains("changed since `poly trust`"), "{}", text(&show));

    let listed = sandbox.poly(fixture.repo.path(), &["trust", "--list"], &[]);
    assert!(text(&listed).contains("trusted at"), "{}", text(&listed));
    let revoked = sandbox.poly(fixture.repo.path(), &["trust", "--revoke", &config_dir], &[]);
    assert!(text(&revoked).contains("revoked trust"), "{}", text(&revoked));
    let listed = sandbox.poly(fixture.repo.path(), &["trust", "--list"], &[]);
    assert!(text(&listed).contains("no repository is trusted"), "{}", text(&listed));
}

#[test]
fn a_store_directory_others_can_write_is_refused() {
    let sandbox = Sandbox::new();
    let fixture = WorkspaceFixture::new(&hostile_hooks(&sandbox));
    let config_dir = fixture.config_dir.path().display().to_string();
    let trusted = sandbox.poly(fixture.repo.path(), &["trust", &config_dir], &[]);
    assert!(trusted.status.success(), "{}", text(&trusted));
    std::fs::set_permissions(sandbox.config_home.path(), std::fs::Permissions::from_mode(0o777)).expect("chmod");

    let listed = sandbox.poly(fixture.repo.path(), &["trust", "--list"], &[]);
    assert_eq!(listed.status.code(), Some(2), "{}", text(&listed));
    assert!(
        text(&listed).contains("writable by group or others"),
        "{}",
        text(&listed)
    );

    // A lint run treats every repository as untrusted rather than reading the store.
    let output = fixture.lint(&sandbox, &[]);
    assert!(!sandbox.marker("job-ran").exists(), "{}", text(&output));
    assert!(
        text(&output).contains("cannot read the trust store"),
        "{}",
        text(&output)
    );
    std::fs::set_permissions(sandbox.config_home.path(), std::fs::Permissions::from_mode(0o700)).expect("chmod");
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

fn git(repo: &Path, args: &[&str]) {
    let status = Command::new("git")
        .args(args)
        .current_dir(repo)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .expect("git");
    assert!(status.status.success(), "git {args:?}: {}", text(&status));
}

/// A git repository whose `pre-commit` job touches a marker.
fn hooks_repo(sandbox: &Sandbox) -> TempDir {
    let repo = tempfile::tempdir().expect("repo");
    git(repo.path(), &["init", "-q"]);
    write(
        &repo.path().join("poly.toml"),
        &format!(
            "[hooks.pre-commit.commands.hostile]\nrun = \"touch {}\"\n",
            sandbox.marker("hook-ran").display()
        ),
    );
    write(&repo.path().join("a.txt"), "a\n");
    git(repo.path(), &["add", "a.txt", "poly.toml"]);
    repo
}

#[test]
fn poly_hooks_run_refuses_an_untrusted_repository() {
    let sandbox = Sandbox::new();
    let repo = hooks_repo(&sandbox);
    let output = sandbox.poly(repo.path(), &["hooks", "run", "pre-commit"], &[]);
    assert_eq!(output.status.code(), Some(2), "{}", text(&output));
    assert!(text(&output).contains("is not trusted"), "{}", text(&output));
    assert!(text(&output).contains("poly trust"), "{}", text(&output));
    assert!(!sandbox.marker("hook-ran").exists(), "{}", text(&output));

    let trusted = sandbox.poly(repo.path(), &["trust"], &[]);
    assert!(trusted.status.success(), "{}", text(&trusted));
    let output = sandbox.poly(repo.path(), &["hooks", "run", "pre-commit"], &[]);
    assert!(!text(&output).contains("is not trusted"), "{}", text(&output));
    assert!(
        sandbox.marker("hook-ran").exists(),
        "a trusted hook must run:\n{}",
        text(&output)
    );
}

#[test]
fn an_untrusted_prepare_commit_msg_hook_warns_instead_of_blocking_the_commit() {
    let sandbox = Sandbox::new();
    let repo = hooks_repo(&sandbox);
    let message = repo.path().join(".git/COMMIT_EDITMSG");
    write(&message, "feat: x\n");
    let message = message.display().to_string();
    let output = sandbox.poly(
        repo.path(),
        &["hooks", "hook-impl", "--hook-type=prepare-commit-msg", "--", &message],
        &[],
    );
    assert_eq!(output.status.code(), Some(0), "{}", text(&output));
    assert!(
        text(&output).contains("skipping prepare-commit-msg"),
        "{}",
        text(&output)
    );
    assert!(!sandbox.marker("hook-ran").exists(), "{}", text(&output));
}

#[test]
fn poly_hooks_install_records_trust() {
    let sandbox = Sandbox::new();
    let repo = hooks_repo(&sandbox);
    let installed = sandbox.poly(repo.path(), &["hooks", "install"], &[]);
    assert!(installed.status.success(), "{}", text(&installed));
    assert!(text(&installed).contains("Trusted"), "{}", text(&installed));
    let show = sandbox.poly(repo.path(), &["trust", "--show"], &[]);
    assert!(
        text(&show).contains("trusted: these keys are honoured"),
        "{}",
        text(&show)
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
