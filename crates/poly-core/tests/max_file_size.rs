//! `[discovery] max_file_size`: a file above the limit is never opened, and is reported as skipped
//! so `--deny-skips` still sees it.

use std::fs;

use poly_core::{Config, RunOptions};
use tempfile::tempdir;

fn options() -> RunOptions {
    RunOptions {
        no_cache: true,
        jobs: Some(1),
        ..RunOptions::default()
    }
}

fn limited(max_file_size: u64) -> Config {
    Config {
        max_file_size,
        ..Config::default()
    }
}

#[test]
fn an_oversized_file_is_skipped_by_lint_and_never_counted_as_checked() {
    let repo = tempdir().unwrap();
    let big = repo.path().join("big.py");
    let small = repo.path().join("small.py");
    fs::write(&big, "x = 1\n".repeat(20)).unwrap();
    fs::write(&small, "x = 1\n").unwrap();

    let run = poly_core::lint_run(&[big.clone(), small.clone()], &limited(64), &options(), false, false).unwrap();

    let skip = run
        .skipped
        .iter()
        .find(|skip| skip.path == big)
        .expect("big.py is skipped");
    assert_eq!(skip.reason, "file too large (120 bytes > 64 bytes)");
    assert!(
        run.skipped
            .iter()
            .all(|skip| skip.path != small || !skip.reason.starts_with("file too large"))
    );
}

#[test]
fn zero_means_no_limit() {
    let repo = tempdir().unwrap();
    let big = repo.path().join("big.py");
    fs::write(&big, "x = 1\n".repeat(20)).unwrap();

    let run = poly_core::lint_run(std::slice::from_ref(&big), &limited(0), &options(), false, false).unwrap();

    assert!(
        run.skipped
            .iter()
            .all(|skip| !skip.reason.starts_with("file too large"))
    );
}

#[test]
fn an_oversized_file_is_skipped_by_format_and_left_untouched() {
    let repo = tempdir().unwrap();
    let big = repo.path().join("big.py");
    let content = "x  =  1\n".repeat(20);
    fs::write(&big, &content).unwrap();

    let run = poly_core::format_run(std::slice::from_ref(&big), &limited(64), &options(), true, false).unwrap();

    assert!(run.results.iter().any(|result| {
        result.path == big
            && result
                .skipped
                .as_deref()
                .is_some_and(|r| r.starts_with("file too large"))
    }));
    assert_eq!(fs::read_to_string(&big).unwrap(), content);
}
