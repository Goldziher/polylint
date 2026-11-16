//! Fixtures for the harper prose backend: a known-bad Markdown file and a source file whose
//! comments are known-bad, each asserting the exact findings and where they land.

use poly_engine_api::config::{EngineConfig, GlobalDefaults};
use poly_engine_api::engine::{Diagnostic, Engine, Severity, SourceFile};
use poly_engine_api::language::Language;
use poly_engines_prose::HarperEngine;

const KNOWN_BAD_MD: &str = include_str!("fixtures/known_bad.md");
const KNOWN_BAD_RS: &str = include_str!("fixtures/known_bad.rs");

fn config(options: &str) -> EngineConfig {
    EngineConfig {
        globals: GlobalDefaults::default(),
        indent_width: 4,
        options: options.parse().expect("test options are valid TOML"),
    }
}

fn lint(language: Language, content: &str, options: &str) -> Vec<Diagnostic> {
    let src = SourceFile {
        path: "fixture".into(),
        language,
        content: content.into(),
    };
    HarperEngine.lint(&src, &config(options)).expect("harper lint")
}

/// `(code, line)` of every finding, in report order.
fn findings(diagnostics: &[Diagnostic]) -> Vec<(&str, u32)> {
    diagnostics
        .iter()
        .map(|d| {
            (
                d.code.as_deref().unwrap_or(""),
                d.span.as_ref().map_or(0, |s| s.start_line),
            )
        })
        .collect()
}

#[test]
fn markdown_prose_is_checked_and_code_is_not() {
    let diagnostics = lint(Language::Markdown, KNOWN_BAD_MD, "enabled = true");
    let found = findings(&diagnostics);
    assert!(found.contains(&("RepeatedWords", 7)), "{found:?}");
    assert!(found.contains(&("AnA", 7)), "{found:?}");
    assert!(
        found.iter().all(|(_, line)| *line == 7),
        "front matter, the fenced block and inline code must be skipped: {found:?}"
    );
    assert!(
        diagnostics
            .iter()
            .all(|d| d.severity == Severity::Warning && d.fix.is_empty())
    );
    assert!(diagnostics.iter().all(|d| d.engine == "harper"));
}

#[test]
fn source_comments_are_checked_and_code_is_not() {
    let diagnostics = lint(Language::Rust, KNOWN_BAD_RS, "enabled = true");
    let found = findings(&diagnostics);
    assert!(found.contains(&("RepeatedWords", 1)), "{found:?}");
    assert!(found.contains(&("RepeatedWords", 3)), "{found:?}");
    assert!(
        found.iter().all(|(_, line)| matches!(line, 1 | 3)),
        "identifiers outside comments must not be read: {found:?}"
    );
}

#[test]
fn a_finding_points_at_the_words_in_the_file() {
    let diagnostics = lint(Language::Rust, KNOWN_BAD_RS, "enabled = true");
    let repeated = diagnostics
        .iter()
        .find(|d| d.code.as_deref() == Some("RepeatedWords") && d.span.as_ref().is_some_and(|s| s.start_line == 1))
        .expect("repeated words on line 1");
    let span = repeated.span.as_ref().expect("span");
    let line = KNOWN_BAD_RS.lines().next().expect("first line");
    let column = |col: u32| usize::try_from(col).expect("column fits") - 1;
    let flagged = &line[column(span.start_col)..column(span.end_col)];
    assert!(flagged.contains("the"), "flagged {flagged:?} in {line:?}");
    assert!(
        column(span.start_col) >= "/// Returns ".len(),
        "span starts inside the comment text"
    );
}

#[test]
fn it_is_off_until_enabled() {
    assert!(lint(Language::Markdown, KNOWN_BAD_MD, "").is_empty());
    assert!(lint(Language::Markdown, KNOWN_BAD_MD, "enabled = false").is_empty());
}

#[test]
fn comments_can_be_switched_off() {
    assert!(lint(Language::Rust, KNOWN_BAD_RS, "enabled = true\ncomments = false").is_empty());
}

#[test]
fn ignore_removes_a_rule_and_select_replaces_the_set() {
    let ignored = lint(
        Language::Markdown,
        KNOWN_BAD_MD,
        "enabled = true\nignore = [\"RepeatedWords\"]",
    );
    assert!(findings(&ignored).iter().all(|(code, _)| *code != "RepeatedWords"));
    assert!(!ignored.is_empty(), "AnA still reports");

    let only = lint(
        Language::Markdown,
        KNOWN_BAD_MD,
        "enabled = true\nselect = [\"repeated-words\"]",
    );
    assert!(findings(&only).iter().all(|(code, _)| *code == "RepeatedWords"));
    assert!(!only.is_empty());
}

#[test]
fn spell_check_is_opt_in_and_honours_the_dictionary() {
    let text = "The zorbulator handles every request.\n";
    assert!(
        lint(Language::Markdown, text, "enabled = true").is_empty(),
        "off by default"
    );
    let spelled = lint(
        Language::Markdown,
        text,
        "enabled = true\nextend_select = [\"SpellCheck\"]",
    );
    assert!(findings(&spelled).iter().any(|(code, _)| *code == "SpellCheck"));
    let known = lint(
        Language::Markdown,
        text,
        "enabled = true\nextend_select = [\"SpellCheck\"]\ndictionary = [\"zorbulator\"]",
    );
    assert!(findings(&known).iter().all(|(code, _)| *code != "SpellCheck"));
}

#[test]
fn a_language_without_a_grammar_has_no_comments_to_read() {
    assert!(
        lint(
            Language::Other("no-such-grammar".into()),
            KNOWN_BAD_RS,
            "enabled = true"
        )
        .is_empty()
    );
}
