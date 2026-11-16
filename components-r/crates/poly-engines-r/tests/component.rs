//! The real `r` component, built by alef from this workspace and loaded from a `file://` dev lock, against the same
//! engines linked in-process.
//!
//! Runs only when `POLY_COMPONENT_DEV_LOCK` names a lock written by `mise run components:dev r`; otherwise it reports
//! that it skipped and passes.

use std::sync::Arc;

use poly_component_host::{ComponentHost, HostOptions, SlotStatus};
use poly_component_protocol::{PolyComponent, decode_describe};
use poly_engine_api::config::{EngineConfig, GlobalDefaults};
use poly_engine_api::engine::{Engine, FormatOutput, SourceFile};
use poly_engine_api::language::Language;
use poly_engines_r::{AirEngine, JarlEngine, RComponent};

const DEV_LOCK_ENV: &str = "POLY_COMPONENT_DEV_LOCK";

/// Needs formatting (spacing, `=`) and lints (`any(is.na())`).
const FIXTURE: &str = "x=c(1,2,NA)\nif (any(is.na(x))) {\n  print(\"missing\")\n}\n";

#[test]
fn the_r_component_formats_and_lints_like_the_in_process_engines() {
    let Some(lock) = std::env::var(DEV_LOCK_ENV)
        .ok()
        .filter(|value| !value.trim().is_empty())
    else {
        eprintln!("skipped: {DEV_LOCK_ENV} is not set; run `mise run components:dev r` first");
        return;
    };
    let lock = std::fs::read_to_string(&lock).unwrap_or_else(|e| panic!("reading {lock}: {e}"));
    let cache = tempfile::tempdir().unwrap();
    let host = ComponentHost::new(HostOptions::new(lock, cache.path())).expect("host over the dev lock");

    let describe = decode_describe(&RComponent::default().describe().unwrap()).expect("in-process describe decodes");
    let wires = describe.engines.clone();
    assert_eq!(
        wires.iter().map(|wire| wire.name.as_str()).collect::<Vec<_>>(),
        ["air", "jarl"]
    );
    let air = host
        .engine("r", &wires[0], Some(describe.clone()))
        .expect("air proxy over the component");
    let jarl = host
        .engine("r", &wires[1], Some(describe))
        .expect("jarl proxy over the component");
    assert_eq!(air.slot().status(), SlotStatus::Unloaded);

    let project = tempfile::tempdir().unwrap();
    let cfg = EngineConfig {
        globals: GlobalDefaults::default(),
        indent_width: Language::R.default_indent_width(),
        options: toml::Table::new(),
    };
    let src = SourceFile {
        path: project.path().join("fixture.R"),
        language: Language::R,
        content: Arc::from(FIXTURE),
    };

    let expected = match AirEngine.format(&src, &cfg).expect("in-process format") {
        FormatOutput::Formatted(text) => text,
        FormatOutput::Unchanged => panic!("the fixture must be reformatted"),
    };
    let actual = match air.format(&src, &cfg).expect("component format") {
        FormatOutput::Formatted(text) => text,
        FormatOutput::Unchanged => panic!("the component left the fixture as it was"),
    };
    assert_eq!(actual, expected);

    let expected = JarlEngine.lint(&src, &cfg).expect("in-process lint");
    let actual = jarl.lint(&src, &cfg).expect("component lint");
    assert_eq!(format!("{actual:?}"), format!("{expected:?}"));
    assert_eq!(actual[0].code.as_deref(), Some("any_is_na"));
    assert_eq!(air.slot().status(), SlotStatus::Ready);
}
