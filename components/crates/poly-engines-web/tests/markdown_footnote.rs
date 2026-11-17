//! Empty footnote definitions and the Markdown formatter (oxc-project/oxc#29733).
//!
//! `oxc_formatter_markdown` panics on a footnote definition with no content. poly declines a file
//! holding one instead of reporting it as an error, and keeps a fenced Markdown example holding one
//! as written while formatting the rest of the document.

use std::fs;
use std::path::PathBuf;

use poly_core::{Config, RunOptions};
use poly_engine_api::config::{EngineConfig, GlobalDefaults};
use poly_engine_api::declined::Declined;
use poly_engine_api::engine::{Engine, FormatOutput, SourceFile};
use poly_engine_api::language::Language;
use poly_engines_web::rumdl::RumdlEngine;

mod common;

const ISSUE: &str = "https://github.com/oxc-project/oxc/issues/29733";

/// No rumdl rule key set, so Markdown goes through `oxc_formatter_markdown`.
fn oxfmt_cfg() -> EngineConfig {
    EngineConfig {
        globals: GlobalDefaults::default(),
        indent_width: 2,
        options: toml::Table::new(),
    }
}

fn md_src(content: &str) -> SourceFile {
    SourceFile {
        path: PathBuf::from("notes.md"),
        language: Language::Markdown,
        content: content.into(),
    }
}

fn format(content: &str) -> anyhow::Result<FormatOutput> {
    RumdlEngine.format(&md_src(content), &oxfmt_cfg())
}

#[test]
fn an_empty_footnote_definition_declines_the_file_with_the_upstream_issue() {
    let error = format("[^1]:\n").expect_err("an empty footnote definition must decline the file");
    let reason = Declined::reason(&error).unwrap_or_else(|| panic!("expected a decline, got the error {error:#}"));
    assert!(reason.contains("empty footnote definition"), "{reason}");
    assert!(reason.contains(ISSUE), "{reason}");
}

#[test]
fn an_empty_footnote_definition_in_a_markdown_fence_keeps_the_fence_and_formats_the_rest() {
    let src = "# T\n* a\n\n```markdown\n[^1]:\n```\n";
    match format(src).expect("the document is formatted") {
        FormatOutput::Formatted(out) => assert_eq!(out, "# T\n\n- a\n\n```markdown\n[^1]:\n```\n"),
        FormatOutput::Unchanged => panic!("expected the heading and list outside the fence to be normalised"),
    }
}

#[test]
fn a_footnote_definition_continued_on_an_indented_line_is_formatted() {
    match format("[^1]:\n    Content.\n").expect("a non-empty definition is formatted") {
        FormatOutput::Formatted(out) => assert_eq!(out, "[^1]: Content.\n"),
        FormatOutput::Unchanged => panic!("expected the lone paragraph to move onto the label's line"),
    }
}

#[test]
fn ordinary_footnotes_are_unaffected() {
    let src = "Text[^1].\n\n[^1]: Note.\n";
    assert!(
        matches!(format(src), Ok(FormatOutput::Unchanged)),
        "an already formatted footnote stays as it is"
    );
}

/// Through the runner the decline is a skip carrying the reason, not a failed file.
#[test]
fn the_runner_reports_the_file_as_skipped_not_errored() {
    common::register();
    let dir = tempfile::tempdir().expect("tempdir");
    fs::write(dir.path().join("empty-footnote.md"), "[^1]:\n").expect("write fixture");

    let opts = RunOptions {
        no_cache: true,
        explicit_config: true,
        ..RunOptions::default()
    };
    let run = poly_core::format_run(&[dir.path().to_path_buf()], &Config::default(), &opts, false, false)
        .expect("format run");
    assert!(
        run.errors.is_empty(),
        "the file must not be reported as failed: {:?}",
        run.errors
    );
    let result = run
        .results
        .iter()
        .find(|r| r.path.file_name().is_some_and(|f| f == "empty-footnote.md"))
        .unwrap_or_else(|| panic!("no result for empty-footnote.md, got {:?}", run.results));
    assert!(
        result.skipped.as_deref().is_some_and(|reason| reason.contains(ISSUE)),
        "the file must be reported as skipped with the reason, got {:?}",
        result.skipped
    );
    assert!(!result.changed, "a declined file must not be reformatted");
}
