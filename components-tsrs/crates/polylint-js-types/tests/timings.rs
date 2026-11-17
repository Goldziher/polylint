//! The costly timings are collected only once the process opts in (`enable_timings`). The switch is process-wide, so
//! this file holds the only test of its binary.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use common::{generated, run};
use polylint_js_types::enable_timings;

#[test]
fn distinct_project_files_are_counted_only_once_the_process_opts_in() {
    let generated = generated(2);
    let off = run(&generated.workspace, &generated.packages, &generated.query);
    assert_eq!(off.reads, generated.reads);
    assert_eq!(off.timings.project_files_distinct, 0);
    enable_timings();
    let on = run(&generated.workspace, &generated.packages, &generated.query);
    assert_eq!(on.reads, generated.reads);
    // The two class files and the two files that read them.
    assert_eq!(on.timings.project_files_distinct, 4);
}
