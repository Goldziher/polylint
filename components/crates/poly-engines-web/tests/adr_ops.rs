//! `Request::Adr` through the `web` component's producer, in-process: the wire path a core build takes
//! for `poly adr` (ADR 0061) plans the same change as the linked planner.

use poly_component_protocol::{
    PROTOCOL_VERSION, Payload, Request, WireAdrOp, WireAdrPlan, WireErrorKind, WireFileSlice, WireLogLevel,
    decode_response, encode_request,
};
use poly_engine_api::component_wire::engine_config_to_wire;
use poly_engine_api::config::{EngineConfig, GlobalDefaults};
use poly_engines::PolyComponent;
use poly_engines_web::WebComponent;

const RECORD: &str =
    "---\nstatus: accepted\ndate: 2026-01-01\n---\n\n# Use Rust\n\n## Context and Problem Statement\n\nText.\n";

fn call(component: &WebComponent, request: &Request, body: &[u8]) -> Result<Payload, WireErrorKind> {
    let header = encode_request(request).expect("encode");
    let bytes = component.call(&header, body).expect("call");
    decode_response(&bytes)
        .expect("decode")
        .result
        .map_err(|error| error.kind)
}

fn initialised() -> WebComponent {
    let component = WebComponent::default();
    let init = Request::Init {
        protocol: PROTOCOL_VERSION,
        host_version: "test".into(),
        threads: 1,
        stack_size: 16 << 20,
        max_log_level: WireLogLevel::Warn,
    };
    assert!(matches!(call(&component, &init, &[]), Ok(Payload::Init { .. })));
    component
}

fn config() -> poly_component_protocol::WireValue {
    engine_config_to_wire(&EngineConfig {
        globals: GlobalDefaults::default(),
        indent_width: 2,
        options: toml::Table::new(),
    })
}

#[test]
fn the_web_component_plans_a_lock_over_the_wire() {
    let component = initialised();
    let request = Request::Adr {
        op: WireAdrOp::Lock {
            all: false,
            check: false,
        },
        config: config(),
        files: vec![WireFileSlice {
            path: "docs/adr/0001-use-rust.md".into(),
            language: "markdown".into(),
            offset: 0,
            len: RECORD.len() as u64,
        }],
    };
    let Ok(Payload::Adr(WireAdrPlan { writes, summary, .. })) = call(&component, &request, RECORD.as_bytes()) else {
        panic!("the web component did not answer Adr with a plan");
    };
    assert_eq!(summary, "locked 1 ADR(s) in adrlint.lock");
    assert_eq!(writes.len(), 1);
    assert_eq!(writes[0].path, "adrlint.lock");
    let manifest = adrlint::lock::parse_manifest(&writes[0].content).expect("manifest parses");
    assert_eq!(
        manifest.entry("docs/adr/0001-use-rust.md"),
        Some(&adrlint::lock::entry(&adrlint::adr::parse(RECORD), 1))
    );
}

#[test]
fn an_engine_error_comes_back_as_a_wire_error() {
    let component = initialised();
    let request = Request::Adr {
        op: WireAdrOp::Unlock {
            path: "docs/adr/0001-use-rust.md".into(),
        },
        config: config(),
        files: vec![],
    };
    assert_eq!(call(&component, &request, &[]), Err(WireErrorKind::Error));
}
