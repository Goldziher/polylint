//! Stale-cache discipline guard: `version()` is folded into the content-hash cache key, so it must embed the
//! version of every tsrs crate the check runs on, as resolved in the components workspace `Cargo.lock`.
//!
//! When the tsrs pins are bumped but `version()` is not, this test fails. The web family's
//! `crates/poly-engines-web/tests/version_audit.rs` audits the other engines of this workspace.

use std::collections::BTreeMap;
use std::path::Path;

use poly_engine_api::engine::Engine;
use poly_engines_tsrs::TsrsEngine;

/// Every tsrs crate the engine links, itself or through `polylint-tsrs`, which must resolve to one version.
const TSRS_CRATES: &[&str] = &[
    "tsrs_ast",
    "tsrs_binder",
    "tsrs_compiler",
    "tsrs_contentmapper",
    "tsrs_core",
    "tsrs_diagnostics",
    "tsrs_module",
    "tsrs_tsoptions",
    "tsrs_vfs",
];

/// `name -> versions` of every package in `Cargo.lock`.
fn locked() -> BTreeMap<String, Vec<String>> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../Cargo.lock");
    let text = std::fs::read_to_string(&path).unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
    let mut map: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for block in text.split("[[package]]") {
        let field = |key: &str| {
            block
                .lines()
                .find_map(|line| line.strip_prefix(key))
                .map(|rest| rest.trim_end_matches('"').to_owned())
        };
        if let (Some(name), Some(version)) = (field("name = \""), field("version = \"")) {
            map.entry(name).or_default().push(version);
        }
    }
    map
}

#[test]
fn version_embeds_the_locked_version_of_every_tsrs_crate() {
    let lock = locked();
    let version = TsrsEngine.version();
    for name in TSRS_CRATES {
        let versions = lock
            .get(*name)
            .unwrap_or_else(|| panic!("crate `{name}` not found in Cargo.lock"));
        assert_eq!(versions.len(), 1, "`{name}` resolves to one version: {versions:?}");
        assert!(
            version.contains(&versions[0]),
            "version() = {version:?} must embed {:?}, the locked version of `{name}`; bump the engine's version() so \
             stale cached output is invalidated",
            versions[0]
        );
    }
}

#[test]
fn version_names_the_engine_and_the_mapping_revision() {
    let version = TsrsEngine.version();
    assert!(version.starts_with("tsrs-"), "{version}");
    assert!(version.contains("+proj"), "{version}");
}
