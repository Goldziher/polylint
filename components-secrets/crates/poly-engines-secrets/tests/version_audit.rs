//! Stale-cache discipline: the engine's `version()` is part of the result cache key, so it must name the kingfisher
//! crates this workspace resolves. A bump in `Cargo.toml` and `Cargo.lock` without one in the engine fails here.

use std::collections::HashMap;
use std::path::Path;

use poly_engine_api::engine::Engine;
use poly_engines_secrets::SecretsEngine;

fn locked_versions() -> HashMap<String, String> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../Cargo.lock");
    let text = std::fs::read_to_string(&path).unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
    let mut versions = HashMap::new();
    for block in text.split("[[package]]") {
        let field = |key: &str| {
            block
                .lines()
                .find_map(|line| line.strip_prefix(key))
                .map(|rest| rest.trim_end_matches('"').to_owned())
        };
        if let (Some(name), Some(version)) = (field("name = \""), field("version = \"")) {
            versions.insert(name, version);
        }
    }
    versions
}

#[test]
fn the_version_names_the_resolved_kingfisher_crates() {
    let locked = locked_versions();
    let version = SecretsEngine.version().to_owned();
    for (krate, short) in [
        ("kingfisher-scanner", "kingfisher-scanner"),
        ("kingfisher-rules", "rules"),
    ] {
        let resolved = locked
            .get(krate)
            .unwrap_or_else(|| panic!("{krate} is not in components-secrets/Cargo.lock"));
        assert!(
            version.contains(&format!("{short}-{resolved}")),
            "SecretsEngine::version() is `{version}` but the lock resolves {krate} {resolved}; bump the version string"
        );
    }
}
