//! Running `poly lint`'s whole-project phase: beside the per-file tier when the
//! run is read-only, after it otherwise.

use poly_config::PolyConfig;
use poly_workspace::{PlannedWorkspace, WorkspaceLintOptions, WorkspaceLintOutcome};

use crate::progress;

/// A resolved phase plan, or the error that resolving it raised.
pub(crate) type Plan = anyhow::Result<Option<PlannedWorkspace>>;

/// Run `per_file` while, when `overlap` names a config, the whole-project phase
/// runs on a scoped thread; join before returning.
///
/// The phase is made of child processes that share nothing with the per-file
/// tier, so the two overlap. Its live progress bars are off while they do, since
/// they would overdraw the per-file spinner. The plan is taken from `plan` only
/// when the phase actually starts here; a caller left holding it runs the phase
/// itself, through [`run`].
pub(crate) fn overlapped<R>(
    overlap: Option<&PolyConfig>,
    plan: &mut Option<Plan>,
    options: &WorkspaceLintOptions,
    animate: bool,
    per_file: impl FnOnce() -> R,
) -> (R, Option<anyhow::Result<WorkspaceLintOutcome>>) {
    std::thread::scope(|scope| {
        let handle = overlap.and_then(|config| {
            plan.take().map(|plan| {
                scope.spawn(move || {
                    plan.and_then(|planned| poly_workspace::run_planned_workspace_lint(config, planned, options, false))
                })
            })
        });
        let result = per_file();
        let outcome = handle.map(|handle| {
            let spinner = progress::Progress::start("running whole-project checks", animate);
            let joined = handle.join();
            spinner.finish();
            joined.unwrap_or_else(|panic| std::panic::resume_unwind(panic))
        });
        (result, outcome)
    })
}

/// Run `poly lint`'s whole-project phase and render its report.
///
/// `overlapped` is the outcome of a phase [`overlapped`] already ran; otherwise
/// the phase runs here from `plan`. Returns whether the phase passed (the caller
/// folds a `false` into a non-zero exit code). The `--workspace never`
/// short-circuit is handled by the caller, before this runs.
pub(crate) fn run(
    config: &PolyConfig,
    plan: Option<Plan>,
    overlapped: Option<anyhow::Result<WorkspaceLintOutcome>>,
    options: &WorkspaceLintOptions,
    animate: bool,
) -> anyhow::Result<bool> {
    let outcome = match overlapped {
        Some(outcome) => outcome?,
        None => {
            // `cargo clippy` over a cold target directory is the longest silence poly
            // ever produces, and the phase renders only once every tool has finished.
            let spinner = progress::Progress::start("running whole-project checks", animate);
            let outcome = match plan {
                Some(plan) => plan.and_then(|planned| {
                    poly_workspace::run_planned_workspace_lint(
                        config,
                        planned,
                        options,
                        poly_workspace::show_progress(),
                    )
                }),
                None => poly_workspace::run_workspace_lint(config, options),
            };
            spinner.finish();
            outcome?
        }
    };
    poly_workspace::render_workspace_outcome(&outcome, options.report_to_stdout);
    Ok(outcome.passed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overlapped_returns_the_per_file_result_and_no_outcome_without_a_config() {
        let options = WorkspaceLintOptions::default();
        let mut plan = Some(Ok(None));
        let (value, outcome) = overlapped(None, &mut plan, &options, false, || 7);
        assert_eq!(value, 7);
        assert!(outcome.is_none());
        assert!(plan.is_some(), "an unstarted phase leaves its plan with the caller");
    }

    #[test]
    fn overlapped_runs_the_phase_and_takes_the_plan() {
        let config = PolyConfig::default();
        let options = WorkspaceLintOptions::default();
        let mut plan = Some(Ok(None));
        let (value, outcome) = overlapped(Some(&config), &mut plan, &options, false, || "per-file");
        assert_eq!(value, "per-file");
        assert!(plan.is_none());
        assert!(outcome.expect("phase ran").expect("empty plan passes").passed);
    }

    #[test]
    fn a_plan_error_surfaces_through_the_join() {
        let config = PolyConfig::default();
        let options = WorkspaceLintOptions::default();
        let mut plan = Some(Err(anyhow::anyhow!("lowering failed")));
        let (_, outcome) = overlapped(Some(&config), &mut plan, &options, false, || ());
        let error = outcome.expect("phase ran").expect_err("plan error");
        assert_eq!(error.to_string(), "lowering failed");
    }
}
