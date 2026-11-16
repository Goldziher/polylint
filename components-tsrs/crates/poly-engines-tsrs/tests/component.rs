//! The real `tsrs` component, built by alef from this workspace and loaded from a `file://` dev lock, against the
//! same engine linked in-process.
//!
//! Runs only when `POLY_COMPONENT_DEV_LOCK` names a lock written by `mise run components:dev tsrs`; otherwise it
//! reports that it skipped and passes.

use poly_component_host::{ComponentHost, HostOptions, SlotStatus};
use poly_component_protocol::{PolyComponent, decode_describe};
use poly_engine_api::config::{EngineConfig, GlobalDefaults};
use poly_engine_api::engine::{ProjectAnalysis, ProjectEnvironment, ProjectFile, ProjectRequest};
use poly_engine_api::language::Language;
use poly_engines_tsrs::{TsrsComponent, TsrsEngine};

const DEV_LOCK_ENV: &str = "POLY_COMPONENT_DEV_LOCK";

const TSCONFIG: &str = r#"{ "compilerOptions": { "strict": true, "target": "es2022", "module": "esnext", "moduleResolution": "bundler" }, "include": ["src"] }"#;
const KNOWN_BAD: &str = "export const count: number = \"three\";\n";

#[test]
fn the_tsrs_component_describes_the_tsrs_engine() {
    let describe = decode_describe(&TsrsComponent::default().describe().unwrap()).expect("in-process describe decodes");
    let names: Vec<_> = describe.engines.iter().map(|engine| engine.name.as_str()).collect();
    assert_eq!(names, ["tsrs"]);
    let engine = &describe.engines[0];
    assert!(engine.capabilities.project);
    let inputs = &engine.project_inputs;
    assert_eq!(inputs.gate_key.as_deref(), Some("enabled"));
    assert!(inputs.dirs.as_ref().is_some_and(|dirs| dirs.whole_tree));
}

#[test]
fn the_tsrs_component_checks_like_the_in_process_engine() {
    let Some(lock) = std::env::var(DEV_LOCK_ENV)
        .ok()
        .filter(|value| !value.trim().is_empty())
    else {
        eprintln!("skipped: {DEV_LOCK_ENV} is not set; run `mise run components:dev tsrs` first");
        return;
    };
    let lock = std::fs::read_to_string(&lock).unwrap_or_else(|e| panic!("reading {lock}: {e}"));
    let cache = tempfile::tempdir().unwrap();
    let host = ComponentHost::new(HostOptions::new(lock, cache.path())).expect("host over the dev lock");

    let describe = decode_describe(&TsrsComponent::default().describe().unwrap()).expect("in-process describe decodes");
    let wire = describe
        .engines
        .first()
        .expect("the tsrs component hosts an engine")
        .clone();
    assert_eq!(wire.name, "tsrs");
    let engine = host
        .engine("tsrs", &wire, Some(describe))
        .expect("engine proxy over the component");
    assert_eq!(engine.slot().status(), SlotStatus::Unloaded);

    let project = tempfile::tempdir().unwrap();
    let file = |name: &str, language: Language, content: &str| ProjectFile {
        path: project.path().join(name),
        language,
        content: content.into(),
    };
    let files = [
        file("tsconfig.json", Language::Other("tsconfig.json".into()), TSCONFIG),
        file("src/app.ts", Language::TypeScript, KNOWN_BAD),
    ];
    let cfg = EngineConfig {
        globals: GlobalDefaults::default(),
        indent_width: 2,
        options: toml::from_str("enabled = true").unwrap(),
    };
    let request = ProjectRequest {
        root: project.path(),
        files: &files,
        config: &cfg,
        environment: &ProjectEnvironment::default(),
        threads: Some(2),
    };
    let expected = TsrsEngine.project_check(&request).expect("in-process check");
    let actual = engine.project_check(&request).expect("component check");
    assert_eq!(format!("{actual:?}"), format!("{expected:?}"));
    assert_eq!(actual[0].1[0].code.as_deref(), Some("TS2322"));
    assert_eq!(engine.slot().status(), SlotStatus::Ready);
}
