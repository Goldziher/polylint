//! `[cache] enabled = false` turns a lint or fmt run's result cache off, as `--no-cache` does. One test per process:
//! `POLY_CACHE_HOME` is process-global.
#![cfg(feature = "engine-python")]

use std::path::{Path, PathBuf};

use poly_core::{Config, RunOptions};

fn files_under(dir: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return found;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            found.extend(files_under(&path));
        } else {
            found.push(path);
        }
    }
    found
}

/// Whether `path` is a stored `namespace` result: a stat-index file or a content-addressed entry.
fn holds_results(path: &Path, namespace: &str) -> bool {
    let name = |path: Option<&Path>| {
        path.and_then(Path::file_name)
            .and_then(|name| name.to_str())
            .map(str::to_owned)
    };
    name(Some(path)) == Some(format!("{namespace}.idx"))
        || (name(path.parent()).as_deref() == Some(namespace)
            && name(path.parent().and_then(Path::parent)).as_deref() == Some("results"))
}

/// Lint then format `file` under a config whose cache lives at `root`, asserting both runs checked it, and
/// return the files the cache holds afterwards.
fn run_with_cache(file: &Path, root: &Path, enabled: bool) -> Vec<PathBuf> {
    let config = Config {
        cache_dir: Some(root.to_path_buf()),
        cache_enabled: enabled,
        ..Config::default()
    };
    let opts = RunOptions {
        explicit_config: true,
        ..RunOptions::default()
    };
    let lint = poly_core::lint_run(&[file.to_path_buf()], &config, &opts, false, false).expect("lint");
    assert_eq!(lint.checked, 1, "the lint run must check the file: {:?}", lint.skipped);
    let fmt = poly_core::format_run(&[file.to_path_buf()], &config, &opts, false, false).expect("fmt");
    assert_eq!(fmt.checked, 1, "the fmt run must check the file: {:?}", fmt.skipped);
    files_under(root)
}

#[test]
fn a_disabled_cache_is_never_written_by_a_lint_or_fmt_run() {
    let home = tempfile::tempdir().expect("tempdir");
    // SAFETY: the only test in this binary, so nothing reads the environment concurrently.
    unsafe {
        std::env::set_var("POLY_CACHE_HOME", home.path());
    }
    let project = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir(project.path().join(".git")).expect("checkout");
    let file = project.path().join("main.py");
    std::fs::write(&file, "x = 1\n").expect("write");
    let configured = tempfile::tempdir().expect("tempdir");

    let enabled_root = configured.path().join("enabled");
    let stored = run_with_cache(&file, &enabled_root, true);
    for namespace in ["lint", "fmt"] {
        assert!(
            stored.iter().any(|path| holds_results(path, namespace)),
            "control: an enabled cache stores {namespace} results: {stored:?}"
        );
    }

    let disabled_root = configured.path().join("disabled");
    let stored = run_with_cache(&file, &disabled_root, false);
    assert_eq!(
        stored,
        Vec::<PathBuf>::new(),
        "a run with the cache disabled must store no results"
    );
    assert!(!disabled_root.exists(), "nor create the cache directory");
    assert_eq!(
        files_under(home.path()),
        Vec::<PathBuf>::new(),
        "nor fall back to the default location"
    );
}
