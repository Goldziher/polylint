//! The per-file lint step: probe the stat fast path, read and vet the input, run
//! the engines, apply fixes, and record the verdict.

use std::ops::ControlFlow;
use std::path::Path;

use anyhow::Context;
use poly_cache::Verdict;

use super::ctx::LintCtx;
use super::edits::apply_edits;
use super::fast::{Outcome, Probe, Resolution, lint_skip_reason, replayed_lint_result};
use super::generated::{invalid_utf8_result, lint_skip_result};
use super::lint_pass::{LintPass, lint_content};
use super::plan::EnginePlan;
use super::skips::{BINARY_SKIP, GENERATED_SKIP, LOCKFILE_SKIP, is_lock_file};
use super::types::LintResult;
use super::write::write_atomic_if_unchanged;
use crate::discover::DiscoveredFile;
use crate::engine::{Diagnostic, Edit};
use crate::filter::suppressed::SuppressedDiagnostic;
use crate::filter::{
    PerFileIgnores, Suppressions, is_binary, is_generated_source, is_hash_stamped_source, relative_for_match,
};

/// Maximum autofix passes per file: applying a fix can surface or resolve
/// others, so re-lint until stable, but cap to guarantee termination.
const MAX_FIX_PASSES: usize = 5;

/// A file's findings after the engines ran, with the suppression state the fix
/// passes keep updating.
struct Linted {
    pass: LintPass,
    suppressed: Vec<SuppressedDiagnostic>,
    directives: Suppressions,
}

/// Inline `poly: allow[…]` directives (ADR 0028) and `[per-file-ignores]`
/// applied to one file's findings.
struct FindingFilter<'a> {
    path: &'a Path,
    ignores: &'a PerFileIgnores,
    rel: Option<String>,
}

impl<'a> FindingFilter<'a> {
    fn new(ctx: &'a LintCtx, f: &'a DiscoveredFile) -> Self {
        let ignores = &ctx.setup.ignores[f.config_id];
        let rel = (!ignores.is_empty()).then(|| relative_for_match(&f.path, &ctx.setup.ignore_bases[f.config_id]));
        Self {
            path: &f.path,
            ignores,
            rel,
        }
    }

    /// Directives are rebuilt from the content they are applied to: a fix pass
    /// rewrites the file, so directive and target line numbers shift. The gate
    /// inside `Suppressions::parse` is a single substring scan, so a file
    /// without a directive — nearly all of them — pays that and nothing else.
    ///
    /// Inline suppression runs before `[per-file-ignores]` so the `lazy-ignore`
    /// findings it appends are themselves silenceable by config while staying
    /// immune to the directives that produced them. The two filters are
    /// independent retains, so this order does not change which real diagnostics
    /// survive.
    ///
    /// `out` is cleared rather than appended to, because the fix loop calls this
    /// once per pass over freshly re-linted diagnostics: appending would report
    /// one suppression per pass for a finding that was suppressed once.
    fn apply(
        &self,
        suppressions: &Suppressions,
        diagnostics: &mut Vec<Diagnostic>,
        out: &mut Vec<SuppressedDiagnostic>,
    ) {
        out.clear();
        if !suppressions.is_empty() {
            suppressions.apply(self.path, diagnostics, out);
        }
        if let Some(rel) = &self.rel {
            self.ignores.apply(rel, self.path, diagnostics, out);
        }
    }
}

fn record(ctx: &LintCtx, probe: Option<&Probe>, bytes: &[u8], outcome: Outcome) {
    if let (Some(fast), Some(probe)) = (ctx.fast, probe) {
        fast.record(probe, bytes, outcome);
    }
}

pub(super) fn lint_one<'f>(
    ctx: &LintCtx<'_, 'f>,
    f: &DiscoveredFile,
    resolution: Resolution<'f>,
) -> anyhow::Result<LintResult> {
    if is_lock_file(&f.path) {
        return Ok(lint_skip_result(f, LOCKFILE_SKIP));
    }
    let filter = FindingFilter::new(ctx, f);
    let probe = match probe_cache(ctx, f, &filter, resolution) {
        ControlFlow::Break(result) => return Ok(result),
        ControlFlow::Continue(probe) => probe,
    };
    let original = match read_input(ctx, f, probe.as_ref())? {
        ControlFlow::Break(result) => return Ok(result),
        ControlFlow::Continue(original) => original,
    };
    let skipped = lint_skip_reason(ctx.plan, f, ctx.externally_linted);
    let engine_plans = ctx.plan.engines(f.config_id, &f.language);
    let mut linted = run_engines(ctx, f, &filter, engine_plans, &original, probe.as_ref())?;

    // Report on a hash-stamped file but never rewrite it — the same question
    // `format_one` asks, deliberately, so the two phases cannot disagree about
    // one file. A bare `DO NOT EDIT` banner is *not* enough: it announces
    // provenance, not a checksum, so a fix there breaks no verify step, and
    // withholding it left a file `poly fmt` reformats unfixable by `poly lint
    // --fix` in the same repository. A stamp is a claim about the bytes, and
    // reformatting them puts the generator's verify step into a regen loop.
    // `--fix-generated` opts back in.
    let stamped = is_hash_stamped_source(&original);
    let generated = ctx.fix && !ctx.fix_generated && stamped;
    if generated {
        tracing::debug!(path = %f.path.display(), "skipping --fix on generated file");
    }
    let mut fixed = 0usize;
    if ctx.fix && !generated {
        fixed = apply_fixes(ctx, f, &filter, engine_plans, &original, &mut linted)?;
    }
    let Linted {
        pass: LintPass {
            diagnostics,
            debug,
            payloads,
        },
        suppressed,
        directives,
    } = linted;
    // A stamped file is never recorded: replaying it would have to reproduce the
    // `--fix` withholding, which is a property of the run, not of the bytes.
    if fixed == 0 && !stamped {
        record_verdict(ctx, probe.as_ref(), &original, payloads, directives.is_empty());
    }

    Ok(LintResult {
        path: f.path.clone(),
        config: f.config_id,
        diagnostics,
        suppressed,
        fix_withheld_generated: generated,
        fixed,
        skipped,
        error: None,
        debug,
    })
}

/// Answer from the stat fast path when it can; otherwise hand back the probe
/// needed to record this file's verdict later.
fn probe_cache<'f>(
    ctx: &LintCtx<'_, 'f>,
    f: &DiscoveredFile,
    filter: &FindingFilter,
    resolution: Resolution<'f>,
) -> ControlFlow<LintResult, Option<Probe<'f>>> {
    let Some(fast) = ctx.fast else {
        return ControlFlow::Continue(None);
    };
    match resolution {
        Resolution::Hit(record) if record.verdict == Verdict::Binary => {
            ControlFlow::Break(lint_skip_result(f, BINARY_SKIP))
        }
        Resolution::Hit(record) => {
            if let Some(mut diagnostics) = fast.decode(f, record) {
                let mut suppressed = Vec::new();
                filter.apply(&Suppressions::default(), &mut diagnostics, &mut suppressed);
                return ControlFlow::Break(replayed_lint_result(
                    f,
                    diagnostics,
                    suppressed,
                    lint_skip_reason(ctx.plan, f, ctx.externally_linted),
                ));
            }
            ControlFlow::Continue(fast.probe(f))
        }
        Resolution::Miss(missed) => ControlFlow::Continue(Some(missed)),
        Resolution::Unavailable => ControlFlow::Continue(None),
    }
}

/// Read the file and apply the guards that decide whether it is linted at all:
/// size limit, binary, UTF-8, and `[discovery] generated`.
fn read_input(
    ctx: &LintCtx,
    f: &DiscoveredFile,
    probe: Option<&Probe>,
) -> anyhow::Result<ControlFlow<LintResult, String>> {
    if let Some(skip) = ctx
        .setup
        .size_limits
        .skip_reason(f.config_id, probe.map(|p| p.size()), &f.path)
    {
        return Ok(ControlFlow::Break(lint_skip_result(f, &skip)));
    }
    let bytes = std::fs::read(&f.path).with_context(|| format!("reading {}", f.path.display()))?;
    // Asked before the decode, because the decode cannot tell the two apart:
    // a compiled artifact and a corrupted source file both fail
    // `String::from_utf8`, and only the second is something to report.
    if is_binary(&bytes) {
        tracing::debug!(path = %f.path.display(), "not linting binary file");
        record(ctx, probe, &bytes, Outcome::binary());
        return Ok(ControlFlow::Break(lint_skip_result(f, BINARY_SKIP)));
    }
    let original = match String::from_utf8(bytes) {
        Ok(content) => content,
        Err(error) => return Ok(ControlFlow::Break(invalid_utf8_result(f, error.utf8_error()))),
    };
    // `[discovery] generated = false` (opt-out; the default is `true`). The bool
    // is tested first so the default path pays one indexed load and never scans
    // the header.
    //
    // It wins over `--fix-generated`, which governs only whether a file poly did
    // check may be rewritten: with the opt-out on there is nothing to fix, so
    // asking to fix more cannot opt the file back into being linted.
    if !ctx.setup.act_on_generated[f.config_id] && is_generated_source(&original) {
        tracing::debug!(path = %f.path.display(), "not linting generated file");
        return Ok(ControlFlow::Break(lint_skip_result(f, GENERATED_SKIP)));
    }
    Ok(ControlFlow::Continue(original))
}

fn run_engines(
    ctx: &LintCtx,
    f: &DiscoveredFile,
    filter: &FindingFilter,
    engine_plans: &[EnginePlan],
    original: &str,
    probe: Option<&Probe>,
) -> anyhow::Result<Linted> {
    let reuse = probe.map_or_else(Vec::new, |probe| probe.reusable(original.as_bytes()));
    let mut pass = lint_content(
        f,
        engine_plans,
        original,
        ctx.collect_debug,
        &reuse,
        probe.is_some(),
        ctx.prefilled,
    )?;
    let mut suppressed: Vec<SuppressedDiagnostic> = Vec::new();
    let directives = Suppressions::parse(original);
    filter.apply(&directives, &mut pass.diagnostics, &mut suppressed);
    Ok(Linted {
        pass,
        suppressed,
        directives,
    })
}

/// Apply autofixes, re-linting until stable, and write the result back.
///
/// Returns the number of diagnostics whose edits were committed. Counted across
/// passes rather than derived from the drop in diagnostic count: applying one
/// fix can surface another, so a before/after delta would understate (or
/// invert) what the run actually did.
fn apply_fixes(
    ctx: &LintCtx,
    f: &DiscoveredFile,
    filter: &FindingFilter,
    engine_plans: &[EnginePlan],
    original: &str,
    linted: &mut Linted,
) -> anyhow::Result<usize> {
    let mut fixed = 0usize;
    let mut content = original.to_owned();
    for _ in 0..MAX_FIX_PASSES {
        let edit_groups: Vec<&[Edit]> = linted
            .pass
            .diagnostics
            .iter()
            .filter(|d| !d.fix.is_empty())
            .map(|d| d.fix.as_slice())
            .collect();
        match apply_edits(&content, &edit_groups) {
            Some((next, applied)) if next != content => {
                content = next;
                fixed += applied;
                let next = lint_content(f, engine_plans, &content, ctx.collect_debug, &[], false, ctx.prefilled)?;
                linted.pass.diagnostics = next.diagnostics;
                filter.apply(
                    &Suppressions::parse(&content),
                    &mut linted.pass.diagnostics,
                    &mut linted.suppressed,
                );
                linted.pass.debug = next.debug;
            }
            _ => break,
        }
    }
    if content != original {
        write_atomic_if_unchanged(&f.path, original, &content)?;
    }
    Ok(fixed)
}

fn record_verdict(ctx: &LintCtx, probe: Option<&Probe>, original: &str, payloads: Vec<Vec<u8>>, replayable: bool) {
    let found_nothing = payloads.iter().all(Vec::is_empty);
    record(
        ctx,
        probe,
        original.as_bytes(),
        Outcome {
            verdict: if found_nothing {
                Verdict::Clean
            } else {
                Verdict::Diagnostics
            },
            replayable,
            payloads,
        },
    );
}
