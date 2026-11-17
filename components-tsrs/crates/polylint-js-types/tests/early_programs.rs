//! Programs that start building before the query is known give the facts of an analysis of the judged files alone,
//! whatever files the early build was given: an early program no judged file needs is freed unwalked, and a program
//! only the judged files need is built once they are handed over.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::num::NonZeroUsize;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;

use common::{Generated, Options, abs, generated_programs, try_run};
use polylint_js_types::{TypeBudget, TypeBuild, TypeError, TypeFacts, TypePackage, TypeTimings, start};

/// Programs of 3, 5 and 2 classes, below `packages/p0` to `packages/p2`.
const PROGRAM_SIZES: [u32; 3] = [3, 5, 2];

const THREADS: usize = 8;

fn threads() -> NonZeroUsize {
    NonZeroUsize::new(THREADS).unwrap()
}

/// The facts of `judged`, the packages of `generated` that `judged` names, after an early build of `early`.
fn after_early_build(generated: &Generated, early: Vec<TypePackage>, judged: &[TypePackage]) -> TypeFacts {
    let build = TypeBuild::new(generated.workspace.fs(), abs(""), early).with_threads(threads());
    std::thread::scope(|scope| start(scope, build, None).finish(judged, &generated.query)).expect("the analysis fails")
}

fn analyzed(generated: &Generated, judged: &[TypePackage]) -> TypeFacts {
    try_run(
        &generated.workspace,
        judged,
        &generated.query,
        Options {
            threads: Some(threads()),
            ..Options::default()
        },
    )
    .expect("the analysis fails")
}

fn without_timings(mut facts: TypeFacts) -> TypeFacts {
    facts.timings = TypeTimings::default();
    facts
}

fn configs(facts: &TypeFacts) -> Vec<PathBuf> {
    facts
        .timings
        .per_program
        .iter()
        .map(|program| program.config.clone())
        .collect()
}

fn config(index: usize) -> PathBuf {
    abs(&format!("packages/p{index}/tsconfig.json"))
}

/// `package` with only its first `count` files.
fn first_files(package: &TypePackage, count: usize) -> TypePackage {
    TypePackage {
        files: package.files.iter().take(count).cloned().collect(),
        ..package.clone()
    }
}

#[test]
fn early_files_other_than_the_judged_ones_give_the_facts_of_the_judged_files() {
    let generated = generated_programs(&PROGRAM_SIZES);
    let all = &generated.packages;
    // `p2` is judged in neither case but planned early; `p1` is judged but in the second case not planned early.
    let judged = [all[0].clone(), all[1].clone()];
    let cases = [
        ("the judged files", judged.to_vec()),
        ("every file", all.clone()),
        (
            "an unjudged program and part of a judged one",
            vec![first_files(&all[0], 1), all[2].clone()],
        ),
        ("no file", Vec::new()),
    ];
    let expected = analyzed(&generated, &judged);
    assert_eq!(configs(&expected), [config(0), config(1)]);
    assert_eq!(
        expected.covered.len(),
        16,
        "the 6 and 10 files of the judged programs are covered"
    );
    let expected = without_timings(expected);
    for (case, early) in cases {
        let facts = after_early_build(&generated, early, &judged);
        assert_eq!(
            configs(&facts),
            [config(0), config(1)],
            "{case}: only judged programs are walked"
        );
        assert_eq!(without_timings(facts), expected, "{case}");
    }
}

#[test]
fn every_judged_program_is_walked_once_with_the_reads_of_its_files() {
    let generated = generated_programs(&PROGRAM_SIZES);
    let facts = after_early_build(&generated, generated.packages.clone(), &generated.packages);
    assert_eq!(facts.reads, generated.reads);
    assert_eq!(configs(&facts), [config(0), config(1), config(2)]);
}

#[test]
fn a_build_given_up_before_the_query_ends_and_frees_its_programs() {
    let generated = generated_programs(&PROGRAM_SIZES);
    let build = TypeBuild::new(generated.workspace.fs(), abs(""), generated.packages.clone()).with_threads(threads());
    // The scope ends only once every program thread has: a program waiting for a query that never comes would hang.
    std::thread::scope(|scope| drop(start(scope, build, None)));
}

#[test]
fn a_build_cancelled_before_the_query_is_cancelled() {
    let generated = generated_programs(&PROGRAM_SIZES);
    let cancel = AtomicBool::new(true);
    let build = TypeBuild::new(generated.workspace.fs(), abs(""), generated.packages.clone()).with_threads(threads());
    let result =
        std::thread::scope(|scope| start(scope, build, Some(&cancel)).finish(&generated.packages, &generated.query));
    assert!(matches!(result, Err(TypeError::Cancelled)), "{result:?}");
}

#[test]
fn a_given_tsconfig_that_cannot_be_read_fails_the_finish() {
    let generated = generated_programs(&PROGRAM_SIZES);
    let build = TypeBuild::new(generated.workspace.fs(), abs(""), generated.packages.clone())
        .with_tsconfig_override(PathBuf::from("missing.json"));
    let result = std::thread::scope(|scope| start(scope, build, None).finish(&generated.packages, &generated.query));
    assert!(
        matches!(&result, Err(TypeError::Config { path, .. }) if path == &PathBuf::from("missing.json")),
        "{result:?}"
    );
}

#[test]
fn analyses_that_fill_every_thread_of_the_global_pool_finish() {
    use rayon::prelude::*;
    // More source directories than tsrs lists inline, so that planning lists the rest on its rayon pool.
    let mut files = vec![("tsconfig.json".to_owned(), common::TSCONFIG.to_owned())];
    let mut judged = Vec::new();
    for index in 0..100 {
        let rel = format!("src/d{index}/m.ts");
        files.push((rel.clone(), format!("export const value{index} = {index};\n")));
        judged.push(rel);
    }
    let judged: Vec<&str> = judged.iter().map(String::as_str).collect();
    let workspace = common::Workspace::from_owned(files);
    let packages = vec![common::package("", "app", &judged)];
    let query = common::query(Vec::new());
    // Twice as many analyses as the global pool has threads, one per task: every thread of the pool runs one and waits
    // for its plan, as a host does with one analysis per root.
    let analyses = rayon::current_num_threads() * 2;
    let (done, finished) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let facts: Vec<TypeFacts> = (0..analyses)
            .into_par_iter()
            .with_max_len(1)
            .map(|_| {
                let build = TypeBuild::new(workspace.fs(), abs(""), packages.clone()).with_threads(threads());
                std::thread::scope(|scope| start(scope, build, None).finish(&packages, &query))
                    .expect("the analysis fails")
            })
            .collect();
        done.send(facts.len()).unwrap();
    });
    // A deadlocked pool never ends; the thread is left behind and the test fails.
    assert_eq!(
        finished.recv_timeout(std::time::Duration::from_secs(120)),
        Ok(analyses),
        "the analyses wait for each other's threads"
    );
}

/// The facts of `analyses` analyses of every program of `generated`, run at once on the global pool and sharing
/// `budget`, as a host runs one per root. Between starting its programs and handing its query over, each host does
/// parallel work that lets its thread run another analysis meanwhile, as building its module graph does.
fn sharing(generated: &Generated, budget: &TypeBudget, analyses: usize) -> Vec<TypeFacts> {
    use rayon::prelude::*;
    let (done, finished) = std::sync::mpsc::channel();
    std::thread::scope(|outer| {
        outer.spawn(|| {
            let facts: Vec<TypeFacts> = (0..analyses)
                .into_par_iter()
                .with_max_len(1)
                .map(|_| {
                    let build = TypeBuild::new(generated.workspace.fs(), abs(""), generated.packages.clone())
                        .with_threads(threads())
                        .with_budget(budget.clone());
                    std::thread::scope(|scope| {
                        let pending = start(scope, build, None);
                        rayon::join(|| (), || std::thread::sleep(std::time::Duration::from_millis(5)));
                        pending.finish(&generated.packages, &generated.query)
                    })
                    .expect("the analysis fails")
                })
                .collect();
            done.send(facts).unwrap();
        });
        finished
            .recv_timeout(std::time::Duration::from_secs(300))
            .expect("the analyses wait for each other's cores")
    })
}

#[test]
fn analyses_that_share_a_budget_never_hold_more_cores_than_it_has_and_keep_their_facts() {
    let generated = generated_programs(&PROGRAM_SIZES);
    let expected = without_timings(analyzed(&generated, &generated.packages));
    let analyses = rayon::current_num_threads() * 2;
    // One core leaves no early share, so no program starts before its query; four leave one core for early programs.
    for cores in [1, 4] {
        let budget = TypeBudget::new(NonZeroUsize::new(cores));
        let facts = sharing(&generated, &budget, analyses);
        assert_eq!(facts.len(), analyses, "{cores} cores");
        assert_eq!(
            budget.takes(),
            analyses * PROGRAM_SIZES.len(),
            "{cores} cores: every program of every analysis takes its cores from the budget"
        );
        assert!(
            (1..=cores).contains(&budget.peak()),
            "{cores} cores: at most the budget is held at once, not {}",
            budget.peak()
        );
        if cores == 1 {
            assert!(
                facts.iter().all(|facts| facts.timings.early == 0),
                "without an early share every program waits for its query"
            );
        }
        for facts in facts {
            assert_eq!(without_timings(facts), expected, "{cores} cores");
        }
    }
}
