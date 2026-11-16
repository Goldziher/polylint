//! The committed component index entry for `r` equals what this crate's engines describe, since `poly-core` cannot
//! link them (the entry is carried over from the committed index by its own `tests/component_index.rs`).
//!
//! Refresh it, after `UPDATE_INDEX=1 cargo test -p poly-core --test component_index` has added the `r` entry, with
//! `UPDATE_INDEX=1 cargo test --manifest-path components-r/Cargo.toml -p poly-engines-r --test component_index`, then
//! `poly fmt --fix` the file.

use poly_component_index::ComponentIndex;
use poly_engine_api::desc::engine_desc;
use poly_engine_api::language::Language;
use poly_engines_r::engines_for;

#[test]
fn the_committed_index_describes_the_r_component() {
    let path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../crates/poly-component-index/data/index.json");
    let text = std::fs::read_to_string(&path).expect("index.json is committed");
    let mut index = ComponentIndex::parse(&text).expect("index.json parses");
    let described: Vec<_> = engines_for(&Language::R)
        .iter()
        .map(|engine| engine_desc(engine.as_ref()))
        .collect();
    if std::env::var_os("UPDATE_INDEX").is_some() {
        index
            .components
            .iter_mut()
            .find(|c| c.name == "r")
            .expect("the index declares the r component; run the poly-core index test with UPDATE_INDEX=1 first")
            .engines = described;
        let mut rendered = serde_json::to_string_pretty(&index).expect("index serializes");
        rendered.push('\n');
        std::fs::write(&path, rendered).expect("write index.json");
        return;
    }
    let component = index.component("r").expect("the index declares the r component");
    assert!(!component.opt_in, "R is formatted and linted by default");
    assert!(component.download_only, "no bundled build links air or jarl");
    assert_eq!(
        component.engines, described,
        "the committed index is stale for r; regenerate it with \
         `UPDATE_INDEX=1 cargo test --manifest-path components-r/Cargo.toml -p poly-engines-r --test component_index`, \
         then `poly fmt --fix` the file"
    );
}
