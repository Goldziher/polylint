//! The real `pyrefly` component, built by alef from this workspace and loaded from a `file://` dev
//! lock, against the same engine linked in-process.
//!
//! Runs only when `POLY_COMPONENT_DEV_LOCK` names a lock written by
//! `mise run components:dev pyrefly`; otherwise it reports that it skipped and passes.

use poly_component_host::{ComponentHost, HostOptions, SlotStatus};
use poly_component_protocol::{PolyComponent, decode_describe};
use poly_engine_api::config::{EngineConfig, GlobalDefaults};
use poly_engine_api::engine::{ProjectAnalysis, ProjectEnvironment, ProjectFile, ProjectReport, ProjectRequest};
use poly_engine_api::language::Language;
use poly_engines_pyrefly::{PyreflyComponent, PyreflyEngine};

const DEV_LOCK_ENV: &str = "POLY_COMPONENT_DEV_LOCK";

const KNOWN_BAD: &str = "count: int = \"three\"\n";

#[test]
fn the_pyrefly_component_checks_like_the_in_process_engine() {
    let Some(lock) = std::env::var(DEV_LOCK_ENV)
        .ok()
        .filter(|value| !value.trim().is_empty())
    else {
        eprintln!("skipped: {DEV_LOCK_ENV} is not set; run `mise run components:dev pyrefly` first");
        return;
    };
    let lock = std::fs::read_to_string(&lock).unwrap_or_else(|e| panic!("reading {lock}: {e}"));
    let cache = tempfile::tempdir().unwrap();
    let host = ComponentHost::new(HostOptions::new(lock, cache.path())).expect("host over the dev lock");

    let describe =
        decode_describe(&PyreflyComponent::default().describe().unwrap()).expect("in-process describe decodes");
    let wire = describe
        .engines
        .first()
        .expect("the pyrefly component hosts an engine")
        .clone();
    assert_eq!(wire.name, "pyrefly");
    assert!(wire.capabilities.project);
    let engine = host
        .engine("pyrefly", &wire, Some(describe))
        .expect("engine proxy over the component");
    assert_eq!(engine.slot().status(), SlotStatus::Unloaded);

    let project = tempfile::tempdir().unwrap();
    let path = project.path().join("app.py");
    std::fs::write(&path, KNOWN_BAD).unwrap();
    let files = [ProjectFile {
        path: path.clone(),
        language: Language::Python,
        content: KNOWN_BAD.into(),
    }];
    let cfg = EngineConfig {
        globals: GlobalDefaults::default(),
        indent_width: 4,
        options: toml::from_str("backend = \"pyrefly\"").unwrap(),
    };
    let request = ProjectRequest {
        root: project.path(),
        files: &files,
        config: &cfg,
        environment: &ProjectEnvironment::default(),
        threads: Some(2),
        targets: None,
        cache: None,
    };
    // `project_report`, not `project_check`: the latter flattens a decline into no findings (ADR 0079), so a
    // component that declined where the in-process engine checked would compare equal to a clean pass.
    let expected = PyreflyEngine.project_report(&request).expect("in-process check");
    let actual = engine.project_report(&request).expect("component check");
    assert_eq!(format!("{actual:?}"), format!("{expected:?}"));
    let ProjectReport::Checked(findings) = actual else {
        panic!("the component declined the project: {actual:?}");
    };
    assert_eq!(findings[0].1[0].code.as_deref(), Some("bad-assignment"));
    assert_eq!(engine.slot().status(), SlotStatus::Ready);
}
