//! Insta fixtures for the jarl linter.
//!
//! The expected findings are what `jarl check` reports for the same file, built from the same revision
//! (`jarl-core` dc3f249): the rule, its message and suggestion, the range and the fix.

use std::path::Path;

use poly_engine_api::config::{EngineConfig, GlobalDefaults};
use poly_engine_api::engine::{Diagnostic, Engine, Severity, SourceFile};
use poly_engine_api::language::Language;
use poly_engines_r::{JarlEngine, engines_for};

/// Three default-on rules: `any_is_na` (performance, safe fix), `equals_na` (correctness, safe fix) and `seq`
/// (suspicious, safe fix).
const KNOWN_BAD: &str =
    "x <- c(1, 2, NA)\nif (any(is.na(x))) {\n  print(\"has missing values\")\n}\ny <- x == NA\nz <- 1:length(x)\n";

fn config(options: &str) -> EngineConfig {
    EngineConfig {
        globals: GlobalDefaults::default(),
        indent_width: Language::R.default_indent_width(),
        options: options.parse().expect("test options are TOML"),
    }
}

fn source(path: &Path, content: &str) -> SourceFile {
    SourceFile {
        path: path.to_path_buf(),
        language: Language::R,
        content: content.into(),
    }
}

fn lint(dir: &Path, content: &str, options: &str) -> Vec<Diagnostic> {
    JarlEngine
        .lint(&source(&dir.join("fixture.R"), content), &config(options))
        .expect("jarl lints the fixture")
}

fn level(severity: Severity) -> &'static str {
    match severity {
        Severity::Error => "error",
        Severity::Warning => "warning",
        Severity::Info => "info",
        Severity::Hint => "hint",
    }
}

fn codes(diagnostics: &[Diagnostic]) -> Vec<&str> {
    diagnostics
        .iter()
        .map(|diagnostic| diagnostic.code.as_deref().unwrap_or(""))
        .collect()
}

/// One line per finding: span, rule, severity, message, suggestion and fix edits.
fn summarize(diagnostics: &[Diagnostic]) -> String {
    diagnostics
        .iter()
        .map(|d| {
            let span = d.span.expect("jarl findings carry a span");
            let fix: Vec<String> = d
                .fix
                .iter()
                .map(|edit| format!("{}..{} -> {}", edit.start_byte, edit.end_byte, edit.replacement))
                .collect();
            format!(
                "{}:{}-{}:{} {} {} {} | {} | fix [{}] | {}",
                span.start_line,
                span.start_col,
                span.end_line,
                span.end_col,
                d.code.as_deref().unwrap_or("-"),
                level(d.severity),
                d.title,
                d.description.as_deref().unwrap_or("-"),
                fix.join(", "),
                d.url.as_deref().unwrap_or("-"),
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Apply every finding's fix to `source`, last edit first so earlier offsets stay valid.
fn apply_fixes(source: &str, diagnostics: &[Diagnostic]) -> String {
    let mut edits: Vec<_> = diagnostics.iter().flat_map(|d| d.fix.iter()).collect();
    edits.sort_by_key(|edit| std::cmp::Reverse(edit.start_byte));
    let mut text = source.to_owned();
    for edit in edits {
        text.replace_range(edit.start_byte..edit.end_byte, &edit.replacement);
    }
    text
}

#[test]
fn known_bad_file_reports_what_the_jarl_cli_reports() {
    let dir = tempfile::tempdir().unwrap();
    let diagnostics = lint(dir.path(), KNOWN_BAD, "");
    insta::assert_snapshot!("jarl_known_bad_findings", summarize(&diagnostics));
}

#[test]
fn a_correctness_rule_is_an_error_and_the_others_warnings() {
    let dir = tempfile::tempdir().unwrap();
    let levels: Vec<(String, &str)> = lint(dir.path(), KNOWN_BAD, "")
        .iter()
        .map(|d| (d.code.clone().unwrap(), level(d.severity)))
        .collect();
    assert_eq!(
        levels,
        [
            ("any_is_na".to_owned(), "warning"),
            ("equals_na".to_owned(), "error"),
            ("seq".to_owned(), "warning"),
        ]
    );
}

#[test]
fn safe_fixes_apply_and_leave_a_file_jarl_accepts() {
    let dir = tempfile::tempdir().unwrap();
    let diagnostics = lint(dir.path(), KNOWN_BAD, "");
    let fixed = apply_fixes(KNOWN_BAD, &diagnostics);
    assert_eq!(
        fixed,
        "x <- c(1, 2, NA)\nif (anyNA(x)) {\n  print(\"has missing values\")\n}\ny <- is.na(x)\nz <- seq_along(x)\n"
    );
    assert_eq!(codes(&lint(dir.path(), &fixed, "")), Vec::<&str>::new());
}

#[test]
fn an_unsafe_fix_is_reported_but_not_attached() {
    let dir = tempfile::tempdir().unwrap();
    let text = "a <- 1\nb <- 2\nif (all.equal(a, b)) {\n  print(\"same\")\n}\n";
    let diagnostics = lint(dir.path(), text, "");
    assert_eq!(codes(&diagnostics), ["all_equal"]);
    assert!(diagnostics[0].fix.is_empty());
    assert_eq!(diagnostics[0].metadata.get("fix").map(String::as_str), Some("unsafe"));
}

#[test]
fn select_and_ignore_pick_the_rules() {
    let dir = tempfile::tempdir().unwrap();
    assert_eq!(codes(&lint(dir.path(), KNOWN_BAD, "select = [\"seq\"]")), ["seq"]);
    assert_eq!(
        codes(&lint(dir.path(), KNOWN_BAD, "ignore = [\"any_is_na\", \"seq\"]")),
        ["equals_na"]
    );
    assert_eq!(
        codes(&lint(dir.path(), KNOWN_BAD, "select = [\"PERF\"]")),
        ["any_is_na"],
        "a category selects its rules"
    );
}

#[test]
fn a_rule_level_overrides_the_default_severity() {
    let dir = tempfile::tempdir().unwrap();
    let diagnostics = lint(
        dir.path(),
        KNOWN_BAD,
        "[rules.seq]\nlevel = \"info\"\n[rules.equals_na]\nlevel = \"warning\"\n",
    );
    let levels: Vec<_> = diagnostics.iter().map(|d| level(d.severity)).collect();
    assert_eq!(levels, ["warning", "warning", "info"]);
}

#[test]
fn the_project_jarl_toml_is_read_and_options_are_laid_over_it() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("jarl.toml"), "[lint]\nignore = [\"any_is_na\"]\n").unwrap();
    assert_eq!(codes(&lint(dir.path(), KNOWN_BAD, "")), ["equals_na", "seq"]);
    assert_eq!(
        codes(&lint(dir.path(), KNOWN_BAD, "select = [\"seq\", \"equals_na\"]")),
        ["equals_na", "seq"],
        "the file's ignore list still applies beside the options' select"
    );
}

#[test]
fn rule_options_beside_level_are_the_rule_s_jarl_toml_options() {
    let dir = tempfile::tempdir().unwrap();
    let text = "x <- 1\ny = 2\n";
    let diagnostics = lint(
        dir.path(),
        text,
        "extend_select = [\"assignment\"]\n[rules.assignment]\noperator = \"=\"\n",
    );
    assert_eq!(codes(&diagnostics), ["assignment"]);
    assert_eq!(apply_fixes(text, &diagnostics), "x = 1\ny = 2\n");
}

#[test]
fn a_syntax_error_is_an_error_finding_with_the_parser_s_message() {
    let dir = tempfile::tempdir().unwrap();
    let diagnostics = lint(dir.path(), "f <- function( {\n", "");
    assert_eq!(codes(&diagnostics), ["syntax-error", "syntax-error"]);
    assert!(diagnostics.iter().all(|d| d.severity == Severity::Error));
    assert_eq!(diagnostics[0].title, "Expected a parameter name but instead found '{'.");
    let span = diagnostics[0].span.unwrap();
    assert_eq!((span.start_line, span.start_col), (1, 16));
}

#[test]
fn a_generated_file_is_not_linted() {
    let dir = tempfile::tempdir().unwrap();
    assert!(lint(dir.path(), "# Generated by roxygen2\nz <- 1:length(x)\n", "").is_empty());
}

#[test]
fn the_cache_key_follows_jarl_toml_and_the_package_around_the_file() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("fixture.R");
    let script = source(&file, "x <- 1\n");
    assert_eq!(
        JarlEngine.lint_cache_salt(&script),
        None,
        "a lone script depends on nothing else"
    );

    std::fs::write(dir.path().join("jarl.toml"), "[lint]\nignore = [\"seq\"]\n").unwrap();
    let first = JarlEngine
        .lint_cache_salt(&script)
        .expect("jarl.toml is part of the key");
    std::fs::write(
        dir.path().join("jarl.toml"),
        "[lint]\nignore = [\"seq\", \"any_is_na\"]\n",
    )
    .unwrap();
    let second = JarlEngine
        .lint_cache_salt(&script)
        .expect("jarl.toml is part of the key");
    assert_ne!(first, second);

    let sourcing = source(&file, "source(\"helpers.R\")\n");
    assert_ne!(
        JarlEngine.lint_cache_salt(&sourcing),
        JarlEngine.lint_cache_salt(&script),
        "a script that sources another depends on its neighbours"
    );
}

#[test]
fn a_package_file_s_cache_key_digests_the_whole_package() {
    let package = |helper: &str| {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("DESCRIPTION"), "Package: demo\nVersion: 0.0.1\n").unwrap();
        std::fs::create_dir(dir.path().join("R")).unwrap();
        std::fs::write(dir.path().join("R/main.R"), "main <- function() helper()\n").unwrap();
        std::fs::write(dir.path().join("R/helper.R"), helper).unwrap();
        let salt = JarlEngine.lint_cache_salt(&source(&dir.path().join("R/main.R"), "main <- function() helper()\n"));
        (dir, salt)
    };
    let (_one, same_a) = package("helper <- function() 1\n");
    let (_two, same_b) = package("helper <- function() 1\n");
    let (_three, other) = package("helper <- function() 2\n");
    assert!(same_a.is_some());
    assert_eq!(same_a, same_b, "the digest does not depend on where the package lives");
    assert_ne!(same_a, other, "a sibling's content is part of the key");
}

#[test]
fn the_provider_serves_r_with_air_then_jarl_and_no_other_language() {
    let names: Vec<_> = engines_for(&Language::R).iter().map(|engine| engine.name()).collect();
    assert_eq!(names, ["air", "jarl"]);
    assert!(engines_for(&Language::Python).is_empty());
    let engine: &dyn Engine = &JarlEngine;
    assert!(engine.provides_language_lint(&Language::R, &config("")));
}
