//! Programs built and checked through `polylint-tsrs` give their memory back when their owners drop (polylint ADR
//! 0078). The arena counter is process-wide, so this file holds the only test of its binary: nothing else builds a
//! program beside it.
//!
//! Everything runs on one thread: the first run leaves the calling thread's own arena behind, which later runs
//! reuse, and nothing else may stay.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::sync::Arc;

use common::{build, host, set_strict, workspace};
use polylint_tsrs::{arena_bytes_in_use, arena_peak_bytes};

const RUNS: usize = 6;

#[test]
fn programs_built_and_checked_free_their_arenas() {
    if arena_peak_bytes().is_none() {
        eprintln!("skipped: `plain-ptrs` counts only the calling thread's arena; run `cargo test -p polylint-tsrs`");
        return;
    }
    let memory = workspace();
    let mut after_first = None;
    let mut expected = None;
    for run_index in 0..RUNS {
        // The files and configs of one run, dropped after its program.
        let (files, configs) = (Arc::default(), Arc::default());
        let host = host(&memory, &files, &configs);
        let built = build(&host, |config| set_strict(config, true), None);
        let diagnostics = built.diagnostics(None);
        assert_eq!(diagnostics.len(), 3, "diagnostics of run {run_index}");
        assert_eq!(&diagnostics, expected.get_or_insert_with(|| diagnostics.clone()));
        drop(built);
        drop(host);
        drop((files, configs));
        let in_use = arena_bytes_in_use();
        let first = *after_first.get_or_insert(in_use);
        assert_eq!(in_use, first, "arena bytes in use after run {run_index}");
    }
}
