//! Every method of the `Engine` trait and its facets must have a wire mapping, and every `Diagnostic`
//! must survive a trip through `WireDiagnostic`, whether or not it is the requested engine's own finding.

use std::collections::BTreeSet;

use poly_component_protocol::{ENGINE_METHODS, WireDiagnostic};
use poly_core::engine::{Diagnostic, Edit, Severity, Span};
use poly_engine_api::component_wire::{diagnostic_from_wire, wire_diagnostic};
use proptest::prelude::*;

/// Method names declared inside `pub trait Engine` and its facet traits, scanned from source text.
fn engine_trait_methods() -> BTreeSet<String> {
    let api = concat!(env!("CARGO_MANIFEST_DIR"), "/../poly-engine-api/src");
    let traits: [(&str, &[&str]); 2] = [
        ("engine.rs", &["Engine"]),
        (
            "engine/facets.rs",
            &["Batching", "ProjectAnalysis", "PlanPolicy", "CatalogTakeover"],
        ),
    ];
    let mut methods = BTreeSet::new();
    for (file, names) in traits {
        let source = std::fs::read_to_string(format!("{api}/{file}")).unwrap();
        for name in names {
            let header = format!("pub trait {name}:");
            let mut lines = source.lines().skip_while(|l| !l.starts_with(&header));
            assert!(lines.next().is_some(), "`pub trait {name}` not found in {file}");
            for line in lines {
                if line == "}" {
                    break;
                }
                if let Some(rest) = line.strip_prefix("    fn ") {
                    let method: String = rest.chars().take_while(|c| c.is_alphanumeric() || *c == '_').collect();
                    methods.insert(method);
                }
            }
        }
    }
    methods
}

#[test]
fn every_engine_method_has_a_protocol_mapping() {
    let trait_methods = engine_trait_methods();
    assert!(trait_methods.len() > 20, "scan found only {trait_methods:?}");
    let mapped: BTreeSet<String> = ENGINE_METHODS.iter().map(|(name, _)| name.to_string()).collect();
    let unmapped: Vec<_> = trait_methods.difference(&mapped).collect();
    let stale: Vec<_> = mapped.difference(&trait_methods).collect();
    assert!(
        unmapped.is_empty(),
        "Engine or facet methods with no entry in poly_component_protocol::ENGINE_METHODS: {unmapped:?}"
    );
    assert!(
        stale.is_empty(),
        "ENGINE_METHODS entries that are not Engine or facet methods: {stale:?}"
    );
}

fn arb_opt_string() -> impl Strategy<Value = Option<String>> {
    prop::option::of(any::<String>())
}

fn arb_diagnostic() -> impl Strategy<Value = Diagnostic> {
    let severity = prop::sample::select(vec![Severity::Error, Severity::Warning, Severity::Info, Severity::Hint]);
    let span = prop::option::of(
        (any::<u32>(), any::<u32>(), any::<u32>(), any::<u32>()).prop_map(|(a, b, c, d)| Span {
            start_line: a,
            start_col: b,
            end_line: c,
            end_col: d,
        }),
    );
    let edit = (any::<u32>(), any::<u32>(), any::<String>()).prop_map(|(s, e, replacement)| Edit {
        start_byte: s as usize,
        end_byte: e as usize,
        replacement,
    });
    (
        any::<String>(),
        arb_opt_string(),
        severity,
        any::<String>(),
        arb_opt_string(),
        span,
        arb_opt_string(),
        prop::collection::vec(edit, 0..4),
        prop::collection::btree_map(any::<String>(), any::<String>(), 0..4),
    )
        .prop_map(
            |(engine, code, severity, title, description, span, url, fix, metadata)| Diagnostic {
                engine,
                code,
                severity,
                title,
                description,
                span,
                url,
                fix,
                metadata,
            },
        )
}

proptest! {
    #[test]
    fn diagnostic_round_trips_through_wire_diagnostic(d in arb_diagnostic(), own in any::<bool>()) {
        // The requested engine is either the finding's own (its id is elided) or another one.
        let requested = if own { d.engine.clone() } else { format!("{}-other", d.engine) };
        let wire = wire_diagnostic(&d, &requested);
        prop_assert_eq!(wire.engine.is_none(), own);
        let back = diagnostic_from_wire(wire, &requested).unwrap();
        prop_assert_eq!(serde_json::to_value(&back).unwrap(), serde_json::to_value(&d).unwrap());
        // `skip_serializing_if` hides empty fix/metadata and None options in JSON; compare them directly too.
        prop_assert_eq!(back.fix.len(), d.fix.len());
        prop_assert_eq!(back.metadata, d.metadata);
        prop_assert_eq!(back.span, d.span);
        prop_assert_eq!(back.code, d.code);
    }
}

/// The encoding is positional (protocol v3), so a diagnostic is an array with a slot for every field:
/// an unset optional is a `nil` in its slot, never a missing element that would shift the rest.
#[test]
fn unset_optionals_stay_present_on_the_wire() {
    let d = Diagnostic {
        engine: "e".into(),
        code: None,
        severity: Severity::Hint,
        title: "t".into(),
        description: None,
        span: None,
        url: None,
        fix: vec![],
        metadata: Default::default(),
    };
    let wire = wire_diagnostic(&d, "e");
    assert_eq!(wire.engine, None);
    let bytes = encoded_diagnostic(&wire);
    // `fixarray(9)`: engine, code, severity, title, description, span, url, fix, metadata.
    let slots = [
        0x99, 0xc0, 0xc0, 0xa4, b'H', b'i', b'n', b't', 0xa1, b't', 0xc0, 0xc0, 0xc0, 0x90, 0x80,
    ];
    assert_eq!(bytes, slots);
}

/// The diagnostic's own bytes: the `Diagnostics` response encoded around it, with the fixed prefix
/// (`[{"Ok": {"Diagnostics": [`) and suffix (`], []]`) stripped.
fn encoded_diagnostic(wire: &WireDiagnostic) -> Vec<u8> {
    use poly_component_protocol::{Payload, Response, encode_response};
    let response = Response {
        result: Ok(Payload::Diagnostics(vec![wire.clone()])),
        logs: vec![],
    };
    let bytes = encode_response(&response).unwrap();
    let mut prefix = vec![0x92, 0x81, 0xa2, b'O', b'k', 0x81, 0xab];
    prefix.extend_from_slice(b"Diagnostics");
    prefix.push(0x91);
    assert!(bytes.starts_with(&prefix), "{bytes:x?}");
    assert!(bytes.ends_with(&[0x90]), "{bytes:x?}");
    bytes[prefix.len()..bytes.len() - 1].to_vec()
}
