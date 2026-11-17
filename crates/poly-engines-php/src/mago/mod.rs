//! PHP backend: lint + format via the `mago` toolchain (carthage-software/mago,
//! MIT OR Apache-2.0).
//!
//! # Lint (`lint.rs`)
//! Parses the file with `mago-syntax`, resolves names with `mago-names`, then
//! runs every enabled `mago-linter` rule. Parse errors in `program.errors` are
//! surfaced first (code `"syntax"` / `"parse"`, [`poly_engine_api::engine::Severity`]
//! `::Error`), followed by the linter's `mago_reporting::IssueCollection`
//! mapped to poly [`Diagnostic`]s. Where a lint issue carries exactly one
//! safe `mago_text_edit::TextEdit`, a [`poly_engine_api::engine::Edit`] fix is wired so
//! `--fix` can apply it.
//!
//! # Format (`format.rs`)
//! Delegates to [`mago_formatter::Formatter`] with settings derived from
//! [`EngineConfig`]: `line_length → print_width`, `indent_width → tab_width`.
//! Returns [`FormatOutput::Unchanged`] when the formatted output equals the
//! input or when parsing fails (the lint pass surfaces the parse error instead).
//!
//! # PHP version
//! Defaults to PHP 8.4, the latest stable version at the time of writing.
//!
//! # Capabilities
//! Lint, format, and single-edit safe autofixes.
//!
//! # Registry caching
//! [`mago_linter::Linter::new`] rebuilds the rule registry on every call.
//! [`MagoEngine`] instead keeps a bounded map from [`EngineConfig::fingerprint`]
//! to the registry and option-derived settings, so files governed by different
//! nested `poly.toml` configs each get a registry matching their own options.

mod format;
mod lint;
pub(super) mod rules;

use poly_engine_api::config::EngineConfig;
use poly_engine_api::engine::{
    Capabilities, Diagnostic, Engine, FormatOutput, OptionKeys, OptionTable, OptionType, SourceFile,
};
use poly_engine_api::engine_guard::refuse_pathological_nesting;
use poly_engine_api::language::Language;

/// PHP backend using the `mago` linter + formatter.
///
/// Holds per-configuration lint plans so the expensive `RuleRegistry::build`
/// step runs once per distinct configuration rather than per file.
#[derive(Default)]
pub struct MagoEngine {
    /// Lint plans shared across all `lint` calls on this instance.
    plans: lint::PlanCache,
}

/// Languages handled by this backend.
pub static LANGUAGES: &[Language] = &[Language::Php];

impl Engine for MagoEngine {
    fn overridden_hooks(&self) -> u32 {
        0
    }

    fn name(&self) -> &'static str {
        "mago"
    }

    fn languages(&self) -> &'static [Language] {
        LANGUAGES
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            lint: true,
            format: true,
            fix: true,
            project: false,
        }
    }

    /// Cache-key version: bump whenever mago output could change.
    ///
    /// `+advisory-sev1`: the `Maintainability` metric rules report `Warning`
    /// instead of mago's `Error` (see `lint::ADVISORY_RULE_CODES`).
    /// `[fmt.php.mago]`'s keys are the fields of mago's own
    /// `RawFormatSettings`, derived from that type rather than copied: the list
    /// is 96 kebab-case entries that change with every mago release, and a
    /// hand-written copy would drift on the first upgrade.
    fn option_keys(&self, table: OptionTable) -> OptionKeys {
        match table {
            OptionTable::Lint => {
                OptionKeys::declared(&[("php_version", OptionType::STRING), ("integrations", OptionType::ARRAY)])
                    .with_rule_selection()
            }
            OptionTable::Format => OptionKeys::declared(&[("php_version", OptionType::STRING)]).with_derived(
                poly_engine_api::option_probe::recognized_by_deserialize::<mago_formatter::settings::RawFormatSettings>,
            ),
            OptionTable::CrossCuttingLint => OptionKeys::UNCHECKED,
        }
    }

    fn version(&self) -> &str {
        "mago-1.60.2+advisory-sev1+filename+fmt-fields"
    }

    fn lint(&self, src: &SourceFile, cfg: &EngineConfig) -> anyhow::Result<Vec<Diagnostic>> {
        refuse_pathological_nesting(&src.path, &src.content)?;
        lint::lint_php(src, cfg, &self.plans)
    }

    fn format(&self, src: &SourceFile, cfg: &EngineConfig) -> anyhow::Result<FormatOutput> {
        refuse_pathological_nesting(&src.path, &src.content)?;
        format::format_php(src, cfg)
    }
}
