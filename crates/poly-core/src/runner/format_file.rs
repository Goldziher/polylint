//! The per-file format step: probe the stat fast path, read and vet the input,
//! drive the engine chain to a fixed point, write the result, and record the
//! verdict.

use std::ops::ControlFlow;
use std::sync::Arc;

use anyhow::Context;
use poly_cache::{CacheKey, Namespace, ResultCache, Verdict};
use poly_engine_api::declined::Declined;

use super::ctx::FormatCtx;
use super::debug_record::{DebugSource, push_engine_debug};
use super::engine_clock::{EngineClock, note_slow_engine};
use super::fast::{Outcome, Probe, Resolution};
use super::generated::format_skip_result;
use super::plan::EnginePlan;
use super::skips::{BINARY_SKIP, GENERATED_SKIP, LOCKFILE_SKIP, is_lock_file};
use super::types::{FormatResult, RunDebug};
use super::write::write_atomic_if_unchanged;
use super::{
    HASH_STAMPED_SKIP, MAX_FORMAT_PASSES, digest_if_enabled, fmt_entry, format_to_fixed_point, should_cache_result,
};
use crate::discover::DiscoveredFile;
use crate::engine::{FormatOutput, SourceFile};
use crate::engine_guard::guard_engine_panic;
use crate::filter::{is_binary, is_format_ignored, is_generated_source, is_hash_stamped_source};

pub(super) fn format_one<'f>(
    ctx: &FormatCtx<'_, 'f>,
    f: &DiscoveredFile,
    resolution: Resolution<'f>,
) -> anyhow::Result<FormatResult> {
    if is_lock_file(&f.path) {
        return Ok(format_skip_result(f, Some(LOCKFILE_SKIP.to_owned())));
    }
    let probe = match probe_cache(f, resolution) {
        ControlFlow::Break(result) => return Ok(result),
        ControlFlow::Continue(probe) => probe,
    };
    let original = match read_input(ctx, f, probe.as_ref())? {
        ControlFlow::Break(result) => return Ok(result),
        ControlFlow::Continue(original) => original,
    };
    if let Some(result) = early_skip(ctx, f, &original) {
        return Ok(result);
    }
    let mut debug = ctx.collect_debug.then(RunDebug::default);
    let mut src = SourceFile {
        path: f.path.clone(),
        language: f.language.clone(),
        content: Arc::from(original.as_str()),
    };
    let engine_plans = ctx.plan.engines(f.config_id, &f.language);

    // When every engine routed to this file declines it, nothing inspected the
    // content — report that rather than letting it read as "checked and clean".
    // Asking is the whole decision: a backend that declined here would only
    // re-derive the same answer inside `format` — for the content-scanning ones
    // (`rumdl`, `yaml`, `markup_fmt`) by scanning the file a second time — and
    // then return `Unchanged`, so the chain below is skipped outright. That
    // equivalence is the [`Engine::skip_reason`] contract: a backend that
    // declines a file must not reformat it.
    let skipped = skip_reason_for(engine_plans, &src);
    if skipped.is_some() {
        return Ok(format_skip_result(f, skipped));
    }
    let initial = Arc::clone(&src.content);

    // A component engine's decline arrives in its `Format` reply ([`Declined`]) instead of from
    // `skip_reason`, which would have cost a round trip carrying the file; it is recorded here per
    // engine and settled after the chain the same way `skip_reason_for` settles an up-front decline.
    let mut declined: Vec<Option<String>> = vec![None; engine_plans.len()];
    let (current, settled) = format_to_fixed_point(initial, |input, record_debug| {
        let mut state = PassState {
            src: &mut src,
            debug: &mut debug,
            record_debug,
            declined: &mut declined,
            engine_index: 0,
        };
        format_pass(ctx, f, engine_plans, &mut state, input)
    })?;

    let changed = *current != *original;
    if !changed && let Some(reason) = declined_by_every_engine(engine_plans, &declined, &src) {
        return Ok(format_skip_result(f, Some(reason)));
    }
    write_and_settle(ctx, f, &original, &current, changed, settled)?;
    if !changed {
        let clean = Outcome {
            verdict: Verdict::Clean,
            replayable: true,
            payloads: Vec::new(),
        };
        record(ctx, probe.as_ref(), original.as_bytes(), clean);
    }
    Ok(FormatResult {
        path: f.path.clone(),
        config: f.config_id,
        changed,
        formatted: if changed { Some(current.to_string()) } else { None },
        skipped,
        error: None,
        debug,
    })
}

fn record(ctx: &FormatCtx, probe: Option<&Probe>, bytes: &[u8], outcome: Outcome) {
    if let (Some(fast), Some(probe)) = (ctx.fast, probe) {
        fast.record(probe, bytes, outcome);
    }
}

fn probe_cache<'f>(f: &DiscoveredFile, resolution: Resolution<'f>) -> ControlFlow<FormatResult, Option<Probe<'f>>> {
    match resolution {
        Resolution::Hit(record) if record.verdict == Verdict::Binary => {
            ControlFlow::Break(format_skip_result(f, Some(BINARY_SKIP.to_owned())))
        }
        Resolution::Hit(_) => ControlFlow::Break(format_skip_result(f, None)),
        Resolution::Miss(probe) => ControlFlow::Continue(Some(probe)),
        Resolution::Unavailable => ControlFlow::Continue(None),
    }
}

/// Read the file, applying the size limit and the binary and UTF-8 guards.
fn read_input(
    ctx: &FormatCtx,
    f: &DiscoveredFile,
    probe: Option<&Probe>,
) -> anyhow::Result<ControlFlow<FormatResult, String>> {
    if let Some(skip) = ctx.limits.skip_reason(f.config_id, probe.map(|p| p.size()), &f.path) {
        return Ok(ControlFlow::Break(format_skip_result(f, Some(skip))));
    }
    let bytes = std::fs::read(&f.path).with_context(|| format!("reading {}", f.path.display()))?;
    // See the same guard on the lint path: a binary file is a skip, malformed
    // text is an error, and only a pre-decode check can distinguish them.
    if is_binary(&bytes) {
        tracing::debug!(path = %f.path.display(), "not formatting binary file");
        record(ctx, probe, &bytes, Outcome::binary());
        return Ok(ControlFlow::Break(format_skip_result(f, Some(BINARY_SKIP.to_owned()))));
    }
    match String::from_utf8(bytes) {
        Ok(content) => Ok(ControlFlow::Continue(content)),
        Err(error) => {
            let error = error.utf8_error();
            let invalid_at = error.valid_up_to();
            let sequence = error.error_len().map_or_else(
                || "an incomplete UTF-8 sequence reaches the end of the file".to_owned(),
                |length| format!("{length} byte(s) do not form a valid UTF-8 sequence"),
            );
            Err(anyhow::anyhow!(
                "file is not valid UTF-8 at byte {invalid_at}; text formatting was skipped: {sequence}, starting at byte {invalid_at}"
            ))
        }
    }
}

/// The content-based reasons a file is left unformatted before any engine runs.
fn early_skip(ctx: &FormatCtx, f: &DiscoveredFile, original: &str) -> Option<FormatResult> {
    if is_format_ignored(original, &f.language) {
        return Some(format_skip_result(f, None));
    }
    // An empty file is already as formatted as it can be. Formatters differ on it (the TOML and JSON ones
    // answer with a lone newline), and `--fix` must not write that into a file that had no content.
    if original.is_empty() {
        return Some(format_skip_result(f, None));
    }
    // A caller instruction that withdrew every formatter for this language is
    // reported here, for walked files as well as named ones. Before this, an
    // emptied format plan fell through `skip_reason_for`'s "ordinary coverage"
    // branch and the file was silently passed over, so `poly fmt --only ruff .`
    // printed "All formatted" over files nothing had touched.
    if let Some(withdrawal) = ctx.plan.withdrawal(f.config_id, &f.language) {
        return Some(format_skip_result(f, Some(withdrawal.reason())));
    }
    // `[discovery] generated = false`, checked before the hash stamp below so
    // the reader's own opt-out is the reason they are shown. Bool first: the
    // default path never scans the header.
    if !ctx.act_on_generated[f.config_id] && is_generated_source(original) {
        tracing::debug!(path = %f.path.display(), "not formatting generated file");
        return Some(format_skip_result(f, Some(GENERATED_SKIP.to_owned())));
    }
    // Skip only when the header stamps a **content hash** over the body:
    // reformatting invalidates it, so a verify step reports drift on a file no
    // human touched and the remedy is a regen that discards the formatting —
    // a loop, and one reporter had 110 of 123 files in it.
    //
    // Deliberately narrower than `is_generated_source`, which `lint --fix` uses.
    // Skipping here removes the file from the format gate entirely, so a bare
    // "DO NOT EDIT" banner must not trigger it: a generator that stamps a
    // hand-written file would otherwise drop it out of enforcement silently.
    // Formatting a banner-only file is harmless; not checking it is not.
    //
    // Skipped rather than reported as drift: poly will not fix these, so
    // flagging them under `--check` would leave a gate that can never go green.
    if !ctx.fix_generated && is_hash_stamped_source(original) {
        return Some(format_skip_result(f, Some(HASH_STAMPED_SKIP.to_owned())));
    }
    None
}

/// Write the formatted content, then refuse to call a file formatted that is
/// still changing.
fn write_and_settle(
    ctx: &FormatCtx,
    f: &DiscoveredFile,
    original: &str,
    current: &str,
    changed: bool,
    settled: bool,
) -> anyhow::Result<()> {
    if changed && ctx.write {
        write_atomic_if_unchanged(&f.path, original, current)?;
    }
    // The content is written first — the passes did real work and throwing it
    // away would leave the file worse — but the run must not then call the file
    // formatted. It is still changing, so a following `poly fmt --check` reports
    // it as unformatted no matter how many times `--fix` is run, and saying
    // "formatted" here is the false pass this whole surface exists to avoid.
    if !settled {
        anyhow::bail!(
            "formatting did not settle after {MAX_FORMAT_PASSES} passes; the file is still changing, \
             so `poly fmt` will keep reporting it. This is a backend that does not converge \
             on this input, not a problem with the file"
        );
    }
    Ok(())
}

/// What one pass over the engine chain mutates: the source the engines see,
/// the debug records, which are collected on the first pass only, and the
/// declines engines answered from their format call, per plan index.
struct PassState<'a> {
    src: &'a mut SourceFile,
    debug: &'a mut Option<RunDebug>,
    record_debug: bool,
    declined: &'a mut [Option<String>],
    /// Plan index of the engine being run.
    engine_index: usize,
}

/// Run every format engine once over `input`, returning the chained output.
/// Debug records are collected on the first pass only (`record_debug`) so the
/// convergence loop does not inflate per-engine timing counts.
fn format_pass(
    ctx: &FormatCtx,
    f: &DiscoveredFile,
    engine_plans: &[EnginePlan],
    state: &mut PassState,
    input: &Arc<str>,
) -> anyhow::Result<Arc<str>> {
    let mut current = Arc::clone(input);
    for (index, plan) in engine_plans.iter().enumerate() {
        if plan.ignores_format(&f.path) {
            continue;
        }
        state.engine_index = index;
        current = format_with_engine(ctx, f, plan, state, &current)?;
    }
    Ok(current)
}

/// One engine's pass over `current`: a cache hit, a batch result, or a run.
fn format_with_engine(
    ctx: &FormatCtx,
    f: &DiscoveredFile,
    plan: &EnginePlan,
    state: &mut PassState,
    current: &Arc<str>,
) -> anyhow::Result<Arc<str>> {
    let record_debug = state.record_debug;
    let debug = &mut *state.debug;
    let version = plan.format_version(&f.path);
    let key = digest_if_enabled(ctx.cache, || {
        let digest = ResultCache::format_file_digest(f.language.id(), &f.path, current);
        ResultCache::key_with_args(
            Namespace::Fmt,
            plan.engine.name(),
            &version,
            &plan.serialized_args,
            &digest,
        )
    });
    if let Some(key) = &key
        && let Some(bytes) = ctx.cache.get(Namespace::Fmt, key)
        && let Some(text) = fmt_entry::decode(current, &bytes)
    {
        if record_debug {
            push_engine_debug(debug.as_mut(), plan, DebugSource::CacheHit);
        }
        return Ok(text);
    }
    // A batch already computed this engine's answer for this exact
    // content. `take` compares the content rather than trusting the
    // path, so a stale or mismatched entry falls through to the tool.
    if let Some(out) = ctx.prefilled.take(&f.path, plan.engine.name(), current) {
        if record_debug {
            push_engine_debug(debug.as_mut(), plan, DebugSource::Batched);
        }
        // Persisted unconditionally, bypassing `should_cache_result`.
        // That guard refuses results cheaper than 5 ms because an
        // in-process engine can recompute faster than the disk can
        // answer — but recomputing *this* costs a process spawn, so the
        // guard is exactly inverted here. Without the bypass poly would
        // decline to cache precisely the results worth caching, and
        // re-batch the whole corpus on every run (ADR 0033 §4).
        if let Some(key) = &key
            && let Err(error) = ctx.cache.put(Namespace::Fmt, key, &fmt_entry::encode(current, &out))
        {
            tracing::warn!(
                engine = plan.engine.name(),
                "failed to store batched fmt cache entry: {error:#}"
            );
        }
        return Ok(out);
    }
    run_engine(ctx, f, plan, state, current, key.as_ref())
}

fn run_engine(
    ctx: &FormatCtx,
    f: &DiscoveredFile,
    plan: &EnginePlan,
    state: &mut PassState,
    current: &Arc<str>,
    key: Option<&CacheKey>,
) -> anyhow::Result<Arc<str>> {
    let record_debug = state.record_debug;
    let debug = &mut *state.debug;
    let src = &mut *state.src;
    src.content = Arc::clone(current);
    let clock = EngineClock::start();
    let started = clock.started();
    let config = plan.engine.format_config(src, &plan.config);
    let formatted = match guard_engine_panic(plan.engine.name(), &f.path, || plan.engine.format(src, &config)) {
        Ok(formatted) => formatted,
        Err(error) => {
            let Some(reason) = Declined::reason(&error).map(str::to_owned) else {
                return Err(error);
            };
            // A declining engine leaves the content as it is. The decline is not cached: a cache hit
            // replays output, not a decline, and would count the file as checked on the next run.
            note_slow_engine(&f.path, src.content.len(), plan.engine.name(), clock.stop());
            if record_debug {
                push_engine_debug(debug.as_mut(), plan, DebugSource::Ran(started));
            }
            if let Some(slot) = state.declined.get_mut(state.engine_index) {
                slot.get_or_insert(reason);
            }
            return Ok(Arc::clone(current));
        }
    };
    let out: Arc<str> = match formatted {
        FormatOutput::Unchanged => Arc::clone(current),
        FormatOutput::Formatted(s) => Arc::from(s),
    };
    let time = clock.stop();
    let elapsed = time.wall;
    if record_debug {
        push_engine_debug(debug.as_mut(), plan, DebugSource::Ran(started));
    }
    note_slow_engine(&f.path, src.content.len(), plan.engine.name(), time);
    if let Some(key) = key
        && should_cache_result(elapsed)
        && let Err(error) = ctx.cache.put(Namespace::Fmt, key, &fmt_entry::encode(current, &out))
    {
        tracing::warn!(
            engine = plan.engine.name(),
            "failed to store fmt cache entry: {error:#}"
        );
    }
    Ok(out)
}

/// The reason no backend inspected `src`, when every engine routed to it
/// declined it, after the chain ran: each either declined from its format call
/// (`declined`, by plan index) or declines up front. The first reason in plan
/// order wins, as in [`skip_reason_for`].
///
/// Asked only when some engine declined from its format call, so the common
/// path never re-asks the up-front engines.
fn declined_by_every_engine(plans: &[EnginePlan], declined: &[Option<String>], src: &SourceFile) -> Option<String> {
    if declined.iter().all(Option::is_none) {
        return None;
    }
    let mut reason = None;
    for (plan, from_call) in plans.iter().zip(declined) {
        let why = match from_call {
            Some(why) => why.clone(),
            None => plan.engine.skip_reason(src)?.into_owned(),
        };
        reason.get_or_insert(why);
    }
    reason
}

/// The reason no backend inspected `src`, when every engine routed to it
/// declines.
///
/// Returns `None` when at least one engine will actually look at the file, so a
/// file served by both a declining and a willing backend is not reported as
/// skipped. An empty plan means no backend covers the language at all, which is
/// ordinary coverage rather than a decline, and is likewise not reported here.
///
/// A `Some` answer stands in for running the chain: every backend declined, and
/// a backend that declines a file must not reformat it, so the format pass is
/// skipped rather than asking each of them the same question a second time.
fn skip_reason_for(plans: &[EnginePlan], src: &SourceFile) -> Option<String> {
    if plans.is_empty() {
        return None;
    }
    let mut reason = None;
    for plan in plans {
        // `?` short-circuits the moment any engine is willing to look at the
        // file: one willing backend means the file was not skipped.
        let why = plan.engine.skip_reason(src)?;
        reason.get_or_insert_with(|| why.into_owned());
    }
    reason
}
