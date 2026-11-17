//! Data-transfer objects and result builders for the MCP tools.
//!
//! Every tool returns rmcp **structured content**: the typed payload lands in
//! [`rmcp::model::CallToolResult::structured_content`] (its JSON schema is a
//! resource, see [`crate::server::resources`]), and a
//! single text [`ContentBlock`] carries either the same JSON or a compact
//! **TOON** rendering, selected per request by [`TextRepr`]. A client thus gets
//! machine-readable structured JSON *and* a human/compact text view from one
//! call.
//!
//! Lint and format return `poly-core`'s own `LintDocument` / `FormatDocument`
//! — the exact value `poly lint --format json` prints. The host renders it
//! (see [`RunReply`]) and publishes its JSON Schema, so this crate links no
//! engine and there is no MCP-local copy: the two surfaces cannot answer the
//! same question differently.
//!
//! The document is built from the whole *run*, not just its results, and states
//! three things a caller would otherwise have to reconstruct by walking every
//! record: files the run **failed** on (also flipping `isError`, the MCP
//! stand-in for the CLI's exit code 2), files nothing **inspected**, and how
//! many files were actually **checked**. A run where everything was skipped
//! reports no diagnostics and is not an error — without those fields it is
//! indistinguishable from a clean pass, which is the defect they close. A skip
//! is coverage information, not a failure, so it deliberately does not set
//! `isError`.
//!
//! A lint document additionally carries `suppressed` — one entry per diagnostic
//! the run found and then dropped, naming the file, the rule, and which
//! mechanism dropped it (`default-path-exclusion`, `per-file-ignore`,
//! `inline-suppression`). None of those is a coverage limitation, so like a
//! skip none sets `isError`; but `results` plus `suppressed` is the unfiltered
//! finding set, so a caller can see what a `poly.toml` or an in-source
//! directive removed without re-running with the filters off. It is omitted
//! when empty, unlike `errors`/`skipped`, which are always present because a
//! consumer must be able to test them unconditionally.
//!
//! The cache/rules/config/workspace DTOs live in [`poly_frontend_api::dto`],
//! shared with the host that builds them, because their CLI counterparts print
//! prose rather than a serializable value.

use std::collections::BTreeMap;

pub use poly_frontend_api::dto::{
    CacheFreedReport, CacheNamespace, CacheStatsReport, ComponentAction, ComponentActions, ComponentRow,
    ComponentSummary, ComponentsStatus, RuleInfo, RuleTestOutcome, RulesReport, ValidateReport, WorkspaceReport,
    WorkspaceToolReport,
};
use poly_frontend_api::dto::{Failure, FailureDocument, FailureKind};
use poly_frontend_api::wire::RunReply;
use rmcp::ErrorData;
use rmcp::model::{CallToolResult, ContentBlock, MetaObject};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::identity::{IDENTITY_KEY, identity_value};

/// Which text representation a tool should place in its text content block.
/// `structured_content` is always JSON regardless; this selects only the text
/// block so a client can ask for the compact TOON view.
#[derive(Debug, Clone, Copy, Default, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum TextRepr {
    /// Pretty JSON (the default) — mirrors the CLI `--format json` text.
    #[default]
    Json,
    /// Token-Oriented Object Notation — mirrors the CLI `--format toon` text.
    Toon,
}

/// Map a serialization or render failure onto an MCP internal error.
///
/// Always an error, never a substitute payload: a caller that receives a
/// well-formed but empty document cannot tell it from a clean run.
fn serialize_error(error: impl std::fmt::Display) -> ErrorData {
    ErrorData::internal_error(format!("failed to serialize tool output: {error}"), None)
}

/// Assemble a [`CallToolResult`] carrying structured JSON plus a chosen text
/// block. `structured` is always the JSON value; the text block is `json_text`
/// or `toon_text` per `repr`.
///
/// Every result also carries the serving binary's [`crate::identity::PolyIdentity`], both as a
/// `poly` key in `structured_content` and in the response `_meta`. An MCP caller
/// has no `poly --version` to fall back on, so a result that does not say which
/// binary produced it is indistinguishable from one produced by a build with
/// known-destructive behaviour.
///
/// `is_error` marks the result as a tool-level **error**: a run that could not process a file
/// did not check it, and a caller that reads only the tool's success/failure flag must not be
/// told otherwise — the MCP counterpart of the CLI's exit code 2. The payload is unchanged: the
/// caller still gets every result the run did produce, alongside the failures.
fn build_with_status(
    structured: Value,
    json_text: String,
    toon_text: String,
    repr: TextRepr,
    is_error: bool,
) -> CallToolResult {
    let text = match repr {
        TextRepr::Json => json_text,
        TextRepr::Toon => toon_text,
    };
    // `CallToolResult::structured` seeds `structured_content` and a JSON text
    // block; override the text block with the representation the caller asked
    // for while leaving `structured_content` as JSON. ~keep
    let mut result = CallToolResult::structured(with_identity(structured));
    result.content = vec![ContentBlock::text(text)];
    result.meta = Some(identity_meta());
    result.is_error = Some(is_error);
    result
}

/// Add the serving binary's identity under the `poly` key. A non-object payload
/// is left alone — `_meta` still carries the identity in that case.
fn with_identity(structured: Value) -> Value {
    match structured {
        Value::Object(mut map) => {
            map.insert(IDENTITY_KEY.to_string(), identity_value().clone());
            Value::Object(map)
        }
        other => other,
    }
}

/// The response `_meta` block carrying the serving binary's identity.
fn identity_meta() -> MetaObject {
    let mut meta = MetaObject::new();
    meta.0.insert(IDENTITY_KEY.to_string(), identity_value().clone());
    meta
}

/// Build a tool result for a self-describing DTO: structured JSON plus a JSON or
/// TOON text block derived from the same value.
pub fn dto_result<T: Serialize>(value: &T, repr: TextRepr) -> Result<CallToolResult, ErrorData> {
    dto_result_with_status(value, repr, false)
}

/// [`dto_result`], marked as a tool error when `is_error` is set.
pub fn dto_result_with_status<T: Serialize>(
    value: &T,
    repr: TextRepr,
    is_error: bool,
) -> Result<CallToolResult, ErrorData> {
    let structured = serde_json::to_value(value).map_err(serialize_error)?;
    let json_text = serde_json::to_string_pretty(value).map_err(serialize_error)?;
    let toon_text = serde_toon::to_string(value).unwrap_or_else(|_| json_text.clone());
    Ok(build_with_status(structured, json_text, toon_text, repr, is_error))
}

/// The result of an install or uninstall. A component that failed or was refused is a tool error:
/// the caller asked for it to be usable (or gone) and it is not.
pub fn actions_result(actions: &ComponentActions, repr: TextRepr) -> Result<CallToolResult, ErrorData> {
    let structured = serde_json::to_value(actions).map_err(serialize_error)?;
    let json_text = serde_json::to_string_pretty(actions).map_err(serialize_error)?;
    let toon_text = serde_toon::to_string(actions).unwrap_or_else(|_| json_text.clone());
    Ok(build_with_status(structured, json_text, toon_text, repr, !actions.ok))
}

/// Build the tool result for a whole lint or format run.
///
/// The payload is the host's rendering of `poly-core`'s own document — the same value
/// `poly lint --format json` prints — so `structured_content` and the text block carry one
/// shape, and the CLI and this server cannot answer "what did you actually check" differently.
/// A run that could not start arrives as a failure document with `is_error` set.
pub fn run_result(reply: RunReply, repr: TextRepr) -> CallToolResult {
    build_with_status(reply.document, reply.json, reply.toon, repr, reply.is_error)
}

/// A failed run the frontend itself has to report: the host could not be reached, or the
/// arguments do not go together.
pub fn failure_reply(kind: FailureKind, message: &str) -> RunReply {
    let document = FailureDocument {
        error: Failure {
            kind,
            message: message.to_owned(),
        },
        summary: None,
    };
    let json = serde_json::to_string_pretty(&document).unwrap_or_else(|_| message.to_owned());
    RunReply {
        toon: serde_toon::to_string(&document).unwrap_or_else(|_| json.clone()),
        document: serde_json::to_value(&document).unwrap_or(Value::Null),
        json,
        is_error: true,
    }
}

/// An `isError` result for arguments that do not go together.
pub fn usage_result(message: &str, repr: TextRepr) -> CallToolResult {
    run_result(failure_reply(FailureKind::Usage, message), repr)
}

/// `{"error": {"message": …}}` for a host lookup that failed: the body of an `info` topic the
/// host could not answer, and of a failed `manage` action.
pub fn failure_value(error: &anyhow::Error) -> Value {
    serde_json::json!({ "error": { "message": format!("{error:#}") } })
}

/// An `isError` result carrying [`failure_value`]. A host failure is a result, not a JSON-RPC
/// error, so the model reads the message and can correct its call.
pub fn error_result(error: &anyhow::Error, repr: TextRepr) -> CallToolResult {
    let value = failure_value(error);
    let json = serde_json::to_string_pretty(&value).unwrap_or_default();
    let toon = serde_toon::to_string(&value).unwrap_or_else(|_| json.clone());
    build_with_status(value, json, toon, repr, true)
}

/// Health of the binary serving this MCP session (the `info` tool's `version` topic).
///
/// The identity itself rides in the `poly` key every response carries; this adds
/// what only a long-lived server can tell you — whether the executable it is
/// running from is still the one installed on disk, and how long it has been
/// answering.
#[derive(Debug, Serialize, JsonSchema)]
pub struct VersionReport {
    /// Whether the executable this server started from is still the file at
    /// that path. `false` means an upgrade (or a delete) happened underneath a
    /// running server, and every other tool is refusing to answer.
    pub executable_current: bool,
    /// Human-readable explanation of the executable's state.
    pub executable_status: String,
    /// Seconds this server process has been running. A server older than your
    /// last upgrade is serving the pre-upgrade build.
    pub uptime_seconds: u64,
    /// Every compiled-in backend and the upstream version it wraps.
    ///
    /// The identity block on every response carries a *digest* of this, which is
    /// enough to tell two binaries apart but not to say how. This is the
    /// expansion, returned only here so the map is not repeated on every result.
    /// Host-toolchain backends are excluded: their version depends on the
    /// machine and the checkout, not on the binary.
    pub engines: BTreeMap<String, String>,
    /// The downloadable components and where each stands (installed, not installed,
    /// unavailable). Empty when the host has no component state to report.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub components: Vec<ComponentSummary>,
}
