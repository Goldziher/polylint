//! The option declarations of the R engines are honest: every declared key is read, and a key that is not declared is
//! not.
//!
//! air's own deserializer rejects a key it does not know (`deny_unknown_fields`), so formatting with a declared key set
//! to a valid value proves the key reaches air under the spelling `air.toml` uses.

use std::collections::BTreeSet;

use poly_engine_api::config::{EngineConfig, GlobalDefaults};
use poly_engine_api::engine::{Engine, OptionTable, OptionType, SourceFile};
use poly_engine_api::language::Language;
use poly_engines_r::{AirEngine, JarlEngine};

fn config(options: &str) -> EngineConfig {
    EngineConfig {
        globals: GlobalDefaults::default(),
        indent_width: 2,
        options: options.parse().expect("test options are TOML"),
    }
}

fn source(dir: &std::path::Path) -> SourceFile {
    SourceFile {
        path: dir.join("a.R"),
        language: Language::R,
        content: "x <- 1\n".into(),
    }
}

fn declared(engine: &dyn Engine, table: OptionTable) -> BTreeSet<&'static str> {
    engine
        .option_keys(table)
        .declared_keys()
        .iter()
        .map(|(key, _)| *key)
        .collect()
}

#[test]
fn every_table_of_both_engines_is_checked() {
    for table in [OptionTable::Lint, OptionTable::Format, OptionTable::CrossCuttingLint] {
        assert!(AirEngine.option_keys(table).is_checked(), "air {table:?}");
        assert!(JarlEngine.option_keys(table).is_checked(), "jarl {table:?}");
    }
}

#[test]
fn air_declares_the_format_keys_it_reads_and_nothing_for_lint() {
    assert_eq!(
        declared(&AirEngine, OptionTable::Format),
        BTreeSet::from([
            "assignment_style",
            "default_table",
            "indent_style",
            "indent_width",
            "line_ending",
            "line_width",
            "persistent_line_breaks",
            "skip",
            "table",
        ])
    );
    assert!(declared(&AirEngine, OptionTable::Lint).is_empty());
}

#[test]
fn every_declared_air_key_is_accepted_by_air() {
    let dir = tempfile::tempdir().unwrap();
    let samples = [
        ("line_width", "100"),
        ("indent_width", "4"),
        ("indent_style", "\"tab\""),
        ("line_ending", "\"lf\""),
        ("persistent_line_breaks", "false"),
        ("assignment_style", "\"equal\""),
        ("skip", "[\"graph_from_literal\"]"),
        ("table", "[\"my_table\"]"),
        ("default_table", "false"),
    ];
    let keys = declared(&AirEngine, OptionTable::Format);
    assert_eq!(samples.iter().map(|(key, _)| *key).collect::<BTreeSet<_>>(), keys);
    let declared_types = AirEngine.option_keys(OptionTable::Format);
    for (key, value) in samples {
        let cfg = config(&format!("{key} = {value}"));
        AirEngine
            .format(&source(dir.path()), &cfg)
            .unwrap_or_else(|error| panic!("`{key} = {value}` is rejected: {error:#}"));
        assert!(
            declared_types.value_problem(key, &cfg.options[key], false).is_none(),
            "{key} = {value} does not match its declared type"
        );
    }
}

#[test]
fn an_option_air_does_not_declare_is_not_read() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = config("bogus_option = \"tab\"\n");
    AirEngine
        .format(&source(dir.path()), &cfg)
        .expect("an undeclared key is ignored, not forwarded to air");
}

#[test]
fn jarl_declares_the_lint_keys_it_reads_beside_the_shared_rule_selection() {
    let keys = JarlEngine.option_keys(OptionTable::Lint);
    assert_eq!(
        keys.declared_keys(),
        [
            ("check_roxygen", OptionType::BOOLEAN),
            ("min_r_version", OptionType::STRING)
        ]
    );
    assert!(keys.has_rule_selection());
    assert!(declared(&JarlEngine, OptionTable::Format).is_empty());
}

#[test]
fn jarl_reads_min_r_version_and_check_roxygen() {
    let dir = tempfile::tempdir().unwrap();
    let ok = JarlEngine.lint(
        &source(dir.path()),
        &config("min_r_version = \"4.1\"\ncheck_roxygen = false\n"),
    );
    assert!(ok.is_ok(), "{ok:?}");
    let bad = JarlEngine.lint(&source(dir.path()), &config("min_r_version = \"not-a-version\"\n"));
    let message = format!("{:#}", bad.unwrap_err());
    assert!(message.contains("Invalid version format"), "{message}");
}
