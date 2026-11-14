//! The committed component index entry for `tsrs` equals what this crate's engine describes, since `poly-core`
//! cannot link it (the entry is carried over from the committed index by its own `tests/component_index.rs`).
//!
//! Refresh it, after `UPDATE_INDEX=1 cargo test -p poly-core --test component_index` has added the `tsrs` entry,
//! with `UPDATE_INDEX=1 cargo test --manifest-path components-tsrs/Cargo.toml -p poly-engines-tsrs --test component_index`,
//! then `poly fmt --fix` the file.

use poly_component_index::ComponentIndex;
use poly_engine_api::desc::engine_desc;
use poly_engine_api::language::Language;
use poly_engines_tsrs::{TsrsEngine, engines_for};

#[test]
fn the_provider_serves_the_languages_the_check_reads_and_no_other() {
    for language in [
        Language::TypeScript,
        Language::Tsx,
        Language::JavaScript,
        Language::Jsx,
        Language::Json,
        Language::Jsonc,
    ] {
        let names: Vec<_> = engines_for(&language).iter().map(|engine| engine.name()).collect();
        assert_eq!(names, ["tsrs"], "{language:?}");
    }
    for language in [Language::Python, Language::Yaml, Language::Markdown, Language::Css] {
        assert!(engines_for(&language).is_empty(), "{language:?}");
    }
}

#[test]
fn the_committed_index_describes_the_tsrs_component() {
    let path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../crates/poly-component-index/data/index.json");
    let text = std::fs::read_to_string(&path).expect("index.json is committed");
    let mut index = ComponentIndex::parse(&text).expect("index.json parses");
    let described = vec![engine_desc(&TsrsEngine)];
    if std::env::var_os("UPDATE_INDEX").is_some() {
        index
            .components
            .iter_mut()
            .find(|c| c.name == "tsrs")
            .expect("the index declares the tsrs component; run the poly-core index test with UPDATE_INDEX=1 first")
            .engines = described;
        let mut rendered = serde_json::to_string_pretty(&index).expect("index serializes");
        rendered.push('\n');
        std::fs::write(&path, rendered).expect("write index.json");
        return;
    }
    let component = index.component("tsrs").expect("the index declares the tsrs component");
    assert!(component.opt_in, "tsrs is opt-in: it is experimental");
    assert!(component.download_only, "no bundled build links tsrs");
    assert_eq!(
        component.engines, described,
        "the committed index is stale for tsrs; regenerate it with \
         `UPDATE_INDEX=1 cargo test --manifest-path components-tsrs/Cargo.toml -p poly-engines-tsrs --test component_index`, \
         then `poly fmt --fix` the file"
    );
}
