//! What the oxc engine's external mode adds to its cache key.
//!
//! `[lint.<lang>.oxc] js_plugins = "external"` hands files governed by a config with `jsPlugins` to the
//! project's own `oxlint`, and `[fmt.<lang>.oxc] external = true` hands files needing JavaScript-side
//! options to `oxfmt`, each found in the nearest `node_modules/.bin` or on `PATH`: nearest to the
//! governing config for `oxlint`, to the file for `oxfmt`, and the key looks where the engine does. The answer then
//! depends on which binary that is and, for lint, on the plugin modules, none of which is in the
//! file, its options or the config digest. This keys them:
//!
//! - **the binary**: the `package.json` of the npm package it belongs to (its version), else the
//!   size and modification time of the executable;
//! - **the plugins**: each `jsPlugins` entry of the governing config, by the bytes of a relative
//!   module, or the `package.json` of a package resolved from `node_modules`. A plugin's own imports
//!   are not followed; upgrading its package changes its `package.json`.
//!
//! Both are memoised per process, like the config chain they hang off.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock, PoisonError};

use crate::config::EngineConfig;
use crate::options::OptionsExt;

/// The extra lint key input for `path` in external mode, `None` when the mode is off.
#[must_use]
pub fn lint_salt(cfg: &EngineConfig, path: &Path) -> Option<String> {
    if cfg.options.opt_str("js_plugins") != Some("external") {
        return None;
    }
    // Without a governing config nothing is handed to `oxlint`, so no binary is an input.
    let Some(governing) = super::oxlintrc::governing_for(cfg, path) else {
        return Some("oxlint-ext:unconfigured".to_owned());
    };
    let config = governing.nearest.as_path();
    static BINARIES: OnceLock<Memo> = OnceLock::new();
    let binary = binary_digest(&BINARIES, "oxlint", config.parent().unwrap_or(Path::new("")));
    Some(format!("oxlint-ext:{binary}:{}", plugins_digest(config)))
}

/// The extra format key input for `path` in external mode, `None` when the mode is off.
#[must_use]
pub fn format_salt(cfg: &EngineConfig, path: &Path) -> Option<String> {
    static BINARIES: OnceLock<Memo> = OnceLock::new();
    cfg.options.opt_bool("external").unwrap_or(false).then(|| {
        let dir = path.parent().unwrap_or(Path::new(""));
        format!("oxfmt-ext:{}", binary_digest(&BINARIES, "oxfmt", dir))
    })
}

type Memo = Mutex<HashMap<PathBuf, String>>;

fn memoised(memo: &'static OnceLock<Memo>, key: &Path, compute: impl FnOnce() -> String) -> String {
    let memo = memo.get_or_init(Memo::default);
    if let Some(known) = memo.lock().unwrap_or_else(PoisonError::into_inner).get(key) {
        return known.clone();
    }
    let value = compute();
    memo.lock()
        .unwrap_or_else(PoisonError::into_inner)
        .insert(key.to_path_buf(), value.clone());
    value
}

/// The binary `name` the engine runs from the directory `dir`: the nearest `node_modules/.bin` at
/// or above it, else `PATH`.
fn locate(name: &str, dir: &Path) -> Option<PathBuf> {
    let dir = if dir.as_os_str().is_empty() {
        Path::new(".")
    } else {
        dir
    };
    let start = std::path::absolute(dir).ok()?;
    start
        .ancestors()
        .map(|dir| dir.join("node_modules").join(".bin").join(name))
        .find(|candidate| candidate.is_file())
        .or_else(|| {
            std::env::var_os("PATH")
                .into_iter()
                .flat_map(|paths| std::env::split_paths(&paths).collect::<Vec<_>>())
                .map(|dir| dir.join(name))
                .find(|candidate| candidate.is_file())
        })
}

/// What identifies the binary `name` found from `dir`, located and read once per directory (`memo`
/// is the caller's, one per binary name).
fn binary_digest(memo: &'static OnceLock<Memo>, name: &str, dir: &Path) -> String {
    memoised(memo, dir, || {
        let Some(binary) = locate(name, dir) else {
            return "absent".to_owned();
        };
        let real = std::fs::canonicalize(&binary).unwrap_or(binary);
        match owning_package(&real) {
            Some(manifest) => blake3::hash(&super::file_bytes::for_digest(&manifest))
                .to_hex()
                .to_string(),
            None => std::fs::metadata(&real).map_or_else(
                |_| "unreadable".to_owned(),
                |metadata| {
                    let modified = metadata
                        .modified()
                        .ok()
                        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
                        .map_or(0, |elapsed| elapsed.as_nanos());
                    format!("{}:{modified}", metadata.len())
                },
            ),
        }
    })
}

/// The `package.json` of the npm package `file` belongs to: the nearest one above it inside a
/// `node_modules` tree.
fn owning_package(file: &Path) -> Option<PathBuf> {
    if !file.components().any(|part| part.as_os_str() == "node_modules") {
        return None;
    }
    file.ancestors()
        .skip(1)
        .map(|dir| dir.join("package.json"))
        .find(|manifest| manifest.is_file())
}

/// A digest of the `jsPlugins` modules the oxlint config at `config` names.
fn plugins_digest(config: &Path) -> String {
    static MEMO: OnceLock<Memo> = OnceLock::new();
    memoised(&MEMO, config, || {
        let dir = config.parent().unwrap_or(Path::new(""));
        let mut hasher = blake3::Hasher::new();
        for specifier in plugin_specifiers(config) {
            hasher.update(specifier.as_bytes());
            hasher.update(b"\0");
            let module = if specifier.starts_with('.') || Path::new(&specifier).is_absolute() {
                Some(dir.join(&specifier))
            } else {
                dir.ancestors()
                    .map(|ancestor| ancestor.join("node_modules").join(&specifier).join("package.json"))
                    .find(|manifest| manifest.is_file())
            };
            match module {
                Some(module) => hasher.update(&super::file_bytes::for_digest(&module)),
                None => hasher.update(b"\0unresolved"),
            };
        }
        hasher.finalize().to_hex().to_string()
    })
}

/// The module specifiers of the config's `jsPlugins` (a string, or an object's `specifier`).
fn plugin_specifiers(config: &Path) -> Vec<String> {
    let Ok(text) = std::fs::read_to_string(config) else {
        return Vec::new();
    };
    let Ok(serde_json::Value::Object(root)) = json5::from_str::<serde_json::Value>(&text) else {
        return Vec::new();
    };
    let Some(serde_json::Value::Array(entries)) = root.get("jsPlugins") else {
        return Vec::new();
    };
    entries
        .iter()
        .filter_map(|entry| match entry {
            serde_json::Value::String(specifier) => Some(specifier.clone()),
            serde_json::Value::Object(object) => object.get("specifier")?.as_str().map(str::to_owned),
            _ => None,
        })
        .collect()
}

// Every test installs `oxlint` as a `node_modules/.bin` symlink, which only unix can create unprivileged.
#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::config::GlobalDefaults;

    fn cfg(options: &str) -> EngineConfig {
        EngineConfig {
            globals: GlobalDefaults::default(),
            indent_width: 2,
            options: options.parse().unwrap(),
        }
    }

    /// The plugin module's bytes and the binary's package are part of the external-mode key.
    #[test]
    fn plugin_and_binary_edits_change_the_external_lint_salt() {
        let repo = tempfile::tempdir().unwrap();
        let rc = repo.path().join(".oxlintrc.json");
        std::fs::write(&rc, r#"{"jsPlugins": ["./plugin.js"]}"#).unwrap();
        std::fs::write(repo.path().join("plugin.js"), "export default {}\n").unwrap();
        let bin = repo.path().join("node_modules/.bin");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::create_dir_all(repo.path().join("node_modules/oxlint")).unwrap();
        std::fs::write(
            repo.path().join("node_modules/oxlint/package.json"),
            r#"{"version": "1.0.0"}"#,
        )
        .unwrap();
        std::fs::write(repo.path().join("node_modules/oxlint/oxlint"), "#!/bin/sh\n").unwrap();
        std::os::unix::fs::symlink("../oxlint/oxlint", bin.join("oxlint")).unwrap();
        let options = format!("js_plugins = \"external\"\noxlintrc = {:?}", rc.to_string_lossy());
        let file = repo.path().join("a.ts");

        assert_eq!(
            lint_salt(&cfg(&format!("oxlintrc = {:?}", rc.to_string_lossy())), &file),
            None
        );
        let salt = lint_salt(&cfg(&options), &file).expect("external mode is keyed");
        assert!(!salt.contains("absent"), "{salt}");
        let manifest = repo.path().join("node_modules/oxlint/package.json");
        let version = blake3::hash(&std::fs::read(manifest).unwrap()).to_hex().to_string();
        assert!(salt.contains(&version), "{salt}");
        assert_ne!(plugins_digest(&rc), {
            let other = repo.path().join("other.json");
            std::fs::write(&other, r#"{"jsPlugins": ["./missing.js"]}"#).unwrap();
            plugins_digest(&other)
        });
        assert_eq!(format_salt(&cfg(""), &file), None);
        assert!(format_salt(&cfg("external = true"), &file).is_some());
    }

    fn install_oxlint(dir: &Path, version: &str) {
        let package = dir.join("node_modules/oxlint");
        std::fs::create_dir_all(&package).unwrap();
        std::fs::create_dir_all(dir.join("node_modules/.bin")).unwrap();
        std::fs::write(package.join("package.json"), format!(r#"{{"version": "{version}"}}"#)).unwrap();
        std::fs::write(package.join("oxlint"), "#!/bin/sh\n").unwrap();
        std::os::unix::fs::symlink("../oxlint/oxlint", dir.join("node_modules/.bin/oxlint")).unwrap();
    }

    /// The engine runs the `oxlint` nearest the governing config, not the one nearest the file, so
    /// that is the one the key reads.
    #[test]
    fn the_lint_salt_reads_the_binary_nearest_the_governing_config() {
        let salt_with = |root_version: &str, package_version: &str| {
            let repo = tempfile::tempdir().unwrap();
            let rc = repo.path().join(".oxlintrc.json");
            std::fs::write(&rc, r#"{"jsPlugins": []}"#).unwrap();
            let package = repo.path().join("packages/a");
            std::fs::create_dir_all(package.join("src")).unwrap();
            install_oxlint(repo.path(), root_version);
            install_oxlint(&package, package_version);
            let options = format!("js_plugins = \"external\"\noxlintrc = {:?}", rc.to_string_lossy());
            lint_salt(&cfg(&options), &package.join("src/a.ts")).expect("external mode is keyed")
        };
        let base = salt_with("1.0.0", "1.0.0");
        assert_ne!(
            base,
            salt_with("2.0.0", "1.0.0"),
            "the root oxlint is the one that runs"
        );
        assert_eq!(base, salt_with("1.0.0", "2.0.0"), "the package's own oxlint is not run");
    }
}
