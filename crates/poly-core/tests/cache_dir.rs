//! `[cache] dir` pins where a lint or fmt run keeps its result cache, as it does for `poly cache` and
//! the hook runner, and a relative one names the same directory whichever directory poly starts in.
//! One test per process: `POLY_CACHE_HOME` is process-global.
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

/// The files a cache at `root` stores results in: everything but its bookkeeping files.
fn stored_results(root: &Path) -> Vec<PathBuf> {
    files_under(root)
        .into_iter()
        .filter(|path| {
            !matches!(
                path.file_name().and_then(|name| name.to_str()),
                Some("VERSION" | "LAST_SWEEP")
            )
        })
        .collect()
}

fn options() -> RunOptions {
    RunOptions {
        explicit_config: true,
        ..RunOptions::default()
    }
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
    let root = configured.path().join("poly-cache");
    let config = Config {
        cache_dir: Some(root.clone()),
        ..Config::default()
    };

    let lint = poly_core::lint_run(std::slice::from_ref(&file), &config, &options(), false, false).expect("lint");
    assert_eq!(lint.checked, 1, "the lint run must check the file: {:?}", lint.skipped);
    let after_lint = stored_results(&root);
    assert!(
        !after_lint.is_empty(),
        "the lint run stores its results in the configured cache"
    );
    let fmt = poly_core::format_run(std::slice::from_ref(&file), &config, &options(), false, false).expect("fmt");
    assert_eq!(fmt.checked, 1, "the fmt run must check the file: {:?}", fmt.skipped);
    assert!(
        stored_results(&root).len() > after_lint.len(),
        "the fmt run stores its results in the configured cache too"
    );
    assert_eq!(
        files_under(home.path()),
        Vec::<PathBuf>::new(),
        "nothing may be written to the default location"
    );

    relative_dir_is_the_same_from_the_root_and_a_subdirectory();
}

/// A relative `[cache] dir` is anchored at the `poly.toml` that declares it, so a run started in a
/// subdirectory uses the cache a run from the root filled rather than a second, cold one.
fn relative_dir_is_the_same_from_the_root_and_a_subdirectory() {
    let project = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir(project.path().join(".git")).expect("checkout");
    std::fs::write(project.path().join("poly.toml"), "[cache]\ndir = \".poly-cache\"\n").expect("config");
    let sub = project.path().join("pkg");
    std::fs::create_dir(&sub).expect("subdirectory");
    let file = sub.join("main.py");
    std::fs::write(&file, "x = 1\n").expect("write");
    let expected = project.path().join(".poly-cache");

    for start in [project.path(), sub.as_path()] {
        let config = Config::from(poly_config::PolyConfig::load(start).expect("load config"));
        assert_eq!(
            config.cache_dir.as_deref(),
            Some(expected.as_path()),
            "from {}",
            start.display()
        );
        let lint = poly_core::lint_run(std::slice::from_ref(&file), &config, &options(), false, false).expect("lint");
        assert_eq!(lint.checked, 1, "{:?}", lint.skipped);
    }
    assert!(
        !stored_results(&expected).is_empty(),
        "both runs share the declared cache"
    );
    assert!(
        !sub.join(".poly-cache").exists(),
        "no second cache next to the subdirectory"
    );
}
