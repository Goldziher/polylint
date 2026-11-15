//! Typst formatter backend, wrapping `typstyle-core`.
//!
//! Format-only: Typst has no lint rules here. A document with syntax errors is returned
//! [`FormatOutput::Unchanged`] (typstyle refuses to print it) rather than failing the run.
//!
//! # Options
//!
//! poly's layout globals are passed through: `max_width` is
//! [`poly_engine_api::config::GlobalDefaults::line_length`] (default 120, typstyle's own
//! default is 80), `tab_spaces` is [`EngineConfig::indent_width`] (2 for Typst) and generated
//! line breaks follow [`poly_engine_api::config::GlobalDefaults::line_ending`]. Every other
//! typstyle option keeps its upstream default; no per-engine option keys are declared.

use poly_engine_api::config::{EngineConfig, LineEnding as PolyLineEnding};
use poly_engine_api::engine::{Capabilities, Engine, FormatOutput, OptionKeys, OptionTable, SourceFile};
use poly_engine_api::engine_guard::refuse_pathological_nesting;
use poly_engine_api::language::Language;
use typstyle_core::{Config, LineEnding, Typstyle};

/// Cache-key version: the `typstyle-core` crate version plus a marker for this backend's option
/// mapping. Bump when the crate is updated or the mapping changes.
const TYPST_VERSION: &str = "typstyle-core-0.15.2+fmt1";

static LANGUAGES: &[Language] = &[Language::Typst];

/// Typst formatter backend. See the module docs.
pub struct TypstEngine;

impl Engine for TypstEngine {
    fn overridden_hooks(&self) -> u32 {
        0
    }

    fn name(&self) -> &'static str {
        "typstyle"
    }

    fn languages(&self) -> &'static [Language] {
        LANGUAGES
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            lint: false,
            format: true,
            fix: false,
            project: false,
        }
    }

    fn option_keys(&self, _table: OptionTable) -> OptionKeys {
        OptionKeys::declared(&[])
    }

    fn version(&self) -> &str {
        TYPST_VERSION
    }

    fn format(&self, src: &SourceFile, cfg: &EngineConfig) -> anyhow::Result<FormatOutput> {
        refuse_pathological_nesting(&src.path, &src.content)?;
        let config = Config::new()
            .with_width(cfg.globals.line_length)
            .with_tab_spaces(cfg.indent_width)
            .with_line_ending(match cfg.globals.line_ending {
                PolyLineEnding::Crlf => LineEnding::Crlf,
                PolyLineEnding::Lf => LineEnding::Lf,
            });
        let typstyle = Typstyle::new(config);
        let rendered = typstyle.format_text(&*src.content).render();
        match rendered {
            Ok(formatted) if formatted != *src.content => Ok(FormatOutput::Formatted(formatted)),
            _ => Ok(FormatOutput::Unchanged),
        }
    }
}
