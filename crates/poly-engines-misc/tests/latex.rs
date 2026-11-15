//! LaTeX formatter fixtures: known-unformatted, known-clean, idempotence and a verbatim region.
#![cfg(feature = "latex")]

use poly_engine_api::{
    config::{EngineConfig, GlobalDefaults},
    engine::{Engine, FormatOutput, SourceFile},
    language::Language,
};
use poly_engines_misc::latex::LatexEngine;

fn engine_cfg() -> EngineConfig {
    EngineConfig {
        globals: GlobalDefaults::default(),
        indent_width: 2,
        options: toml::Table::new(),
    }
}

fn format(content: &str) -> FormatOutput {
    let src = SourceFile {
        path: "fixture.tex".into(),
        language: Language::Latex,
        content: content.into(),
    };
    LatexEngine
        .format(&src, &engine_cfg())
        .expect("latex format does not error")
}

fn format_to_string(content: &str) -> String {
    match format(content) {
        FormatOutput::Formatted(text) => text,
        FormatOutput::Unchanged => content.to_string(),
    }
}

const KNOWN_UNFORMATTED: &str = "\\begin{center}\nhello\n\\end{center}\n";
const KNOWN_FORMATTED: &str = "\\begin{center}\n  hello\n\\end{center}\n";

#[test]
fn known_unformatted_is_indented() {
    assert_eq!(format_to_string(KNOWN_UNFORMATTED), KNOWN_FORMATTED);
}

#[test]
fn known_clean_stays_unchanged() {
    assert!(matches!(format(KNOWN_FORMATTED), FormatOutput::Unchanged));
}

#[test]
fn formatting_is_idempotent() {
    let once = format_to_string(KNOWN_UNFORMATTED);
    assert_eq!(format_to_string(&once), once);
}

#[test]
fn verbatim_regions_are_left_alone() {
    let verbatim = "\\begin{verbatim}\n   keep   this\n\\end{verbatim}\n";
    assert!(matches!(format(verbatim), FormatOutput::Unchanged));
}

#[test]
fn crlf_input_keeps_crlf_output() {
    let formatted = format_to_string("\\begin{center}\r\nhello\r\n\\end{center}\r\n");
    assert_eq!(formatted, "\\begin{center}\r\n  hello\r\n\\end{center}\r\n");
}

#[test]
fn engine_is_format_only() {
    let caps = LatexEngine.capabilities();
    assert!(caps.format && !caps.lint && !caps.fix);
}
