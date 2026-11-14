//! The `secrets` engine: kingfisher's built-in rule catalog over every text file.
//!
//! Cross-cutting (no language) and opt-in: it scans nothing until `[engines.secrets] enabled = true`. Lint only,
//! never an autofix: a leaked credential has to be revoked and rotated, which no text edit does.
//!
//! The scan is offline. kingfisher's `validation` feature, which would call the credential's provider to check
//! whether it is live, is not compiled in. The scanner is built to redact: the matched value never leaves kingfisher,
//! so it cannot reach a diagnostic, a log line or the result cache, which stores diagnostics.

use std::sync::{Arc, OnceLock};

use anyhow::{Context, anyhow};
use kingfisher_rules::{RulesDatabase, get_builtin_rules};
use kingfisher_scanner::{Finding, Scanner, ScannerConfig};
use poly_engine_api::config::EngineConfig;
use poly_engine_api::engine::{
    Capabilities, Diagnostic, Engine, OptionKeys, OptionTable, OptionType, Severity, SourceFile, Span,
};
use poly_engine_api::language::Language;
use poly_engine_api::line_index::LineIndex;

/// The engine id: the `[engines.<id>]` key, the diagnostic's `engine` field and the cache-key id.
pub const ENGINE_NAME: &str = "secrets";

/// Folded into the cache key. The two crate versions are the resolved ones in `components-secrets/Cargo.lock`
/// (`tests/version_audit.rs` fails when they differ); the suffix marks this crate's own mapping logic.
const VERSION: &str = "kingfisher-scanner-3.0.3+rules-3.2.0+map1";

const ENABLED_KEY: &str = "enabled";

static OPTION_KEYS: [(&str, OptionType); 1] = [(ENABLED_KEY, OptionType::BOOLEAN)];

static LANGUAGES: &[Language] = &[];

/// Compiled once per process: compiling the catalog into a vectorscan database costs far more than a scan.
static SCANNER: OnceLock<Result<Scanner, String>> = OnceLock::new();

/// Secret scanning over every language, backed by kingfisher's built-in catalog.
pub struct SecretsEngine;

fn build_scanner() -> anyhow::Result<Scanner> {
    let rules = get_builtin_rules(None).context("loading the built-in secret rules")?;
    let database = RulesDatabase::from_rule_collection(rules).context("compiling the built-in secret rules")?;
    Ok(Scanner::with_config(
        Arc::new(database),
        ScannerConfig {
            redact_secrets: true,
            ..ScannerConfig::default()
        },
    ))
}

fn scanner() -> anyhow::Result<&'static Scanner> {
    SCANNER
        .get_or_init(|| build_scanner().map_err(|error| format!("{error:#}")))
        .as_ref()
        .map_err(|error| anyhow!("the secret scanner is unavailable: {error}"))
}

fn enabled(cfg: &EngineConfig) -> bool {
    cfg.options.get(ENABLED_KEY).and_then(toml::Value::as_bool) == Some(true)
}

impl Engine for SecretsEngine {
    fn overridden_hooks(&self) -> u32 {
        0
    }

    fn name(&self) -> &'static str {
        ENGINE_NAME
    }

    fn languages(&self) -> &'static [Language] {
        LANGUAGES
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            lint: true,
            format: false,
            fix: false,
            project: false,
        }
    }

    fn option_keys(&self, table: OptionTable) -> OptionKeys {
        match table {
            OptionTable::Lint | OptionTable::CrossCuttingLint => OptionKeys::declared(&OPTION_KEYS),
            OptionTable::Format => OptionKeys::declared(&[]),
        }
    }

    fn version(&self) -> &str {
        VERSION
    }

    fn lint(&self, src: &SourceFile, cfg: &EngineConfig) -> anyhow::Result<Vec<Diagnostic>> {
        if !enabled(cfg) || src.content.is_empty() {
            return Ok(Vec::new());
        }
        let findings = scanner()?
            .scan_bytes(src.content.as_bytes())
            .context("scanning for secrets")?;
        let index = LineIndex::new(&src.content);
        Ok(findings
            .iter()
            .filter(|finding| finding.rule().visible())
            .map(|finding| to_diagnostic(finding, &index))
            .collect())
    }
}

/// Maps a finding to a diagnostic without ever reading `Finding::secret`: the scanner already replaced it, and the
/// message carries only the rule, the length and the confidence.
fn to_diagnostic(finding: &Finding, index: &LineIndex<'_>) -> Diagnostic {
    let location = &finding.location;
    let (start_line, start_col) = index.line_col(location.start_offset);
    let (end_line, end_col) = index.line_col(location.end_offset);
    let length = location.end_offset.saturating_sub(location.start_offset);
    let mut metadata = std::collections::BTreeMap::new();
    metadata.insert(
        "confidence".to_owned(),
        format!("{:?}", finding.confidence).to_ascii_lowercase(),
    );
    Diagnostic {
        engine: ENGINE_NAME.to_owned(),
        code: Some(format!("{ENGINE_NAME}/{}", finding.rule_id)),
        severity: Severity::Error,
        title: format!("{} ({length} bytes, value redacted)", finding.rule_name),
        description: Some(format!(
            "Remove the secret from the source and revoke it. A deliberate fixture is allowed with \
             `poly: allow[{ENGINE_NAME}/{}] <reason>` or a `[per_file_ignores]` entry.",
            finding.rule_id
        )),
        span: Some(Span {
            start_line,
            start_col,
            end_line,
            end_col,
        }),
        url: None,
        fix: Vec::new(),
        metadata,
    }
}
