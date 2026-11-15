//! ADR 0057: `quality` and `astgrep` report exactly the same diagnostics whether
//! each parses the file itself or is handed the tree the other one parsed, in
//! either order, and a shared lint pass parses each file once.
#![cfg(all(feature = "engine-quality", feature = "engine-astgrep"))]

use std::path::PathBuf;
use std::sync::Arc;

use poly_core::{
    Language,
    config::{EngineConfig, GlobalDefaults},
    engine::{Diagnostic, Engine, SourceFile},
    engines::astgrep::AstGrepEngine,
    engines::quality::QualityEngine,
};
use poly_grammar::{parse_count, with_shared_parses};

/// Each fixture trips a default-on built-in pack rule and a quality rule, so
/// both engines demonstrably consumed a tree.
fn fixtures() -> [(&'static str, Language, &'static str, &'static [&'static str]); 2] {
    [
        (
            "src/lib.rs",
            Language::Rust,
            "fn f(a: u8, b: u8, c: u8, d: u8, e: u8, g: u8, h: u8) {\n    \
         match r() {\n        Ok(v) => v,\n        Err(_) => {}\n    }\n}\n",
            &["swallowed-error", "too-many-parameters"],
        ),
        (
            "app/show.go",
            Language::Go,
            "package app\n\nimport . \"fmt\"\n\nfunc show(a, b, c, d, e, f, g int) {\n\tPrintln(a)\n}\n",
            &["go-dot-import", "too-many-parameters"],
        ),
    ]
}

fn engine_config() -> EngineConfig {
    EngineConfig {
        globals: GlobalDefaults::default(),
        indent_width: 4,
        options: toml::Table::new(),
    }
}

fn source(path: &str, language: &Language, content: &str) -> SourceFile {
    SourceFile {
        path: PathBuf::from(path),
        language: language.clone(),
        content: Arc::from(content),
    }
}

fn lint(engine: &dyn Engine, src: &SourceFile) -> Vec<Diagnostic> {
    engine.lint(src, &engine_config()).expect("lint")
}

fn json(diagnostics: &[Diagnostic]) -> String {
    serde_json::to_string(diagnostics).expect("serialize")
}

fn codes(diagnostics: &[Diagnostic]) -> Vec<&str> {
    let mut codes: Vec<&str> = diagnostics.iter().filter_map(|d| d.code.as_deref()).collect();
    codes.sort_unstable();
    codes
}

/// `first` parses inside a shared scope and `second` reuses its tree; each
/// engine's output must match a run that parsed on its own.
fn assert_identical(first: &dyn Engine, second: &dyn Engine) {
    for (path, language, content, expected) in fixtures() {
        let src = source(path, &language, content);
        let alone_first = lint(first, &src);
        let alone_second = lint(second, &src);

        let before = parse_count();
        let (shared_first, shared_second) = with_shared_parses(|| (lint(first, &src), lint(second, &src)));
        assert_eq!(parse_count() - before, 1, "{path}: one parse for both engines");

        assert_eq!(json(&shared_first), json(&alone_first), "{path}: {}", first.name());
        assert_eq!(json(&shared_second), json(&alone_second), "{path}: {}", second.name());

        let mut all = shared_first;
        all.extend(shared_second);
        assert_eq!(codes(&all), expected, "{path}: both engines must have reported");
    }
}

#[test]
fn quality_on_the_ast_grep_tree_is_unchanged() {
    assert_identical(&AstGrepEngine, &QualityEngine);
}

#[test]
fn ast_grep_on_the_quality_tree_is_unchanged() {
    assert_identical(&QualityEngine, &AstGrepEngine);
}
