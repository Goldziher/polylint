//! Freed project file regions are retired only once the process opts in (`retire_freed_sources`). The opt-in and the
//! arena counter are process-wide, so this file holds the only test of its binary.
//!
//! A retired region's range is never handed out again, so the arena bytes in use grow with the runs that retire; a
//! run that frees its regions instead leaves them as the first run left them (`tests/arenas.rs`).

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::num::NonZeroUsize;

use common::{Options, generated, try_run};
use polylint_js_types::{arena_bytes_in_use, arena_peak_bytes, retire_freed_sources};

const RUNS: usize = 3;

#[test]
fn freed_project_files_are_retired_only_once_the_process_opts_in() {
    if arena_peak_bytes().is_none() {
        eprintln!(
            "skipped: `plain-ptrs` counts only the calling thread's arena; run `cargo test -p polylint-js-types`"
        );
        return;
    }
    let generated = generated(4);
    let run = || {
        let options = Options {
            threads: NonZeroUsize::new(1),
            ..Options::default()
        };
        let facts = try_run(&generated.workspace, &generated.packages, &generated.query, options).unwrap();
        assert_eq!(facts.reads, generated.reads);
        arena_bytes_in_use()
    };
    let first = run();
    for run_index in 1..RUNS {
        assert_eq!(run(), first, "run {run_index} without the opt-in retires nothing");
    }
    retire_freed_sources();
    // A retired region's slab is retired once its last chunk goes, after the thread moved on to another slab: the
    // ranges given up show from the next run on.
    for _ in 1..RUNS {
        run();
    }
    let retired = run();
    assert!(
        retired > first,
        "runs with the opt-in retire the regions they free: {retired} bytes in use after them, {first} before"
    );
}
