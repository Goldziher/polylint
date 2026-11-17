//! End-to-end coverage for `poly cache` driving the result-cache maintenance
//! API over an explicit `--cache-dir`.
//!
//! These shell out to the built `poly` binary (via `CARGO_BIN_EXE_poly`) and
//! seed a cache tree on disk, so they exercise the full path: arg parsing →
//! root resolution → `ResultCache` maintenance → human output → exit code.
#![cfg(unix)]

use std::path::Path;
use std::process::{Command, Output};

use tempfile::TempDir;

const POLY: &str = env!("CARGO_BIN_EXE_poly");

/// Seed a cache tree with `entries` files (each `bytes` long) in `results/hook`.
fn seed_cache(cache_dir: &Path, entries: usize, bytes: usize) {
    let hook = cache_dir.join("results").join("hook");
    std::fs::create_dir_all(&hook).expect("create results/hook");
    std::fs::create_dir_all(cache_dir.join("results").join("lint")).expect("create results/lint");
    std::fs::create_dir_all(cache_dir.join("results").join("fmt")).expect("create results/fmt");
    std::fs::write(cache_dir.join("VERSION"), "1").expect("write VERSION");
    for index in 0..entries {
        std::fs::write(hook.join(format!("entry{index}")), vec![b'x'; bytes]).expect("write entry");
    }
}

fn poly_cache(cache_dir: &Path, args: &[&str]) -> Output {
    Command::new(POLY)
        .arg("cache")
        .arg("--cache-dir")
        .arg(cache_dir)
        .args(args)
        .output()
        .expect("poly invocation")
}

#[test]
fn stats_reports_entries_for_a_populated_cache() {
    let tmp = TempDir::new().expect("tempdir");
    let cache_dir = tmp.path().join("cache");
    seed_cache(&cache_dir, 3, 100);

    let output = poly_cache(&cache_dir, &["stats"]);
    assert!(output.status.success(), "stats should succeed");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("entries"), "stats output: {stdout}");
    assert!(stdout.contains("total: 3 entries"), "stats output: {stdout}");
}

#[test]
fn size_subcommand_is_removed() {
    let tmp = TempDir::new().expect("tempdir");
    let output = poly_cache(&tmp.path().join("cache"), &["size"]);
    assert_eq!(
        output.status.code(),
        Some(2),
        "`poly cache size` was removed: a usage error"
    );
}

#[test]
fn clean_empties_the_cache() {
    let tmp = TempDir::new().expect("tempdir");
    let cache_dir = tmp.path().join("cache");
    seed_cache(&cache_dir, 4, 25);

    let clean = poly_cache(&cache_dir, &["clean"]);
    assert!(clean.status.success(), "clean should succeed");

    let stats = poly_cache(&cache_dir, &["stats"]);
    assert!(stats.status.success());
    let stdout = String::from_utf8_lossy(&stats.stdout);
    assert!(
        stdout.contains("total: 0 entries"),
        "clean must empty the cache: {stdout}"
    );
}

#[test]
fn default_subcommand_is_stats() {
    let tmp = TempDir::new().expect("tempdir");
    let cache_dir = tmp.path().join("cache");
    seed_cache(&cache_dir, 1, 10);

    let output = poly_cache(&cache_dir, &[]);
    assert!(output.status.success(), "default (stats) should succeed");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("cache format version"), "output: {stdout}");
}

#[test]
fn stats_takes_format_like_every_other_command() {
    let tmp = TempDir::new().expect("tempdir");
    let cache_dir = tmp.path().join("cache");
    seed_cache(&cache_dir, 2, 10);

    let output = poly_cache(&cache_dir, &["stats", "--format", "json"]);
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).expect("stdout is one JSON document");
    assert_eq!(json["total_entries"], 2, "{json}");
    assert!(json["namespaces"].as_array().is_some_and(|n| !n.is_empty()), "{json}");

    let global_before = poly_cache(&cache_dir, &["--format", "json"]);
    assert!(
        global_before.status.success(),
        "the default subcommand takes --format too"
    );
    let freed = poly_cache(&cache_dir, &["clean", "--format", "json"]);
    let json: serde_json::Value = serde_json::from_slice(&freed.stdout).expect("clean emits JSON");
    assert_eq!(json["command"], "clean");
    assert_eq!(json["freed_bytes"], 20);
}

#[test]
fn cache_dir_is_accepted_after_the_subcommand() {
    let tmp = TempDir::new().expect("tempdir");
    seed_cache(tmp.path(), 2, 10);
    let output = Command::new(POLY)
        .arg("cache")
        .arg("stats")
        .arg("--cache-dir")
        .arg(tmp.path())
        .output()
        .expect("poly invocation");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("2 entries"));
}

/// Run the built `poly` in `cwd` with a sandboxed cache home and config home, trusting the
/// repository so `[cache] dir` is honoured.
fn poly_in(cwd: &Path, home: &Path, args: &[&str]) -> Output {
    Command::new(POLY)
        .args(args)
        .current_dir(cwd)
        .env("POLY_CACHE_HOME", home.join("cache-home"))
        .env("POLY_CONFIG_HOME", home.join("config-home"))
        .env("POLY_TRUST", "1")
        .env("POLY_OFFLINE", "1")
        .env("NO_COLOR", "1")
        .output()
        .expect("poly invocation")
}

fn json_of(output: &Output) -> serde_json::Value {
    assert!(
        output.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("stdout is one JSON document")
}

/// A relative `[cache] dir` is anchored at the root `poly.toml`: a run from a subdirectory reuses
/// the cache a run from the root filled, `poly cache stats` there measures it, and a nested config's
/// own `[cache]` is ignored with a warning naming it.
#[test]
fn a_relative_cache_dir_is_one_cache_from_every_directory() {
    let tmp = TempDir::new().expect("tempdir");
    let repo = tmp.path().join("repo");
    let sub = repo.join("pkg");
    std::fs::create_dir_all(&sub).expect("create the repository");
    let init = Command::new("git").args(["init", "-q"]).current_dir(&repo).status();
    assert!(init.expect("git init").success());
    std::fs::write(repo.join("poly.toml"), "[cache]\ndir = \".poly-cache\"\n").expect("root config");
    std::fs::write(sub.join("poly.toml"), "[cache]\ndir = \"nested-cache\"\n").expect("nested config");
    std::fs::write(sub.join("main.py"), "x = 1\n").expect("source");
    std::fs::write(repo.join(".gitignore"), ".poly-cache/\n").expect("ignore the cache");
    // Named rather than `.`: the TOML engine is a component this test build has no lock for.
    let lint = |path| ["lint", "--workspace", "never", "--format", "json", path];

    let from_root = json_of(&poly_in(&repo, tmp.path(), &lint("pkg/main.py")));
    assert!(from_root["summary"]["checked"].as_u64() >= Some(1), "{from_root}");
    let stats_from_root = json_of(&poly_in(&repo, tmp.path(), &["cache", "stats", "--format", "json"]));
    assert!(stats_from_root["total_entries"].as_u64() > Some(0), "{stats_from_root}");

    let output = poly_in(&sub, tmp.path(), &lint("main.py"));
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    let from_sub = json_of(&output);
    assert!(from_sub["summary"]["checked"].as_u64() >= Some(1), "{from_sub}");
    assert!(
        stderr.contains(&sub.join("poly.toml").display().to_string()) && stderr.contains("[cache]"),
        "the nested `[cache]` is reported: {stderr}"
    );

    let stats_from_sub = json_of(&poly_in(&sub, tmp.path(), &["cache", "stats", "--format", "json"]));
    assert_eq!(
        stats_from_sub["total_entries"], stats_from_root["total_entries"],
        "the subdirectory run neither started a second cache nor grew this one"
    );
    assert!(repo.join(".poly-cache").join("VERSION").is_file());
    for stray in [sub.join(".poly-cache"), sub.join("nested-cache")] {
        assert!(!stray.exists(), "{} must not exist", stray.display());
    }
    let default_slots = std::fs::read_dir(tmp.path().join("cache-home"))
        .into_iter()
        .flatten()
        .flatten()
        .filter(|slot| slot.path().join("VERSION").is_file())
        .count();
    assert_eq!(default_slots, 0, "no result cache in the default location");
}
