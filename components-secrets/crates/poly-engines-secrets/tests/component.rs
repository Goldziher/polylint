//! The real `secrets` component, built by alef from this workspace and loaded from a `file://` dev
//! lock, against the same engine linked in-process.
//!
//! Runs only when `POLY_COMPONENT_DEV_LOCK` names a lock written by
//! `mise run components:dev secrets`; otherwise it reports that it skipped and passes. CI's secrets
//! job sets it and requires the test to have run.

use std::sync::Arc;

use poly_component_host::{ComponentHost, HostOptions, SlotStatus};
use poly_component_protocol::{PolyComponent, decode_describe};
use poly_engine_api::config::{EngineConfig, GlobalDefaults};
use poly_engine_api::engine::{Engine, SourceFile};
use poly_engine_api::language::Language;
use poly_engines_secrets::{SecretsComponent, SecretsEngine};

const DEV_LOCK_ENV: &str = "POLY_COMPONENT_DEV_LOCK";

#[test]
fn the_secrets_component_scans_like_the_in_process_engine() {
    let Some(lock) = std::env::var(DEV_LOCK_ENV)
        .ok()
        .filter(|value| !value.trim().is_empty())
    else {
        eprintln!("skipped: {DEV_LOCK_ENV} is not set; run `mise run components:dev secrets` first");
        return;
    };
    let lock = std::fs::read_to_string(&lock).unwrap_or_else(|e| panic!("reading {lock}: {e}"));
    let cache = tempfile::tempdir().unwrap();
    let host = ComponentHost::new(HostOptions::new(lock, cache.path())).expect("host over the dev lock");

    let describe =
        decode_describe(&SecretsComponent::default().describe().unwrap()).expect("in-process describe decodes");
    let wire = describe
        .engines
        .first()
        .expect("the secrets component hosts an engine")
        .clone();
    assert_eq!(wire.name, "secrets");
    assert!(wire.capabilities.lint && !wire.capabilities.format);
    assert!(wire.languages.is_empty(), "secrets is cross-cutting");
    let engine = host
        .engine("secrets", &wire, Some(describe))
        .expect("engine proxy over the component");
    assert_eq!(engine.slot().status(), SlotStatus::Unloaded);

    let content = format!(
        "# settings\nTOKEN = \"ghp_{}\"\n",
        "e1qlnTsd5awqKXOhdBYm9PuddQWMOumr7sKv"
    );
    let src = SourceFile {
        path: "settings.py".into(),
        language: Language::Python,
        content: Arc::from(content.as_str()),
    };
    let mut options = toml::Table::new();
    options.insert("enabled".into(), toml::Value::Boolean(true));
    let cfg = EngineConfig {
        globals: GlobalDefaults::default(),
        indent_width: 4,
        options,
    };
    let expected = SecretsEngine.lint(&src, &cfg).expect("in-process scan");
    let actual = engine.lint(&src, &cfg).expect("component scan");
    assert!(!expected.is_empty(), "the control finds the fixture token");
    assert_eq!(format!("{actual:?}"), format!("{expected:?}"));
    assert!(
        actual
            .iter()
            .any(|d| d.code.as_deref() == Some("secrets/betterleaks.github-pat"))
    );
    assert_eq!(engine.slot().status(), SlotStatus::Ready);
}
