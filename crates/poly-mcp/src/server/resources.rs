//! The output schemas, served as resources instead of declared on the tools.
//!
//! A tool's `outputSchema` rides along with every `tools/list`, and the lint document's alone is
//! 18 KB, most of it rustdoc prose a model rarely needs. As resources they cost nothing until a
//! client reads one. The contract is unchanged: every result is the document these describe,
//! plus the `poly` identity block (`poly://schema/identity`).

use std::sync::OnceLock;

use poly_frontend_api::dto::{
    CacheFreedReport, CacheStatsReport, ComponentActions, ComponentsStatus, FailureDocument, RulesReport,
    ValidateReport, WorkspaceReport,
};
use poly_frontend_api::wire::Schemas;
use rmcp::model::{Resource, ResourceContents};
use schemars::JsonSchema;
use schemars::generate::SchemaSettings;
use serde_json::Value;

use crate::dto::VersionReport;
use crate::identity::PolyIdentity;

/// The prefix every schema resource's URI starts with.
const PREFIX: &str = "poly://schema/";

/// The schemas the host published (lint and format documents, `poly.toml`), installed once at
/// startup.
static HOST_SCHEMAS: OnceLock<Schemas> = OnceLock::new();

/// Remember the host's schemas. The first call wins: the host's documents do not change while it
/// runs.
pub fn install_host_schemas(schemas: Schemas) {
    let _ = HOST_SCHEMAS.set(schemas);
}

/// `(name, what it describes)` for every schema resource.
const RESOURCES: &[(&str, &str)] = &[
    (
        "lint",
        "`check`/`fix` with `kind: lint`: the `poly lint --format json` document.",
    ),
    (
        "format",
        "`check`/`fix` with `kind: format`: the `poly fmt --format json` document.",
    ),
    (
        "failure",
        "A run that could not start: `{error: {kind, message}, summary: null}`.",
    ),
    (
        "workspace",
        "`scope: workspace`, and the `workspace` key of `scope: all`.",
    ),
    ("info/version", "`info` topic `version`."),
    ("info/rules", "`info` topic `rules`."),
    ("info/components", "`info` topic `components`."),
    ("info/cache", "`info` topic `cache`."),
    ("info/validate", "`info` topic `validate`."),
    ("manage/cache", "`manage` actions `cache_clean` and `cache_gc`."),
    ("manage/components", "`manage` component actions."),
    ("identity", "The `poly` block every result carries."),
    (
        "poly.toml",
        "The `poly.toml` configuration schema (`poly config schema`).",
    ),
];

/// The resource listing.
pub fn list() -> Vec<Resource> {
    RESOURCES
        .iter()
        .map(|(name, description)| {
            Resource::new(format!("{PREFIX}{name}"), *name)
                .with_description(*description)
                .with_mime_type("application/schema+json")
        })
        .collect()
}

/// The contents of `uri`, or `None` when no such resource exists.
pub fn read(uri: &str) -> Option<ResourceContents> {
    let name = uri.strip_prefix(PREFIX)?;
    let schema = match name {
        "lint" => HOST_SCHEMAS.get().map(|schemas| schemas.lint.clone())?,
        "format" => HOST_SCHEMAS.get().map(|schemas| schemas.format.clone())?,
        "poly.toml" => HOST_SCHEMAS.get().map(|schemas| schemas.poly_toml.clone())?,
        "failure" => schema::<FailureDocument>(),
        "workspace" => schema::<WorkspaceReport>(),
        "info/version" => schema::<VersionReport>(),
        "info/rules" => schema::<RulesReport>(),
        "info/components" => schema::<ComponentsStatus>(),
        "info/cache" => schema::<CacheStatsReport>(),
        "info/validate" => schema::<ValidateReport>(),
        "manage/cache" => schema::<CacheFreedReport>(),
        "manage/components" => schema::<ComponentActions>(),
        "identity" => schema::<PolyIdentity>(),
        _ => return None,
    };
    let text = serde_json::to_string(&schema).ok()?;
    Some(ResourceContents::text(text, uri).with_mime_type("application/schema+json"))
}

fn schema<T: JsonSchema>() -> Value {
    let schema = SchemaSettings::draft2020_12()
        .into_generator()
        .into_root_schema_for::<T>();
    serde_json::to_value(schema).unwrap_or(Value::Null)
}
