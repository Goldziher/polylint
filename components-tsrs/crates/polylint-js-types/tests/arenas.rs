//! Repeated analyses free the memory of their programs (skif ADR 0010). The arena counter is process-wide, so this file
//! holds the only test of its binary: no other analysis runs beside it.
//!
//! The analysis runs on one thread, as a long-lived host would run it per request: the first run leaves the calling
//! thread's own arena behind, which later runs reuse, and nothing else may stay. With several checker threads, the
//! tsrs worker threads keep arenas of their own, which is why long-lived hosts run without types (skif ADR 0010).

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::num::NonZeroUsize;

use common::{Options, generated, try_run};
use polylint_js_types::{arena_bytes_in_use, arena_peak_bytes};

const RUNS: usize = 50;

#[test]
fn repeated_analyses_free_their_arenas() {
    if arena_peak_bytes().is_none() {
        eprintln!(
            "skipped: `plain-ptrs` counts only the calling thread's arena; run `cargo test -p polylint-js-types`"
        );
        return;
    }
    let generated = generated(4);
    let options = || Options {
        threads: NonZeroUsize::new(1),
        ..Options::default()
    };
    let mut after_first = None;
    for run_index in 0..RUNS {
        let facts = try_run(&generated.workspace, &generated.packages, &generated.query, options()).unwrap();
        assert_eq!(facts.reads, generated.reads, "reads of run {run_index}");
        let in_use = arena_bytes_in_use();
        let first = *after_first.get_or_insert(in_use);
        assert_eq!(in_use, first, "arena bytes in use after run {run_index}");
    }
}
