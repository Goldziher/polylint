//! The run request both surfaces build a lint or format run from, and the verdict both read
//! back from it.
//!
//! The CLI and the MCP server used to assemble [`RunOptions`] separately, and a flag added to
//! one never reached the other (`force_exclude` was once hard-coded on the MCP side). Both now
//! fill one [`RunRequest`] and call [`run_options`], so a new run option is one field here; the
//! `mcp_flag_coverage` test in `poly-cli` fails until a new CLI flag is mapped or excluded with a
//! reason. [`Verdict`] is the one classifier of a finished run: the CLI maps it to its exit code,
//! the MCP server to `isError`.

use std::path::PathBuf;
use std::sync::Arc;

use poly_config::BaseConfigResolver;
use poly_core::{Config, FormatRun, LintRun, RunOptions, Severity, SkippedFile};
use poly_frontend_api::wire::{OnMissingMode, RunRequest};

use crate::ops::install_policy;

/// The paths a request names, as the run receives them: the current directory when it names
/// none, matching the CLI default.
#[must_use]
pub fn request_paths(paths: &[String]) -> Vec<PathBuf> {
    if paths.is_empty() {
        vec![PathBuf::from(".")]
    } else {
        paths.iter().map(PathBuf::from).collect()
    }
}

/// The [`RunOptions`] a request stands for under `config`.
///
/// `resolver` resolves the `extends` bases of the nested configs the run discovers; the caller
/// passes the one it loaded `config` with. `force_exclude` follows the CLI's precedence:
/// `include_excluded` beats `[discovery] force_exclude`, whose built-in default is on. An
/// explicit `on_missing` beats the one `install: never` implies.
#[must_use]
pub fn run_options(request: &RunRequest, config: &Config, resolver: Option<Arc<dyn BaseConfigResolver>>) -> RunOptions {
    let (install, implied_on_missing) = install_policy(request.install);
    RunOptions {
        no_cache: request.no_cache,
        jobs: request.jobs,
        exclude: request.exclude.clone(),
        force_exclude: !request.include_excluded && config.force_exclude,
        fix_generated: request.fix_generated,
        generated: None,
        explicit_config: request.config.is_some(),
        config_resolver: resolver,
        only: request.only.clone(),
        skip: request.skip.clone(),
        on_missing: request.on_missing.map(on_missing).or(implied_on_missing),
        install,
        ..RunOptions::default()
    }
}

/// The `[components] on_missing` value a request's `on_missing` stands for.
#[must_use]
pub fn on_missing(mode: OnMissingMode) -> poly_config::v2::OnMissing {
    match mode {
        OnMissingMode::Fail => poly_config::v2::OnMissing::Fail,
        OnMissingMode::Skip => poly_config::v2::OnMissing::Skip,
    }
}

/// Refuse a request the CLI's argument parser would have refused.
///
/// # Errors
///
/// When `only` and `skip` are both set: one narrows to a set, the other removes from it, and
/// the CLI makes them mutually exclusive.
pub fn validate(request: &RunRequest) -> anyhow::Result<()> {
    anyhow::ensure!(
        request.only.is_empty() || request.skip.is_empty(),
        "`only` and `skip` cannot be combined"
    );
    Ok(())
}

/// The skipped files that put a run over `budget` (`max_skips`), or nothing when it stays
/// within it or names no budget.
///
/// A file left unchecked by an instruction the caller wrote — `only` / `skip`, or
/// `enabled = false` — is not charged. The budget catches coverage the run lost *without saying
/// so*, and an instruction that names itself in the report is the opposite of that.
#[must_use]
pub fn over_budget(skipped: &[SkippedFile], budget: Option<usize>) -> Vec<&SkippedFile> {
    let Some(budget) = budget else {
        return Vec::new();
    };
    let charged: Vec<&SkippedFile> = skipped
        .iter()
        .filter(|entry| !poly_core::is_withdrawal_reason(&entry.reason))
        .collect();
    if charged.len() <= budget {
        return Vec::new();
    }
    charged
}

/// What a finished run proved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// Everything named was checked and nothing needs attention.
    Clean,
    /// Everything named was checked; there are error-severity findings, or formatting drift.
    Findings,
    /// The run did not check everything it was asked to; each entry says why.
    NotVerified(Vec<String>),
}

impl Verdict {
    /// The verdict on a lint run under a skip `budget`.
    #[must_use]
    pub fn of_lint(run: &LintRun, budget: Option<usize>) -> Self {
        let error_findings = run
            .results
            .iter()
            .any(|result| result.diagnostics.iter().any(|d| d.severity == Severity::Error));
        Self::classify(
            run.errors.len(),
            run.components.blocking(),
            &over_budget(&run.skipped, budget),
            budget,
            error_findings,
        )
    }

    /// The verdict on a format run under a skip `budget`: drift is the finding.
    #[must_use]
    pub fn of_format(run: &FormatRun, budget: Option<usize>) -> Self {
        let drift = run.results.iter().any(|result| result.changed);
        Self::classify(
            run.errors.len(),
            run.components.blocking(),
            &over_budget(&run.skipped, budget),
            budget,
            drift,
        )
    }

    fn classify(
        errors: usize,
        blocking_components: bool,
        over_budget: &[&SkippedFile],
        budget: Option<usize>,
        findings: bool,
    ) -> Self {
        let mut reasons = Vec::new();
        if errors > 0 {
            reasons.push(format!("the run failed on {}", poly_core::report::files(errors)));
        }
        if blocking_components {
            reasons.push("a needed component is not installed".to_owned());
        }
        if !over_budget.is_empty() {
            reasons.push(format!(
                "{} skipped (limit {})",
                poly_core::report::files(over_budget.len()),
                budget.unwrap_or_default()
            ));
        }
        if !reasons.is_empty() {
            Self::NotVerified(reasons)
        } else if findings {
            Self::Findings
        } else {
            Self::Clean
        }
    }

    /// The CLI exit code: 0 clean, 1 findings, 2 not verified.
    #[must_use]
    pub fn exit_code(&self) -> u8 {
        match self {
            Self::Clean => 0,
            Self::Findings => 1,
            Self::NotVerified(_) => 2,
        }
    }

    /// The MCP `isError` flag: only a run that did not verify is a tool error. Findings are the
    /// result a caller asked for, not a failure of the call.
    #[must_use]
    pub fn is_error(&self) -> bool {
        matches!(self, Self::NotVerified(_))
    }
}

#[cfg(test)]
mod tests;
