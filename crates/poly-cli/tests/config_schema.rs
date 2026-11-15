//! The published `poly.toml` JSON Schema must match what the binary generates.
//!
//! `website/public/poly.schema.json` is served for editor validation and
//! autocomplete, and referenced from the configuration guide. A change to any
//! config type, or to an engine's declared option keys, silently stales it —
//! this test is what makes that a build failure instead. Compare *parsed* JSON,
//! not bytes: the committed file is normalised by `poly fmt`, which need not
//! match `serde_json`'s pretty printer byte-for-byte.

/// Path of the published schema, relative to this crate.
const PUBLISHED: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../website/public/poly.schema.json");

#[test]
fn published_config_schema_is_current() {
    let generated = poly_core::config_schema::poly_toml_schema();
    let text = std::fs::read_to_string(PUBLISHED).unwrap_or_else(|error| panic!("reading {PUBLISHED}: {error}"));
    let committed: serde_json::Value =
        serde_json::from_str(&text).unwrap_or_else(|error| panic!("parsing {PUBLISHED}: {error}"));

    assert_eq!(
        generated, committed,
        "the published poly.toml schema is stale; regenerate it with \
         `poly config schema --output website/public/poly.schema.json` then `poly fmt --fix` \
         website/public/poly.schema.json"
    );
}

/// Path of the v2 schema as committed under `schemas/`, the source of the published copy.
const V2_SOURCE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../schemas/poly-v2.schema.json");

/// Path of the published v2 schema, the target of `poly migrate --to-v2`'s `#:schema` line.
const V2_PUBLISHED: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../website/public/poly-v2.schema.json");

#[test]
fn published_v2_schema_matches_the_committed_one() {
    let read = |path: &str| -> serde_json::Value {
        let text = std::fs::read_to_string(path).unwrap_or_else(|error| panic!("reading {path}: {error}"));
        serde_json::from_str(&text).unwrap_or_else(|error| panic!("parsing {path}: {error}"))
    };
    assert_eq!(
        read(V2_SOURCE),
        read(V2_PUBLISHED),
        "website/public/poly-v2.schema.json is stale; copy schemas/poly-v2.schema.json over it"
    );
}
