//! The committed component index entry for `secrets` equals what this crate's engine describes,
//! since `poly-core` cannot link it (kingfisher lives in this workspace).
//!
//! Refresh it with `UPDATE_INDEX=1 cargo test --manifest-path components-secrets/Cargo.toml
//! -p poly-engines-secrets --test index`, then `poly fmt --fix` the file.

use poly_component_index::ComponentIndex;
use poly_engine_api::desc::engine_desc;
use poly_engines_secrets::SecretsEngine;

#[test]
fn the_committed_index_describes_the_secrets_component() {
    let path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../crates/poly-component-index/data/index.json");
    let text = std::fs::read_to_string(&path).expect("index.json is committed");
    let mut index = ComponentIndex::parse(&text).expect("index.json parses");
    let described = vec![engine_desc(&SecretsEngine)];
    if std::env::var_os("UPDATE_INDEX").is_some() {
        index
            .components
            .iter_mut()
            .find(|c| c.name == "secrets")
            .expect("the index declares the secrets component")
            .engines = described;
        let mut rendered = serde_json::to_string_pretty(&index).expect("index serializes");
        rendered.push('\n');
        std::fs::write(&path, rendered).expect("write index.json");
        return;
    }
    let component = index
        .component("secrets")
        .expect("the index declares the secrets component");
    assert!(component.opt_in, "secrets is opt-in: nothing scans until it is enabled");
    assert!(
        component.download_only,
        "secrets lives outside `components/`, so no bundled build links it"
    );
    assert_eq!(
        component.engines, described,
        "the committed index is stale for secrets; regenerate it with \
         `UPDATE_INDEX=1 cargo test --manifest-path components-secrets/Cargo.toml -p poly-engines-secrets --test index`, \
         then `poly fmt --fix` the file"
    );
}
