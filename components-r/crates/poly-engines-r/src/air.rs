//! R formatting with [air](https://github.com/posit-dev/air), Posit's R formatter, in process.
//!
//! The engine runs air's own parser and formatter (`air_r_parser`, `air_r_formatter`) over the file, so its output
//! is what `air format` writes for the same source and settings. Settings come from the project's `air.toml`, as
//! the CLI reads them, and `[engines.air]` keys override it (see [`crate::air_config`] for the layering):
//!
//! | Key | Type | air default | Description |
//! |-----|------|-------------|-------------|
//! | `line_width` | integer | 80 (poly: 120) | The width lines wrap at |
//! | `indent_width` | integer | 2 | Spaces per indent level, and the width of a tab |
//! | `indent_style` | string | `"space"` | `"space"` or `"tab"` |
//! | `line_ending` | string | `"auto"` (poly: `"lf"`) | `"auto"`, `"lf"`, `"crlf"` or `"native"` |
//! | `persistent_line_breaks` | bool | `true` | Respect line breaks that keep a call or signature expanded |
//! | `assignment_style` | string | `"arrow"` | `"arrow"`, `"equal"` or `"preserve"` |
//! | `skip` | array | none | Functions whose calls are left as written |
//! | `table` | array | none | Functions whose calls are formatted as tables |
//! | `default_table` | bool | `true` | Format `tribble()`, `fcase()` and `rowwiseDT()` as tables |
//!
//! A file that does not parse is an error, not a clean file: air refuses it and so does this engine.

use std::borrow::Cow;

use air_r_formatter::context::RFormatOptions;
use air_r_parser::RParserOptions;
use poly_engine_api::config::EngineConfig;
use poly_engine_api::engine::{Capabilities, Engine, FormatOutput, OptionKeys, OptionTable, SourceFile};
use poly_engine_api::language::Language;

use crate::air_config::{self, FORMAT_KEYS};

/// The engine's id: the `[engines.air]` table and the `engine` of its findings.
pub const AIR_NAME: &str = "air";

/// What the cache key folds in: air at its pinned rev, and the biome formatter it is built on, both in `Cargo.lock`
/// (`tests/version_audit.rs`). Bump the trailing number when this crate's own handling of the options changes.
const VERSION: &str = "air-git:9b7627a+biome-8c43bc0+opts-1";

/// Tier-1 languages handled by this backend.
static LANGUAGES: &[Language] = &[Language::R];

/// The air formatter.
#[derive(Debug, Default, Clone, Copy)]
pub struct AirEngine;

impl Engine for AirEngine {
    /// The format cache key folds in the project's `air.toml`, and `skip_reason` leaves out what its patterns name.
    ///
    /// `file_context` repeats the salt because the stat fast path keys a file by it, and a component host asks the
    /// component only the hooks declared here.
    fn overridden_hooks(&self) -> u32 {
        use poly_engine_api::desc::hooks;
        hooks::FILE_CONTEXT | hooks::FORMAT_INPUT_SALT | hooks::SKIP_REASON
    }

    fn name(&self) -> &'static str {
        AIR_NAME
    }

    fn languages(&self) -> &'static [Language] {
        LANGUAGES
    }

    /// Format only: air has no lint rules (jarl is the R linter).
    fn capabilities(&self) -> Capabilities {
        Capabilities {
            lint: false,
            format: true,
            fix: false,
            project: false,
        }
    }

    fn option_keys(&self, table: OptionTable) -> OptionKeys {
        match table {
            OptionTable::Format => OptionKeys::declared(FORMAT_KEYS),
            OptionTable::Lint | OptionTable::CrossCuttingLint => OptionKeys::declared(&[]),
        }
    }

    fn version(&self) -> &str {
        VERSION
    }

    fn format_input_salt(&self, path: &std::path::Path, _cfg: &EngineConfig) -> Option<String> {
        air_config::digest(path)
    }

    fn file_context(&self, path: &std::path::Path, cfg: &EngineConfig) -> Option<String> {
        self.format_input_salt(path, cfg)
    }

    /// Files air's own `exclude` and `default-exclude` patterns leave out (`renv/`, `RcppExports.R`, ...), as
    /// `air format` leaves them out of a directory walk and of an explicit path alike.
    fn skip_reason(&self, src: &SourceFile) -> Option<Cow<'static, str>> {
        let project = air_config::find(&src.path).ok().flatten();
        let settings = match &project {
            Some(found) => air_config::project_settings(found).ok()?,
            None => air_workspace::settings::Settings::default(),
        };
        let pattern = air_config::excluded_by(&settings, &src.path)?;
        Some(Cow::Owned(format!("excluded by air's `{pattern}` pattern")))
    }

    fn format(&self, src: &SourceFile, cfg: &EngineConfig) -> anyhow::Result<FormatOutput> {
        let project = air_config::find(&src.path)?;
        let settings = air_config::resolve(cfg, project.as_ref())?;
        let options = settings.format.to_format_options(&src.content);
        let formatted =
            format_source(&src.content, options).map_err(|error| error.context(src.path.display().to_string()))?;
        if formatted == *src.content {
            Ok(FormatOutput::Unchanged)
        } else {
            Ok(FormatOutput::Formatted(formatted))
        }
    }
}

/// Parse and format `source`, as `air format` does.
fn format_source(source: &str, options: RFormatOptions) -> anyhow::Result<String> {
    let parse = air_r_parser::parse(source, RParserOptions::default());
    if parse.has_errors() {
        let first = parse
            .diagnostics()
            .first()
            .map_or_else(String::new, |diagnostic| format!(": {}", diagnostic.message));
        anyhow::bail!("air could not parse the file{first}");
    }
    let formatted = air_r_formatter::format_node(options, &parse.syntax())
        .map_err(|error| anyhow::anyhow!("air could not format the file: {error}"))?;
    let printed = formatted
        .print()
        .map_err(|error| anyhow::anyhow!("air could not print the formatted file: {error}"))?;
    Ok(printed.into_code())
}
