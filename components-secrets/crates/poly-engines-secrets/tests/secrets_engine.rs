//! Fixtures for the kingfisher-backed secret scanner.
//!
//! The fake credential is a GitHub-style personal access token whose 36-character tail is random text with no meaning.
//! It is assembled at run time so no source line carries the whole token. `betterleaks.github-pat` matches the shape,
//! and its entropy filter accepts the tail.

use std::sync::Arc;

use poly_engine_api::config::{EngineConfig, GlobalDefaults};
use poly_engine_api::engine::{Diagnostic, Engine, OptionTable, Severity, SourceFile};
use poly_engine_api::language::Language;
use poly_engines_secrets::{ENGINE_NAME, SecretsEngine};

const TAIL: &str = "e1qlnTsd5awqKXOhdBYm9PuddQWMOumr7sKv";
const RULE: &str = "secrets/betterleaks.github-pat";

fn token() -> String {
    format!("ghp_{TAIL}")
}

fn config(enabled: Option<bool>) -> EngineConfig {
    let mut options = toml::Table::new();
    if let Some(enabled) = enabled {
        options.insert("enabled".into(), toml::Value::Boolean(enabled));
    }
    EngineConfig {
        globals: GlobalDefaults::default(),
        indent_width: 4,
        options,
    }
}

fn source(name: &str, language: Language, content: &str) -> SourceFile {
    SourceFile {
        path: name.into(),
        language,
        content: Arc::from(content),
    }
}

fn lint(content: &str, enabled: Option<bool>) -> Vec<Diagnostic> {
    SecretsEngine
        .lint(&source("settings.py", Language::Python, content), &config(enabled))
        .expect("the scan runs")
}

fn known_bad() -> String {
    format!("# deploy settings\n\nGITHUB_TOKEN = \"{}\"\n", token())
}

#[test]
fn a_known_fake_token_is_reported_with_its_rule_and_line() {
    let diagnostics = lint(&known_bad(), Some(true));
    let hit = diagnostics
        .iter()
        .find(|diagnostic| diagnostic.code.as_deref() == Some(RULE))
        .unwrap_or_else(|| panic!("no {RULE} finding in {diagnostics:?}"));
    assert_eq!(hit.engine, ENGINE_NAME);
    assert_eq!(hit.severity, Severity::Error);
    let span = hit.span.expect("a finding carries its location");
    assert_eq!((span.start_line, span.end_line), (3, 3));
    let line = known_bad().lines().nth(2).unwrap().to_owned();
    let column = line.find(&token()).expect("the token is on line 3") + 1;
    assert_eq!(span.start_col as usize, column);
    assert!(hit.fix.is_empty(), "a leaked secret has no mechanical fix");
}

#[test]
fn clean_files_report_nothing() {
    assert!(lint("def main() -> None:\n    print(\"hello\")\n", Some(true)).is_empty());
    assert!(lint("token: ${{ secrets.GITHUB_TOKEN }}\n", Some(true)).is_empty());
    assert!(lint("", Some(true)).is_empty());
}

#[test]
fn no_diagnostic_carries_the_secret() {
    let diagnostics = lint(&known_bad(), Some(true));
    assert!(!diagnostics.is_empty());
    let rendered = format!("{diagnostics:?}");
    let serialized = serde_json::to_string(&diagnostics).expect("diagnostics serialize");
    for text in [&rendered, &serialized] {
        assert!(!text.contains(TAIL), "the secret leaked into: {text}");
        assert!(!text.contains(&token()), "the token leaked into: {text}");
    }
    let hit = diagnostics
        .iter()
        .find(|diagnostic| diagnostic.code.as_deref() == Some(RULE))
        .expect("the github rule fires");
    assert!(hit.title.contains("40 bytes"), "{}", hit.title);
}

#[test]
fn findings_are_deterministic() {
    let first = format!("{:?}", lint(&known_bad(), Some(true)));
    let second = format!("{:?}", lint(&known_bad(), Some(true)));
    assert_eq!(first, second);
}

#[test]
fn the_engine_is_off_until_enabled() {
    assert!(
        lint(&known_bad(), None).is_empty(),
        "opt-in: an absent key scans nothing"
    );
    assert!(lint(&known_bad(), Some(false)).is_empty());
    assert!(!lint(&known_bad(), Some(true)).is_empty());
}

#[test]
fn the_engine_is_cross_cutting_lint_only_and_claims_no_language_coverage() {
    let engine = SecretsEngine;
    assert_eq!(engine.name(), "secrets");
    assert!(engine.languages().is_empty());
    let capabilities = engine.capabilities();
    assert!(capabilities.lint && !capabilities.format && !capabilities.fix && !capabilities.project);
    let cfg = config(Some(true));
    for language in [Language::Python, Language::Rust, Language::Go] {
        assert!(
            !engine.provides_language_lint(&language, &cfg),
            "scanning for secrets is not lint coverage of {language:?}"
        );
    }
}

#[test]
fn only_the_enabled_key_is_declared() {
    for table in [OptionTable::Lint, OptionTable::CrossCuttingLint] {
        let keys = SecretsEngine.option_keys(table);
        assert!(keys.is_checked());
        let names: Vec<_> = keys.declared_keys().iter().map(|(name, _)| *name).collect();
        assert_eq!(names, ["enabled"]);
    }
}
