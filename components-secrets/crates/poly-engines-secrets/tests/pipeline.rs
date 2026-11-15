//! The secrets engine through the real `lint` pipeline: the central filters apply to its findings without any
//! engine-side code, so an inline `poly: allow[...]` and a `[per_file_ignores]` entry are the allowlist.
//!
//! Each case is a pair: the same file without the allowlist reports the rule, with it the rule is gone and the
//! run names the mechanism in `suppressed`. Asserting only the absence would pass against an engine that stopped
//! reporting.

use std::collections::BTreeMap;
use std::path::Path;

use poly_config::v2::ComponentsConfig;
use poly_core::engine::Engine;
use poly_core::runner::SuppressionReason;
use poly_core::{Config, Language, LintRun, RunOptions, register_engine_provider};
use poly_engines_secrets::SecretsEngine;

const TAIL: &str = "e1qlnTsd5awqKXOhdBYm9PuddQWMOumr7sKv";
const RULE: &str = "secrets/betterleaks.github-pat";

/// The core build leaves cross-cutting component engines to the host; this stands in for it. The index lists
/// `secrets` as a download-only component this build cannot fetch, so the config disables the component and
/// the provider's engine is the one that runs. The `.ini` fixtures' own linter is the `misc` component, which
/// this lockless build cannot fetch either, so it is disabled too.
fn provider(_: &Language) -> Vec<Box<dyn Engine>> {
    vec![Box::new(SecretsEngine)]
}

fn setup() {
    // The slot is process-global and set once; every test in this binary registers the same provider.
    let _ = register_engine_provider(provider);
}

fn enabled(per_file_ignores: BTreeMap<String, Vec<String>>) -> Config {
    Config {
        lint: toml::from_str("[secrets]\nenabled = true\n").expect("valid lint config"),
        per_file_ignores,
        components: ComponentsConfig {
            disable: vec!["secrets".to_owned(), "misc".to_owned()],
            ..ComponentsConfig::default()
        },
        ..Config::default()
    }
}

fn run(dir: &Path, config: &Config) -> LintRun {
    let options = RunOptions {
        no_cache: true,
        jobs: Some(1),
        explicit_config: true,
        ..RunOptions::default()
    };
    poly_core::lint_run(&[dir.to_path_buf()], config, &options, false, false).expect("lint run")
}

fn reports_rule(run: &LintRun) -> bool {
    run.results
        .iter()
        .flat_map(|result| &result.diagnostics)
        .any(|diagnostic| diagnostic.code.as_deref() == Some(RULE))
}

fn settings(allow: &str) -> String {
    format!("TOKEN = \"ghp_{TAIL}\"{allow}\n")
}

#[test]
fn an_inline_allow_suppresses_the_finding_and_names_itself() {
    setup();
    let flagged = tempfile::tempdir().unwrap();
    std::fs::write(flagged.path().join("settings.ini"), settings("")).unwrap();
    assert!(
        reports_rule(&run(flagged.path(), &enabled(BTreeMap::new()))),
        "the control reports the rule"
    );

    let allowed = tempfile::tempdir().unwrap();
    let path = allowed.path().join("settings.ini");
    std::fs::write(
        &path,
        settings("  # poly: allow[secrets/betterleaks.github-pat] fake fixture token"),
    )
    .unwrap();
    let result = run(allowed.path(), &enabled(BTreeMap::new()));
    assert!(!reports_rule(&result));
    let entries: Vec<_> = result
        .suppressed
        .iter()
        .filter(|s| s.code.as_deref() == Some(RULE))
        .collect();
    assert_eq!(entries.len(), 1, "{:?}", result.suppressed);
    assert_eq!(entries[0].path, path);
    assert_eq!(entries[0].reason, SuppressionReason::InlineSuppression);
}

#[test]
fn the_engine_family_prefix_allows_every_rule_on_the_line() {
    setup();
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("settings.ini"),
        settings("  # poly: allow[secrets] fixture file"),
    )
    .unwrap();
    assert!(!reports_rule(&run(dir.path(), &enabled(BTreeMap::new()))));
}

#[test]
fn a_per_file_ignore_suppresses_the_finding_for_matching_paths_only() {
    setup();
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("tests")).unwrap();
    std::fs::create_dir_all(dir.path().join("src")).unwrap();
    std::fs::write(dir.path().join("tests/fixture.ini"), settings("")).unwrap();
    std::fs::write(dir.path().join("src/settings.ini"), settings("")).unwrap();
    let ignores = BTreeMap::from([("tests/**".to_owned(), vec!["secrets".to_owned()])]);

    let result = run(dir.path(), &enabled(ignores));
    let reported: Vec<_> = result
        .results
        .iter()
        .filter(|r| r.diagnostics.iter().any(|d| d.code.as_deref() == Some(RULE)))
        .map(|r| r.path.clone())
        .collect();
    assert_eq!(reported, vec![dir.path().join("src/settings.ini")]);
    assert!(
        result
            .suppressed
            .iter()
            .any(|s| s.reason == SuppressionReason::PerFileIgnore)
    );
}

#[test]
fn nothing_is_scanned_unless_the_engine_is_enabled() {
    setup();
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("settings.ini"), settings("")).unwrap();
    assert!(!reports_rule(&run(dir.path(), &Config::default())));
}
