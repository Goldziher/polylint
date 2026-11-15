//! The JSON Schema validation backend: lints JSON, JSONC, YAML and TOML against a local schema.
//!
//! Opt-in (`[engines.jsonschema] enabled = true`), lint only, and offline: a schema is a file under
//! the project root or in the local schema cache, never fetched. A file names its schema itself
//! (`$schema` in JSON, a `#:schema` or `# yaml-language-server: $schema=` comment in YAML and TOML)
//! or matches an entry of `schemas = [{ glob = "...", schema = "path/schema.json" }]`; resolution
//! lives in [`poly_engine_api::host_hooks::jsonschema_source`], shared with the cache-key hook.
//!
//! A violation is an error at the offending node, coded `schema/<keyword>` (`schema/required`,
//! `schema/type`, ...). A schema that is named but unusable is reported, never passed over:
//! `schema-unavailable` (info) for a remote schema that is not cached, a missing file or one outside
//! the project root, `schema-invalid` (error) for a schema that does not compile, and
//! `schema-unparseable` (info) for a document that is not valid in its language.

use std::collections::BTreeMap;

use poly_engine_api::config::EngineConfig;
use poly_engine_api::engine::{
    Capabilities, Diagnostic, Engine, OptionKeys, OptionTable, OptionType, Severity, SourceFile, Span,
};
use poly_engine_api::host_hooks::HostHook;
use poly_engine_api::host_hooks::jsonschema_source::{
    SchemaSource, Scope, declared_json_refs, directive_ref, is_metaschema, resolve_in,
};
use poly_engine_api::language::Language;

use crate::doc::{self, Document};
use crate::validator::{self, Finding};

/// Cache-key identity: the validator and YAML parser versions, plus a marker for this engine's own
/// logic (span mapping, value conversion). Bump when either crate moves or the logic changes.
const VERSION: &str = "jsonschema-0.61.2+granit-parser-2.1.0+spans2";

const LANGUAGES: &[Language] = &[Language::Json, Language::Jsonc, Language::Yaml, Language::Toml];

/// Every option key the backend reads, with the type it reads it as.
static OPTION_KEYS: &[(&str, OptionType)] = &[("schemas", OptionType::ARRAY), ("cache_dir", OptionType::STRING)];

/// Validates JSON, JSONC, YAML and TOML documents against a local JSON Schema.
#[derive(Debug, Clone, Copy, Default)]
pub struct JsonSchemaEngine;

/// Whether `[engines.jsonschema] enabled` is `true`; the backend is opt-in.
fn enabled(cfg: &EngineConfig) -> bool {
    cfg.options
        .get("enabled")
        .and_then(toml::Value::as_bool)
        .unwrap_or(false)
}

fn note(code: &str, severity: Severity, title: String, span: Option<Span>, description: Option<String>) -> Diagnostic {
    Diagnostic {
        engine: JsonSchemaEngine.name().to_owned(),
        code: Some(code.to_owned()),
        severity,
        title,
        description,
        span,
        url: None,
        fix: Vec::new(),
        metadata: BTreeMap::new(),
    }
}

/// Where a note about the whole file sits: its first character.
fn file_start() -> Option<Span> {
    Some(Span {
        start_line: 1,
        start_col: 1,
        end_line: 1,
        end_col: 1,
    })
}

impl Engine for JsonSchemaEngine {
    fn name(&self) -> &'static str {
        "jsonschema"
    }

    fn languages(&self) -> &'static [Language] {
        LANGUAGES
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            lint: true,
            ..Capabilities::default()
        }
    }

    fn option_keys(&self, table: OptionTable) -> OptionKeys {
        match table {
            OptionTable::Lint => OptionKeys::declared(OPTION_KEYS),
            _ => OptionKeys::declared(&[]),
        }
    }

    fn version(&self) -> &str {
        VERSION
    }

    fn host_hook(&self) -> HostHook {
        HostHook::JsonSchema
    }

    fn overridden_hooks(&self) -> u32 {
        poly_engine_api::desc::hooks::PROVIDES_LANGUAGE
    }

    /// Never claims a language: the engine checks only the files that name a schema, so it
    /// establishes no lint coverage of JSON, YAML or TOML as a whole. The same lookup `lint` runs
    /// answers per file; per language the honest answer is no.
    fn provides_language_lint(&self, _language: &Language, _cfg: &EngineConfig) -> bool {
        false
    }

    fn lint(&self, src: &SourceFile, cfg: &EngineConfig) -> anyhow::Result<Vec<Diagnostic>> {
        if !enabled(cfg) || !LANGUAGES.contains(&src.language) {
            return Ok(Vec::new());
        }
        let content: &str = &src.content;
        let json = matches!(src.language, Language::Json | Language::Jsonc);
        let textual = if json {
            declared_json_refs(content).into_iter().next()
        } else {
            directive_ref(content)
        };
        let scope = Scope::for_file(&src.path, cfg);
        if textual.is_none() && scope.mapped(cfg).is_none() {
            return Ok(Vec::new());
        }

        let documents = match doc::parse_lazy(&src.language, &src.content) {
            Ok(documents) => documents,
            Err(reason) => {
                return Ok(match resolve_in(&scope, textual.as_deref(), cfg) {
                    SchemaSource::None => Vec::new(),
                    _ => vec![note(
                        "schema-unparseable",
                        Severity::Info,
                        format!("not validated against a schema: {reason}"),
                        file_start(),
                        None,
                    )],
                });
            }
        };

        let in_file = if json {
            documents
                .first()
                .and_then(|document| document.value.get("$schema"))
                .and_then(serde_json::Value::as_str)
                .filter(|reference| !is_metaschema(reference))
                .map(str::to_owned)
        } else {
            textual
        };
        match resolve_in(&scope, in_file.as_deref(), cfg) {
            SchemaSource::None => Ok(Vec::new()),
            SchemaSource::Unavailable { reference, reason } => Ok(vec![note(
                "schema-unavailable",
                Severity::Info,
                format!("not validated: schema `{reference}` is unavailable ({reason})"),
                file_start(),
                Some("The engine reads schemas only from the project root and the local schema cache.".to_owned()),
            )]),
            SchemaSource::Local(schema) => Ok(validator::with_validator(&scope, &schema, |compiled| match compiled {
                Err(reason) => vec![note(
                    "schema-invalid",
                    Severity::Error,
                    format!("schema `{}` {reason}", schema.display()),
                    file_start(),
                    None,
                )],
                Ok(validator) => {
                    let name = schema
                        .strip_prefix(&scope.root)
                        .unwrap_or(&schema)
                        .display()
                        .to_string();
                    let mut found: Vec<Diagnostic> = documents
                        .iter()
                        .flat_map(|document| violations(validator, document, &name))
                        .collect();
                    found.sort_by_cached_key(|d| {
                        let at = d.span.map_or((0, 0), |s| (s.start_line, s.start_col));
                        (at, d.code.clone(), d.title.clone())
                    });
                    found.dedup_by(|a, b| a.span == b.span && a.code == b.code && a.title == b.title);
                    found
                }
            })),
        }
    }
}

/// The diagnostics for every way `document` breaks the schema `name`, each at the node it names.
fn violations(validator: &jsonschema::Validator, document: &Document, name: &str) -> Vec<Diagnostic> {
    validator::findings(validator, document)
        .into_iter()
        .map(
            |Finding {
                 pointer,
                 keyword,
                 message,
                 schema_path,
             }| {
                let shown = if pointer.is_empty() { "<root>" } else { &pointer };
                note(
                    &format!("schema/{keyword}"),
                    Severity::Error,
                    format!("{shown}: {message}"),
                    document.span_of(&pointer).or_else(file_start),
                    Some(format!("Schema `{name}`, keyword at `{schema_path}`.")),
                )
            },
        )
        .collect()
}
