//! Parallel orchestration (rayon): discover files, route to backends, run with
//! content-hash caching, collect results. Defaults to all logical cores.

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Once};

use crate::config::{Config, Kind};
use crate::discover::{DiscoveredFile, DiscoveryReport, discover_reporting};
use crate::filter::{PerFileIgnores, is_generated_lockfile, match_bases};
use crate::resolve::ConfigSet;
use crate::timings;
use poly_cache::{Namespace, ResultCache};
use rayon::prelude::*;
use rustc_hash::FxHashSet;

mod batch;
mod ctx;
mod debug_record;
mod edits;
mod engine_clock;
mod fast;
mod fmt_entry;
mod format_file;
mod generated;
mod lint_file;
mod lint_pass;
mod plan;
mod project;
pub(crate) use project::ancestor_pairs;
pub(crate) mod selection;
mod skips;
mod types;
mod write;

use crate::fingerprint::run_fingerprints as config_fingerprints;
use batch::{LintPrefill, PendingLint, prefill, prefill_lint};
use ctx::{FormatCtx, LintCtx, LintSetup, RunEnv};
use fast::{FastPath, Replay, Resolution, flush_stat_indexes, log_fast_path};
use format_file::format_one;
use generated::acts_on_generated;
use lint_file::lint_one;
use plan::{RunPlan, plan_by_config_language, prefetch_tier2_grammars};
#[doc(hidden)]
pub use project::with_extra_project_engines;
use selection::{EngineSelection, validate_selection};
pub use skips::{
    BINARY_SKIP, DISABLED_SKIP_PREFIX, FILTERED_SKIP, GENERATED_SKIP, LOCKFILE_SKIP, NO_ENGINE_SKIP,
    NO_LINT_RULES_SKIP_PREFIX, SkippedFile, TOO_LARGE_SKIP_PREFIX, is_withdrawal_reason,
};
use skips::{SizeLimits, unmatched_explicit_paths};
// The suppression record lives beside the filters that produce it; re-exported
// here so a consumer reaches it next to `SkippedFile`, the accounting it
// completes.
pub use crate::filter::suppressed::{SuppressedDiagnostic, SuppressionReason};
// Re-exported so `poly_core::runner::LintResult` keeps naming the same type it
// always has: the split below is a file boundary, not an API one.
pub use types::{
    EngineDebug, FormatError, FormatResult, FormatRun, LintError, LintResult, LintRun, RunDebug, RunOptions,
};

/// Minimum engine runtime for which persisting and reloading a cache entry is
/// expected to cost less than recomputing the result.
const MIN_CACHE_DURATION: std::time::Duration = std::time::Duration::from_millis(5);

/// Maximum format passes per file. A formatter is not guaranteed to be
/// idempotent — line-wrap/reflow can shift on a second run (observed with
/// clang-format on `.h`, csharpier on `.cs`, google-java-format on `.java`) —
/// so we re-run the whole engine chain until the content stops changing. Capped
/// to guarantee termination when a backend genuinely oscillates. Without this,
/// a single `poly fmt --fix` could leave a file that a subsequent `poly fmt
/// --check` still reports as unformatted.
const MAX_FORMAT_PASSES: usize = 5;

/// Reason reported when a phase declines to rewrite a file whose header stamps
/// a content hash over its body.
///
/// Narrower than [`GENERATED_SKIP`], and a different kind of decision: that one
/// is the reader's opt-out, this one is a correctness guard poly applies on its
/// own — reformatting a stamped body invalidates the hash, so the generator's
/// verify step reports drift on a file no human touched and the remedy is a
/// regen that discards the formatting. `--fix-generated` is the way out.
const HASH_STAMPED_SKIP: &str = "hash-stamped generated file (pass --fix-generated to format)";

/// Reason recorded when a routed text file cannot be decoded for linting.
const INVALID_UTF8_SKIP: &str = "file is not valid UTF-8; text linting was skipped";

/// Lint all discovered files under `paths`. Returns one [`LintResult`] per file
/// that still has at least one diagnostic. When `fix` is true, each file's
/// available autofixes are applied in place (re-linting until stable) before
/// the remaining, unfixable diagnostics are reported.
pub fn lint(
    paths: &[PathBuf],
    config: &Config,
    opts: &RunOptions,
    fix: bool,
    collect_debug: bool,
) -> anyhow::Result<Vec<LintResult>> {
    Ok(lint_run(paths, config, opts, fix, collect_debug)?.results)
}

/// [`lint`], additionally returning the run-level accounting ([`LintRun`]) a
/// self-qualifying summary needs: how many files were actually linted, and what
/// the exclude set pruned before they were.
pub fn lint_run(
    paths: &[PathBuf],
    config: &Config,
    opts: &RunOptions,
    fix: bool,
    collect_debug: bool,
) -> anyhow::Result<LintRun> {
    start_timings(opts);
    configure_pool(opts.jobs);
    let cache = ResultCache::open_default(!opts.no_cache)?;
    let phase = timings::phase("config resolve");
    let configs = build_config_set(paths, config, opts)?;
    let selection = EngineSelection::new(&opts.only, &opts.skip);
    validate_selection(&configs, crate::components::run_index(opts), &opts.only, &opts.skip)?;
    drop(phase);
    let phase = timings::phase("discovery");
    let (files, discovery) = discover_reporting(paths, &configs, &opts.exclude, opts.force_exclude);
    drop(phase);
    let phase = timings::phase("planning");
    let plan = plan_by_config_language(
        &files,
        &configs,
        Kind::Lint,
        selection,
        opts,
        &project::ancestor_configs(&configs, paths),
    );
    prefetch_tier2_grammars(&plan);
    drop(phase);
    let setup = lint_setup(&configs, paths, opts);
    // One relaxed increment per file, so the summary can state how many files
    // were genuinely linted rather than how many were handed to the pipeline.
    // Negligible next to reading and parsing the file it counts.
    let checked = AtomicUsize::new(0);
    let replay = Replay {
        diagnostics: !fix && !collect_debug,
        verdicts: !collect_debug,
    };
    let phase = timings::phase("stat index resolve");
    let fast = FastPath::new(
        &cache,
        Namespace::Lint,
        &plan,
        &|id| u8::from(setup.act_on_generated[id]),
        &setup.size_limits,
        replay,
    );
    let resolutions = resolve_all(fast.as_ref(), &files);
    drop(phase);
    let phase = timings::phase("batch prefill");
    let prefilled = lint_prefill(&files, &resolutions, &plan, &setup.size_limits);
    drop(phase);
    let ctx = LintCtx {
        plan: &plan,
        fix,
        fix_generated: opts.fix_generated,
        collect_debug,
        setup: &setup,
        externally_linted: &opts.externally_linted_languages,
        prefilled: &prefilled,
        fast: fast.as_ref(),
    };
    let phase = timings::phase("per-file loop");
    let (mut linted, errs) = lint_files(&ctx, &files, resolutions, &checked);
    drop(phase);
    let phase = timings::phase("cache flush");
    flush_stat_indexes(&cache);
    drop(phase);
    log_fast_path(fast.as_ref(), "lint");
    let env = RunEnv {
        paths,
        files: &files,
        configs: &configs,
        opts,
        plan: &plan,
        cache: &cache,
        selection,
    };
    let phase = timings::phase("project phase");
    let project_reports = project_phase(&env, &setup, fix, &mut linted);
    drop(phase);
    Ok(assemble_lint_run(
        &env,
        linted,
        errs,
        project_reports,
        checked.into_inner(),
        discovery,
    ))
}

/// Turn on `--timings` collection when the caller asked for it. Never turns it off,
/// so phases a caller recorded before the run are kept.
fn start_timings(opts: &RunOptions) {
    if opts.timings {
        timings::enable();
    }
}

/// Per-config lint inputs, resolved once per run rather than once per file.
fn lint_setup(configs: &ConfigSet, paths: &[PathBuf], opts: &RunOptions) -> LintSetup {
    // Rule-declared `ignores:` globs (the built-in ast-grep pack's, plus any a
    // user rule file declares) layer in beneath the config's own
    // `[per-file-ignores]`, resolved once per config rather than per file.
    let ignores: Vec<PerFileIgnores> = configs
        .iter()
        .map(|c| {
            // Host-side, so a core build serving ast-grep from a component drops the same findings.
            let defaults =
                poly_engine_api::host_hooks::astgrep_rules::default_path_ignores(&c.rules_dirs, c.rules_builtin_pack);
            PerFileIgnores::compile_with_defaults(&c.per_file_ignores, &defaults)
        })
        .collect();
    // Resolved once per config rather than once per file: `ignore_bases` clones
    // a `PathBuf` per run root, and the answer depends only on the config id.
    let run_bases = match_bases(paths);
    let ignore_bases: Vec<Vec<PathBuf>> = (0..configs.len())
        .map(|config_id| configs.ignore_bases(config_id, &run_bases))
        .collect();
    LintSetup {
        ignores,
        ignore_bases,
        // `[discovery] generated`, resolved once per config and once per run rather
        // than per file. See `generated::acts_on_generated`.
        act_on_generated: acts_on_generated(configs, opts.generated),
        size_limits: SizeLimits::of(configs),
    }
}

/// Stat every file against the fast path; `Unavailable` for all of them when
/// the cache is off.
fn resolve_all<'f>(fast: Option<&FastPath<'f>>, files: &[DiscoveredFile]) -> Vec<Resolution<'f>> {
    files
        .par_iter()
        .map(|f| fast.map_or(Resolution::Unavailable, |fast| fast.resolve(f)))
        .collect()
}

/// The lint-side counterpart of the format prefill: an accelerator only, with
/// the per-file path as the answer for anything it did not cover. A file the
/// stat index already answered is never batched, and a file the index can partly
/// answer is batched only for the engines it cannot.
fn lint_prefill(
    files: &[DiscoveredFile],
    resolutions: &[Resolution],
    plan: &RunPlan,
    limits: &SizeLimits,
) -> LintPrefill {
    let pending: Vec<PendingLint> = files
        .iter()
        .zip(resolutions)
        .filter_map(|(file, resolution)| match resolution {
            Resolution::Hit(_) => None,
            Resolution::Miss(probe) => Some((file, Some(probe))),
            Resolution::Unavailable => Some((file, None)),
        })
        .collect();
    let prefilled = prefill_lint(&pending, plan, limits);
    tracing::debug!(
        prefilled = prefilled.len(),
        files = files.len(),
        "lint batch prefill complete"
    );
    prefilled
}

/// Lint every file in parallel. An engine error is carried, not swallowed:
/// dropping the file here is what let `poly lint` report success on a file it
/// had failed to process.
fn lint_files<'f>(
    ctx: &LintCtx<'_, 'f>,
    files: &[DiscoveredFile],
    resolutions: Vec<Resolution<'f>>,
    checked: &AtomicUsize,
) -> (Vec<LintResult>, Vec<LintError>) {
    let (oks, errs): (Vec<_>, Vec<_>) = files
        .par_iter()
        .zip(resolutions)
        .map(|(f, resolution)| match lint_one(ctx, f, resolution) {
            Ok(result) => {
                // A file no backend has rules for is not part of the linted
                // count: it was routed and read, but nothing in the run knew
                // its language, so counting it is the claim this fix removes.
                if result.skipped.is_none() {
                    checked.fetch_add(1, Ordering::Relaxed);
                }
                Ok(result)
            }
            Err(error) => Err(LintError {
                path: f.path.clone(),
                message: format!("{error:#}"),
            }),
        })
        .partition(Result::is_ok);
    (
        oks.into_iter().map(Result::unwrap).collect(),
        errs.into_iter().map(Result::unwrap_err).collect(),
    )
}

/// Whole-project engines run after the per-file tier, on the files as the fix passes left them,
/// and their findings join the per-file results before anything is counted or sorted.
fn project_phase(
    env: &RunEnv,
    setup: &LintSetup,
    fix: bool,
    linted: &mut Vec<LintResult>,
) -> (Vec<LintError>, Vec<SkippedFile>) {
    let inputs = project::RunInputs::new(
        env.configs,
        &setup.act_on_generated,
        env.paths,
        env.opts,
        (&setup.ignores, &setup.ignore_bases),
    );
    let fix_mode = project::FixMode {
        apply: fix,
        generated: env.opts.fix_generated,
    };
    project::run_and_merge(
        linted,
        env.files,
        &inputs,
        env.selection,
        env.cache,
        env.plan.resolved_components(),
        fix_mode,
    )
}

/// Sort, count and fold the per-file and project-phase outcomes into the run's
/// report.
fn assemble_lint_run(
    env: &RunEnv,
    mut linted: Vec<LintResult>,
    mut errors: Vec<LintError>,
    (project_errors, project_skips): (Vec<LintError>, Vec<SkippedFile>),
    checked: usize,
    discovery: DiscoveryReport,
) -> LintRun {
    linted.sort_by(|a, b| a.path.cmp(&b.path));
    for result in &mut linted {
        crate::filter::sort_diagnostics(&mut result.diagnostics);
    }
    // Taken before the filter below, which keeps only files with something to
    // report: a skipped file has nothing to report and would be dropped, which
    // is how it became invisible in the first place.
    let mut skipped: Vec<SkippedFile> = linted
        .iter()
        .filter_map(|result| {
            result.skipped.as_ref().map(|reason| SkippedFile {
                path: result.path.clone(),
                reason: reason.clone(),
            })
        })
        .collect();
    // Taken here for the same reason `skipped` is: a file whose every finding
    // was suppressed has nothing left to report and is dropped by the filter
    // below, which is exactly the file whose suppressions a reader most needs.
    let suppressed: Vec<SuppressedDiagnostic> = linted
        .iter()
        .flat_map(|result| result.suppressed.iter().cloned())
        .collect();
    // A fully fixed file has no diagnostics left, but dropping it here is
    // what made `--fix` silent about the files it rewrote: keep it so the
    // summary — and the JSON payload — can report the fixes.
    let (config_fingerprints, config_index) = config_fingerprints(env.configs, env.opts);
    let results: Vec<LintResult> = linted
        .into_iter()
        .filter(|r| !r.diagnostics.is_empty() || r.fixed > 0)
        .map(|mut r| {
            r.config = config_index[r.config];
            r
        })
        .collect();
    errors.extend(project_errors);
    errors.sort_by(|a, b| a.path.cmp(&b.path));
    for error in &errors {
        tracing::debug!(path = %error.path.display(), "lint failed: {}", error.message);
    }
    skipped.extend(project_skips);
    add_unmatched_paths(env, &mut skipped);
    LintRun {
        results,
        configs: config_fingerprints,
        errors,
        checked,
        skipped,
        suppressed,
        discovery,
        components: env.plan.components().clone(),
    }
}

/// Append the paths the caller named that no engine covers, so a strict-mode
/// check and the summary read one list.
fn add_unmatched_paths(env: &RunEnv, skipped: &mut Vec<SkippedFile>) {
    let unmatched = unmatched_explicit_paths(
        env.paths,
        env.files,
        env.plan,
        env.configs,
        &env.opts.exclude,
        env.opts.force_exclude,
        skipped,
    );
    skipped.extend(unmatched);
}

/// Build the run's [`ConfigSet`]: a single explicit config (`--config`) bypasses
/// hierarchical resolution; otherwise `config` is the root and the walked paths
/// are scanned for nested `poly.toml` files (ADR 0018).
fn build_config_set(paths: &[PathBuf], config: &Config, opts: &RunOptions) -> anyhow::Result<ConfigSet> {
    if opts.explicit_config {
        Ok(ConfigSet::single(config.clone()))
    } else if let Some(resolver) = &opts.config_resolver {
        ConfigSet::build_with(paths, config.clone(), resolver.as_ref())
    } else {
        ConfigSet::build(paths, config.clone())
    }
}

/// Format all discovered files under `paths`. When `write` is true, changed
/// files are rewritten atomically; otherwise this is a dry run (`--check`).
pub fn format(
    paths: &[PathBuf],
    config: &Config,
    opts: &RunOptions,
    write: bool,
    collect_debug: bool,
) -> anyhow::Result<Vec<FormatResult>> {
    Ok(format_run(paths, config, opts, write, collect_debug)?.results)
}

/// [`format()`], additionally returning the run-level accounting ([`FormatRun`])
/// a self-qualifying summary needs: what the exclude set pruned before the
/// discovered files were formatted.
pub fn format_run(
    paths: &[PathBuf],
    config: &Config,
    opts: &RunOptions,
    write: bool,
    collect_debug: bool,
) -> anyhow::Result<FormatRun> {
    start_timings(opts);
    configure_pool(opts.jobs);
    let cache = ResultCache::open_default(!opts.no_cache)?;
    let explicit: FxHashSet<&std::path::Path> = paths.iter().map(PathBuf::as_path).collect();
    let phase = timings::phase("config resolve");
    let configs = build_config_set(paths, config, opts)?;
    let selection = EngineSelection::new(&opts.only, &opts.skip);
    validate_selection(&configs, crate::components::run_index(opts), &opts.only, &opts.skip)?;
    drop(phase);
    let phase = timings::phase("discovery");
    let (discovered, discovery) = discover_reporting(paths, &configs, &opts.exclude, opts.force_exclude);
    let files: Vec<DiscoveredFile> = discovered
        .into_iter()
        .filter(|f| explicit.contains(f.path.as_path()) || !is_generated_lockfile(&f.path))
        .collect();
    drop(phase);
    let phase = timings::phase("planning");
    let plan = plan_by_config_language(&files, &configs, Kind::Format, selection, opts, &[]);
    prefetch_tier2_grammars(&plan);
    drop(phase);
    // Same key, same resolution, same phase-agnostic answer as `lint_run`.
    let act_on_generated = acts_on_generated(&configs, opts.generated);
    let size_limits = SizeLimits::of(&configs);
    // Batch-capable formatters run once over many files here, before the loop.
    // Whatever this produces is an accelerator only: every filter below still
    // runs, and a file with no entry simply takes the per-file path (ADR 0033).
    let phase = timings::phase("stat index resolve");
    let fast = FastPath::new(
        &cache,
        Namespace::Fmt,
        &plan,
        &|id| u8::from(act_on_generated[id]) | u8::from(opts.fix_generated) << 1,
        &size_limits,
        Replay {
            diagnostics: false,
            verdicts: !collect_debug,
        },
    );
    let resolutions = resolve_all(fast.as_ref(), &files);
    drop(phase);
    let phase = timings::phase("batch prefill");
    let prefilled = format_prefill(&files, &resolutions, &plan, &cache, &size_limits);
    drop(phase);
    let ctx = FormatCtx {
        plan: &plan,
        cache: &cache,
        write,
        fix_generated: opts.fix_generated,
        collect_debug,
        act_on_generated: &act_on_generated,
        limits: &size_limits,
        prefilled: &prefilled,
        fast: fast.as_ref(),
    };
    let phase = timings::phase("per-file loop");
    let (results, errors) = format_files(&ctx, &files, resolutions);
    drop(phase);
    let phase = timings::phase("cache flush");
    flush_stat_indexes(&cache);
    drop(phase);
    log_fast_path(fast.as_ref(), "format");
    let env = RunEnv {
        paths,
        files: &files,
        configs: &configs,
        opts,
        plan: &plan,
        cache: &cache,
        selection,
    };
    Ok(assemble_format_run(&env, results, errors, discovery))
}

/// Run the batch-capable formatters over the files the stat index did not answer.
fn format_prefill(
    files: &[DiscoveredFile],
    resolutions: &[Resolution],
    plan: &RunPlan,
    cache: &ResultCache,
    limits: &SizeLimits,
) -> batch::BatchPrefill {
    let unresolved: Vec<DiscoveredFile> = files
        .iter()
        .zip(resolutions)
        .filter(|(_, resolution)| !matches!(resolution, Resolution::Hit(_)))
        .map(|(f, _)| f.clone())
        .collect();
    let prefilled = prefill(&unresolved, plan, cache, limits);
    tracing::debug!(
        prefilled = prefilled.len(),
        files = files.len(),
        "batch prefill complete"
    );
    prefilled
}

/// Format every file in parallel. An engine error is carried, not swallowed:
/// dropping the file here is what let `poly fmt --check` report success on a
/// file it could not parse.
fn format_files<'f>(
    ctx: &FormatCtx<'_, 'f>,
    files: &[DiscoveredFile],
    resolutions: Vec<Resolution<'f>>,
) -> (Vec<FormatResult>, Vec<FormatError>) {
    let (oks, errs): (Vec<_>, Vec<_>) = files
        .par_iter()
        .zip(resolutions)
        .map(|(f, resolution)| {
            format_one(ctx, f, resolution).map_err(|error| FormatError {
                path: f.path.clone(),
                message: format!("{error:#}"),
            })
        })
        .partition(Result::is_ok);
    (
        oks.into_iter().map(Result::unwrap).collect(),
        errs.into_iter().map(Result::unwrap_err).collect(),
    )
}

fn assemble_format_run(
    env: &RunEnv,
    mut results: Vec<FormatResult>,
    mut errors: Vec<FormatError>,
    discovery: DiscoveryReport,
) -> FormatRun {
    results.sort_by(|a, b| a.path.cmp(&b.path));
    errors.sort_by(|a, b| a.path.cmp(&b.path));
    for error in &errors {
        tracing::debug!(path = %error.path.display(), "format failed: {}", error.message);
    }
    // Declined files first (they are already sorted by path), then the paths the
    // caller named that no engine covers — one list, so a strict-mode check and
    // the summary do not have to know which kind they are looking at.
    let mut skipped: Vec<SkippedFile> = results
        .iter()
        .filter_map(|result| {
            result.skipped.as_ref().map(|reason| SkippedFile {
                path: result.path.clone(),
                reason: reason.clone(),
            })
        })
        .collect();
    add_unmatched_paths(env, &mut skipped);
    // Mirrors the lint side: a file every backend declined was read, not
    // inspected, so it is not part of what the run claims to have formatted.
    let checked = results.iter().filter(|result| result.skipped.is_none()).count();
    let (config_fingerprints, config_index) = config_fingerprints(env.configs, env.opts);
    for result in &mut results {
        result.config = config_index[result.config];
    }
    FormatRun {
        results,
        checked,
        configs: config_fingerprints,
        errors,
        skipped,
        discovery,
        components: env.plan.components().clone(),
    }
}

fn digest_if_enabled<T>(cache: &ResultCache, compute: impl FnOnce() -> T) -> Option<T> {
    cache.enabled().then(compute)
}

fn should_cache_result(elapsed: std::time::Duration) -> bool {
    elapsed >= MIN_CACHE_DURATION
}

/// Drive `run_pass` (one full format-engine chain over the content) to a fixed
/// point: re-run it until the output stops changing, bounded by
/// [`MAX_FORMAT_PASSES`] so a genuinely oscillating backend still terminates.
///
/// The first pass receives `record_debug == true`; every retry receives `false`
/// so per-engine debug counts reflect a single logical run. A run that is already
/// stable costs exactly one pass; each additional pass only happens when the
/// previous one changed the content (so `poly fmt --fix` is a fixed point and a
/// following `poly fmt --check` is clean).
///
/// Returns the content and **whether it actually settled**. Exhausting the cap
/// is not a detail to swallow: it means the file was left still changing, so the
/// `--fix`-then-`--check` guarantee above does not hold for it, and reporting
/// the file as formatted would be claiming something the run did not do.
fn format_to_fixed_point<F>(initial: Arc<str>, mut run_pass: F) -> anyhow::Result<(Arc<str>, bool)>
where
    F: FnMut(&Arc<str>, bool) -> anyhow::Result<Arc<str>>,
{
    let mut current = initial;
    for pass in 0..MAX_FORMAT_PASSES {
        let next = run_pass(&current, pass == 0)?;
        let stable = *next == *current;
        current = next;
        if stable {
            return Ok((current, true));
        }
    }
    Ok((current, false))
}

/// Stack size for rayon worker threads, in bytes (16 MiB).
///
/// rayon workers default to Rust's spawned-thread stack of 2 MiB, but the
/// per-file engines run recursive-descent parsers/formatters (oxc, mago,
/// markup_fmt, the tree-sitter reindent) whose recursion depth tracks source
/// nesting. On real-world files that 2 MiB is not enough and a worker overflows
/// its stack — an uncatchable abort that takes down the whole run. The process
/// main thread already gets 8 MiB (which is why single-file, inline runs never
/// crashed); we give workers a generous 16 MiB so a deeply nested file degrades
/// to a normal result instead of aborting.
const WORKER_STACK_SIZE: usize = 16 * 1024 * 1024;

static POOL_JOBS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

fn configure_pool(jobs: Option<usize>) {
    static INIT: Once = Once::new();
    static WARNED: Once = Once::new();
    if INIT.is_completed() && jobs.unwrap_or(0) != POOL_JOBS.load(std::sync::atomic::Ordering::Relaxed) {
        WARNED.call_once(|| tracing::debug!(?jobs, "the worker pool is already sized; `jobs` is ignored"));
    }
    INIT.call_once(|| {
        POOL_JOBS.store(jobs.unwrap_or(0), std::sync::atomic::Ordering::Relaxed);
        let mut builder = rayon::ThreadPoolBuilder::new().stack_size(WORKER_STACK_SIZE);
        if let Some(n) = jobs
            && n > 0
        {
            builder = builder.num_threads(n);
        }
        let _ = builder.build_global();
    });
}

// These tests lint real Python/JS/... sources, so they need every engine family.
#[cfg(all(test, feature = "all-engines"))]
mod tests;
