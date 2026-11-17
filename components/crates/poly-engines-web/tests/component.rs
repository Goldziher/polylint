//! The real `web` component, built by alef from this workspace and loaded from a `file://` dev lock,
//! against the same engine linked in-process.
//!
//! Runs only when `POLY_COMPONENT_DEV_LOCK` names a lock written by
//! `mise run components:dev web`; otherwise it reports that it skipped and passes.

use std::path::PathBuf;
use std::sync::Arc;

use poly_component_host::{ComponentHost, HostOptions, SlotStatus};
use poly_component_protocol::{PolyComponent, decode_describe};
use poly_engine_api::config::{EngineConfig, GlobalDefaults};
use poly_engine_api::engine::{Engine, FormatOutput, SourceFile};
use poly_engine_api::language::Language;
use poly_engines_web::{OxcEngine, WebComponent};

const DEV_LOCK_ENV: &str = "POLY_COMPONENT_DEV_LOCK";

/// Irregular spacing, single quotes and a missing semicolon, all of which oxc normalizes.
const UNFORMATTED: &str = "const   a  =  'x'\n";

fn formatted(output: FormatOutput) -> String {
    match output {
        FormatOutput::Formatted(text) => text,
        FormatOutput::Unchanged => panic!("the fixture must be reformatted"),
    }
}

#[test]
fn the_web_component_describes_every_web_engine() {
    let describe = decode_describe(&WebComponent::default().describe().unwrap()).expect("in-process describe decodes");
    let mut names: Vec<_> = describe.engines.iter().map(|engine| engine.name.as_str()).collect();
    names.sort_unstable();
    assert_eq!(
        names,
        [
            "adrlint",
            "malva",
            "markup_fmt",
            "oxc",
            "rumdl",
            "skif",
            "taplo",
            "yaml"
        ]
    );
}

/// The lock `POLY_COMPONENT_DEV_LOCK` names, or `None` (with a "skipped" line) when it is unset.
fn dev_lock() -> Option<String> {
    let Some(lock) = std::env::var(DEV_LOCK_ENV)
        .ok()
        .filter(|value| !value.trim().is_empty())
    else {
        eprintln!("skipped: {DEV_LOCK_ENV} is not set; run `mise run components:dev web` first");
        return None;
    };
    Some(std::fs::read_to_string(&lock).unwrap_or_else(|e| panic!("reading {lock}: {e}")))
}

/// The oxc engine served by the `web` component `host` loads.
fn oxc_through(host: &ComponentHost) -> poly_component_host::ComponentEngine {
    let describe = decode_describe(&WebComponent::default().describe().unwrap()).expect("in-process describe decodes");
    let wire = describe
        .engines
        .iter()
        .find(|engine| engine.name == "oxc")
        .expect("the web component hosts the oxc engine")
        .clone();
    host.engine("web", &wire, Some(describe))
        .expect("engine proxy over the component")
}

#[test]
fn the_web_component_formats_like_the_in_process_engine() {
    let Some(lock) = dev_lock() else {
        return;
    };
    let cache = tempfile::tempdir().unwrap();
    let host = ComponentHost::new(HostOptions::new(lock, cache.path())).expect("host over the dev lock");
    let engine = oxc_through(&host);
    assert_eq!(engine.slot().status(), SlotStatus::Unloaded);

    let cfg = EngineConfig {
        globals: GlobalDefaults::default(),
        indent_width: Language::JavaScript.default_indent_width(),
        options: toml::Table::new(),
    };
    let src = SourceFile {
        path: PathBuf::from("test.js"),
        language: Language::JavaScript,
        content: Arc::from(UNFORMATTED),
    };
    let expected = formatted(OxcEngine.format(&src, &cfg).expect("in-process format"));
    let actual = formatted(engine.format(&src, &cfg).expect("component format"));
    assert_eq!(actual, expected);
    assert_eq!(engine.slot().status(), SlotStatus::Ready);
}

/// A long-lived host (`poly mcp` on a core build) starts many runs over one loaded component.
/// The component memoises the governing `.oxlintrc.json` and the linter built from it in statics
/// of its own, which the host cannot clear: it has to be told that a new run started.
#[test]
fn the_web_component_rereads_the_oxlintrc_when_the_host_starts_a_run() {
    let Some(lock) = dev_lock() else {
        return;
    };
    let cache = tempfile::tempdir().unwrap();
    let host = ComponentHost::new(HostOptions::new(lock, cache.path())).expect("host over the dev lock");
    let engine = oxc_through(&host);

    let repo = tempfile::tempdir().unwrap();
    std::fs::create_dir(repo.path().join(".git")).unwrap();
    let rc = repo.path().join(".oxlintrc.json");
    std::fs::write(&rc, r#"{"rules": {"no-debugger": "error", "eqeqeq": "off"}}"#).unwrap();
    // The host's config load names the project's root oxlint config in the engine options.
    let mut options = toml::Table::new();
    options.insert(
        poly_engine_api::host_hooks::oxlintrc::RC_KEY.to_owned(),
        toml::Value::String(rc.to_string_lossy().into_owned()),
    );
    let cfg = EngineConfig {
        globals: GlobalDefaults::default(),
        indent_width: Language::TypeScript.default_indent_width(),
        options,
    };
    let src = SourceFile {
        path: repo.path().join("a.ts"),
        language: Language::TypeScript,
        content: Arc::from("debugger;\nif (a == 1) {}\n"),
    };
    let codes = || -> Vec<String> {
        let mut codes: Vec<String> = engine
            .lint(&src, &cfg)
            .expect("component lint")
            .into_iter()
            .filter_map(|diagnostic| diagnostic.code)
            .collect();
        codes.sort();
        codes
    };

    poly_engine_api::host_hooks::begin_run();
    let first = codes();
    assert!(first.iter().any(|code| code.contains("no-debugger")), "{first:?}");
    assert!(!first.iter().any(|code| code.contains("eqeqeq")), "{first:?}");

    std::fs::write(&rc, r#"{"rules": {"no-debugger": "off", "eqeqeq": "error"}}"#).unwrap();
    assert_eq!(codes(), first, "within one run the component keeps the config it read");

    poly_engine_api::host_hooks::begin_run();
    let second = codes();
    assert!(
        !second.iter().any(|code| code.contains("no-debugger")),
        "the edit turned it off: {second:?}"
    );
    assert!(
        second.iter().any(|code| code.contains("eqeqeq")),
        "the edit turned it on: {second:?}"
    );
}
