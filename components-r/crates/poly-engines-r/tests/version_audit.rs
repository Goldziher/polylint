//! Stale-cache discipline guard: `version()` is folded into the content-hash cache key, so it must embed the pinned
//! git rev of every crate the engine's output depends on, as resolved in this workspace's `Cargo.lock`.
//!
//! When a pin is bumped but `version()` is not, this test fails and names the engine to bump. The web family's
//! `crates/poly-engines-web/tests/version_audit.rs` audits the engines of `components/`.

use std::collections::HashMap;
use std::path::Path;

use poly_engine_api::engine::Engine;
use poly_engines_r::{AirEngine, JarlEngine};

/// The number of git-rev characters that must appear in `version()`.
const GIT_REV_PREFIX: usize = 7;

/// `name -> pinned git rev` of every git package in `Cargo.lock`.
fn locked_revs() -> HashMap<String, String> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../Cargo.lock");
    let text = std::fs::read_to_string(&path).unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
    let mut map = HashMap::new();
    for block in text.split("[[package]]") {
        let field = |key: &str| {
            block
                .lines()
                .find_map(|line| line.strip_prefix(key))
                .map(|rest| rest.trim_end_matches('"').to_owned())
        };
        let (Some(name), Some(source)) = (field("name = \""), field("source = \"git+")) else {
            continue;
        };
        if let Some(hash) = source.rfind('#') {
            map.insert(name, source[hash + 1..].to_owned());
        }
    }
    map
}

fn assert_embeds(engine: &dyn Engine, crates: &[&str], lock: &HashMap<String, String>) {
    let version = engine.version();
    for name in crates {
        let rev = lock
            .get(*name)
            .unwrap_or_else(|| panic!("crate `{name}` is not a git dependency in Cargo.lock"));
        let short = &rev[..GIT_REV_PREFIX];
        assert!(
            version.contains(short),
            "engine `{}` version() = {version:?} must embed the git rev {short:?} of `{name}` (from Cargo.lock). The \
             pin was bumped but version() was not; bump the engine's version() so stale cached output is invalidated.",
            engine.name()
        );
    }
}

#[test]
fn engine_versions_track_cargo_lock() {
    let lock = locked_revs();
    // air's output is the formatter on its biome; jarl's findings come from its own rules on air's parser and ark's
    // semantic analysis.
    assert_embeds(
        &AirEngine,
        &["air_r_formatter", "air_r_parser", "biome_formatter"],
        &lock,
    );
    assert_embeds(
        &JarlEngine,
        &["jarl-core", "jarl-semantic", "air_r_parser", "oak_core"],
        &lock,
    );
}

#[test]
fn air_and_jarl_share_one_air_and_one_biome() {
    let lock = locked_revs();
    for family in [
        ["air_r_formatter", "air_r_parser", "workspace"].as_slice(),
        ["biome_formatter", "biome_rowan", "biome_parser"].as_slice(),
    ] {
        let revs: std::collections::BTreeSet<_> = family.iter().filter_map(|name| lock.get(*name)).collect();
        assert_eq!(revs.len(), 1, "{family:?} resolve to one rev each: {revs:?}");
    }
}
