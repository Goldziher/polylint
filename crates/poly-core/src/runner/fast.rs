//! The stat-validated fast path and the per-file record that backs it.
//!
//! [`FastPath::resolve`] runs before a file is opened. It answers from the
//! [`StatIndex`] when the file's `stat`, the run's scope and every engine's key
//! equal what was recorded, and falls back to a content digest when only the
//! `stat` moved (a `touch`, a checkout that rewrote identical bytes). Anything
//! else is a [`Resolution::Miss`] carrying a [`Probe`]: the slow path reads the
//! file, reuses whichever recorded engine segments are still valid for its
//! bytes, and records the outcome against the `stat` taken **before** it read.
//!
//! What goes into the keys is the whole correctness argument. A **scope** key
//! folds in the cache format, the build identity, the run switches that change
//! a verdict without changing an engine (`[discovery] generated`,
//! `--fix-generated`) and the withdrawal reason if an instruction emptied the
//! plan. An **engine** key folds in the engine's name, `version()` and
//! serialized args — which already carry the config's options and the engine's
//! [`run_context`] — and, per file, its [`file_context`] and, for a lint run of an
//! engine that declares the hook, its [`lint_cache_salt`]. The salt is computed
//! from the file's content, so a file whose engine declares it is read even when
//! its `stat` is unchanged; every other engine's key costs no read.
//!
//! [`run_context`]: crate::engine::Engine::run_context
//! [`file_context`]: crate::engine::Engine::file_context
//! [`lint_cache_salt`]: crate::engine::Engine::lint_cache_salt

use poly_cache::{
    ContentDigest, FileStat, Namespace, PathKey, PlanKey, Record, ResultCache, Segment, StatIndex, Verdict,
};
use poly_engine_api::desc::hooks;
use rustc_hash::FxHashMap;
use std::cell::OnceCell;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::discover::DiscoveredFile;
use crate::engine::{Diagnostic, SourceFile};
use crate::language::Language;

use super::plan::{EnginePlan, RunPlan, provides_language_lint};
use super::skips::{SizeLimits, SkippedFile, Withdrawal};
use super::types::LintResult;
use crate::filter::suppressed::SuppressedDiagnostic;

/// How the run's files were resolved, reported once at the end so a test (or a
/// reader with `RUST_LOG=poly_core=debug`) can see whether the fast path worked.
#[derive(Default)]
struct Tally {
    stat_hits: AtomicUsize,
    content_hits: AtomicUsize,
    misses: AtomicUsize,
}

/// What let a recorded verdict stand in for a lint pass.
enum VouchedBy {
    Stat,
    Content,
}

pub(super) struct FastPath<'a> {
    cache: &'a ResultCache,
    index: &'a StatIndex,
    tally: Tally,
    /// Per config id, each planned language's scope and engines. Nested so a
    /// file is looked up by borrowed `&Language`: a `(usize, Language)` key
    /// would clone the language, and so allocate for `Language::Other`, per file.
    slots: Vec<FxHashMap<Language, Slot<'a>>>,
    limits: &'a SizeLimits,
    replay: Replay,
}

/// What is fixed for one `(config, language)` pair across the run.
struct Slot<'a> {
    scope: PlanKey,
    engines: &'a [EnginePlan],
    bases: Vec<PlanKey>,
    /// Per engine, whether its key folds in [`Engine::lint_cache_salt`](crate::engine::Engine::lint_cache_salt):
    /// a lint run of an engine that declares the hook.
    salted: Vec<bool>,
}

/// Tags the lint cache salt within an engine key, so it can never stand in for a file context.
const LINT_CACHE_SALT_PART: &str = "lint_cache_salt";

/// The file being resolved, read at most once and only when something needs its bytes: a salted
/// engine's key or the content check of a touched file.
struct FileContent<'f> {
    file: &'f DiscoveredFile,
    bytes: OnceCell<Option<Vec<u8>>>,
    source: OnceCell<Option<SourceFile>>,
}

impl<'f> FileContent<'f> {
    fn new(file: &'f DiscoveredFile) -> Self {
        Self {
            file,
            bytes: OnceCell::new(),
            source: OnceCell::new(),
        }
    }

    fn bytes(&self) -> Option<&[u8]> {
        self.bytes
            .get_or_init(|| std::fs::read(&self.file.path).ok())
            .as_deref()
    }

    /// The file as an engine sees it; `None` when it cannot be read as UTF-8 text, which the
    /// per-file path reports or skips on its own.
    fn source(&self) -> Option<&SourceFile> {
        self.source
            .get_or_init(|| {
                let text = std::str::from_utf8(self.bytes()?).ok()?;
                Some(SourceFile {
                    path: self.file.path.clone(),
                    language: self.file.language.clone(),
                    content: Arc::from(text),
                })
            })
            .as_ref()
    }
}

thread_local! {
    /// Scratch for the per-file engine keys, so a stat hit allocates none.
    static KEY_SCRATCH: std::cell::RefCell<Vec<PlanKey>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// What a stat hit may replay without opening the file.
#[derive(Clone, Copy)]
pub(super) struct Replay {
    /// Reported findings: not while fixing (the content is needed to apply
    /// them) and not while collecting per-engine debug.
    pub(super) diagnostics: bool,
    /// Any verdict at all: not while collecting debug, which reports per-engine
    /// timing a replay never produced.
    pub(super) verdicts: bool,
}

/// A file the index could not vouch for, with what is needed to record it.
pub(super) struct Probe<'a> {
    path: PathKey,
    stat: FileStat,
    scope: PlanKey,
    keys: Vec<PlanKey>,
    /// The path's record whatever plan wrote it: its segments are reused by engine key, and kept
    /// beside this run's when it is recorded, so a narrowed run (`--only`) evicts nothing.
    prior: Option<&'a Record>,
}

/// A record that answers for a file, and where each planned engine's segment sits in it.
pub(super) struct Hit<'a> {
    record: &'a Record,
    /// Per planned engine, the index of its segment, when the record was written by another plan
    /// (a narrowed run's record holding the full run's segments, or the reverse); `None` when the
    /// record's own plan was this one, so its leading segments are this plan's in order.
    order: Option<Vec<usize>>,
}

impl std::ops::Deref for Hit<'_> {
    type Target = Record;

    fn deref(&self) -> &Record {
        self.record
    }
}

impl Hit<'_> {
    /// The planned engines' segments, in plan order.
    fn segments(&self) -> impl Iterator<Item = &Segment> {
        let positional = self.order.is_none().then(|| self.record.segments.iter());
        let mapped = self
            .order
            .iter()
            .flat_map(|order| order.iter().map(|&index| &self.record.segments[index]));
        positional.into_iter().flatten().chain(mapped)
    }
}

pub(super) enum Resolution<'a> {
    Hit(Hit<'a>),
    Miss(Probe<'a>),
    /// The file could not be stat-ed; the slow path will report why.
    Unavailable,
}

impl<'a> FastPath<'a> {
    /// A fast path for `namespace`, or `None` when the cache is disabled.
    ///
    /// `flags(config_id)` supplies the per-config run switches that change a
    /// verdict without changing any engine.
    pub(super) fn new(
        cache: &'a ResultCache,
        namespace: Namespace,
        plan: &'a RunPlan,
        flags: &dyn Fn(usize) -> u8,
        limits: &'a SizeLimits,
        replay: Replay,
    ) -> Option<Self> {
        if !cache.enabled() {
            return None;
        }
        let mut slots: Vec<FxHashMap<Language, Slot<'a>>> = Vec::new();
        for ((config_id, language), engines) in plan.iter() {
            let mut scope = PlanKey::builder(namespace);
            scope.part([flags(*config_id)]);
            scope.part(language.id());
            scope.part(
                plan.withdrawal(*config_id, language)
                    .map(|withdrawal| withdrawal.reason())
                    .unwrap_or_default(),
            );
            if slots.len() <= *config_id {
                slots.resize_with(*config_id + 1, FxHashMap::default);
            }
            slots[*config_id].insert(
                language.clone(),
                Slot {
                    scope: scope.finish(),
                    engines: engines.as_slice(),
                    bases: engines
                        .iter()
                        .map(|engine| base_key(namespace, language, engine))
                        .collect(),
                    salted: engines.iter().map(|engine| salted(namespace, engine)).collect(),
                },
            );
        }
        Some(Self {
            cache,
            index: cache.stat_index(namespace),
            tally: Tally::default(),
            slots,
            limits,
            replay,
        })
    }

    fn slot(&self, file: &DiscoveredFile) -> Option<&Slot<'a>> {
        self.slots.get(file.config_id)?.get(&file.language)
    }

    /// Fill `out` with each engine's key for `file`: the run-wide base, extended
    /// by the engine's per-file context when it has one and, for a salted engine,
    /// by its lint cache salt.
    ///
    /// The one place a key is made: the index lookup and the record the per-file
    /// path writes both take their keys from here, so they cannot disagree.
    fn engine_keys(slot: &Slot<'_>, file: &DiscoveredFile, content: &FileContent, out: &mut Vec<PlanKey>) {
        out.clear();
        out.extend(
            slot.bases
                .iter()
                .zip(slot.engines)
                .zip(&slot.salted)
                .map(|((base, engine), &salted)| {
                    let key = match engine.engine.file_context(&file.path, &engine.config) {
                        Some(context) => base.extended([context.as_str()]),
                        None => *base,
                    };
                    if !salted {
                        return key;
                    }
                    match content.source().and_then(|src| engine.engine.lint_cache_salt(src)) {
                        Some(salt) => key.extended([LINT_CACHE_SALT_PART, salt.as_str()]),
                        None => key,
                    }
                }),
        );
    }

    /// The stat and keys for `file`, without consulting the index.
    pub(super) fn probe(&self, file: &DiscoveredFile) -> Option<Probe<'a>> {
        let stat = self.cache.file_stat(&file.path).ok()?;
        let slot = self.slot(file)?;
        let mut keys = Vec::with_capacity(slot.bases.len());
        Self::engine_keys(slot, file, &FileContent::new(file), &mut keys);
        let path = self.cache.path_key(&file.path);
        let prior = self.index.get(&path);
        Some(Probe {
            path,
            stat,
            scope: slot.scope,
            keys,
            prior,
        })
    }

    /// Whether `hit` may stand in for a lint pass. A record written by another plan carries that
    /// plan's verdict, so this plan's is read from its own segments.
    fn may_replay(&self, hit: &Hit) -> bool {
        let verdict = match (&hit.order, hit.verdict) {
            (Some(_), Verdict::Clean | Verdict::Diagnostics) => {
                if hit.segments().all(|segment| segment.payload.is_empty()) {
                    Verdict::Clean
                } else {
                    Verdict::Diagnostics
                }
            }
            (_, verdict) => verdict,
        };
        self.replay.verdicts
            && match verdict {
                Verdict::Binary => true,
                Verdict::Clean => hit.replayable,
                Verdict::Diagnostics => hit.replayable && self.replay.diagnostics,
            }
    }

    pub(super) fn resolve(&self, file: &DiscoveredFile) -> Resolution<'a> {
        let Ok(stat) = self.cache.file_stat(&file.path) else {
            return Resolution::Unavailable;
        };
        let Some(slot) = self.slot(file) else {
            return Resolution::Unavailable;
        };
        // A recorded verdict predates the limit it would now exceed; the per-file path skips the file.
        if self
            .limits
            .skip_reason(file.config_id, Some(stat.size()), &file.path)
            .is_some()
        {
            return Resolution::Unavailable;
        }
        let path = self.cache.path_key(&file.path);
        let raw = self.index.get(&path);
        let prior = raw.filter(|record| record.scope == slot.scope);
        let content = FileContent::new(file);
        let vouched = KEY_SCRATCH.with_borrow_mut(|keys| {
            Self::engine_keys(slot, file, &content, keys);
            self.vouched_for(&content, path, &stat, prior, keys)
                .ok_or_else(|| keys.clone())
        });
        match vouched {
            Ok((hit, how)) => {
                let counter = match how {
                    VouchedBy::Stat => &self.tally.stat_hits,
                    VouchedBy::Content => &self.tally.content_hits,
                };
                counter.fetch_add(1, Ordering::Relaxed);
                Resolution::Hit(hit)
            }
            Err(keys) => {
                self.tally.misses.fetch_add(1, Ordering::Relaxed);
                Resolution::Miss(Probe {
                    path,
                    stat,
                    scope: slot.scope,
                    keys,
                    prior: raw,
                })
            }
        }
    }

    /// The record that lets `file` be answered without a lint pass, and which
    /// tally line to count it on.
    fn vouched_for(
        &self,
        content: &FileContent,
        path: PathKey,
        stat: &FileStat,
        prior: Option<&'a Record>,
        keys: &[PlanKey],
    ) -> Option<(Hit<'a>, VouchedBy)> {
        let record = prior?;
        let order = if record.has_keys(keys) {
            None
        } else {
            Some(
                keys.iter()
                    .map(|key| record.segments.iter().position(|segment| segment.key == *key))
                    .collect::<Option<Vec<usize>>>()?,
            )
        };
        let hit = Hit { record, order };
        if !self.may_replay(&hit) {
            return None;
        }
        if record.trusts(stat) {
            return Some((hit, VouchedBy::Stat));
        }
        if record.digest != ContentDigest::of(content.bytes()?) {
            return None;
        }
        self.index.vouch(path, *stat, record.digest);
        Some((hit, VouchedBy::Content))
    }

    /// Report how the run's files were resolved.
    pub(super) fn log_summary(&self, run: &str) {
        tracing::debug!(
            run,
            stat_hits = self.tally.stat_hits.load(Ordering::Relaxed),
            content_hits = self.tally.content_hits.load(Ordering::Relaxed),
            misses = self.tally.misses.load(Ordering::Relaxed),
            "stat index resolution"
        );
    }

    /// The diagnostics a recorded file produced, severity overrides applied, or
    /// `None` if a payload no longer decodes.
    pub(super) fn decode(&self, file: &DiscoveredFile, hit: &Hit) -> Option<Vec<Diagnostic>> {
        let engines = self.slot(file).map_or(&[][..], |slot| slot.engines);
        let mut all = Vec::new();
        for (engine, segment) in engines.iter().zip(hit.segments()) {
            let mut diagnostics = decode_payload(&segment.payload)?;
            if !engine.severity_remap.is_empty() {
                engine.severity_remap.apply(&mut diagnostics);
            }
            all.extend(diagnostics);
        }
        Some(all)
    }

    /// Remember how `probe`'s file, whose bytes are `bytes`, was judged.
    ///
    /// When the path's record already holds every segment this run produced for these bytes (a
    /// narrowed run over a file a full run recorded), it stays as it is, re-stamped with the new
    /// `stat`: replacing it would evict the engines this run left out. Otherwise the new record keeps
    /// the old one's other segments for the same bytes ([`Record::merged`]).
    pub(super) fn record(&self, probe: &Probe, bytes: &[u8], outcome: Outcome) {
        let digest = ContentDigest::of(bytes);
        if let Some(prior) = probe.prior
            && prior.digest == digest
            && (prior.verdict == Verdict::Binary) == (outcome.verdict == Verdict::Binary)
            && probe.keys.iter().all(|key| prior.segment(key).is_some())
        {
            self.index.vouch(probe.path, probe.stat, digest);
            return;
        }
        let segments = probe
            .keys
            .iter()
            .zip(outcome.payloads.into_iter().chain(std::iter::repeat_with(Vec::new)))
            .map(|(key, payload)| Segment { key: *key, payload })
            .collect();
        let record = Record {
            stat: probe.stat,
            scope: probe.scope,
            digest,
            verdict: outcome.verdict,
            replayable: outcome.replayable,
            segments,
        };
        self.index.put(probe.path, record.merged(probe.prior));
    }
}

/// What a slow-path run concluded about one file.
pub(super) struct Outcome {
    pub(super) verdict: Verdict,
    pub(super) replayable: bool,
    /// Raw per-engine output in plan order; empty for fmt, where only the
    /// verdict is remembered.
    pub(super) payloads: Vec<Vec<u8>>,
}

impl Outcome {
    pub(super) fn binary() -> Self {
        Self {
            verdict: Verdict::Binary,
            replayable: true,
            payloads: Vec::new(),
        }
    }
}

impl Probe<'_> {
    /// The file's size when it was probed.
    pub(super) fn size(&self) -> u64 {
        self.stat.size()
    }

    /// The recorded output of each engine that is still valid for `bytes`:
    /// the same bytes, produced under the same engine key, by whichever plan recorded it.
    pub(super) fn reusable(&self, bytes: &[u8]) -> Vec<Option<&[u8]>> {
        let Some(prior) = self.prior else {
            return Vec::new();
        };
        if prior.digest != ContentDigest::of(bytes) {
            return Vec::new();
        }
        self.keys
            .iter()
            .map(|key| prior.segment(key).map(|segment| segment.payload.as_slice()))
            .collect()
    }
}

/// Whether `engine`'s key in a `namespace` run folds in its lint cache salt: only in a lint run,
/// and only for an engine that declares the hook, since computing the salt reads the file.
fn salted(namespace: Namespace, engine: &EnginePlan) -> bool {
    namespace == Namespace::Lint && engine.engine.overridden_hooks() & hooks::LINT_CACHE_SALT != 0
}

/// The engine's key for files of `language`. The language is part of it because a segment is reused
/// by key across plans and scopes (see [`Probe::reusable`]), and one engine with one config can judge
/// the same bytes differently as another language.
fn base_key(namespace: Namespace, language: &Language, engine: &EnginePlan) -> PlanKey {
    let mut builder = PlanKey::builder(namespace);
    builder
        .part(language.id())
        .part(engine.engine.name())
        .part(engine.engine.version())
        .part(engine.serialized_args.as_str());
    builder.finish()
}

/// Decode one engine's recorded output; an empty payload is "nothing found".
pub(super) fn decode_payload(payload: &[u8]) -> Option<Vec<Diagnostic>> {
    if payload.is_empty() {
        return Some(Vec::new());
    }
    serde_json::from_slice(payload).ok()
}

/// Encode one engine's raw output for a record; "nothing found" is empty.
pub(super) fn encode_payload(diagnostics: &[Diagnostic]) -> Vec<u8> {
    if diagnostics.is_empty() {
        Vec::new()
    } else {
        serde_json::to_vec(diagnostics).unwrap_or_default()
    }
}

/// Persist the verdicts recorded this run. Failing to is never the run's failure:
/// the next run simply finds fewer records.
pub(super) fn flush_stat_indexes(cache: &ResultCache) {
    if let Err(error) = cache.flush_stat_indexes() {
        tracing::warn!("failed to store the stat index: {error:#}");
    }
}

/// Why a file that was read and found clean still does not count as linted, if
/// it does not.
pub(super) fn lint_skip_reason(plan: &RunPlan, f: &DiscoveredFile, externally_linted: &[Language]) -> Option<String> {
    let engine_plans = plan.engines(f.config_id, &f.language);
    // A language nothing holds rules for is not a clean file: the cross-cutting
    // backends still run and can still report findings, but no rule in this run
    // knows the language, so the file must not be counted as linted.
    //
    // `externally_linted` is the caller's declaration that another phase of the
    // *same* run does lint the language (`cargo clippy` over Rust). A linear
    // scan is the right shape here: the list holds one entry per whole-project
    // tool, so it is a handful of comparisons against a slice already in cache —
    // cheaper than the hash it would take to avoid them.
    //
    // A caller who narrowed the run with `--only` gets their own reason
    // instead: nothing linted the file, but "no lint rules for Rust" would
    // blame poly for a restriction the invocation asked for.
    //
    // A component that is absent by policy is different: its files are skipped whatever else
    // covers the language, since the linked `quality` and ast-grep tiers cover most of what a
    // component would have handled and would otherwise hide the missing coverage.
    if let Some(Withdrawal::ComponentMissing(reason)) = plan.withdrawal(f.config_id, &f.language) {
        return Some(reason.clone());
    }
    // Whatever the selection kept did run over the file and may have reported on it, so calling
    // the file skipped would contradict its own findings: the caller asked for exactly those engines.
    if matches!(plan.withdrawal(f.config_id, &f.language), Some(Withdrawal::Selection)) && !engine_plans.is_empty() {
        return None;
    }
    (!provides_language_lint(engine_plans) && !externally_linted.contains(&f.language)).then(|| {
        match plan.withdrawal(f.config_id, &f.language) {
            Some(withdrawal) => withdrawal.reason(),
            None if quality_models(engine_plans, &f.language) => SkippedFile::quality_only_reason(&f.language),
            None => SkippedFile::no_lint_rules_reason(&f.language),
        }
    })
}

/// Whether the `quality` tier ran a structural model over `language` in this plan, so an
/// otherwise uncovered file is "measured but not linted" rather than untouched.
#[cfg(feature = "engine-quality")]
fn quality_models(engine_plans: &[EnginePlan], language: &Language) -> bool {
    engine_plans.iter().any(|plan| {
        plan.engine.name() == "quality"
            && crate::engines::quality::coverage::models_language(
                language,
                &crate::engines::quality::settings::Settings::from_config(&plan.config),
            )
    })
}

/// Without the linked `quality` engine nothing in the plan can have measured the file.
#[cfg(not(feature = "engine-quality"))]
fn quality_models(_engine_plans: &[EnginePlan], _language: &Language) -> bool {
    false
}

/// The result for a file whose findings were replayed from its record.
pub(super) fn replayed_lint_result(
    f: &DiscoveredFile,
    diagnostics: Vec<Diagnostic>,
    suppressed: Vec<SuppressedDiagnostic>,
    skipped: Option<String>,
) -> LintResult {
    LintResult {
        path: f.path.clone(),
        config: f.config_id,
        diagnostics,
        suppressed,
        fix_withheld_generated: false,
        fixed: 0,
        skipped,
        error: None,
        debug: None,
    }
}

pub(super) fn log_fast_path(fast: Option<&FastPath>, run: &str) {
    if let Some(fast) = fast {
        fast.log_summary(run);
    }
}

#[cfg(test)]
mod tests;
