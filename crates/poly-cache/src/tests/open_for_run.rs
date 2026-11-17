//! [`ResultCache::open_for_run`]: the configured directory, its precedence over `POLY_CACHE_HOME`,
//! and that healing it never touches files poly did not write.

use tempfile::TempDir;

use super::*;

/// A checkout (a directory with a `.git` marker) inside `tmp`.
fn checkout_in(tmp: &TempDir) -> PathBuf {
    let repo = tmp.path().join("repo");
    std::fs::create_dir_all(repo.join(".git")).unwrap();
    repo
}

#[test]
fn open_for_run_opens_a_configured_dir_in_place() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = checkout_in(&tmp);
    let configured = tmp.path().join("configured");

    let cache = ResultCache::open_for_run(Some(&configured), &repo, true).expect("open");

    assert_eq!(cache.root(), configured);
    assert!(cache.enabled());
    assert_eq!(
        std::fs::read_to_string(configured.join("VERSION")).unwrap(),
        CACHE_FORMAT_VERSION
    );
}

#[test]
fn open_for_run_rejects_a_relative_configured_dir() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = checkout_in(&tmp);
    let relative = Path::new("poly-open-for-run-relative-cache");

    let error = ResultCache::open_for_run(Some(relative), &repo, true).expect_err("a relative dir is rejected");

    assert!(error.to_string().contains("absolute"), "{error:#}");
    assert!(
        !relative.exists(),
        "nothing may be created against the working directory"
    );
}

#[test]
fn a_disabled_open_for_run_creates_nothing_at_the_configured_dir() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = checkout_in(&tmp);
    let configured = tmp.path().join("configured");

    let cache = ResultCache::open_for_run(Some(&configured), &repo, false).expect("open");

    assert!(!cache.enabled());
    assert_eq!(cache.root(), configured);
    assert!(
        !configured.exists(),
        "a disabled cache must not materialize its directory"
    );
}

#[test]
fn healing_a_configured_dir_removes_only_polys_own_entries() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = checkout_in(&tmp);
    let configured = tmp.path().join("configured");
    std::fs::create_dir_all(configured.join("results/lint")).unwrap();
    std::fs::create_dir_all(configured.join("index")).unwrap();
    std::fs::create_dir_all(configured.join("notes")).unwrap();
    std::fs::write(configured.join("results/lint/stale-entry"), b"cached").unwrap();
    std::fs::write(configured.join("index/lint.idx"), b"PSI5").unwrap();
    std::fs::write(configured.join("VERSION"), "5").unwrap();
    std::fs::write(configured.join("README.md"), b"mine").unwrap();
    std::fs::write(configured.join("notes/todo.txt"), b"mine").unwrap();
    std::fs::write(configured.join("results/README.md"), b"mine").unwrap();

    ResultCache::open_for_run(Some(&configured), &repo, true).expect("open");

    assert!(!configured.join("results/lint/stale-entry").exists());
    assert!(!configured.join("index/lint.idx").exists());
    for foreign in ["README.md", "notes/todo.txt", "results/README.md"] {
        assert_eq!(
            std::fs::read(configured.join(foreign)).unwrap(),
            b"mine",
            "{foreign} is not poly's and must survive"
        );
    }
}

#[test]
fn a_configured_dir_with_a_foreign_version_file_is_never_cleaned() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = checkout_in(&tmp);
    let configured = tmp.path().join("configured");
    std::fs::create_dir_all(configured.join("results/lint")).unwrap();
    std::fs::write(configured.join("VERSION"), "1.4.2\n").unwrap();
    std::fs::write(configured.join("results/lint/file"), b"mine").unwrap();

    let error = ResultCache::open_for_run(Some(&configured), &repo, true).expect_err("refused");
    assert!(error.to_string().contains("VERSION"), "{error:#}");
    let clean = ResultCache::open(configured.clone(), true).unwrap().clean();
    assert!(clean.is_err(), "`poly cache clean` refuses it too");

    assert_eq!(std::fs::read_to_string(configured.join("VERSION")).unwrap(), "1.4.2\n");
    assert_eq!(std::fs::read(configured.join("results/lint/file")).unwrap(), b"mine");
}

/// `POLY_CACHE_HOME` relocates only the default cache: a configured directory is used as named.
///
/// The environment is process-global and this crate forbids the `unsafe` `set_var`, so the test
/// re-runs itself in a child process with the variable set; the child leaves a marker so a filter
/// that matched no test cannot pass for it.
#[test]
fn a_configured_dir_takes_precedence_over_poly_cache_home() {
    const MARKER_ENV: &str = "POLY_CACHE_TEST_PRECEDENCE_MARKER";
    let Some(marker) = std::env::var_os(MARKER_ENV) else {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        std::fs::create_dir(&home).unwrap();
        let marker = tmp.path().join("ran");
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "tests::open_for_run::a_configured_dir_takes_precedence_over_poly_cache_home",
                "--test-threads=1",
            ])
            .env(CACHE_HOME_ENV, &home)
            .env(MARKER_ENV, &marker)
            .status()
            .expect("run the child test");
        assert!(status.success(), "the child test failed");
        assert!(marker.is_file(), "the child test did not run");
        return;
    };
    let home = PathBuf::from(std::env::var_os(CACHE_HOME_ENV).expect("the parent sets the cache home"));
    let tmp = tempfile::tempdir().unwrap();
    let repo = checkout_in(&tmp);
    let configured = tmp.path().join("configured");

    let cache = ResultCache::open_for_run(Some(&configured), &repo, true).expect("open configured");
    assert_eq!(cache.root(), configured);
    assert!(
        std::fs::read_dir(&home).unwrap().next().is_none(),
        "a configured dir writes nothing under POLY_CACHE_HOME"
    );

    let default = ResultCache::open_for_run(None, &repo, true).expect("open default");
    assert!(default.root().starts_with(&home), "{}", default.root().display());
    std::fs::write(marker, b"ran").unwrap();
}
