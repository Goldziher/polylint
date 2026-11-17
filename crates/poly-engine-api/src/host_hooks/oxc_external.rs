//! What the oxc engine's external mode adds to its cache key.
//!
//! `[lint.<lang>.oxc] js_plugins = "external"` hands files governed by a config with `jsPlugins` to the
//! project's own `oxlint`, and `[fmt.<lang>.oxc] external = true` hands files needing JavaScript-side
//! options to `oxfmt`, each found in the nearest `node_modules/.bin` or on `PATH`. The answer then
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
    let config = super::oxlintrc::governing_for(cfg, path).map(|governing| governing.nearest.clone());
    let plugins = config.as_deref().map(plugins_digest).unwrap_or_default();
    Some(format!("oxlint-ext:{}:{plugins}", binary_digest("oxlint", path)))
}

/// The extra format key input for `path` in external mode, `None` when the mode is off.
#[must_use]
pub fn format_salt(cfg: &EngineConfig, path: &Path) -> Option<String> {
    cfg.options
        .opt_bool("external")
        .unwrap_or(false)
        .then(|| format!("oxfmt-ext:{}", binary_digest("oxfmt", path)))
}

type Memo = Mutex<HashMap<PathBuf, String>>;

fn memoised(memo: &'static OnceLock<Memo>, key: PathBuf, compute: impl FnOnce() -> String) -> String {
    let memo = memo.get_or_init(Memo::default);
    if let Some(known) = memo.lock().unwrap_or_else(PoisonError::into_inner).get(&key) {
        return known.clone();
    }
    let value = compute();
    memo.lock()
        .unwrap_or_else(PoisonError::into_inner)
        .insert(key, value.clone());
    value
}

/// The binary `name` the engine would run for `path` (the nearest `node_modules/.bin`, else `PATH`).
fn locate(name: &str, path: &Path) -> Option<PathBuf> {
    let start = std::path::absolute(path).ok()?;
    start
        .ancestors()
        .skip(1)
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

fn binary_digest(name: &str, path: &Path) -> String {
    static MEMO: OnceLock<Memo> = OnceLock::new();
    let Some(binary) = locate(name, path) else {
        return "absent".to_owned();
    };
    memoised(&MEMO, binary.clone(), || {
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
    memoised(&MEMO, config.to_path_buf(), || {
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

#[cfg(test)]
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
    #[cfg(unix)]
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
        assert_eq!(binary_digest("oxlint", &file), {
            let manifest = repo.path().join("node_modules/oxlint/package.json");
            blake3::hash(&std::fs::read(manifest).unwrap()).to_hex().to_string()
        });
        assert_ne!(plugins_digest(&rc), {
            let other = repo.path().join("other.json");
            std::fs::write(&other, r#"{"jsPlugins": ["./missing.js"]}"#).unwrap();
            plugins_digest(&other)
        });
        assert_eq!(format_salt(&cfg(""), &file), None);
        assert!(format_salt(&cfg("external = true"), &file).is_some());
    }
}
