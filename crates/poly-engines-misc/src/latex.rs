//! LaTeX formatter backend, wrapping `tex-fmt`.
//!
//! Format-only: there are no lint rules. `tex-fmt` is tolerant by design (unbalanced environments
//! only produce its own warnings, which are discarded here), so every input yields a result.
//!
//! # Options
//!
//! poly's layout globals are passed through: `wraplen` is
//! [`poly_engine_api::config::GlobalDefaults::line_length`] (default 120, tex-fmt's own default
//! is 80) and `tabsize` is [`EngineConfig::indent_width`] (2 for LaTeX). Every other tex-fmt
//! option keeps its upstream default; no per-engine option keys are declared.
//!
//! # Line endings
//!
//! `tex-fmt` terminates lines with the host's native ending, so the output is normalised to `\n`
//! and converted to `\r\n` only when the input used it, keeping results identical across hosts.
//!
//! BibTeX (`.bib`) is deliberately not claimed: it is not LaTeX and poly does not detect it.

use std::path::Path;

use poly_engine_api::config::EngineConfig;
use poly_engine_api::engine::{Capabilities, Engine, FormatOutput, OptionKeys, OptionTable, SourceFile};
use poly_engine_api::language::Language;
use tex_fmt::args::{Args, OptionArgs};
use tex_fmt::format::format_file;

/// Cache-key version: the `tex-fmt` crate version plus a marker for this backend's option
/// mapping. Bump when the crate is updated or the mapping changes.
const LATEX_VERSION: &str = "tex-fmt-0.5.7+fmt1";

/// Largest indent width handed to tex-fmt, whose indent arithmetic is `i8`-based.
const MAX_TAB_SIZE: u8 = 16;

static LANGUAGES: &[Language] = &[Language::Latex];

/// LaTeX formatter backend. See the module docs.
pub struct LatexEngine;

impl Engine for LatexEngine {
    fn overridden_hooks(&self) -> u32 {
        0
    }

    fn name(&self) -> &'static str {
        "tex-fmt"
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
        LATEX_VERSION
    }

    fn format(&self, src: &SourceFile, cfg: &EngineConfig) -> anyhow::Result<FormatOutput> {
        let args = Args::from(OptionArgs {
            wraplen: Some(cfg.globals.line_length),
            tabsize: Some(u8::try_from(cfg.indent_width).unwrap_or(MAX_TAB_SIZE).min(MAX_TAB_SIZE)),
            ..OptionArgs::default()
        });

        let mut logs = Vec::new();
        let formatted = format_file(&src.content, Path::new(&src.path), &args, &mut logs);
        let formatted = normalize_line_endings(&formatted, src.content.contains("\r\n"));

        if formatted == *src.content {
            Ok(FormatOutput::Unchanged)
        } else {
            Ok(FormatOutput::Formatted(formatted))
        }
    }
}

/// Rewrite tex-fmt's host-dependent line endings to `\n`, or to `\r\n` when the source used them.
fn normalize_line_endings(text: &str, crlf: bool) -> String {
    let lf = text.replace("\r\n", "\n");
    if crlf { lf.replace('\n', "\r\n") } else { lf }
}
