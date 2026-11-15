//! Typst formatter fixtures: known-unformatted, known-clean, idempotence and a rejected document.
#![cfg(feature = "typst")]

use poly_engine_api::{
    config::{EngineConfig, GlobalDefaults},
    engine::{Engine, FormatOutput, SourceFile},
    language::Language,
};
use poly_engines_misc::typst::TypstEngine;

fn engine_cfg() -> EngineConfig {
    EngineConfig {
        globals: GlobalDefaults::default(),
        indent_width: 2,
        options: toml::Table::new(),
    }
}

fn format(content: &str) -> FormatOutput {
    let src = SourceFile {
        path: "fixture.typ".into(),
        language: Language::Typst,
        content: content.into(),
    };
    TypstEngine
        .format(&src, &engine_cfg())
        .expect("typst format does not error")
}

fn format_to_string(content: &str) -> String {
    match format(content) {
        FormatOutput::Formatted(text) => text,
        FormatOutput::Unchanged => content.to_string(),
    }
}

const KNOWN_UNFORMATTED: &str = "#let x=1\n#let f(a,b)=a+b\n";
const KNOWN_FORMATTED: &str = "#let x = 1\n#let f(a, b) = a + b\n";

#[test]
fn known_unformatted_is_canonicalized() {
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
fn a_document_with_syntax_errors_stays_unchanged_without_error() {
    assert!(matches!(format("#let x = (\n"), FormatOutput::Unchanged));
}

#[test]
fn engine_is_format_only() {
    let caps = TypstEngine.capabilities();
    assert!(caps.format && !caps.lint && !caps.fix);
}
