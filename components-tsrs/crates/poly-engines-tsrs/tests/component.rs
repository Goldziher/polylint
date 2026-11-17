//! The real `tsrs` component, built by alef from this workspace and loaded from a `file://` dev lock, against the
//! same engine linked in-process.
//!
//! Runs only when `POLY_COMPONENT_DEV_LOCK` names a lock written by `mise run components:dev tsrs`; otherwise it
//! reports that it skipped and passes.

use poly_component_host::{ComponentHost, HostOptions, SlotStatus};
use poly_component_protocol::{
    PROTOCOL_VERSION, Payload, PolyComponent, Request, WireFileSlice, WireLogLevel, decode_describe, decode_response,
    encode_request,
};
use poly_engine_api::component_wire::engine_config_to_wire;
use poly_engine_api::config::{EngineConfig, GlobalDefaults};
use poly_engine_api::engine::{ProjectAnalysis, ProjectEnvironment, ProjectFile, ProjectReport, ProjectRequest};
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

/// The component answers a project without a `tsconfig.json` with a decline, not an empty finding list, so the
/// host reports a skip.
#[test]
fn the_tsrs_component_declines_a_project_without_a_tsconfig() {
    let component = TsrsComponent::default();
    let send = |request: &Request, body: &[u8]| {
        let bytes = component
            .call(&encode_request(request).unwrap(), body)
            .expect("the component answers");
        decode_response(&bytes).expect("the answer decodes").result
    };
    let init = send(
        &Request::Init {
            protocol: PROTOCOL_VERSION,
            host_version: "0.0.1".into(),
            threads: 1,
            stack_size: 16 << 20,
            max_log_level: WireLogLevel::Info,
        },
        b"",
    );
    assert!(init.is_ok(), "{init:?}");
    let cfg = EngineConfig {
        globals: GlobalDefaults::default(),
        indent_width: 2,
        options: toml::from_str("enabled = true").unwrap(),
    };
    let registered = send(
        &Request::RegisterConfig {
            id: 1,
            config: engine_config_to_wire(&cfg),
        },
        b"",
    );
    assert_eq!(registered, Ok(Payload::Ok));
    let answer = send(
        &Request::ProjectCheck {
            engine: "tsrs".into(),
            root: std::env::temp_dir().join("poly-tsrs-no-tsconfig").display().to_string(),
            files: vec![WireFileSlice {
                path: "src/app.ts".into(),
                language: "typescript".into(),
                offset: 0,
                len: KNOWN_BAD.len() as u64,
            }],
            config: 1,
            environment: Vec::new(),
            threads: Some(1),
            targets: None,
        },
        KNOWN_BAD.as_bytes(),
    );
    assert_eq!(answer, Ok(Payload::ProjectDeclined("no tsconfig.json".into())));
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
        file("src/other.ts", Language::TypeScript, KNOWN_BAD),
        file("src/third.ts", Language::TypeScript, KNOWN_BAD),
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
        targets: None,
        cache: None,
    };
    // `project_report`, not `project_check`: the latter flattens a decline into no findings (ADR 0079), so a
    // component that declined where the in-process engine checked would compare equal to a clean pass.
    let expected = TsrsEngine.project_report(&request).expect("in-process check");
    let actual = engine.project_report(&request).expect("component check");
    assert_eq!(format!("{actual:?}"), format!("{expected:?}"));
    let actual = checked(actual);
    assert_eq!(
        actual.len(),
        3,
        "the project root is not the working directory: {actual:?}"
    );
    assert_eq!(actual[0].1[0].code.as_deref(), Some("TS2322"));
    assert_eq!(engine.slot().status(), SlotStatus::Ready);

    // One target of three sources on one checker narrows: the targets cross the wire, relative to the root.
    let targets = [project.path().join("src/other.ts")];
    let narrowed = ProjectRequest {
        threads: Some(1),
        targets: Some(&targets),
        ..request
    };
    let expected = TsrsEngine.project_report(&narrowed).expect("in-process check");
    let actual = engine.project_report(&narrowed).expect("component check");
    assert_eq!(format!("{actual:?}"), format!("{expected:?}"));
    let actual = checked(actual);
    let paths: Vec<_> = actual.iter().map(|(path, _)| path.clone()).collect();
    assert_eq!(paths, targets, "only the target was checked");
}

/// The findings of a check that ran; a decline fails the test, naming its reason.
fn checked(report: ProjectReport) -> Vec<(std::path::PathBuf, Vec<poly_engine_api::engine::Diagnostic>)> {
    match report {
        ProjectReport::Checked(findings) => findings,
        ProjectReport::Declined(reason) => panic!("the project was declined: {reason}"),
    }
}
