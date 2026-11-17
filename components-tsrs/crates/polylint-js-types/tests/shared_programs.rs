//! The analyses of a run over several roots share their programs ([`TypePrograms`]): a program the judged files of
//! several roots go to, an outer root's `tsconfig.json` that includes the files of the roots nested in it, is built
//! once for the run, kept while a root that holds one of its files has not finished, and gives each root the facts a
//! build of its own gives.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::num::NonZeroUsize;
use std::path::PathBuf;

use common::{RootAnalysis, Workspace, abs, roots_under_one_tsconfig};
use polylint_js_types::{TypeBudget, TypeBuild, TypeFacts, TypePrograms, TypeTimings, start};

const THREADS: usize = 8;

/// An outer root with 3 classes and two roots nested in it with 2 each, all under the outer root's one config.
const NESTED: [(&str, u32); 3] = [("", 3), ("roots/a", 2), ("roots/b", 2)];

/// Roots side by side below a config none of them is the root of.
const SIDE_BY_SIDE: [(&str, u32); 6] = [
    ("roots/r0", 2),
    ("roots/r1", 3),
    ("roots/r2", 1),
    ("roots/r3", 2),
    ("roots/r4", 4),
    ("roots/r5", 2),
];

fn threads() -> NonZeroUsize {
    NonZeroUsize::new(THREADS).unwrap()
}

fn build_of(workspace: &Workspace, analysis: &RootAnalysis) -> TypeBuild {
    TypeBuild::new(workspace.fs(), analysis.root.clone(), analysis.packages.clone()).with_threads(threads())
}

fn finished(build: TypeBuild, analysis: &RootAnalysis) -> TypeFacts {
    std::thread::scope(|scope| start(scope, build, None).finish(&analysis.packages, &analysis.query))
        .expect("the analysis fails")
}

/// The facts of `analysis` on programs it builds for itself alone.
fn alone(workspace: &Workspace, analysis: &RootAnalysis) -> TypeFacts {
    finished(build_of(workspace, analysis), analysis)
}

/// The facts of `analysis` in a run that shares `programs` and `budget`.
fn sharing(workspace: &Workspace, analysis: &RootAnalysis, programs: &TypePrograms, budget: &TypeBudget) -> TypeFacts {
    let build = build_of(workspace, analysis)
        .with_budget(budget.clone())
        .with_programs(programs.clone());
    finished(build, analysis)
}

fn programs_of(analyses: &[RootAnalysis]) -> TypePrograms {
    TypePrograms::new(analyses.iter().map(|analysis| analysis.root.clone()))
}

fn without_timings(mut facts: TypeFacts) -> TypeFacts {
    facts.timings = TypeTimings::default();
    facts
}

/// The configs of the programs `facts` walked, each with whether another analysis built it.
fn walked(facts: &TypeFacts) -> Vec<(PathBuf, bool)> {
    facts
        .timings
        .per_program
        .iter()
        .map(|program| (program.config.clone(), program.shared))
        .collect()
}

/// The facts of every analysis alone, each checked to be the facts of its classes on its own build of the one
/// program.
fn expected(workspace: &Workspace, analyses: &[RootAnalysis]) -> Vec<TypeFacts> {
    analyses
        .iter()
        .map(|analysis| {
            let facts = alone(workspace, analysis);
            let root = analysis.root.display();
            assert_eq!(facts.reads, analysis.reads, "{root}: the reads of its classes");
            assert_eq!(
                facts.covered.len(),
                analysis.packages[0].files.len(),
                "{root}: every judged file is covered"
            );
            assert_eq!(
                walked(&facts),
                [(abs("tsconfig.json"), false)],
                "{root}: alone, it builds the one program itself"
            );
            without_timings(facts)
        })
        .collect()
}

#[test]
fn roots_nested_under_the_outer_roots_config_walk_its_one_build_and_keep_their_facts() {
    let (workspace, analyses) = roots_under_one_tsconfig(&NESTED);
    let expected = expected(&workspace, &analyses);
    let programs = programs_of(&analyses);
    let budget = TypeBudget::new(Some(threads()));
    // The outer root first, then the nested ones, as `skif -r` lints roots by nesting depth.
    let facts: Vec<TypeFacts> = analyses
        .iter()
        .map(|analysis| sharing(&workspace, analysis, &programs, &budget))
        .collect();
    assert_eq!(
        programs.builds(),
        1,
        "the outer root's program is built once for the run"
    );
    assert_eq!(programs.kept(), 0, "no program outlives the roots that hold its files");
    let shared: Vec<Vec<(PathBuf, bool)>> = facts.iter().map(walked).collect();
    assert_eq!(
        shared,
        [
            vec![(abs("tsconfig.json"), false)],
            vec![(abs("tsconfig.json"), true)],
            vec![(abs("tsconfig.json"), true)],
        ],
        "the outer root builds the program and the nested roots walk its build"
    );
    let facts: Vec<TypeFacts> = facts.into_iter().map(without_timings).collect();
    assert_eq!(facts, expected);
}

#[test]
fn roots_analysed_at_once_wait_for_one_build_of_the_program_they_share() {
    use rayon::prelude::*;
    let (workspace, analyses) = roots_under_one_tsconfig(&SIDE_BY_SIDE);
    let expected = expected(&workspace, &analyses);
    let programs = programs_of(&analyses);
    let budget = TypeBudget::new(Some(threads()));
    let (done, finished) = std::sync::mpsc::channel();
    let facts = std::thread::scope(|outer| {
        outer.spawn(|| {
            let facts: Vec<TypeFacts> = analyses
                .par_iter()
                .with_max_len(1)
                .map(|analysis| sharing(&workspace, analysis, &programs, &budget))
                .collect();
            done.send(facts).unwrap();
        });
        // An analysis waiting for a build that never ends would hang the run.
        finished
            .recv_timeout(std::time::Duration::from_secs(300))
            .expect("the analyses wait for each other's builds")
    });
    assert_eq!(
        programs.builds(),
        1,
        "one analysis builds the program, the others wait for it"
    );
    assert_eq!(programs.kept(), 0);
    assert_eq!(
        facts
            .iter()
            .filter(|facts| walked(facts) == [(abs("tsconfig.json"), true)])
            .count(),
        SIDE_BY_SIDE.len() - 1,
        "every analysis but the one that built it walks the shared build"
    );
    let facts: Vec<TypeFacts> = facts.into_iter().map(without_timings).collect();
    assert_eq!(facts, expected);
}

#[test]
fn a_kept_program_is_freed_once_every_root_holding_its_files_is_done() {
    let (workspace, analyses) = roots_under_one_tsconfig(&NESTED);
    let programs = programs_of(&analyses);
    let budget = TypeBudget::new(Some(threads()));
    sharing(&workspace, &analyses[0], &programs, &budget);
    assert_eq!(programs.kept(), 1, "kept for the nested roots, which have not finished");
    // The nested roots are analysed without types: their host marks them done.
    programs.done(&analyses[1].root);
    assert_eq!(programs.kept(), 1, "still kept for the other nested root");
    programs.done(&analyses[2].root);
    assert_eq!(programs.kept(), 0, "no root left that holds one of its files");
    assert_eq!(programs.builds(), 1);
}

#[test]
fn a_program_no_other_root_holds_a_file_of_is_freed_after_its_walk() {
    let (workspace, analyses) = roots_under_one_tsconfig(&NESTED[..1]);
    // A root of the run outside the program's files: nothing keeps the program for it.
    let programs = TypePrograms::new([abs(""), abs("elsewhere")]);
    let budget = TypeBudget::new(Some(threads()));
    let facts = sharing(&workspace, &analyses[0], &programs, &budget);
    assert_eq!(facts.reads, analyses[0].reads);
    assert_eq!(programs.builds(), 1);
    assert_eq!(
        programs.kept(),
        0,
        "the program is freed with the walk of the root that built it"
    );
    sharing(&workspace, &analyses[0], &programs, &budget);
    assert_eq!(
        programs.builds(),
        2,
        "a later analysis that needs the freed program builds it again"
    );
}
