//! The option declarations of the tsrs engine are honest: every declared key is one its sources read, and every
//! key its sources read is declared.

use std::collections::BTreeSet;
use std::path::Path;

use poly_engine_api::engine::{Engine, OptionTable, OptionType};
use poly_engines_tsrs::TsrsEngine;

/// The files that read the engine's options, relative to this crate's `src/`.
const SOURCES: &[&str] = &["settings.rs", "engine.rs"];

fn source() -> String {
    SOURCES
        .iter()
        .map(|file| {
            std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("src").join(file))
                .unwrap_or_else(|error| panic!("read {file}: {error}"))
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// The literal keys read as `options.get("key")`.
fn read_keys(source: &str) -> BTreeSet<String> {
    let mut keys = BTreeSet::new();
    for marker in ["options.get(\""] {
        for part in source.split(marker).skip(1) {
            if let Some(end) = part.find('"') {
                keys.insert(part[..end].to_owned());
            }
        }
    }
    keys
}

#[test]
fn the_tables_are_declared() {
    for table in [OptionTable::Lint, OptionTable::Format] {
        assert!(TsrsEngine.option_keys(table).is_checked(), "{table:?} is unchecked");
    }
}

#[test]
fn the_declared_keys_are_the_keys_the_sources_read() {
    let declared = TsrsEngine.option_keys(OptionTable::Lint).declared_keys();
    let names: BTreeSet<String> = declared.iter().map(|(key, _)| (*key).to_owned()).collect();
    assert_eq!(
        read_keys(&source()),
        names,
        "a key is read without being declared, or declared without being read"
    );
    let type_of = |key: &str| declared.iter().find(|(name, _)| *name == key).map(|(_, ty)| *ty);
    for key in ["enabled", "strict", "skip_lib_check", "report_unresolved_modules"] {
        assert_eq!(type_of(key), Some(OptionType::BOOLEAN), "{key}");
    }
    for key in ["project", "dir"] {
        assert_eq!(type_of(key), Some(OptionType::STRING), "{key}");
    }
    assert_eq!(type_of("threads"), Some(OptionType::INTEGER));
}
