//! R linting with [jarl](https://github.com/etiennebacher/jarl), a linter for R in the spirit of lintr, in process.
//!
//! The engine hands the file to jarl's own checker (`jarl_core::check::check_contents`), which parses it with the
//! same parser air uses and reports what `jarl check` reports for it. No R installation is involved: jarl is given
//! no library of installed packages, so rules that look up what a package exports resolve a bare call to its
//! package only from what the file and the project's `NAMESPACE` say.
//!
//! Findings carry jarl's rule name as their code and link to the rule's page. A rule in the `CORR` category (code
//! that is outright wrong or useless) is an error; every other rule is a warning. `[engines.jarl.rules.<name>]
//! level = "..."` moves one rule. A fix is attached only when jarl marks it safe; the unsafe ones are reported with
//! `fix = "unsafe"` in their metadata and left to jarl.
//!
//! | Key | Type | Description |
//! |-----|------|-------------|
//! | `select` | array | Replace the default rule set (rule names or categories such as `PERF`) |
//! | `extend_select` | array | Add rules, or categories, to the default set |
//! | `ignore` | array | Remove rules, or categories, from the set |
//! | `rules.<name>.level` | string | `"error"`, `"warning"`, `"info"` or `"hint"` for one rule |
//! | `check_roxygen` | bool | Lint the `@examples` of roxygen comments (default `true`) |
//! | `min_r_version` | string | The oldest R the code must run on, which switches off rules that need a newer one |
//!
//! `jarl.toml` supplies everything else (per-rule options, `fixable`, `per-file-ignores`). Its `exclude` and
//! `include` file filters are not applied: poly's own `[discovery]` and `per_file_ignores` decide which files run.

use std::collections::BTreeMap;

use jarl_core::check::check_contents;
use jarl_core::diagnostic::Diagnostic as JarlDiagnostic;
use jarl_core::error::ParseError;
use jarl_core::rule_set::Category;
use poly_engine_api::config::EngineConfig;
use poly_engine_api::engine::{
    Capabilities, Diagnostic, Edit, Engine, OptionKeys, OptionTable, Severity, SourceFile, Span,
};
use poly_engine_api::language::Language;
use poly_engine_api::line_index::LineIndex;
use poly_engine_api::rule_config::RuleSelection;

use crate::jarl_config::{self, LINT_KEYS};
use crate::jarl_context;

/// The engine's id: the `[engines.jarl]` table and the `engine` of its findings.
pub const JARL_NAME: &str = "jarl";

/// What the cache key folds in: jarl at its pinned rev, the air parser and the ark semantic crates it is built on,
/// all in `Cargo.lock` (`tests/version_audit.rs`). Bump the trailing number when this crate's own mapping changes.
const VERSION: &str = "jarl-git:dc3f249+air-9b7627a+ark-d2d011b+map-1";

/// Where each rule is documented.
const RULE_DOCS: &str = "https://jarl.etiennebacher.com/rules/";

/// Tier-1 languages handled by this backend.
static LANGUAGES: &[Language] = &[Language::R];

/// The jarl linter.
#[derive(Debug, Default, Clone, Copy)]
pub struct JarlEngine;

impl Engine for JarlEngine {
    /// The lint cache key folds in the project's `jarl.toml` and the package files a verdict depends on.
    fn overridden_hooks(&self) -> u32 {
        poly_engine_api::desc::hooks::LINT_CACHE_SALT
    }

    fn name(&self) -> &'static str {
        JARL_NAME
    }

    fn languages(&self) -> &'static [Language] {
        LANGUAGES
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            lint: true,
            format: false,
            fix: true,
            project: false,
        }
    }

    fn option_keys(&self, table: OptionTable) -> OptionKeys {
        match table {
            OptionTable::Lint => OptionKeys::declared(LINT_KEYS).with_rule_selection(),
            OptionTable::Format | OptionTable::CrossCuttingLint => OptionKeys::declared(&[]),
        }
    }

    fn version(&self) -> &str {
        VERSION
    }

    fn lint_cache_salt(&self, src: &SourceFile) -> Option<String> {
        let config = jarl_config::digest(&src.path);
        let context = jarl_context::salt(&src.path, &src.content);
        match (config, context) {
            (None, None) => None,
            (config, context) => Some(format!(
                "{}|{}",
                config.unwrap_or_default(),
                context.unwrap_or_default()
            )),
        }
    }

    fn lint(&self, src: &SourceFile, cfg: &EngineConfig) -> anyhow::Result<Vec<Diagnostic>> {
        let config = jarl_config::config_for(&src.path, cfg)?;
        let levels = RuleSelection::from_options(cfg).rules;
        let path = jarl_config::absolute(&src.path);
        let index = LineIndex::new(&src.content);
        let (findings, syntax_errors) = match check_contents(&src.content, &path, (*config).clone()) {
            Ok(findings) => (findings, Vec::new()),
            Err(error) => {
                let parse = error.downcast::<ParseError>()?;
                (parse.diagnostics, parse.syntax_errors)
            }
        };
        let mut diagnostics: Vec<Diagnostic> = syntax_errors
            .into_iter()
            .map(|error| Diagnostic {
                engine: JARL_NAME.to_owned(),
                code: Some("syntax-error".to_owned()),
                severity: Severity::Error,
                title: error.message,
                description: None,
                span: Some(span(
                    &index,
                    usize::from(error.range.start()),
                    usize::from(error.range.end()),
                )),
                url: None,
                fix: Vec::new(),
                metadata: BTreeMap::new(),
            })
            .collect();
        diagnostics.extend(findings.iter().map(|finding| {
            let level = levels
                .get(finding.message.rule.name())
                .and_then(|options| options.level);
            convert(finding, &index, level)
        }));
        Ok(diagnostics)
    }
}

/// A 1-based span of the byte range `start..end` of the indexed text.
fn span(index: &LineIndex<'_>, start: usize, end: usize) -> Span {
    let (start_line, start_col) = index.line_col(start);
    let (end_line, end_col) = index.line_col(end.max(start));
    Span {
        start_line,
        start_col,
        end_line,
        end_col,
    }
}

/// One jarl finding as a poly finding. `level` is the user's override for the rule, if any.
fn convert(finding: &JarlDiagnostic, index: &LineIndex<'_>, level: Option<Severity>) -> Diagnostic {
    let rule = finding.message.rule;
    let default_level = if rule.has_category(Category::Corr) {
        Severity::Error
    } else {
        Severity::Warning
    };
    let mut metadata = BTreeMap::new();
    let fix = if finding.has_safe_fix() {
        finding
            .fix
            .edits
            .iter()
            .map(|edit| Edit {
                start_byte: edit.start(),
                end_byte: edit.end(),
                replacement: edit.content.clone(),
            })
            .collect()
    } else {
        if finding.has_unsafe_fix() {
            metadata.insert("fix".to_owned(), "unsafe".to_owned());
        }
        Vec::new()
    };
    Diagnostic {
        engine: JARL_NAME.to_owned(),
        code: Some(rule.name().to_owned()),
        severity: level.unwrap_or(default_level),
        title: finding.message.body.clone(),
        description: finding.message.suggestion.clone(),
        span: Some(span(
            index,
            usize::from(finding.range.start()),
            usize::from(finding.range.end()),
        )),
        url: Some(format!("{RULE_DOCS}{}", rule.name())),
        fix,
        metadata,
    }
}
