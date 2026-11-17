//! [`ENGINE`] names the tsrs release the crate is built against: it is written by hand, so it is held to the exact
//! version the manifest pins every `tsrs_*` crate at.

use polylint_js_types::ENGINE;

/// The manifest as written here or, in a packaged crate, as cargo normalized it.
const MANIFEST: &str = include_str!("../Cargo.toml");

/// The exact versions (`=X`) of a manifest's `tsrs_*` dependencies, written inline (`tsrs_core = { version = "=X" }`)
/// or as a table (`[dependencies.tsrs_core]` then `version = "=X"`, the form `cargo package` writes).
fn tsrs_pins(manifest: &str) -> Vec<(&str, &str)> {
    let mut pins = Vec::new();
    let mut table = None;
    for line in manifest.lines().map(str::trim) {
        if let Some(header) = line.strip_prefix('[') {
            table = header
                .strip_prefix("dependencies.")
                .and_then(|name| name.strip_suffix(']'))
                .filter(|name| name.starts_with("tsrs_"));
            continue;
        }
        let name = if line.starts_with("tsrs_") {
            line.split_once(" = ").map(|(name, _)| name)
        } else if line.starts_with("version = ") {
            table
        } else {
            None
        };
        if let Some(name) = name {
            let version = line
                .split("version = \"=")
                .nth(1)
                .and_then(|tail| tail.split('"').next())
                .unwrap_or_else(|| panic!("`{name}` is not pinned to an exact version: {line}"));
            pins.push((name, version));
        }
    }
    pins
}

#[test]
fn the_engine_names_the_pinned_tsrs_version() {
    let pins = tsrs_pins(MANIFEST);
    assert!(pins.len() >= 2, "the manifest pins the tsrs crates: {pins:?}");
    let (_, version) = pins[0];
    assert!(
        pins.iter().all(|(_, pin)| *pin == version),
        "the tsrs crates move together: {pins:?}"
    );
    assert_eq!(ENGINE, format!("tsrs {version}"));
}

#[test]
fn the_pins_read_the_same_from_a_normalized_manifest() {
    let normalized = "[dependencies.polylint-tsrs]\nversion = \"0.1.0\"\n\n[dependencies.tsrs_core]\nversion = \
                      \"=1.2.3\"\ndefault-features = false\n\n[dev-dependencies.tempfile]\nversion = \"3\"\n";
    assert_eq!(tsrs_pins(normalized), [("tsrs_core", "1.2.3")]);
    let inline = "tsrs_core = { version = \"=1.2.3\", default-features = false }\nrayon = \"1\"\n";
    assert_eq!(tsrs_pins(inline), [("tsrs_core", "1.2.3")]);
}
