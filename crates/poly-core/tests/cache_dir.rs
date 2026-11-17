//! `[cache] dir` pins where a lint or fmt run keeps its result cache, as it does for `poly cache` and
//! the hook runner. One test per process: `POLY_CACHE_HOME` is process-global.

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

#[test]
fn a_lint_and_a_fmt_run_use_the_configured_cache_dir() {
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
    let config = Config {
        cache_dir: Some(configured.path().join("poly-cache")),
        ..Config::default()
    };
    let opts = RunOptions {
        explicit_config: true,
        ..RunOptions::default()
    };

    poly_core::lint_run(std::slice::from_ref(&file), &config, &opts, false, false).expect("lint");
    poly_core::format_run(std::slice::from_ref(&file), &config, &opts, false, false).expect("fmt");

    let root = configured.path().join("poly-cache");
    assert!(root.join("VERSION").is_file(), "the run must open the configured cache");
    assert!(root.join("results/lint").is_dir() && root.join("results/fmt").is_dir());
    assert_eq!(
        files_under(home.path()),
        Vec::<PathBuf>::new(),
        "nothing may be written to the default location"
    );
}
