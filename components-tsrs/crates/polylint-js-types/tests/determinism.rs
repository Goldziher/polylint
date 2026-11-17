//! The facts do not depend on how many threads the checker uses or how the programs are scheduled (skif ADR 0010).

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::num::NonZeroUsize;
use std::path::PathBuf;
use std::time::Duration;

use common::{Generated, Options, abs, generated, generated_programs, try_run};
use polylint_js_types::{ProgramTiming, TypeFacts, TypeTimings};

fn analyze_at(threads: usize) -> TypeFacts {
    without_timings(analyze_with_timings(&generated(24), threads))
}

fn analyze_with_timings(generated: &Generated, threads: usize) -> TypeFacts {
    let facts = try_run(
        &generated.workspace,
        &generated.packages,
        &generated.query,
        Options {
            threads: NonZeroUsize::new(threads),
            ..Options::default()
        },
    )
    .expect("the analysis fails");
    assert_eq!(facts.reads, generated.reads, "reads at {threads} threads");
    facts
}

fn without_timings(mut facts: TypeFacts) -> TypeFacts {
    facts.timings = TypeTimings::default();
    facts
}

/// Programs of 2, 12, 3, 1 and 4 classes: the second holds most of the judged files.
const PROGRAM_SIZES: [u32; 5] = [2, 12, 3, 1, 4];

#[test]
fn type_facts_are_identical_at_one_and_eight_threads() {
    assert_eq!(analyze_at(1), analyze_at(8));
}

#[test]
fn programs_scheduled_by_size_give_the_facts_of_a_sequential_run() {
    let generated = generated_programs(&PROGRAM_SIZES);
    let sequential = without_timings(analyze_with_timings(&generated, 1));
    let scheduled = without_timings(analyze_with_timings(&generated, 16));
    assert_eq!(sequential.covered.len(), 44, "every judged file is covered");
    assert_eq!(sequential, scheduled);
}

#[test]
fn every_program_reports_its_size_checkers_and_times() {
    let generated = generated_programs(&PROGRAM_SIZES);
    let timings = analyze_with_timings(&generated, 16).timings;
    assert_eq!(timings.threads, 16);
    let programs: Vec<(PathBuf, usize, usize)> = timings
        .per_program
        .iter()
        .map(|program| (program.config.clone(), program.judged_files, program.checkers))
        .collect();
    // 44 judged files on 16 threads, four programs at once: the program with 24 of them gets its share of 9
    // checkers capped at 8, the one with 2 a checker for each, and the others the even share of 4.
    let expected: Vec<(PathBuf, usize, usize)> = [(4, 4), (24, 8), (6, 4), (2, 2), (8, 4)]
        .into_iter()
        .enumerate()
        .map(|(index, (judged, checkers))| (abs(&format!("packages/p{index}/tsconfig.json")), judged, checkers))
        .collect();
    assert_eq!(programs, expected);
    assert!(
        timings
            .per_program
            .iter()
            .all(|program| program.files > program.judged_files),
        "a program also holds the default libraries"
    );
    let sum = |time: fn(&ProgramTiming) -> Duration| timings.per_program.iter().map(time).sum::<Duration>();
    assert_eq!(timings.programs, sum(|program| program.build));
    assert_eq!(timings.queries, sum(|program| program.query));

    let sequential = analyze_with_timings(&generated, 1).timings;
    assert!(sequential.per_program.iter().all(|program| program.checkers == 1));
}
