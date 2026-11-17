//! `[cache] enabled = false` turns a lint or fmt run's result cache off, as `--no-cache` does. One test per process:
//! `POLY_CACHE_HOME` is process-global.

use poly_core::{Config, RunOptions};

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
    let root = configured.path().join("poly-cache");
    let config = Config {
        cache_dir: Some(root.clone()),
        cache_enabled: false,
        ..Config::default()
    };
    let opts = RunOptions {
        explicit_config: true,
        ..RunOptions::default()
    };

    poly_core::lint_run(std::slice::from_ref(&file), &config, &opts, false, false).expect("lint");
    poly_core::format_run(std::slice::from_ref(&file), &config, &opts, false, false).expect("fmt");

    assert!(
        !root.join("results/lint").exists() && !root.join("results/fmt").exists(),
        "a run with the cache disabled must store no results"
    );
    assert!(
        std::fs::read_dir(home.path()).expect("home").next().is_none(),
        "nor fall back to the default location"
    );
}
