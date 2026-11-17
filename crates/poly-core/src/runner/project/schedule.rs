//! Scheduling of the project phase: each plan's inputs are loaded, digested, looked up in the cache
//! and checked in parallel, then folded into the outcome in plan order so the report does not depend
//! on which engine finished first.

use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};

use poly_cache::ResultCache;
use rayon::prelude::*;
use rustc_hash::{FxHashMap, FxHashSet};

use super::{
    ProjectFinding, ProjectOutcome, ProjectPlan, RunInputs, collect_findings, covering_plan, ignored_options_note,
    inputs, report_for, root_for, scope,
};
use crate::discover::DiscoveredFile;
use crate::engine::{Diagnostic, ProjectEnvironment, ProjectFile};
use crate::runner::types::LintError;

/// Environments resolved so far, one per project root.
type Environments = Mutex<FxHashMap<PathBuf, Arc<ProjectEnvironment>>>;

/// What one plan produced, in the order its serial predecessor would have recorded it.
pub(super) enum Settled {
    /// Nothing to run or to say.
    Skip,
    /// A note about options an enclosing plan ignores.
    Note(Option<ProjectFinding>),
    /// The inputs could not be built, or the check failed.
    Failed(LintError),
    /// The check ran, or was answered from the cache.
    Ran(Box<Ran>),
}

/// A finished check and the inputs it saw.
pub(super) struct Ran {
    dirs: Option<inputs::ResolvedDirs>,
    loaded: Vec<ProjectFile>,
    reported: Vec<(PathBuf, Vec<Diagnostic>)>,
}

/// Settle every plan in parallel and fold the results into `outcome` in plan order.
pub(super) fn run_plans(
    outcome: &mut ProjectOutcome,
    plans: &[ProjectPlan],
    files: &[DiscoveredFile],
    run: &RunInputs<'_>,
    only: Option<&FxHashSet<&'static str>>,
    cache: &ResultCache,
) {
    let schedule = Schedule {
        plans,
        roots: plans.iter().map(|plan| root_for(run.configs, plan.config_id)).collect(),
        files,
        run,
        only,
        run_scope: scope::RunScope::new(run.paths),
        environments: Mutex::new(FxHashMap::default()),
        cache,
    };
    let settled: Vec<Settled> = plans
        .par_iter()
        .enumerate()
        .map(|(at, plan)| schedule.settle(at, plan))
        .collect();
    let run_scope = schedule.run_scope;
    merge(outcome, settled, |outcome, at, ran| {
        let plan = &plans[at];
        let Ran { dirs, loaded, reported } = ran;
        outcome.reads.push((
            plan.engine.name(),
            loaded.iter().map(|file| file.path.clone()).collect(),
        ));
        let narrowed = dirs.is_some().then_some(&run_scope);
        // A whole-tree check saw files the run's excludes drop; it reports only on the ones the run covers.
        let covered = dirs
            .as_ref()
            .is_some_and(|dirs| dirs.whole_tree)
            .then(|| scope::covered(files, &reported));
        collect_findings(
            plan,
            run.configs,
            reported,
            narrowed,
            covered.as_ref(),
            &loaded,
            &mut outcome.findings,
        );
    });
}

/// Fold `settled` into `outcome` strictly by plan index; `fold_ran` records a finished check.
pub(super) fn merge(
    outcome: &mut ProjectOutcome,
    settled: Vec<Settled>,
    mut fold_ran: impl FnMut(&mut ProjectOutcome, usize, Ran),
) {
    for (at, step) in settled.into_iter().enumerate() {
        match step {
            Settled::Skip => {}
            Settled::Note(note) => outcome.findings.extend(note),
            Settled::Failed(error) => outcome.errors.push(error),
            Settled::Ran(ran) => fold_ran(outcome, at, *ran),
        }
    }
}

/// What every plan of one run is settled against.
struct Schedule<'a, 'r> {
    plans: &'a [ProjectPlan],
    /// The project root of each plan, by plan index.
    roots: Vec<PathBuf>,
    files: &'a [DiscoveredFile],
    run: &'a RunInputs<'r>,
    only: Option<&'a FxHashSet<&'static str>>,
    run_scope: scope::RunScope,
    environments: Environments,
    cache: &'a ResultCache,
}

impl Schedule<'_, '_> {
    /// Settle the plan at index `at`.
    fn settle(&self, at: usize, plan: &ProjectPlan) -> Settled {
        let Schedule {
            plans,
            roots,
            files,
            run,
            only,
            run_scope,
            environments,
            cache,
        } = self;
        if only.is_some_and(|only| !only.contains(plan.engine.name())) {
            return Settled::Skip;
        }
        if let Some(outer) = covering_plan(plans, roots, at) {
            return Settled::Note(ignored_options_note(plans, roots, at, outer));
        }
        let served = plan.engine.languages();
        let scoped: Vec<&DiscoveredFile> = files
            .iter()
            .filter(|f| f.config_id == plan.config_id && (served.is_empty() || served.contains(&f.language)))
            .collect();
        if scoped.is_empty() && !plan.ancestor {
            return Settled::Skip;
        }
        let root = roots[at].clone();
        let failed = |message: String| {
            Settled::Failed(LintError {
                path: root.clone(),
                message: format!("project check '{}': {message}", plan.engine.name()),
            })
        };
        let dirs = match inputs::plan_dirs(plan) {
            Ok(dirs) => dirs,
            Err(message) => return failed(message),
        };
        if let Some(dirs) = &dirs
            && !dirs.absolute(&root).any(|dir| run_scope.reaches(&dir))
        {
            return Settled::Skip;
        }
        let environment = Arc::clone(
            environments
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .entry(root.clone())
                .or_insert_with(|| Arc::new(crate::project::env::resolve_environment(&root))),
        );
        let loaded = match inputs::load(plan, &root, dirs.as_ref(), &scoped, run) {
            Ok(loaded) => loaded,
            Err(message) => return failed(message),
        };
        if !inputs::has_substance(&loaded) {
            return Settled::Skip;
        }
        // A check over a tree of its own reports only what the run named, so it is asked for no more.
        let targets = dirs
            .as_ref()
            .and_then(|dirs| run_scope.targets(dirs.absolute(&root), &loaded));
        match report_for(plan, &root, &loaded, targets.as_deref(), &environment, cache) {
            Ok(reported) => Settled::Ran(Box::new(Ran { dirs, loaded, reported })),
            Err(error) => Settled::Failed(error),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn failure(message: &str) -> Settled {
        Settled::Failed(LintError {
            path: PathBuf::from("."),
            message: message.to_owned(),
        })
    }

    #[test]
    fn merge_records_errors_in_plan_order_whatever_finished_first() {
        let mut outcome = ProjectOutcome::default();
        let settled = vec![failure("first"), Settled::Skip, failure("third"), Settled::Note(None)];
        merge(&mut outcome, settled, |_, _, _| unreachable!("nothing ran"));
        let messages: Vec<&str> = outcome.errors.iter().map(|e| e.message.as_str()).collect();
        assert_eq!(messages, ["first", "third"]);
        assert!(outcome.findings.is_empty());
    }

    #[test]
    fn merge_hands_each_finished_check_its_plan_index() {
        let ran = |name: &str| {
            Settled::Ran(Box::new(Ran {
                dirs: None,
                loaded: Vec::new(),
                reported: vec![(PathBuf::from(name), Vec::new())],
            }))
        };
        let mut outcome = ProjectOutcome::default();
        let mut seen = Vec::new();
        merge(
            &mut outcome,
            vec![Settled::Skip, ran("b"), failure("x"), ran("d")],
            |_, at, ran| seen.push((at, ran.reported[0].0.clone())),
        );
        assert_eq!(seen, [(1, PathBuf::from("b")), (3, PathBuf::from("d"))]);
        assert_eq!(outcome.errors.len(), 1);
    }
}
