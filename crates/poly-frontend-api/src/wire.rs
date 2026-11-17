//! Service names, request and reply bodies, and the caller seam.

use std::collections::BTreeMap;

use anyhow::Context;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::dto::{ComponentSummary, Failure};

/// Names of the services a host answers.
pub mod service {
    /// Lint paths ([`super::RunCall`] -> [`super::RunReply`]).
    pub const LINT: &str = "lint.run";
    /// Check or apply formatting ([`super::RunCall`] -> [`super::RunReply`]).
    pub const FORMAT: &str = "format.run";
    /// The whole-project lint phase ([`super::WorkspaceRequest`] -> [`super::RunReply`] whose document is a
    /// `WorkspaceReport`, or a failure document when the phase did not run).
    pub const WORKSPACE_LINT: &str = "workspace.lint";
    /// Result-cache footprint ([`super::CacheRequest`] -> `CacheStatsReport`). A host also reads
    /// `null`, the body a frontend sent before the request existed, as the default request.
    pub const CACHE_STATS: &str = "cache.stats";
    /// Empty the result cache ([`super::CacheRequest`] -> `CacheFreedReport`); `null` as for
    /// [`CACHE_STATS`].
    pub const CACHE_CLEAN: &str = "cache.clean";
    /// Evict stale or oversized cache entries ([`super::CacheGcRequest`] -> `CacheFreedReport`).
    pub const CACHE_GC: &str = "cache.gc";
    /// Validate config files statically ([`super::ValidateRequest`] -> `ValidateReport`).
    pub const CONFIG_VALIDATE: &str = "config.validate";
    /// The effective config ([`super::ConfigRequest`] -> the `poly config show --format json` document).
    pub const CONFIG_SHOW: &str = "config.show";
    /// The ast-grep rule inventory ([`super::RulesRequest`] -> `RulesReport`).
    pub const RULES: &str = "rules.report";
    /// The downloadable components (no request -> `Vec<ComponentSummary>`).
    pub const COMPONENTS: &str = "components.list";
    /// What the components need and where each stands ([`super::ComponentsStatusRequest`] -> `ComponentsStatus`).
    pub const COMPONENTS_STATUS: &str = "components.status";
    /// Download components ([`super::ComponentsInstallRequest`] -> `ComponentActions`).
    pub const COMPONENTS_INSTALL: &str = "components.install";
    /// Remove components ([`super::ComponentsUninstallRequest`] -> `ComponentActions`).
    pub const COMPONENTS_UNINSTALL: &str = "components.uninstall";
    /// Compiled-in engine versions (no request -> [`super::Engines`]).
    pub const ENGINES: &str = "engines.versions";
    /// JSON Schemas of the lint and format documents (no request -> [`super::Schemas`]).
    pub const SCHEMAS: &str = "schemas.output";
}

/// What a frontend receives from the host when it is started (`PolyFrontend::run`).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FrontendArgs {
    /// Config file pinned on the command line, used for every request that names none.
    #[serde(default)]
    pub config: Option<String>,
}

/// Whether a run may download a missing component.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[serde(rename_all = "lowercase")]
pub enum InstallMode {
    /// Follow `[components] install` in the config.
    #[default]
    Config,
    /// Download a missing component, whatever the config says.
    Auto,
    /// Download nothing; a missing component is skipped.
    Never,
}

/// What a run does when a file needs an engine component that is not installed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[serde(rename_all = "lowercase")]
pub enum OnMissingMode {
    /// The run fails.
    Fail,
    /// The files are skipped and charged to the skip budget.
    Skip,
}

/// The run options the CLI and a frontend share: one field per `poly lint` / `poly fmt` flag that
/// shapes the run. The CLI maps its flags onto it and the MCP server derives its `check` / `fix`
/// input schema from it, so a new run option reaches both surfaces by construction.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct RunRequest {
    /// Files or directories (default: the current directory).
    #[serde(default)]
    pub paths: Vec<String>,
    /// Gitignore-style globs excluded on top of `[discovery] exclude` (`e2e/**`: any depth; `/e2e/**`: the config
    /// root).
    #[serde(default)]
    pub exclude: Vec<String>,
    /// Config file; default: discovered from the working directory.
    #[serde(default)]
    pub config: Option<String>,
    /// Check named paths even when they match the exclude set.
    #[serde(default)]
    pub include_excluded: bool,
    /// Run only these engines (`ruff`, `oxc`, ...). Never enables one the config leaves off.
    #[serde(default)]
    pub only: Vec<String>,
    /// Drop these engines. Not with `only`.
    #[serde(default)]
    pub skip: Vec<String>,
    /// Not verified when more files than this were skipped (0: any skip).
    #[serde(default)]
    pub max_skips: Option<usize>,
    /// Missing engine component: `config` (default, `[components] install`), `auto` download, `never` (skip its
    /// files).
    #[serde(default)]
    pub install: InstallMode,
    /// Missing component: `fail` or `skip` its files; overrides `[components] on_missing`.
    #[serde(default)]
    pub on_missing: Option<OnMissingMode>,
    /// Bypass the result cache.
    #[serde(default)]
    pub no_cache: bool,
    /// Worker threads (default: all cores).
    #[serde(default)]
    pub jobs: Option<usize>,
    /// Also rewrite hash-stamped generated files.
    #[serde(default)]
    pub fix_generated: bool,
    /// Attach per-engine debug records (cache hits, timings).
    #[serde(default)]
    pub debug: bool,
}

/// Request body of `lint.run` and `format.run`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RunCall {
    /// The run options.
    #[serde(flatten)]
    pub request: RunRequest,
    /// Apply fixes (`lint.run`) or write files (`format.run`).
    #[serde(default)]
    pub apply: bool,
    /// Also run the whole-project phase, as `poly lint` does (`lint.run` only). Its report rides in
    /// the reply document under `workspace`.
    #[serde(default)]
    pub workspace: bool,
}

/// Request body of `components.status`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ComponentsStatusRequest {
    /// Files or directories to inspect; empty lists every component without judging need.
    #[serde(default)]
    pub paths: Vec<String>,
    /// Config file to use instead of discovery.
    #[serde(default)]
    pub config: Option<String>,
    /// The install policy `would_download` is judged under.
    #[serde(default)]
    pub install: InstallMode,
}

/// Request body of `components.install`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ComponentsInstallRequest {
    /// Component names or presets.
    #[serde(default)]
    pub names: Vec<String>,
    /// Languages whose components to install (ids or preset aliases such as `python`).
    #[serde(default)]
    pub languages: Vec<String>,
    /// Files or directories; installs every component they need.
    #[serde(default)]
    pub paths: Vec<String>,
    /// Config file to use instead of discovery (for `paths`).
    #[serde(default)]
    pub config: Option<String>,
    /// Report what would be downloaded and download nothing.
    #[serde(default)]
    pub dry_run: bool,
}

/// Request body of `components.uninstall`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ComponentsUninstallRequest {
    /// Component names or presets.
    #[serde(default)]
    pub names: Vec<String>,
    /// Also remove cached archives the lock no longer references and nothing used recently.
    #[serde(default)]
    pub stale: bool,
}

/// Request body of `config.show`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ConfigRequest {
    /// Config file to use instead of discovery.
    #[serde(default)]
    pub config: Option<String>,
}

/// Request body of `cache.stats` and `cache.clean`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CacheRequest {
    /// Config file whose `[cache] dir` locates the cache, instead of discovery.
    #[serde(default)]
    pub config: Option<String>,
}

/// Request body of `cache.gc`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CacheGcRequest {
    /// Config file whose `[cache] dir` locates the cache, instead of discovery.
    #[serde(default)]
    pub config: Option<String>,
    /// Evict entries older than this many days.
    #[serde(default)]
    pub max_age_days: Option<u64>,
    /// Evict oldest-first until under this budget (`500M`, `2G`).
    #[serde(default)]
    pub max_size: Option<String>,
}

/// Request body of `config.validate`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ValidateRequest {
    /// Config files, or directories holding a `poly.toml`; empty means the nearest `poly.toml`.
    #[serde(default)]
    pub paths: Vec<String>,
}

/// Request body of `rules.report`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RulesRequest {
    /// User rule directories; empty means `[rules] dirs`.
    #[serde(default)]
    pub dirs: Vec<String>,
    /// Config file to use instead of discovery.
    #[serde(default)]
    pub config: Option<String>,
    /// Also run the rule tests.
    #[serde(default)]
    pub test: bool,
}

/// Request body of `workspace.lint`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct WorkspaceRequest {
    /// Config file to use instead of discovery.
    #[serde(default)]
    pub config: Option<String>,
    /// Run the tools in fix mode.
    #[serde(default)]
    pub fix: bool,
    /// Concurrency override.
    #[serde(default)]
    pub jobs: Option<usize>,
    /// Bypass the result cache.
    #[serde(default)]
    pub no_cache: bool,
}

/// Reply of `lint.run` and `format.run`: the run rendered three ways, so the frontend adds
/// nothing and cannot disagree with the CLI.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunReply {
    /// The `--format json` document as a value (the tool's structured content).
    pub document: Value,
    /// The document as the CLI prints it with `--format json`.
    pub json: String,
    /// The document as the CLI prints it with `--format toon`.
    pub toon: String,
    /// Whether the run did not verify what it was asked to (the tool's `isError`): it failed on a
    /// file, a component blocks it, its skips exceed the budget, or it could not start.
    pub is_error: bool,
}

/// Reply of `engines.versions`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Engines {
    /// Every compiled-in backend and the upstream version it wraps.
    pub engines: BTreeMap<String, String>,
    /// `bundled`, `core` or `partial`; `unknown` for a host with no component manager.
    #[serde(default = "unknown_flavor")]
    pub flavor: String,
    /// Digest of the component lock the host embeds, or none.
    #[serde(default)]
    pub components_lock: Option<String>,
}

fn unknown_flavor() -> String {
    "unknown".to_owned()
}

/// Reply of `schemas.output`: draft 2020-12 root schemas.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Schemas {
    /// Schema of the lint document.
    pub lint: Value,
    /// Schema of the format document.
    pub format: Value,
    /// The `poly.toml` schema (`poly config schema`).
    #[serde(default)]
    pub poly_toml: Value,
}

/// Reply of `components.list`.
pub type Components = Vec<ComponentSummary>;

/// One way to reach the host's services.
///
/// `Send + Sync` because a frontend calls from blocking worker threads.
pub trait ServiceCaller: Send + Sync + 'static {
    /// Send `request` to `service` and return the response bytes; the error is the host's message.
    ///
    /// # Errors
    ///
    /// When the host has no such service or the service failed.
    fn call(&self, service: &str, request: &[u8]) -> Result<Vec<u8>, String>;
}

/// A host service failure that carries its class.
///
/// The service table's error is a string, so a host sends a classified failure as the JSON of a
/// [`Failure`] (see [`encode_failure`]) and [`call`] decodes it into this error. A frontend then
/// reports the kind a run would for the same cause rather than a generic one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceFailure {
    /// The service that failed.
    pub service: String,
    /// Its class and message.
    pub failure: Failure,
}

impl std::fmt::Display for ServiceFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "host service `{}` failed: {}",
            self.service, self.failure.message
        )
    }
}

impl std::error::Error for ServiceFailure {}

/// The service-table error string for a classified failure, which [`call`] decodes into a
/// [`ServiceFailure`].
#[must_use]
pub fn encode_failure(failure: &Failure) -> String {
    serde_json::to_string(failure).unwrap_or_else(|_| failure.message.clone())
}

/// The classified failure a host's error string carries, if any. A kind this frontend does not
/// know (a newer host's) reads as [`FailureKind::Pipeline`], keeping the host's message.
///
/// [`FailureKind::Pipeline`]: crate::dto::FailureKind::Pipeline
fn decode_failure(message: &str) -> Option<Failure> {
    /// A [`Failure`] whose kind is not checked against this frontend's classes.
    #[derive(Deserialize)]
    struct AnyFailure {
        #[serde(rename = "kind")]
        _kind: serde::de::IgnoredAny,
        message: String,
    }

    if let Ok(failure) = serde_json::from_str::<Failure>(message) {
        return Some(failure);
    }
    let AnyFailure { message, .. } = serde_json::from_str(message).ok()?;
    Some(Failure {
        kind: crate::dto::FailureKind::Pipeline,
        message,
    })
}

/// Encode `request`, call `service` and decode the reply.
///
/// # Errors
///
/// When a body does not encode or decode, or the host reports a failure: a [`ServiceFailure`] when
/// the host classified it.
pub fn call<Req: Serialize, Rep: DeserializeOwned>(
    caller: &dyn ServiceCaller,
    service: &str,
    request: &Req,
) -> anyhow::Result<Rep> {
    let bytes = serde_json::to_vec(request).with_context(|| format!("cannot encode the `{service}` request"))?;
    let reply = caller
        .call(service, &bytes)
        .map_err(|message| match decode_failure(&message) {
            Some(failure) => anyhow::Error::new(ServiceFailure {
                service: service.to_owned(),
                failure,
            }),
            None => anyhow::anyhow!("host service `{service}` failed: {message}"),
        })?;
    serde_json::from_slice(&reply).with_context(|| format!("cannot decode the `{service}` reply"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dto::FailureKind;

    /// A host whose every service fails with `message`.
    struct Failing(String);

    impl ServiceCaller for Failing {
        fn call(&self, _: &str, _: &[u8]) -> Result<Vec<u8>, String> {
            Err(self.0.clone())
        }
    }

    #[test]
    fn a_classified_failure_keeps_its_kind_across_the_service_table() {
        let failure = Failure {
            kind: FailureKind::Config,
            message: "bad toml".to_owned(),
        };
        let error = call::<_, Value>(&Failing(encode_failure(&failure)), service::CONFIG_SHOW, &()).unwrap_err();
        let decoded = error.downcast_ref::<ServiceFailure>().expect("a classified failure");
        assert_eq!(decoded.failure, failure);
        assert_eq!(format!("{error:#}"), "host service `config.show` failed: bad toml");
    }

    /// A newer host may classify a failure with a kind this frontend does not know: the call
    /// still fails with the host's message, as the generic `pipeline` class, not the raw JSON.
    #[test]
    fn a_failure_of_an_unknown_kind_keeps_its_message_as_a_pipeline_failure() {
        let message = r#"{"kind":"some_future_kind","message":"cache unreadable"}"#;
        let error = call::<_, Value>(&Failing(message.to_owned()), service::CACHE_STATS, &()).unwrap_err();
        let decoded = error.downcast_ref::<ServiceFailure>().expect("a classified failure");
        assert_eq!(decoded.failure.kind, FailureKind::Pipeline);
        assert_eq!(
            format!("{error:#}"),
            "host service `cache.stats` failed: cache unreadable"
        );
    }

    /// The `cache.gc` body before e366526e, which had no `config`: a host built then must still
    /// decode what this frontend sends, losing only `config`.
    #[derive(Deserialize)]
    struct CacheGcRequestWithoutConfig {
        #[serde(default)]
        max_age_days: Option<u64>,
        #[serde(default)]
        max_size: Option<String>,
    }

    #[test]
    fn a_cache_request_decodes_on_a_host_that_predates_config() {
        let request = CacheGcRequest {
            config: Some("poly.toml".to_owned()),
            max_age_days: Some(7),
            max_size: Some("2G".to_owned()),
        };
        let bytes = serde_json::to_vec(&request).unwrap();
        assert_eq!(
            bytes, br#"{"config":"poly.toml","max_age_days":7,"max_size":"2G"}"#,
            "the wire shape an old host decodes"
        );
        let old: CacheGcRequestWithoutConfig = serde_json::from_slice(&bytes).unwrap();
        assert_eq!((old.max_age_days, old.max_size.as_deref()), (Some(7), Some("2G")));
        let stats = serde_json::to_vec(&CacheRequest::default()).unwrap();
        assert_eq!(stats, br#"{"config":null}"#, "an old host ignores the cache.stats body");
    }

    #[test]
    fn a_plain_failure_message_stays_unclassified() {
        let error = call::<_, Value>(&Failing("boom".to_owned()), service::CONFIG_SHOW, &()).unwrap_err();
        assert!(error.downcast_ref::<ServiceFailure>().is_none());
        assert_eq!(format!("{error:#}"), "host service `config.show` failed: boom");
    }
}
