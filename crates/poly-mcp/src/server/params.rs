//! The four tools' arguments and definitions.
//!
//! Descriptions are terse on purpose: they are sent to the model with every tool listing, and
//! the result semantics every tool shares are stated once, in the server instructions.

use std::sync::Arc;

use poly_frontend_api::wire::{InstallMode, RunRequest};
use rmcp::model::{JsonObject, Tool, ToolAnnotations};
use schemars::JsonSchema;
use serde::Deserialize;

use crate::dto::TextRepr;

/// The check-only tool.
pub(crate) const CHECK: &str = "check";
/// The writing counterpart of [`CHECK`].
pub(crate) const FIX: &str = "fix";
/// The read-only state tool.
pub(crate) const INFO: &str = "info";
/// The cache and component maintenance tool.
pub(crate) const MANAGE: &str = "manage";

/// What a run checks or fixes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RunKind {
    /// Lint diagnostics.
    #[default]
    Lint,
    /// Formatting.
    Format,
}

/// Which tier a run covers.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Scope {
    /// The per-file tier over `paths`.
    #[default]
    Files,
    /// The whole-project phase only (lint).
    Workspace,
    /// Both, as `poly lint` runs them (lint).
    All,
}

/// Arguments of `check` and `fix`.
#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
pub struct RunParams {
    /// `lint` (default) or `format`.
    #[serde(default)]
    pub kind: RunKind,
    /// `files` (default): per-file tier over `paths`. `workspace`: whole-project phase (cargo clippy, inline
    /// jobs; whole repo, no `paths`). `all`: both, as `poly lint`. `workspace`/`all` are lint-only.
    #[serde(default)]
    pub scope: Scope,
    /// The run options, shared with the CLI.
    #[serde(flatten)]
    pub run: RunRequest,
    /// Text block format: `json` (default) or `toon`.
    #[serde(default)]
    pub format: TextRepr,
}

/// One `info` topic.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Topic {
    /// The serving binary, its engines and whether it is still the file on disk.
    Version,
    /// The effective config, as `poly config show --format json`.
    Config,
    /// `poly config validate` over `paths` (default: the nearest poly.toml).
    Validate,
    /// The ast-grep rules a run applies; `test` runs their snippets.
    Rules,
    /// Downloadable components; with `paths`, what they need.
    Components,
    /// Result-cache footprint.
    Cache,
}

/// Arguments of `info`.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct InfoParams {
    /// Topics to report, one result key each. Default `["version"]`.
    #[serde(default = "default_topics")]
    pub topics: Vec<Topic>,
    /// Config file (`poly.toml`); default: discovered from the server's directory.
    #[serde(default)]
    pub config: Option<String>,
    /// `components`: the paths to judge. `validate`: config files or directories.
    #[serde(default)]
    pub paths: Vec<String>,
    /// `components`: the install policy `would_download` assumes.
    #[serde(default)]
    pub install: InstallMode,
    /// `rules`: user rule directories (default: the config's).
    #[serde(default)]
    pub dirs: Vec<String>,
    /// `rules`: also run each rule's `*-test.yml` snippets; a failure sets `isError`.
    #[serde(default)]
    pub test: bool,
    /// Text block format: `json` (default) or `toon`.
    #[serde(default)]
    pub format: TextRepr,
}

impl Default for InfoParams {
    fn default() -> Self {
        Self {
            topics: default_topics(),
            config: None,
            paths: Vec::new(),
            install: InstallMode::default(),
            dirs: Vec::new(),
            test: false,
            format: TextRepr::default(),
        }
    }
}

fn default_topics() -> Vec<Topic> {
    vec![Topic::Version]
}

/// One `manage` action.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    /// Remove every result-cache entry.
    CacheClean,
    /// Evict cache entries older than `max_age_days` or beyond `max_size`.
    CacheGc,
    /// Download components by `names`, `languages` and/or `paths`.
    ComponentsInstall,
    /// Remove components by `names`.
    ComponentsUninstall,
    /// Remove cached component archives nothing references or used recently.
    ComponentsPrune,
}

/// Arguments of `manage`.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct ManageParams {
    /// The operation.
    pub action: Action,
    /// Components or presets (`all`, a family, a language alias such as `python`).
    #[serde(default)]
    pub names: Vec<String>,
    /// `components_install`: languages whose components to install.
    #[serde(default)]
    pub languages: Vec<String>,
    /// `components_install`: install every component these paths need.
    #[serde(default)]
    pub paths: Vec<String>,
    /// Config file used to judge `paths`.
    #[serde(default)]
    pub config: Option<String>,
    /// `components_install`: report what would be downloaded, download nothing.
    #[serde(default)]
    pub dry_run: bool,
    /// `cache_gc`: evict entries older than this many days.
    #[serde(default)]
    pub max_age_days: Option<u64>,
    /// `cache_gc`: evict oldest-first to this budget (`500M`, `2G`).
    #[serde(default)]
    pub max_size: Option<String>,
    /// Text block format: `json` (default) or `toon`.
    #[serde(default)]
    pub format: TextRepr,
}

/// The input schema of `T`.
fn input_schema<T: JsonSchema + std::any::Any>() -> Arc<JsonObject> {
    rmcp::handler::server::tool::schema_for_input::<T>().expect("tool params have a valid input schema")
}

/// The four tool definitions, in listing order.
pub(crate) fn definitions() -> Vec<Tool> {
    vec![
        Tool::new(
            CHECK,
            "Lint, or check formatting, without writing. Mirrors `poly lint --format json` (`scope: all`; `files` is \
             `--workspace never`) and `poly fmt --format json`. Runs the repository's configured tools.",
            input_schema::<RunParams>(),
        )
        .annotate(
            ToolAnnotations::new()
                .read_only(false)
                .destructive(false)
                .idempotent(true)
                .open_world(true),
        ),
        Tool::new(
            FIX,
            "Apply lint autofixes or formatting in place, then report what remains. Mirrors `poly lint --fix` and \
             `poly fmt --fix`.",
            input_schema::<RunParams>(),
        )
        .annotate(
            ToolAnnotations::new()
                .read_only(false)
                .destructive(true)
                .idempotent(false)
                .open_world(true),
        ),
        Tool::new(
            INFO,
            "Read poly's state: version, config, validate, rules, components, cache. Never downloads or runs \
             repository tools.",
            input_schema::<InfoParams>(),
        )
        .annotate(
            ToolAnnotations::new()
                .read_only(true)
                .destructive(false)
                .idempotent(true)
                .open_world(false),
        ),
        Tool::new(
            MANAGE,
            "Maintain the result cache and the downloadable components. Mirrors `poly cache clean|gc` and `poly \
             components install|uninstall|prune`.",
            input_schema::<ManageParams>(),
        )
        .annotate(
            ToolAnnotations::new()
                .read_only(false)
                .destructive(true)
                .idempotent(false)
                .open_world(true),
        ),
    ]
}
