//! The optional facets of an [`Engine`]: concerns only some backends have, each its own trait
//! reached through an accessor on [`Engine`] that defaults to `None` (ADR 0072).
//!
//! A backend implements a facet only when it has the concern, and returns `Some(self)` from the
//! facet's accessor. Callers holding a `dyn Engine` never reach for the accessor themselves: they
//! ask the facet's question through the inherent method of the same name on `dyn Engine` below,
//! which answers with the backend's facet when it has one and with the facet's default otherwise,
//! so each default is written once.
//!
//! None of these questions is asked per file per engine on the lint or format path: they are asked
//! while planning, once per project, or while grouping files for a batch. For a backend without
//! the facet, the accessor's `None` replaces the default method call it had before, one call for
//! one call.

use std::path::PathBuf;

use super::{
    BatchSupport, Diagnostic, Engine, FormatOutput, ProjectInputs, ProjectReport, ProjectRequest, SourceFile,
    WireSelection,
};
use crate::config::EngineConfig;
use crate::language::Language;

/// [`Batching::batch_shard_floor`] for a backend that does not raise it.
const DEFAULT_BATCH_SHARD_FLOOR: usize = 2;

/// The error a batch call answers for a backend that does not format in batches.
fn no_format_batch() -> anyhow::Error {
    anyhow::anyhow!("this backend does not implement batch formatting")
}

/// The error a batch call answers for a backend that does not lint in batches.
fn no_lint_batch() -> anyhow::Error {
    anyhow::anyhow!("this backend does not implement batch linting")
}

/// [`PlanPolicy::provides_language_lint`]'s default: a backend registered for languages lints them.
fn default_provides_language_lint<E: Engine + ?Sized>(engine: &E) -> bool {
    !engine.languages().is_empty()
}

/// [`PlanPolicy::provides_language_format`]'s default: the declared capability.
fn default_provides_language_format<E: Engine + ?Sized>(engine: &E) -> bool {
    engine.capabilities().format
}

/// One call over a whole batch of files ([`Engine::batching`]).
///
/// Batching exists for a backend whose per-file cost is dominated by process start-up; see
/// [`BatchSupport`]. An in-process engine gains nothing from it, which is why no tier-1 backend
/// has this facet.
pub trait Batching: Engine {
    /// Which operations this backend can perform over a whole batch at once,
    /// given the config governing this file.
    ///
    /// Defaults to none, so a backend that has the facet but does not opt in under `cfg` keeps the
    /// per-file path untouched.
    ///
    /// `cfg` is passed because batching is both a capability *and* a policy:
    /// whether a tool can rewrite named files is static, but whether doing so
    /// is faster than spawning per file depends on the environment, so it is
    /// opt-in. A backend answering `true` here is claiming both.
    fn batch_support(&self, _cfg: &EngineConfig) -> BatchSupport {
        BatchSupport::default()
    }

    /// Format every member of `batch` in one call.
    ///
    /// # Contract
    ///
    /// The returned vector **must** have the same length as `batch`, and
    /// element `i` **must** describe `batch[i]`. A caller is entitled to treat a
    /// length mismatch as a failure of the batch mechanism rather than as a
    /// short answer, because the alternative — trusting a ragged vector — is
    /// what would attribute one file's formatting to another.
    ///
    /// The outer `Err` means *the batch as a whole failed* (the spawn failed,
    /// the output was unusable). It is never an error for the files involved:
    /// the caller re-runs them through the per-file path, so a batch failure
    /// costs time and not correctness. An inner `Err` is a genuine per-file
    /// failure and is attributed to that one file.
    ///
    /// Implementations receive **content**, never a list of paths to go and
    /// read. `SourceFile::path` travels for naming and config discovery only;
    /// the bytes handed over are authoritative, and may not correspond to
    /// what is currently on disk at that path.
    fn format_batch(
        &self,
        _batch: &[SourceFile],
        _cfg: &EngineConfig,
    ) -> anyhow::Result<Vec<anyhow::Result<FormatOutput>>> {
        Err(no_format_batch())
    }

    /// Lint every member of `batch` in one call.
    ///
    /// The contract is [`Batching::format_batch`]'s: one answer per input in
    /// order, an outer `Err` for a batch that cannot be trusted, and an inner
    /// `Err` for a member to be linted on its own. The answer for a member must
    /// be exactly what [`Engine::lint`] would have returned for it.
    fn lint_batch(
        &self,
        _batch: &[SourceFile],
        _cfg: &EngineConfig,
    ) -> anyhow::Result<Vec<anyhow::Result<Vec<Diagnostic>>>> {
        Err(no_lint_batch())
    }

    /// Fewest files worth a separate [`Batching::lint_batch`] / [`Batching::format_batch`]
    /// call when the runner shards a group across the thread pool.
    ///
    /// The runner splits a group into up to one shard per core. That is right
    /// for a tool with a small start-up and wrong for a JVM tool, where each
    /// extra shard buys another second of start-up and nothing else; such a
    /// backend raises the floor so a small group stays one invocation.
    fn batch_shard_floor(&self) -> usize {
        DEFAULT_BATCH_SHARD_FLOOR
    }
}

/// Whole-project analysis ([`Engine::project_analysis`]), run once per project after the per-file
/// tier, behind [`Capabilities::project`](super::Capabilities::project).
pub trait ProjectAnalysis: Engine {
    /// Whole-project analysis, run once per project after the per-file tier. Only called when
    /// [`Capabilities::project`](super::Capabilities::project) is set. Returns findings grouped by
    /// the file they belong to; a file need not be among `req.files`. Defaults to no findings.
    ///
    /// Must be a pure function of the request: it may read nothing but `req` (the host already
    /// loaded file contents and resolved the toolchain in `req.environment`), and must never spawn
    /// a process, so the result can be cached on the request's inputs.
    fn project_check(&self, _req: &ProjectRequest<'_>) -> anyhow::Result<Vec<(PathBuf, Vec<Diagnostic>)>> {
        Ok(Vec::new())
    }

    /// [`ProjectAnalysis::project_check`] with a channel for declining the project (ADR 0079); this is what the
    /// host calls. Defaults to the findings of [`ProjectAnalysis::project_check`].
    ///
    /// An engine that can find nothing to check in a project (tsrs without a `tsconfig.json`) overrides this to
    /// answer [`ProjectReport::Declined`], and answers [`ProjectAnalysis::project_check`] with
    /// [`ProjectReport::into_findings`]. The same purity rules apply.
    fn project_report(&self, req: &ProjectRequest<'_>) -> anyhow::Result<ProjectReport> {
        self.project_check(req).map(ProjectReport::Checked)
    }

    /// What [`ProjectAnalysis::project_check`] needs from the host besides the files it serves: a
    /// gating option, side files and the directories it is about. Defaults to nothing.
    fn project_inputs(&self) -> ProjectInputs {
        ProjectInputs::default()
    }

    /// How a config table picks this engine among alternatives sharing it (ty and pyrefly share
    /// `[engines.python-types]`, told apart by its `backend`). `None` for an engine nothing chooses between.
    fn selection(&self) -> Option<WireSelection> {
        None
    }
}

/// How the run planner counts, enables and orders an engine ([`Engine::plan_policy`]).
///
/// Every answer is resolved once per (config, language) plan, never per file.
pub trait PlanPolicy: Engine {
    /// Whether this backend carries lint rules **for the language of the files
    /// routed to it**, under `cfg`.
    ///
    /// Consulted only on a lint plan, and only to answer one question: did
    /// anything in this run actually know how to lint this file? A `.kt` file
    /// routes to the cross-cutting backends (spell-check, ast-grep, comment
    /// removal) and to nothing that knows Kotlin, yet it was counted in
    /// `N file(s) linted` exactly like a `.py` file ruff had examined. A
    /// consumer with Kotlin, Swift and Zig in the tree read a green
    /// `poly lint .` as full coverage of languages poly has no rules for —
    /// which is the one thing this project promises never to do.
    ///
    /// The default answers from [`Engine::languages`]: a backend registered for
    /// specific languages lints those languages, and one that declares none is
    /// cross-cutting (it applies to any file, so it never establishes coverage
    /// of a *language*). Backends whose answer depends on the host or the
    /// config — a native tool that must be installed, a rule engine that needs
    /// rules — override this; a `true` here is a claim the run relies on, so it
    /// must not be made speculatively.
    ///
    /// `language` is the language being planned, since a cross-cutting backend
    /// can still hold rules for one specific language.
    fn provides_language_lint(&self, _language: &Language, _cfg: &EngineConfig) -> bool {
        default_provides_language_lint(self)
    }

    /// Whether this backend would actually **format** `language` under `cfg`.
    ///
    /// The format-side counterpart of [`PlanPolicy::provides_language_lint`], and it
    /// exists for the same reason: an opt-in native tool that is switched off or
    /// missing from `PATH` formats nothing, and a planner that counts it as a
    /// working formatter withdraws the catalog tier on its behalf — leaving the
    /// language with no formatter at all while the config reads as though one
    /// were configured.
    ///
    /// Defaults to the declared capability. Backends whose answer depends on the
    /// host or the config override it; as with the lint side, a `true` here is a
    /// claim the run relies on and must not be made speculatively.
    fn provides_language_format(&self, _language: &Language, _cfg: &EngineConfig) -> bool {
        default_provides_language_format(self)
    }

    /// The host tool this backend was meant to run for `language` under `cfg` and could not find,
    /// or `None` when it found it, was switched off, or runs no host tool.
    ///
    /// A native tool that is wanted but absent hands its files to a lower-fidelity path (the
    /// tier-2 reindenter, ADR 0014) or lints nothing, and the run still reads as a pass. The
    /// planner asks this once per plan so the report can name the files that path served
    /// (ADR 0075); asking the engine call instead would lose the answer on a cache hit.
    fn missing_tool(&self, _language: &Language, _cfg: &EngineConfig) -> Option<&'static str> {
        None
    }

    /// Whether this backend reads [`ENABLED_OPTION_KEY`](super::ENABLED_OPTION_KEY) itself and
    /// degrades gracefully when it is `false`, rather than expecting the runner to drop it from
    /// the plan.
    ///
    /// The runner honours `enabled = false` by removing an engine before the
    /// file loop, which is right for a backend whose absence simply means one
    /// less check. It is wrong for a backend that *is* its language's only
    /// registry slot and hands the work to the tier-2 reindenter when switched
    /// off: removing it there leaves the language with no formatter at all, so
    /// one config key silently drops every file of that language from the run.
    ///
    /// A backend answering `true` takes on the whole contract — it must read the
    /// key in every method that acts, and must do something defensible with a
    /// `false`.
    fn self_manages_enabled(&self) -> bool {
        false
    }

    /// Whether this backend is the authoritative formatter for its language and
    /// should displace poly's generic tree-sitter reindenter.
    ///
    /// Formatters chain, so a configured external formatter running *alongside*
    /// the generic reindenter makes the two fight over indentation and prevents
    /// the format loop from converging. Only backends that are both configured
    /// and actually runnable should answer `true` — otherwise a missing binary
    /// would leave the language with no formatter at all.
    fn supersedes_generic_formatter(&self) -> bool {
        false
    }
}

/// How a catalog backend displaces or joins the language's tier-one formatter
/// ([`Engine::catalog_takeover`]).
pub trait CatalogTakeover: Engine {
    /// Whether this catalog backend, enabled by name and covering the whole
    /// language, replaces the language's tier-one formatter.
    fn takes_over_tier_one(&self) -> bool {
        false
    }

    /// Glob patterns of the files this backend formats in place of the language's
    /// tier-one formatter (see [`CatalogTakeover::supplements_tier_one`]).
    fn claimed_paths(&self) -> Vec<String> {
        Vec::new()
    }

    /// Whether this catalog backend, confined to named files, may run in
    /// addition to the language's tier-one formatter.
    fn supplements_tier_one(&self) -> bool {
        false
    }
}

/// Each facet's questions asked of any engine: the engine's facet answers when it has one, the
/// facet's default otherwise.
impl dyn Engine + '_ {
    /// [`Batching::batch_support`], or no batching for an engine without the facet.
    pub fn batch_support(&self, cfg: &EngineConfig) -> BatchSupport {
        self.batching()
            .map_or_else(BatchSupport::default, |facet| facet.batch_support(cfg))
    }

    /// [`Batching::format_batch`], or a batch failure for an engine without the facet.
    ///
    /// # Errors
    ///
    /// When the batch as a whole failed, or the engine does not format in batches.
    pub fn format_batch(
        &self,
        batch: &[SourceFile],
        cfg: &EngineConfig,
    ) -> anyhow::Result<Vec<anyhow::Result<FormatOutput>>> {
        match self.batching() {
            Some(facet) => facet.format_batch(batch, cfg),
            None => Err(no_format_batch()),
        }
    }

    /// [`Batching::lint_batch`], or a batch failure for an engine without the facet.
    ///
    /// # Errors
    ///
    /// When the batch as a whole failed, or the engine does not lint in batches.
    pub fn lint_batch(
        &self,
        batch: &[SourceFile],
        cfg: &EngineConfig,
    ) -> anyhow::Result<Vec<anyhow::Result<Vec<Diagnostic>>>> {
        match self.batching() {
            Some(facet) => facet.lint_batch(batch, cfg),
            None => Err(no_lint_batch()),
        }
    }

    /// [`Batching::batch_shard_floor`], or its default for an engine without the facet.
    pub fn batch_shard_floor(&self) -> usize {
        self.batching()
            .map_or(DEFAULT_BATCH_SHARD_FLOOR, |facet| facet.batch_shard_floor())
    }

    /// [`ProjectAnalysis::project_check`], or no findings for an engine without the facet.
    ///
    /// # Errors
    ///
    /// When the engine's analysis fails.
    pub fn project_check(&self, req: &ProjectRequest<'_>) -> anyhow::Result<Vec<(PathBuf, Vec<Diagnostic>)>> {
        match self.project_analysis() {
            Some(facet) => facet.project_check(req),
            None => Ok(Vec::new()),
        }
    }

    /// [`ProjectAnalysis::project_report`], or a clean report for an engine without the facet.
    ///
    /// # Errors
    ///
    /// When the engine's analysis fails.
    pub fn project_report(&self, req: &ProjectRequest<'_>) -> anyhow::Result<ProjectReport> {
        match self.project_analysis() {
            Some(facet) => facet.project_report(req),
            None => Ok(ProjectReport::Checked(Vec::new())),
        }
    }

    /// [`ProjectAnalysis::project_inputs`], or none for an engine without the facet.
    pub fn project_inputs(&self) -> ProjectInputs {
        self.project_analysis()
            .map_or_else(ProjectInputs::default, |facet| facet.project_inputs())
    }

    /// [`ProjectAnalysis::selection`], or `None` for an engine without the facet.
    pub fn selection(&self) -> Option<WireSelection> {
        self.project_analysis().and_then(|facet| facet.selection())
    }

    /// [`PlanPolicy::provides_language_lint`], or its default for an engine without the facet.
    pub fn provides_language_lint(&self, language: &Language, cfg: &EngineConfig) -> bool {
        match self.plan_policy() {
            Some(facet) => facet.provides_language_lint(language, cfg),
            None => default_provides_language_lint(self),
        }
    }

    /// [`PlanPolicy::provides_language_format`], or its default for an engine without the facet.
    pub fn provides_language_format(&self, language: &Language, cfg: &EngineConfig) -> bool {
        match self.plan_policy() {
            Some(facet) => facet.provides_language_format(language, cfg),
            None => default_provides_language_format(self),
        }
    }

    /// [`PlanPolicy::missing_tool`], or `None` for an engine without the facet.
    pub fn missing_tool(&self, language: &Language, cfg: &EngineConfig) -> Option<&'static str> {
        self.plan_policy().and_then(|facet| facet.missing_tool(language, cfg))
    }

    /// [`PlanPolicy::self_manages_enabled`], or `false` for an engine without the facet.
    pub fn self_manages_enabled(&self) -> bool {
        self.plan_policy().is_some_and(|facet| facet.self_manages_enabled())
    }

    /// [`PlanPolicy::supersedes_generic_formatter`], or `false` for an engine without the facet.
    pub fn supersedes_generic_formatter(&self) -> bool {
        self.plan_policy()
            .is_some_and(|facet| facet.supersedes_generic_formatter())
    }

    /// [`CatalogTakeover::takes_over_tier_one`], or `false` for an engine without the facet.
    pub fn takes_over_tier_one(&self) -> bool {
        self.catalog_takeover().is_some_and(|facet| facet.takes_over_tier_one())
    }

    /// [`CatalogTakeover::claimed_paths`], or none for an engine without the facet.
    pub fn claimed_paths(&self) -> Vec<String> {
        self.catalog_takeover()
            .map_or_else(Vec::new, |facet| facet.claimed_paths())
    }

    /// [`CatalogTakeover::supplements_tier_one`], or `false` for an engine without the facet.
    pub fn supplements_tier_one(&self) -> bool {
        self.catalog_takeover()
            .is_some_and(|facet| facet.supplements_tier_one())
    }
}
