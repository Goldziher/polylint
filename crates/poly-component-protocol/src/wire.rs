//! Wire DTOs. Engine types are never serialized directly; these mirror them with every field present.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::WireValue;

/// Identifier of a config registered once per component with [`Request::RegisterConfig`].
pub type ConfigId = u32;

/// Severity of a [`WireDiagnostic`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub enum WireSeverity {
    /// A problem that should fail the run.
    Error,
    /// A likely problem.
    Warning,
    /// Informational note.
    Info,
    /// A low-priority suggestion.
    Hint,
}

/// 1-based line/column span.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct WireSpan {
    /// 1-based start line.
    pub start_line: u32,
    /// 1-based start column.
    pub start_col: u32,
    /// 1-based end line.
    pub end_line: u32,
    /// 1-based end column.
    pub end_col: u32,
}

/// A byte-range replacement.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct WireEdit {
    /// Inclusive start byte offset.
    pub start_byte: u64,
    /// Exclusive end byte offset.
    pub end_byte: u64,
    /// Replacement text.
    pub replacement: String,
}

/// A normalized lint finding. Every field is always present on the wire.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct WireDiagnostic {
    /// Id of the engine that produced the finding, when it is not the engine the request named.
    /// `None` (the common case) means the requested engine, so the id is not repeated per finding.
    pub engine: Option<String>,
    /// Tool-specific rule code.
    pub code: Option<String>,
    /// Severity.
    pub severity: WireSeverity,
    /// One-line title.
    pub title: String,
    /// Longer explanation.
    pub description: Option<String>,
    /// Source location.
    pub span: Option<WireSpan>,
    /// Rule documentation URL.
    pub url: Option<String>,
    /// Suggested autofix, applied atomically; empty when there is none.
    pub fix: Vec<WireEdit>,
    /// Tool-specific extras.
    pub metadata: BTreeMap<String, String>,
}

/// A file whose content is the entire `body` of the call.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct WireFile {
    /// Path, used for naming and config discovery only.
    pub path: String,
    /// Language id (see `Language::id`).
    pub language: String,
    /// Byte length of the content in `body`; must equal `body.len()`.
    pub len: u64,
}

/// One member of a batch: a byte range of the call's `body`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct WireFileSlice {
    /// Path, used for naming and config discovery only.
    pub path: String,
    /// Language id.
    pub language: String,
    /// Offset of the content within `body`.
    pub offset: u64,
    /// Byte length of the content.
    pub len: u64,
}

/// Which config table an option declaration describes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub enum WireOptionTable {
    /// `[lint.<lang>.<engine>]`.
    Lint,
    /// `[fmt.<lang>.<engine>]`.
    Format,
    /// The language-agnostic `[lint.<engine>]` table of a cross-cutting engine.
    CrossCuttingLint,
}

/// Whether a query concerns linting or formatting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub enum WireKind {
    /// Lint.
    Lint,
    /// Format.
    Format,
}

/// One declared option key.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct WireOptionKey {
    /// Key name.
    pub name: String,
    /// Accepted value types as bits: 1 boolean, 2 integer, 4 float, 8 string, 16 array, 32 table, 64 datetime.
    pub types: u8,
}

/// The static option keys an engine reads from one table.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct WireOptionKeys {
    /// The table described.
    pub table: WireOptionTable,
    /// `false` when the engine declares nothing and no key is reported.
    pub checked: bool,
    /// Keys read by name.
    pub declared: Vec<WireOptionKey>,
    /// The engine accepts poly's uniform rule-selection vocabulary.
    pub rule_selection: bool,
    /// Further keys are answered at run time by [`Request::DerivedOptionKeys`].
    pub has_derived: bool,
    /// Guidance shown with unknown-key warnings.
    pub note: Option<String>,
}

/// What an engine can do.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct WireCapabilities {
    /// Can report diagnostics.
    pub lint: bool,
    /// Can reformat source.
    pub format: bool,
    /// Can produce autofixes.
    pub fix: bool,
    /// Answers `Request::ProjectCheck` over a whole file set.
    pub project: bool,
}

/// Which batch operations an engine supports.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct WireBatchSupport {
    /// `format_batch` is implemented.
    pub format: bool,
    /// `lint_batch` is implemented.
    pub lint: bool,
}

/// Bits of [`WireEngineDesc::hooks`]: which optional per-engine requests the host must send.
/// An unset bit means the host answers with the trait default and never makes the call.
pub mod hooks {
    /// [`Request::RunContext`](crate::Request::RunContext).
    pub const RUN_CONTEXT: u32 = 1 << 0;
    /// [`Request::FileContext`](crate::Request::FileContext).
    pub const FILE_CONTEXT: u32 = 1 << 1;
    /// [`Request::ProvidesLanguage`](crate::Request::ProvidesLanguage).
    pub const PROVIDES_LANGUAGE: u32 = 1 << 2;
    /// `Engine::skip_reason`. No request carries it: the component answers it inside `Format` and
    /// `FormatBatch` with [`WireFormatOutput::Declined`](crate::WireFormatOutput::Declined), so the
    /// host never asks. The bit only records that the engine overrides the method.
    pub const SKIP_REASON: u32 = 1 << 3;
    /// [`Request::LintInputSalt`](crate::Request::LintInputSalt).
    pub const LINT_INPUT_SALT: u32 = 1 << 4;
    /// [`Request::FormatInputSalt`](crate::Request::FormatInputSalt).
    pub const FORMAT_INPUT_SALT: u32 = 1 << 5;
    /// [`Request::LintCacheSalt`](crate::Request::LintCacheSalt).
    pub const LINT_CACHE_SALT: u32 = 1 << 6;
    /// [`Request::BatchSupport`](crate::Request::BatchSupport).
    pub const BATCH_SUPPORT: u32 = 1 << 7;
}

/// A side file of a project check: the option naming its path and the default path.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct WireProjectSideFile {
    /// Option naming the path, relative to the project root.
    pub option_key: String,
    /// Path used when the option is unset.
    pub default_path: String,
}

/// The directories a project check is about, named by options.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct WireProjectDirs {
    /// Option naming one directory, relative to the project root.
    pub dir_key: String,
    /// Directory used when the option is unset.
    pub dir_default: String,
    /// Option holding a list of directories that replaces `dir_key`.
    pub paths_key: Option<String>,
    /// Boolean option extending the directories to their subdirectories.
    pub recursive_key: Option<String>,
    /// Whether the directories extend to their subdirectories when the option is unset. Absent in
    /// descriptions written before it existed, which read as `false`.
    #[serde(default)]
    pub recursive_default: bool,
    /// Whether the walk honors only `.gitignore` and the always-pruned directories. Absent reads as `false`.
    #[serde(default)]
    pub whole_tree: bool,
    /// File names handed over wherever the walk finds them, whatever their language. Absent reads as empty.
    #[serde(default)]
    pub file_names: Vec<String>,
    /// Whether the host also lists the files the engine does not serve (names only).
    pub list_unserved: bool,
    /// Whether the check computes findings only for the request's `targets`, which the host then sends. Absent
    /// reads as `false`: the host sends no targets.
    #[serde(default)]
    pub narrows_to_targets: bool,
}

/// `Engine::project_inputs`: what a project check needs from the host besides its own files.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct WireProjectInputs {
    /// Option that must be set for the engine to be planned.
    pub gate_key: Option<String>,
    /// Side files the host loads when they exist.
    #[serde(default)]
    pub files: Vec<WireProjectSideFile>,
    /// Directories that scope the served files and the listing.
    pub dirs: Option<WireProjectDirs>,
}

/// Static description of one engine hosted by a component.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct WireEngineDesc {
    /// Tool id (`Engine::name`).
    pub name: String,
    /// Language ids served; empty means cross-cutting.
    pub languages: Vec<String>,
    /// Capabilities.
    pub capabilities: WireCapabilities,
    /// Tool version, folded into the cache key.
    pub version: String,
    /// Static option keys per table.
    pub option_keys: Vec<WireOptionKeys>,
    /// Bit set of [`hooks`] the host must call.
    pub hooks: u32,
    /// `Engine::takes_over_tier_one`.
    pub takes_over_tier_one: bool,
    /// `Engine::claimed_paths`.
    pub claimed_paths: Vec<String>,
    /// `Engine::host_hook` in kebab-case (`none`, `oxfmt-scope`, `oxc`, `ruff-context`): which
    /// cache-path hook the host computes locally for this engine, so a cache hit never loads
    /// the component.
    pub host_hook: String,
    /// `Engine::supplements_tier_one`.
    pub supplements_tier_one: bool,
    /// `Engine::self_manages_enabled`.
    pub self_manages_enabled: bool,
    /// `Engine::supersedes_generic_formatter`.
    pub supersedes_generic_formatter: bool,
    /// `Engine::batch_shard_floor`.
    pub batch_shard_floor: u32,
    /// `Engine::project_inputs`. Absent in an older description, which reads as "nothing".
    #[serde(default)]
    pub project_inputs: WireProjectInputs,
    /// `Engine::selection`: how a config table picks this engine among alternatives. Absent for an engine
    /// no table chooses between, and in an older description.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selection: Option<WireSelection>,
}

/// How one config table chooses between the engines that share it.
///
/// `[engines.python-types] backend = "pyrefly"` is the engine table `python-types` with its key `backend`
/// set to `pyrefly`: the engine declaring that value is the one that runs. Engines that declare the same
/// `table` and `key` are alternatives; none runs until the key is set.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct WireSelection {
    /// The config table the alternatives share (an engine id).
    pub table: String,
    /// The key of that table that picks one.
    pub key: String,
    /// The key value that picks this engine.
    pub value: String,
    /// Runs when the key is set to a value no alternative declares, so it can report that value as unknown.
    pub default: bool,
}

/// The answer to `describe()`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct Describe {
    /// Protocol version the component speaks.
    pub protocol: u32,
    /// Version of the component itself.
    pub component_version: String,
    /// Hosted engines.
    pub engines: Vec<WireEngineDesc>,
}

/// Log level, ordered from most to least severe.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub enum WireLogLevel {
    /// Errors.
    Error,
    /// Warnings.
    Warn,
    /// Informational.
    Info,
    /// Debug.
    Debug,
    /// Trace.
    Trace,
}

/// A log record emitted by the component during a call.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct WireLog {
    /// Level.
    pub level: WireLogLevel,
    /// Emitting target (module path).
    pub target: String,
    /// Message.
    pub message: String,
}

/// Classification of a [`WireError`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub enum WireErrorKind {
    /// The engine returned an error.
    Error,
    /// The engine panicked; the call was caught.
    Panic,
    /// A request named a [`ConfigId`] that was never registered.
    UnknownConfig,
    /// A request named an engine the component does not host.
    UnknownEngine,
    /// The request was malformed or violated the protocol.
    Protocol,
    /// The component does not serve this request (for example `Rules` on a component that hosts no
    /// rule engine).
    Unsupported,
}

/// A failed call.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct WireError {
    /// Classification.
    pub kind: WireErrorKind,
    /// Top-level message.
    pub message: String,
    /// Causes, outermost first.
    pub chain: Vec<String>,
}

/// A request header. File content never appears here; it travels in the call's `body`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub enum Request {
    /// First call: negotiate the protocol and pass host settings.
    Init {
        /// Protocol version the host speaks.
        protocol: u32,
        /// Host (poly) version.
        host_version: String,
        /// Worker threads the component may use.
        threads: u32,
        /// Stack size in bytes of the host's worker threads.
        stack_size: u64,
        /// Most verbose level the host wants in [`Response::logs`].
        max_log_level: WireLogLevel,
    },
    /// Register a config; later requests refer to it by `id`.
    RegisterConfig {
        /// Id chosen by the host, unique per component.
        id: ConfigId,
        /// The engine config: a table with `indent_width` and `options`, plus globals.
        config: WireValue,
    },
    /// Lint one file (content in `body`).
    Lint {
        /// Engine name.
        engine: String,
        /// Registered config.
        config: ConfigId,
        /// The file.
        file: WireFile,
    },
    /// Format one file (content in `body`).
    Format {
        /// Engine name.
        engine: String,
        /// Registered config.
        config: ConfigId,
        /// The file.
        file: WireFile,
    },
    /// Lint a batch; each member is a slice of `body`.
    LintBatch {
        /// Engine name.
        engine: String,
        /// Registered config.
        config: ConfigId,
        /// Members, in order.
        files: Vec<WireFileSlice>,
    },
    /// Format a batch; each member is a slice of `body`.
    FormatBatch {
        /// Engine name.
        engine: String,
        /// Registered config.
        config: ConfigId,
        /// Members, in order.
        files: Vec<WireFileSlice>,
    },
    /// Whole-project check (no file content; the component reads from `root`).
    ProjectCheck {
        /// Engine name (a component may host several project engines).
        engine: String,
        /// Project root.
        root: String,
        /// Files in scope, relative to `root`; their contents are slices of `body`.
        files: Vec<WireFileSlice>,
        /// Registered config.
        config: ConfigId,
        /// The host-resolved environment as flat pairs (`python.interpreter`,
        /// `python.site_packages`, `python.version`); the component never probes the system.
        environment: Vec<(String, String)>,
        /// The worker threads the host's run budgets for the check; `None` is the component's choice.
        threads: Option<u32>,
        /// The files whose findings the host reports, spelled as in `files`; `None` is every file (see
        /// `ProjectRequest::targets`). The last field, so a request one element shorter, from a host that
        /// predates it, decodes as `None`; and left out when `None`, so a component that predates it, which
        /// rejects a longer request, still decodes a check of every file.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        targets: Option<Vec<String>>,
    },
    /// `Engine::lint_cache_salt` for one file (content in `body`).
    LintCacheSalt {
        /// Engine name.
        engine: String,
        /// The file.
        file: WireFile,
    },
    /// `Engine::provides_language_lint` / `provides_language_format`.
    ProvidesLanguage {
        /// Engine name.
        engine: String,
        /// Registered config.
        config: ConfigId,
        /// Language id.
        language: String,
        /// Which of the two predicates.
        kind: WireKind,
    },
    /// `Engine::batch_support`.
    BatchSupport {
        /// Engine name.
        engine: String,
        /// Registered config.
        config: ConfigId,
    },
    /// The keys a serde-derived probe recognises in `options` (see [`WireOptionKeys::has_derived`]).
    DerivedOptionKeys {
        /// Engine name.
        engine: String,
        /// Table the keys belong to.
        table: WireOptionTable,
        /// The user's options table.
        options: WireValue,
    },
    /// `Engine::run_context`.
    RunContext {
        /// Engine name.
        engine: String,
        /// Registered config.
        config: ConfigId,
    },
    /// `Engine::file_context`.
    FileContext {
        /// Engine name.
        engine: String,
        /// Registered config.
        config: ConfigId,
        /// File path.
        path: String,
    },
    /// `Engine::lint_input_salt`.
    LintInputSalt {
        /// Engine name.
        engine: String,
        /// Registered config.
        config: ConfigId,
        /// File path.
        path: String,
    },
    /// `Engine::format_input_salt`.
    FormatInputSalt {
        /// Engine name.
        engine: String,
        /// Registered config.
        config: ConfigId,
        /// File path.
        path: String,
    },
    /// The rule inventory (and optionally the rule-test run) of the component's rule engine.
    /// Only the component that hosts the ast-grep engine answers it; any other answers
    /// [`WireErrorKind::Unsupported`].
    Rules {
        /// Rule directories to search for user rules.
        dirs: Vec<String>,
        /// The resolved engine config of the rule engine, as in [`Request::RegisterConfig`].
        config: WireValue,
        /// Also run the user rules' `*-test.yml` snippets.
        test: bool,
    },
    /// An ADR log maintenance operation (`poly adr`). Only the component that hosts the adrlint
    /// engine answers it; any other answers [`WireErrorKind::Unsupported`]. The component reads and
    /// writes nothing: the host hands over the files and writes the answer's
    /// [`WireAdrPlan::writes`] itself.
    Adr {
        /// The operation.
        op: WireAdrOp,
        /// The resolved adrlint engine config, as in [`Request::RegisterConfig`].
        config: WireValue,
        /// The files the operation reads, relative to the project root; their contents are slices of
        /// `body`. The lock manifest is the member whose path is the config's `lock` option.
        files: Vec<WireFileSlice>,
    },
    /// The host started a new run: forget what the component memoised from project files (the
    /// lookups of `poly_engine_api::host_hooks` and whatever its engines registered there), so
    /// a long-lived host's next run reads them as they are on disk now. Sent once per run, before
    /// the run's first request to a component an earlier run loaded; answered with
    /// [`Payload::Ok`].
    RunBoundary {
        /// The host's run counter, for logs: it only ever grows within one host process.
        run: u64,
    },
}

/// Names of every [`Request`] variant, in declaration order.
pub const REQUEST_VARIANTS: &[&str] = &[
    "Init",
    "RegisterConfig",
    "Lint",
    "Format",
    "LintBatch",
    "FormatBatch",
    "ProjectCheck",
    "LintCacheSalt",
    "ProvidesLanguage",
    "BatchSupport",
    "DerivedOptionKeys",
    "RunContext",
    "FileContext",
    "LintInputSalt",
    "FormatInputSalt",
    "Rules",
    "Adr",
    "RunBoundary",
];

impl Request {
    /// The variant name, as listed in [`REQUEST_VARIANTS`].
    pub fn variant_name(&self) -> &'static str {
        match self {
            Request::Init { .. } => "Init",
            Request::RegisterConfig { .. } => "RegisterConfig",
            Request::Lint { .. } => "Lint",
            Request::Format { .. } => "Format",
            Request::LintBatch { .. } => "LintBatch",
            Request::FormatBatch { .. } => "FormatBatch",
            Request::ProjectCheck { .. } => "ProjectCheck",
            Request::LintCacheSalt { .. } => "LintCacheSalt",
            Request::ProvidesLanguage { .. } => "ProvidesLanguage",
            Request::BatchSupport { .. } => "BatchSupport",
            Request::DerivedOptionKeys { .. } => "DerivedOptionKeys",
            Request::RunContext { .. } => "RunContext",
            Request::FileContext { .. } => "FileContext",
            Request::LintInputSalt { .. } => "LintInputSalt",
            Request::FormatInputSalt { .. } => "FormatInputSalt",
            Request::Rules { .. } => "Rules",
            Request::Adr { .. } => "Adr",
            Request::RunBoundary { .. } => "RunBoundary",
        }
    }

    /// The registered config the request names, for the host to re-point after re-registering it.
    pub fn config_mut(&mut self) -> Option<&mut ConfigId> {
        match self {
            Request::Lint { config, .. }
            | Request::Format { config, .. }
            | Request::LintBatch { config, .. }
            | Request::FormatBatch { config, .. }
            | Request::ProjectCheck { config, .. }
            | Request::ProvidesLanguage { config, .. }
            | Request::BatchSupport { config, .. }
            | Request::RunContext { config, .. }
            | Request::FileContext { config, .. }
            | Request::LintInputSalt { config, .. }
            | Request::FormatInputSalt { config, .. } => Some(config),
            Request::Init { .. }
            | Request::RegisterConfig { .. }
            | Request::LintCacheSalt { .. }
            | Request::DerivedOptionKeys { .. }
            | Request::Rules { .. }
            | Request::Adr { .. }
            | Request::RunBoundary { .. } => None,
        }
    }
}

/// Result of formatting one file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub enum WireFormatOutput {
    /// Already formatted.
    Unchanged,
    /// The formatted source.
    Formatted(String),
    /// The engine declines the file (`Engine::skip_reason`) and did not format it; the reason is
    /// reported as the file's skip.
    Declined(String),
}

/// Per-member result of a batch lint.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub enum WireLintItem {
    /// Diagnostics for the member.
    Ok(Vec<WireDiagnostic>),
    /// The member failed on its own.
    Err(WireError),
}

/// Per-member result of a batch format.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub enum WireFormatItem {
    /// Output for the member.
    Ok(WireFormatOutput),
    /// The member failed on its own.
    Err(WireError),
}

/// Diagnostics of one file from a project check.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct WireFileDiagnostics {
    /// Path as given in the request.
    pub path: String,
    /// Findings for that file.
    pub diagnostics: Vec<WireDiagnostic>,
}

/// One resolved rule of a [`WireRulesReport`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct WireRule {
    /// The rule's `id`, as reported in a diagnostic's `code`.
    pub id: String,
    /// The language name the rule targets.
    pub language: String,
    /// `builtin` or `user`.
    pub source: String,
    /// The severity the rule's own YAML declares (`off` for an opt-in rule).
    pub default_severity: String,
    /// The severity a finding reports at under the request's config.
    pub severity: String,
    /// Whether the rule participates in a scan under the request's config.
    pub enabled: bool,
}

/// The outcome of one rule-test snippet.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct WireRuleTestOutcome {
    /// The rule id the snippet was checked against.
    pub rule_id: String,
    /// `valid`, `invalid` or `fixed`.
    pub kind: String,
    /// Index of the snippet within its list.
    pub index: u64,
    /// Whether the snippet met its expectation.
    pub passed: bool,
    /// Failure detail, if any.
    pub detail: Option<String>,
}

/// The result of running the rule tests.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct WireRuleTests {
    /// Rules discovered under the searched directories.
    pub total_rules: u64,
    /// Per-snippet outcomes.
    pub outcomes: Vec<WireRuleTestOutcome>,
    /// Test files naming an unknown rule id.
    pub missing_rule_ids: Vec<String>,
    /// Rules with no test file.
    pub untested_rule_ids: Vec<String>,
}

/// The answer to [`Request::Rules`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct WireRulesReport {
    /// Whether poly's built-in rule pack is enabled in the request's config.
    pub builtin_pack_enabled: bool,
    /// Every resolved rule, active or not.
    pub rules: Vec<WireRule>,
    /// Present only when the request asked for tests.
    pub tests: Option<WireRuleTests>,
}

/// The operation of a [`Request::Adr`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub enum WireAdrOp {
    /// Write or refresh the lock manifest for every record whose status is locked (`accepted`,
    /// `rejected`), or for every numbered record when `all` is set. `files` holds the log's Markdown
    /// and the manifest, when it exists.
    Lock {
        /// Lock every numbered record whatever its status; also re-locks a manifest written by an
        /// older canonical serializer.
        all: bool,
        /// Plan for `--check`: a stale manifest is a pending write rather than an error, and a log with
        /// no manifest and nothing to lock is an empty plan, as `adrlint lock --check` reports them.
        check: bool,
    },
    /// Remove one record's entry from the manifest. `files` holds the manifest.
    Unlock {
        /// The record, relative to the project root with `/` separators: its manifest key.
        path: String,
    },
    /// Create the next numbered record in `dir`. `files` holds the Markdown of every configured ADR
    /// directory: the next number follows the highest among them, and a `template.md` or
    /// `NNNN-template.md` directly in `dir` is the template, otherwise the MADR one is used.
    New {
        /// The ADR directory, relative to the project root with `/` separators (empty for the root).
        dir: String,
        /// The record's title.
        title: String,
        /// The date written into the record's front matter, `YYYY-MM-DD`, chosen by the host.
        date: String,
    },
}

/// One file a [`WireAdrPlan`] asks the host to write.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct WireAdrWrite {
    /// Path relative to the project root, `/` separated.
    pub path: String,
    /// The whole new content.
    pub content: String,
    /// The file must not exist yet (a new record); otherwise it is replaced (the manifest).
    pub create: bool,
}

/// The answer to [`Request::Adr`]: what to write and what to tell the user.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct WireAdrPlan {
    /// Files to write; empty when the log is already in the requested state.
    pub writes: Vec<WireAdrWrite>,
    /// One line per changed manifest entry or record (`add <key>`, `update <key>`, `remove <key>`,
    /// `create <path>`).
    pub changes: Vec<String>,
    /// A one-line summary of the outcome.
    pub summary: String,
}

/// The successful payload of a [`Response`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub enum Payload {
    /// Success with no data (`RegisterConfig`).
    Ok,
    /// Answer to `Init`.
    Init {
        /// Protocol version the component speaks.
        protocol: u32,
        /// Component version.
        component_version: String,
    },
    /// Answer to `Lint`.
    Diagnostics(Vec<WireDiagnostic>),
    /// Answer to `Format`.
    Formatted(WireFormatOutput),
    /// Answer to `LintBatch`: one item per member, in order.
    LintBatch(Vec<WireLintItem>),
    /// Answer to `FormatBatch`: one item per member, in order.
    FormatBatch(Vec<WireFormatItem>),
    /// Answer to `ProjectCheck`.
    Project(Vec<WireFileDiagnostics>),
    /// Answer to `ProvidesLanguage`.
    Bool(bool),
    /// Answer to the salts and `FileContext`; `None` is the trait default.
    OptString(Option<String>),
    /// Answer to `RunContext`.
    Text(String),
    /// Answer to `BatchSupport`.
    BatchSupport(WireBatchSupport),
    /// Answer to `DerivedOptionKeys`; `None` means the probe could not tell.
    OptionKeys(Option<Vec<String>>),
    /// Answer to `Rules`.
    Rules(WireRulesReport),
    /// Answer to `Adr`.
    Adr(WireAdrPlan),
    /// Answer to `ProjectCheck` when the engine declined the project (`ProjectReport::Declined`, ADR 0079): it
    /// checked nothing, for the reason given, which the host reports as a skip. Appended after protocol 3
    /// shipped: a host that predates it fails to decode the answer and reports the check as failed, never as
    /// a clean pass.
    ProjectDeclined(String),
}

/// The answer to every call.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct Response {
    /// Outcome.
    pub result: Result<Payload, WireError>,
    /// Log records produced during the call.
    pub logs: Vec<WireLog>,
}

/// Root type used to emit one JSON Schema covering every top-level message.
///
/// `request` and `response` are encoded positionally: a struct (or struct variant) is a MessagePack
/// array of its fields in declaration order, every field present (`null` for `None`). Enums are
/// externally tagged by variant name (`{"Variant": value}`, a unit variant as its bare name).
/// `describe` keeps named fields (structs as maps), so a description can grow defaulted fields.
///
/// Transport framing is not part of the messages. A dylib component receives `call(header, body)`
/// directly. A sidecar executable (alef stdio transport) receives one `invoke_bytes(method, payload)`
/// per call: method `describe` with an empty payload returns the encoded `describe` message; method
/// `call` carries `u32_le(header_len) ++ header ++ body` and returns the encoded `response`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct ProtocolSchema {
    /// Request header.
    pub request: Request,
    /// Response.
    pub response: Response,
    /// `describe()` payload.
    pub describe: Describe,
}
