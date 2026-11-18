//! `poly lock` re-resolves the ref each `[[hooks.sources]]` entry declares, so a
//! floating tag that moved upstream (ai-rulez retags `v5` on every release) is
//! picked up, while the auto-lock path of `poly hooks install` keeps recorded
//! pins and only re-pins an entry whose declared revision changed.
//!
//! Producers are local Git repositories addressed by path, so `git ls-remote`
//! never leaves the machine. `POLY_CACHE_HOME` is redirected at a process-shared
//! tempdir so the real per-user cache is never touched.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::{Once, OnceLock};

use poly_cli::auto_lock::lock_missing_at;
use poly_cli::lock_cmd::{HookPins, LockOutcome, sync};
use poly_cli::polylock::{self, LockedHookSource};
use tempfile::TempDir;

const POLY: &str = env!("CARGO_BIN_EXE_poly");

/// Redirect `POLY_CACHE_HOME` at a process-shared tempdir exactly once; hook
/// sources are keyed by URL and every test uses its own producer.
fn init_cache_home() {
    static INIT: Once = Once::new();
    static CACHE_DIR: OnceLock<TempDir> = OnceLock::new();
    INIT.call_once(|| {
        let dir = tempfile::tempdir().expect("create cache home");
        // SAFETY: run exactly once (via `Once`) before any test in this binary
        // reads `POLY_CACHE_HOME`, so there is no concurrent env access.
        unsafe {
            std::env::set_var("POLY_CACHE_HOME", dir.path());
        }
        let _ = CACHE_DIR.set(dir);
    });
}

fn git(repo: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .args(["-c", "user.name=Poly Test", "-c", "user.email=poly@example.invalid"])
        .args(["-c", "commit.gpgSign=false", "-c", "tag.gpgSign=false"])
        .args(args)
        .current_dir(repo)
        .output()
        .expect("git invocation");
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

/// A producer repository with one commit; returns it, its URL and HEAD.
fn producer() -> (TempDir, String, String) {
    let dir = TempDir::new().expect("tempdir");
    git(dir.path(), &["init", "-q"]);
    let head = commit(dir.path(), "catalog");
    let url = dir.path().to_string_lossy().replace('\\', "/");
    (dir, url, head)
}

/// Commit an empty change in `repo` and return the new HEAD.
fn commit(repo: &Path, message: &str) -> String {
    git(repo, &["commit", "-q", "--allow-empty", "-m", message]);
    git(repo, &["rev-parse", "HEAD"])
}

/// Point the lightweight tag `name` at HEAD, moving it if it exists.
fn tag(repo: &Path, name: &str) {
    git(repo, &["tag", "-f", name]);
}

fn source_config(url: &str, revision: &str) -> String {
    format!("[[hooks.sources]]\nid = \"rules\"\ngit = \"{url}\"\nrevision = \"{revision}\"\nhooks = [\"validate\"]\n")
}

/// A consumer directory whose `poly.toml` declares `url` at `revision`.
fn consumer(url: &str, revision: &str) -> (TempDir, PathBuf) {
    let dir = TempDir::new().expect("consumer dir");
    let path = dir.path().join("poly.toml");
    std::fs::write(&path, source_config(url, revision)).expect("write poly.toml");
    (dir, path)
}

fn locked_entry(root: &Path) -> LockedHookSource {
    let lock = polylock::read(root).unwrap().expect("poly.lock exists");
    assert_eq!(lock.hook_sources.len(), 1, "{lock:?}");
    lock.hook_sources[0].clone()
}

fn check(root: &Path, config: &Path, pins: HookPins) -> LockOutcome {
    sync(root, config, false, true, pins).unwrap()
}

#[test]
fn plain_lock_refreshes_a_moved_tag_and_auto_lock_keeps_the_pin() {
    init_cache_home();
    let (producer, url, first) = producer();
    tag(producer.path(), "v1");
    let (consumer, config) = consumer(&url, "v1");
    let root = consumer.path();

    assert!(matches!(
        sync(root, &config, false, false, HookPins::Refresh).unwrap(),
        LockOutcome::Written(_)
    ));
    let entry = locked_entry(root);
    assert_eq!(
        (entry.revision.as_str(), entry.declared.as_deref()),
        (first.as_str(), Some("v1"))
    );
    assert!(matches!(
        check(root, &config, HookPins::Refresh),
        LockOutcome::InSync(_)
    ));

    let second = commit(producer.path(), "release");
    tag(producer.path(), "v1");

    assert!(lock_missing_at(root, &config).unwrap().is_empty());
    assert_eq!(locked_entry(root).revision, first, "auto-lock never moves a pin");
    assert!(matches!(check(root, &config, HookPins::Keep), LockOutcome::InSync(_)));
    assert_eq!(check(root, &config, HookPins::Refresh), LockOutcome::Drift);
    assert_eq!(locked_entry(root).revision, first, "--check writes nothing");

    assert!(matches!(
        sync(root, &config, false, false, HookPins::Refresh).unwrap(),
        LockOutcome::Written(_)
    ));
    assert_eq!(locked_entry(root).revision, second, "plain lock follows the moved tag");
    assert!(matches!(
        check(root, &config, HookPins::Refresh),
        LockOutcome::InSync(_)
    ));
    assert!(matches!(
        sync(root, &config, false, false, HookPins::Refresh).unwrap(),
        LockOutcome::InSync(_)
    ));
}

#[test]
fn a_changed_declared_revision_is_repinned_on_both_paths() {
    init_cache_home();
    let (producer, url, first) = producer();
    tag(producer.path(), "v1");
    let second = commit(producer.path(), "release");
    tag(producer.path(), "v2");
    let (consumer, config) = consumer(&url, "v1");
    let root = consumer.path();
    sync(root, &config, false, false, HookPins::Refresh).unwrap();
    assert_eq!(locked_entry(root).revision, first);

    std::fs::write(&config, source_config(&url, "v2")).unwrap();
    assert_eq!(check(root, &config, HookPins::Refresh), LockOutcome::Drift);
    assert_eq!(check(root, &config, HookPins::Keep), LockOutcome::Drift);
    let changes = lock_missing_at(root, &config).unwrap();
    assert_eq!(changes.len(), 1, "{changes:?}");
    let entry = locked_entry(root);
    assert_eq!(
        (entry.revision.as_str(), entry.declared.as_deref()),
        (second.as_str(), Some("v2"))
    );

    std::fs::write(&config, source_config(&url, "v1")).unwrap();
    assert!(matches!(
        sync(root, &config, false, false, HookPins::Refresh).unwrap(),
        LockOutcome::Written(_)
    ));
    let entry = locked_entry(root);
    assert_eq!(
        (entry.revision.as_str(), entry.declared.as_deref()),
        (first.as_str(), Some("v1"))
    );
}

#[test]
fn an_entry_without_ref_is_kept_by_auto_lock_and_completed_by_lock() {
    init_cache_home();
    let (producer, url, head) = producer();
    tag(producer.path(), "v1");
    let (consumer, config) = consumer(&url, "v1");
    let root = consumer.path();
    sync(root, &config, false, false, HookPins::Refresh).unwrap();
    let lock_path = root.join(polylock::LOCK_FILE_NAME);
    let legacy: String = std::fs::read_to_string(&lock_path)
        .unwrap()
        .lines()
        .filter(|line| !line.starts_with("ref = "))
        .map(|line| format!("{line}\n"))
        .collect();
    std::fs::write(&lock_path, &legacy).unwrap();
    assert_eq!(locked_entry(root).declared, None, "the fixture is an old-format entry");

    assert!(matches!(check(root, &config, HookPins::Keep), LockOutcome::InSync(_)));
    assert!(lock_missing_at(root, &config).unwrap().is_empty());
    assert_eq!(std::fs::read_to_string(&lock_path).unwrap(), legacy);

    assert_eq!(check(root, &config, HookPins::Refresh), LockOutcome::Drift);
    sync(root, &config, false, false, HookPins::Refresh).unwrap();
    let entry = locked_entry(root);
    assert_eq!(
        (entry.revision.as_str(), entry.declared.as_deref()),
        (head.as_str(), Some("v1"))
    );
}

#[test]
fn a_locked_full_sha_is_rechecked_without_the_network() {
    init_cache_home();
    let (producer, url, head) = producer();
    let (consumer, config) = consumer(&url, &head);
    let root = consumer.path();
    sync(root, &config, false, false, HookPins::Refresh).unwrap();
    assert_eq!(locked_entry(root).declared.as_deref(), Some(head.as_str()));

    // With the producer gone, any `ls-remote` or fetch would fail.
    drop(producer);
    assert!(matches!(
        check(root, &config, HookPins::Refresh),
        LockOutcome::InSync(_)
    ));
    assert!(matches!(
        sync(root, &config, false, false, HookPins::Refresh).unwrap(),
        LockOutcome::InSync(_)
    ));
}

fn poly(repo: &Path, cache: &Path, args: &[&str], offline: bool) -> Output {
    let mut command = Command::new(POLY);
    command
        .args(args)
        .current_dir(repo)
        .env("POLY_CACHE_HOME", cache)
        .env("POLY_CONFIG_HOME", cache.join("config"))
        .env("POLY_TRUST", "1")
        .env_remove("POLY_OFFLINE");
    if offline {
        command.env("POLY_OFFLINE", "1");
    }
    command.output().expect("poly invocation")
}

#[test]
fn poly_lock_reports_only_moved_pins_and_tolerates_offline() {
    let (producer, url, first) = producer();
    tag(producer.path(), "v1");
    let (consumer, _) = consumer(&url, "v1");
    let repo = consumer.path();
    git(repo, &["init", "-q"]);
    let cache = TempDir::new().expect("cache");

    let output = poly(repo, cache.path(), &["lock"], false);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    assert!(stdout.contains(&format!("{url} v1 -> {first}")), "{stdout}");

    let output = poly(repo, cache.path(), &["lock"], false);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success());
    assert!(!stdout.contains("->"), "an unmoved pin is not reported: {stdout}");

    let second = commit(producer.path(), "release");
    tag(producer.path(), "v1");

    for args in [&["lock"][..], &["lock", "--check"][..]] {
        let output = poly(repo, cache.path(), args, true);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(output.status.success(), "offline {args:?}: {stderr}");
        assert!(stderr.contains("not refreshed"), "offline {args:?}: {stderr}");
    }
    assert_eq!(locked_entry(repo).revision, first);

    let output = poly(repo, cache.path(), &["lock", "--check"], false);
    assert_eq!(
        output.status.code(),
        Some(1),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let output = poly(repo, cache.path(), &["lock"], false);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    assert!(stdout.contains(&format!("{url} v1 -> {second}")), "{stdout}");
    assert_eq!(locked_entry(repo).revision, second);
}
