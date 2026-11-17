//! Synchronous engine operations behind the host services.
//!
//! These functions are deliberately free of any `rmcp`/`tokio` types: they run
//! the same `poly-core` pipeline the CLI runs and return **typed** results — the
//! `poly-core` *run* structs for lint/format (results **plus** the files the run
//! failed on or declined), and the report types in
//! [`poly_frontend_api::dto`] for cache/rules/config/workspace. The MCP
//! frontend reaches them through the host services in [`crate::Services`] and
//! calls those from a blocking task, so the synchronous, rayon-driven engine
//! never runs on a tokio worker thread.
//!
//! Config resolution is **network-free**: local `extends` bases resolve as usual, and a remote
//! **git** base resolves only through the resolver the host injects
//! ([`crate::Services::with_config_resolver`]). `poly mcp` injects one that reads the base from
//! the local source cache at its `poly.lock`-pinned revision and never fetches (ADR 0076), so a
//! server does not reach the network on a model's behalf; without a resolver a git base is an
//! error naming the CLI.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use poly_cache::ResultCache;
use poly_config::{BaseConfigResolver, LocalPathResolver, PolyConfig};
use poly_core::{Config, FormatRun, Kind, Language, LintRun, RunOptions};

use poly_config::effective::EffectiveConfig;
use poly_config::v2::{ComponentInstallPolicy, OnMissing};
use poly_frontend_api::dto::FailureKind;
use poly_frontend_api::dto::{
    CacheFreedReport, CacheStatsReport, ConfigProblem, PartialValidation, RulesReport, ValidateReport, ValidatedFile,
    WorkspaceReport, WorkspaceToolReport,
};
use poly_frontend_api::wire::{InstallMode, RunCall, RunRequest};

use crate::cache;
use crate::failure::RunFailure;
use crate::state_cache::{self, StateCache, ToolLockGuard};

static CONFIGS: StateCache<Config> = StateCache::new();
static POLY_CONFIGS: StateCache<PolyConfig> = StateCache::new();

/// Where a request's config resolution starts: the explicit file, else the working directory.
fn config_root(explicit: Option<&Path>) -> (PathBuf, bool) {
    match explicit {
        Some(path) => (path.to_path_buf(), true),
        None => (std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")), false),
    }
}

/// The `extends` resolver a host injects (see [`crate::Services::with_config_resolver`]).
pub type Resolver = Arc<dyn BaseConfigResolver>;

/// Resolve the `poly-core` run configuration the way the CLI does, but
/// network-free (see the module docs): load an explicit file when supplied,
/// otherwise discover `poly.toml` from the working directory.
///
/// The parse is reused across requests while the files it read are unchanged
/// (see [`crate::state_cache`]).
pub fn resolve_config(explicit: Option<&Path>) -> anyhow::Result<Arc<Config>> {
    resolve_config_with(explicit, None)
}

/// [`resolve_config`], resolving `extends` bases through `resolver` when the host supplied one.
pub fn resolve_config_with(explicit: Option<&Path>, resolver: Option<&Resolver>) -> anyhow::Result<Arc<Config>> {
    let (root, is_file) = config_root(explicit);
    let local = LocalPathResolver;
    let resolver: &dyn BaseConfigResolver = resolver.map_or(&local, |resolver| resolver.as_ref());
    CONFIGS.get_or_load(&root, is_file, || {
        if is_file {
            Config::load_file_with(&root, resolver)
        } else {
            Config::load_with(&root, resolver)
        }
    })
}

/// Pin the repository's `poly.lock` tools for the duration of a run, as the CLI's config resolver
/// does, so a run served here downloads the locked releases rather than resolving `latest`
/// afresh. The guard serializes against concurrent runs holding different pins.
fn tool_lock_scope(explicit: Option<&Path>) -> anyhow::Result<ToolLockGuard> {
    state_cache::enter_repo(&config_root(explicit).0)
}

/// Resolve the full parsed [`PolyConfig`] (needed by the whole-project phase and the cache
/// root), network-free — mirroring [`resolve_config`].
pub fn resolve_poly_config(explicit: Option<&Path>) -> anyhow::Result<Arc<PolyConfig>> {
    resolve_poly_config_with(explicit, None)
}

/// [`resolve_poly_config`], resolving `extends` bases through `resolver` when the host supplied one.
pub fn resolve_poly_config_with(
    explicit: Option<&Path>,
    resolver: Option<&Resolver>,
) -> anyhow::Result<Arc<PolyConfig>> {
    let (root, is_file) = config_root(explicit);
    let local = LocalPathResolver;
    let resolver: &dyn BaseConfigResolver = resolver.map_or(&local, |resolver| resolver.as_ref());
    POLY_CONFIGS.get_or_load(&root, is_file, || {
        if is_file {
            PolyConfig::load_file_with(&root, resolver)
        } else {
            PolyConfig::load_with(&root, resolver)
        }
    })
}

/// The request's paths, defaulting to the current directory, refused when any does not exist
/// (see [`crate::paths::missing_paths_message`]).
fn existing_paths(paths: &[String]) -> anyhow::Result<Vec<PathBuf>> {
    let resolved = crate::run::request_paths(paths);
    match crate::paths::missing_paths_message(&resolved) {
        None => Ok(resolved),
        Some(message) => Err(RunFailure::new(FailureKind::Paths, message).into()),
    }
}

/// Pin the repository's tool lock and load the run's config, classifying a failure of either as
/// a config failure.
fn load_for_run(config: Option<&str>, resolver: Option<&Resolver>) -> anyhow::Result<(ToolLockGuard, Arc<Config>)> {
    let explicit = config.map(Path::new);
    let guard = tool_lock_scope(explicit).map_err(|error| RunFailure::from_anyhow(FailureKind::Config, &error))?;
    let config = resolve_config_with(explicit, resolver)
        .map_err(|error| RunFailure::from_anyhow(FailureKind::Config, &error))?;
    Ok((guard, config))
}

/// The run overrides an `install` parameter stands for. `never` also makes a missing component a
/// skip rather than a failure, since the caller asked for no download and would otherwise get an
/// error for honouring it; `config` and `auto` leave `on_missing` to the config.
#[must_use]
pub fn install_policy(mode: InstallMode) -> (Option<ComponentInstallPolicy>, Option<OnMissing>) {
    match mode {
        InstallMode::Config => (None, None),
        InstallMode::Auto => (Some(ComponentInstallPolicy::Auto), None),
        InstallMode::Never => (Some(ComponentInstallPolicy::Never), Some(OnMissing::Skip)),
    }
}

/// What a run needs before it starts: the paths, the config and the options, in the order the
/// CLI prepares them, so both surfaces refuse the same requests for the same reasons.
fn prepare(
    request: &RunRequest,
    resolver: Option<&Resolver>,
) -> anyhow::Result<(ToolLockGuard, Vec<PathBuf>, Arc<Config>, RunOptions)> {
    crate::run::validate(request).map_err(|error| RunFailure::from_anyhow(FailureKind::Usage, &error))?;
    let paths = existing_paths(&request.paths)?;
    let (guard, config) = load_for_run(request.config.as_deref(), resolver)?;
    let options = crate::run::run_options(request, &config, resolver.cloned());
    Ok((guard, paths, config, options))
}

/// Lint what `call` names, returning the **whole run** — the per-file results plus the files
/// the run failed on and the ones it declined. With `call.apply`, available autofixes are
/// applied in place before the remaining diagnostics are collected.
///
/// Deliberately [`poly_core::lint_run`] rather than `poly_core::lint`: the latter returns only
/// the results and drops [`LintRun::errors`], which for an MCP caller — who has no exit code to
/// fall back on — turns a file poly failed to read into a clean-looking answer.
///
/// # Errors
///
/// A [`RunFailure`] when the run cannot start, else the pipeline's own error.
pub fn lint(call: &RunCall, resolver: Option<&Resolver>) -> anyhow::Result<LintRun> {
    lint_with(call, resolver, Vec::new())
}

/// [`lint`], declaring the languages another phase of the same run lints (the whole-project
/// phase), so the per-file tier does not report them as unlinted.
pub(crate) fn lint_with(
    call: &RunCall,
    resolver: Option<&Resolver>,
    externally_linted: Vec<Language>,
) -> anyhow::Result<LintRun> {
    let (_tool_lock, paths, config, mut options) = prepare(&call.request, resolver)?;
    options.externally_linted_languages = externally_linted;
    poly_core::lint_run(&paths, &config, &options, call.apply, call.request.debug)
}

/// Format what `call` names, returning the **whole run** (see [`lint`] for why the run and not
/// just the results). With `call.apply`, changed files are rewritten in place; otherwise this
/// is a dry run.
///
/// # Errors
///
/// As [`lint`].
pub fn format(call: &RunCall, resolver: Option<&Resolver>) -> anyhow::Result<FormatRun> {
    let (_tool_lock, paths, config, options) = prepare(&call.request, resolver)?;
    poly_core::format_run(&paths, &config, &options, call.apply, call.request.debug)
}

fn call_for(paths: &[String], exclude: &[String], config: Option<&str>, apply: bool) -> RunCall {
    RunCall {
        request: RunRequest {
            paths: paths.to_vec(),
            exclude: exclude.to_vec(),
            config: config.map(str::to_owned),
            ..RunRequest::default()
        },
        apply,
        workspace: false,
    }
}

/// [`lint`] over `paths` with every other option at its default.
///
/// # Errors
///
/// As [`lint`].
pub fn lint_run(paths: &[String], exclude: &[String], config: Option<&str>, fix: bool) -> anyhow::Result<LintRun> {
    lint(&call_for(paths, exclude, config, fix), None)
}

/// [`format`] over `paths` with every other option at its default.
///
/// # Errors
///
/// As [`format`].
pub fn format_run(
    paths: &[String],
    exclude: &[String],
    config: Option<&str>,
    write: bool,
) -> anyhow::Result<FormatRun> {
    format(&call_for(paths, exclude, config, write), None)
}

/// Open the result cache the way `poly cache` does: honor `[cache] dir` from the
/// resolved config, else fall back to the default per-repo anchor walk (which
/// respects `POLY_CACHE_HOME`).
fn open_result_cache(resolver: Option<&Resolver>) -> anyhow::Result<ResultCache> {
    let config = resolve_poly_config_with(None, resolver)?;
    cache::open(cache::root(None, config.cache.dir.as_deref())?)
}

/// Report cache footprint: the `poly cache stats --format json` document.
pub fn cache_stats(resolver: Option<&Resolver>) -> anyhow::Result<CacheStatsReport> {
    cache::stats(&open_result_cache(resolver)?)
}

/// Remove every cached entry: the `poly cache clean --format json` document.
pub fn cache_clean(resolver: Option<&Resolver>) -> anyhow::Result<CacheFreedReport> {
    cache::clean(&open_result_cache(resolver)?)
}

/// Evict stale or oversized entries: the `poly cache gc --format json` document.
pub fn cache_gc(
    max_age_days: Option<u64>,
    max_size: Option<&str>,
    resolver: Option<&Resolver>,
) -> anyhow::Result<CacheFreedReport> {
    cache::gc(&open_result_cache(resolver)?, max_age_days, max_size)
}

/// List (and optionally test) the ast-grep rules a run would apply (mirrors
/// `poly rules`): poly's **built-in rule pack** plus the user rules in `dirs`,
/// which defaults to `[rules] dirs` from the resolved config when empty. When
/// `test` is true the user rule-test snippets are run too.
///
/// The listing goes through the ast-grep engine's own merge + selection path, the
/// one `lint` runs, so it reflects `[rules] builtin` and the `[lint.astgrep]`
/// `select` / `extend_select` / `ignore` / per-rule `level` keys rather than
/// reporting a raw on-disk inventory. A build that links the engine family
/// answers in-process; a core build asks the `astgrep` component (see
/// [`rules_report_for`]).
///
/// Never downloads: a core build without the `astgrep` component reports it as not installed
/// instead, because the frontend serves this from a read-only, closed-world tool.
pub fn rules_report(
    dirs: &[String],
    config: Option<&str>,
    test: bool,
    resolver: Option<&Resolver>,
) -> anyhow::Result<RulesReport> {
    let mut resolved = (*resolve_config_with(config.map(Path::new), resolver)?).clone();
    resolved.components.install = ComponentInstallPolicy::Never;
    rules_report_for(&resolved, dirs, test, None)
}

/// [`rules_report`] over an already resolved config.
///
/// With the ast-grep engine family linked the report is built in-process; otherwise the
/// `astgrep` component is made available (honouring `--offline` and `[components] install`)
/// and answers `Request::Rules`. Either way the answer is the same
/// [`poly_component_protocol::WireRulesReport`], converted by one function, so the two
/// flavours cannot drift. `injection` replaces the component host and index (tests).
pub fn rules_report_for(
    resolved: &Config,
    dirs: &[String],
    test: bool,
    injection: Option<&poly_core::components::ComponentInjection>,
) -> anyhow::Result<RulesReport> {
    let engine_config = resolved.engine_config(&Language::Other("astgrep".to_string()), "astgrep", Kind::Lint);
    let dirs = if dirs.is_empty() {
        resolved.rules_dirs.clone()
    } else {
        dirs.to_vec()
    };
    let wire = rules_wire(resolved, &dirs, &engine_config, test, injection)?;
    Ok(rules_report_from_wire(dirs, wire))
}

#[cfg(feature = "engine-astgrep")]
fn rules_wire(
    resolved: &Config,
    dirs: &[String],
    engine_config: &poly_core::config::EngineConfig,
    test: bool,
    injection: Option<&poly_core::components::ComponentInjection>,
) -> anyhow::Result<poly_component_protocol::WireRulesReport> {
    if injection.is_some() {
        return rules_wire_via_component(resolved, dirs, engine_config, test, injection);
    }
    poly_core::engines::astgrep::report::rules_report(dirs, engine_config, test)
}

#[cfg(not(feature = "engine-astgrep"))]
fn rules_wire(
    resolved: &Config,
    dirs: &[String],
    engine_config: &poly_core::config::EngineConfig,
    test: bool,
    injection: Option<&poly_core::components::ComponentInjection>,
) -> anyhow::Result<poly_component_protocol::WireRulesReport> {
    rules_wire_via_component(resolved, dirs, engine_config, test, injection)
}

fn rules_wire_via_component(
    resolved: &Config,
    dirs: &[String],
    engine_config: &poly_core::config::EngineConfig,
    test: bool,
    injection: Option<&poly_core::components::ComponentInjection>,
) -> anyhow::Result<poly_component_protocol::WireRulesReport> {
    use poly_component_protocol::{Payload, Request};
    use poly_core::components::call_engine_component;

    let request = Request::Rules {
        dirs: dirs.to_vec(),
        config: poly_engine_api::component_wire::engine_config_to_wire(engine_config),
        test,
    };
    match call_engine_component(injection, resolved, "astgrep", &request)? {
        Payload::Rules(report) => Ok(report),
        other => anyhow::bail!("the astgrep component answered Rules with an unexpected payload: {other:?}"),
    }
}

/// The one conversion from the engine's report to the frontend DTO.
pub fn rules_report_from_wire(dirs: Vec<String>, wire: poly_component_protocol::WireRulesReport) -> RulesReport {
    use poly_frontend_api::dto::{RuleInfo, RuleTestOutcome};

    let rules = wire
        .rules
        .into_iter()
        .map(|rule| RuleInfo {
            id: rule.id,
            language: rule.language,
            source: rule.source,
            default_severity: rule.default_severity,
            severity: rule.severity,
            enabled: rule.enabled,
        })
        .collect();
    let mut report = RulesReport {
        dirs,
        builtin_pack_enabled: wire.builtin_pack_enabled,
        rules,
        tests: None,
        total_rules: None,
        missing_rule_ids: None,
        untested_rule_ids: None,
        ok: None,
    };
    if let Some(tests) = wire.tests {
        report.ok = Some(tests.outcomes.iter().all(|outcome| outcome.passed) && tests.missing_rule_ids.is_empty());
        report.total_rules = Some(tests.total_rules as usize);
        report.tests = Some(
            tests
                .outcomes
                .into_iter()
                .map(|outcome| RuleTestOutcome {
                    rule_id: outcome.rule_id,
                    kind: outcome.kind,
                    index: outcome.index as usize,
                    passed: outcome.passed,
                    detail: outcome.detail,
                })
                .collect(),
        );
        report.missing_rule_ids = Some(tests.missing_rule_ids);
        report.untested_rule_ids = Some(tests.untested_rule_ids);
    }
    report
}

/// The effective, merged config: the `poly config show --format json` document, from the same
/// [`EffectiveConfig`] the CLI prints.
pub fn config_show(config: Option<&str>, resolver: Option<&Resolver>) -> anyhow::Result<EffectiveConfig> {
    let local = LocalPathResolver;
    let resolver: &dyn BaseConfigResolver = resolver.map_or(&local, |resolver| resolver.as_ref());
    EffectiveConfig::resolve(config.map(Path::new), resolver)
}

/// Check config files against poly's static knowledge of every engine, as `poly config validate`
/// does, without loading or downloading any component. `paths` are config files or directories
/// holding a `poly.toml`; empty means the nearest `poly.toml`.
pub fn config_validate(paths: &[String]) -> anyhow::Result<ValidateReport> {
    use anyhow::Context as _;
    let files: Vec<PathBuf> = if paths.is_empty() {
        let cwd = std::env::current_dir().context("reading the current directory")?;
        vec![poly_config::find_config(&cwd).context("no poly.toml found")?]
    } else {
        paths
            .iter()
            .map(PathBuf::from)
            .map(|path| {
                if path.is_dir() {
                    path.join(poly_config::CONFIG_FILE_NAMES[0])
                } else {
                    path
                }
            })
            .collect()
    };
    let mut report = ValidateReport {
        valid: true,
        files: Vec::new(),
        partial: Vec::new(),
    };
    let mut partial: std::collections::BTreeMap<String, std::collections::BTreeSet<String>> = Default::default();
    for file in files {
        let source = std::fs::read_to_string(&file).with_context(|| format!("reading {}", file.display()))?;
        let checked = poly_core::config_validate::validate_source(&source);
        report.valid &= checked.is_valid();
        report.files.push(ValidatedFile {
            path: file.display().to_string(),
            problems: checked
                .problems
                .into_iter()
                .map(|problem| ConfigProblem {
                    path: problem.path,
                    key: problem.key,
                    message: problem.message,
                })
                .collect(),
        });
        for entry in checked.partial {
            partial.entry(entry.component).or_default().extend(entry.engines);
        }
    }
    report.partial = partial
        .into_iter()
        .map(|(component, engines)| PartialValidation {
            component,
            engines: engines.into_iter().collect(),
        })
        .collect();
    Ok(report)
}

/// Run the whole-project (workspace) lint phase against the live worktree
/// (mirrors `poly lint`'s whole-project phase). When `fix` is true the tools run
/// in fix mode. Reporting is captured, never written to stdout, so the caller's
/// stdio MCP transport stays a clean JSON-RPC stream.
pub fn workspace_lint(
    config: Option<&str>,
    fix: bool,
    jobs: Option<usize>,
    no_cache: bool,
    resolver: Option<&Resolver>,
) -> anyhow::Result<WorkspaceReport> {
    let _tool_lock = tool_lock_scope(config.map(Path::new))?;
    let resolved = resolve_poly_config_with(config.map(Path::new), resolver)?;
    let plan = poly_workspace::plan_workspace_lint(&resolved)?;
    run_planned(&resolved, plan, fix, jobs, no_cache)
}

fn run_planned(
    config: &PolyConfig,
    plan: Option<poly_workspace::PlannedWorkspace>,
    fix: bool,
    jobs: Option<usize>,
    no_cache: bool,
) -> anyhow::Result<WorkspaceReport> {
    let planned = plan.is_some();
    let outcome = poly_workspace::run_planned_workspace_lint(
        config,
        plan,
        &poly_workspace::WorkspaceLintOptions {
            fix,
            jobs,
            no_cache,
            report_to_stdout: false,
        },
        false,
    )?;
    Ok(WorkspaceReport {
        passed: outcome.passed,
        fixed: fix,
        tools: outcome
            .tools
            .iter()
            .map(|tool| WorkspaceToolReport {
                id: tool.id.clone(),
                failed: tool.failed,
                cached: tool.cached,
                output: String::from_utf8_lossy(&tool.output).into_owned(),
            })
            .collect(),
        untrusted: outcome.untrusted.then(|| poly_workspace::UNTRUSTED_SKIP.to_owned()),
        // `passed` with no tools reads as a pass; say when nothing ran.
        notes: if planned || outcome.untrusted {
            Vec::new()
        } else {
            vec!["whole-project phase did not run: no whole-project tool is configured, or it is disabled".to_owned()]
        },
    })
}

/// Lint what `call` names and run the whole-project phase, as `poly lint` does: the per-file
/// tier is told which languages the phase lints, so it does not report them as unlinted, and
/// the phase's report comes back beside the run.
///
/// As on the CLI, naming engines (`only` / `skip`) skips the phase — it is made of tools, not
/// engines — and named paths do not narrow it: the tools cover the whole repository. Both
/// facts are stated in the report's `notes`.
///
/// # Errors
///
/// As [`lint`], or when the phase cannot be set up.
pub fn lint_all(call: &RunCall, resolver: Option<&Resolver>) -> anyhow::Result<(LintRun, WorkspaceReport)> {
    let request = &call.request;
    if !request.only.is_empty() || !request.skip.is_empty() {
        let run = lint(call, resolver)?;
        let report = WorkspaceReport {
            passed: true,
            fixed: false,
            tools: Vec::new(),
            untrusted: None,
            notes: vec![
                "whole-project phase skipped: `only`/`skip` name engines, and that phase runs tools".to_owned(),
            ],
        };
        return Ok((run, report));
    }
    let explicit = request.config.as_deref().map(Path::new);
    let poly_config = resolve_poly_config_with(explicit, resolver)
        .map_err(|error| RunFailure::from_anyhow(FailureKind::Config, &error))?;
    let plan = poly_workspace::plan_workspace_lint(&poly_config);
    let languages = crate::workspace_coverage::workspace_lint_languages(&plan);
    let run = lint_with(call, resolver, languages)?;
    let _tool_lock = tool_lock_scope(explicit)?;
    let mut report = run_planned(&poly_config, plan?, call.apply, request.jobs, request.no_cache)?;
    if !request.paths.is_empty() {
        report.notes.push(
            "whole-project phase covers the entire repository, not just the named paths \
             (and not filtered by [discovery] exclude)"
                .to_owned(),
        );
    }
    Ok((run, report))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_stats_reports_namespaces() {
        let _serial = crate::CONFIG_LOADING
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let report = cache_stats(None).unwrap();
        // The stats always carry the current format version.
        assert!(!report.format_version.is_empty(), "format version is populated");
    }

    #[test]
    fn a_run_scope_installs_the_repositorys_tool_lock() {
        let _serial = crate::CONFIG_LOADING
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let repo = tempfile::tempdir().unwrap();
        std::fs::create_dir(repo.path().join(".git")).unwrap();
        let config = repo.path().join("poly.toml");
        std::fs::write(&config, "").unwrap();
        std::fs::write(
            repo.path().join("poly.lock"),
            "version = 1\n[[tools]]\nname = \"mcp-locked-tool\"\nversion = \"3.2.1\"\n\
             [[tools.assets]]\nplatform = \"any\"\nurl = \"https://example.invalid/t\"\nsha256 = \"aa\"\n",
        )
        .unwrap();
        let scope = tool_lock_scope(Some(&config)).unwrap();
        let locked = poly_config::tool_lock::get("mcp-locked-tool");
        drop(scope);
        poly_config::tool_lock::install(Vec::new());
        assert_eq!(locked.expect("the MCP run scope installs poly.lock").version, "3.2.1");
    }

    #[test]
    fn resolve_poly_config_is_reachable() {
        let _serial = crate::CONFIG_LOADING
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        // A missing explicit config is an error, exercising the PolyConfig path.
        let err = resolve_poly_config(Some(Path::new("/nonexistent/poly.toml")));
        assert!(err.is_err(), "missing explicit config should error");
    }

    #[test]
    fn the_install_parameter_maps_to_the_run_overrides() {
        assert_eq!(install_policy(InstallMode::Config), (None, None));
        assert_eq!(
            install_policy(InstallMode::Auto),
            (Some(ComponentInstallPolicy::Auto), None)
        );
        assert_eq!(
            install_policy(InstallMode::Never),
            (Some(ComponentInstallPolicy::Never), Some(OnMissing::Skip)),
            "never downloads nothing and skips rather than fails"
        );
    }
}
