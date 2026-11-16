//! The native StyLua config (`stylua.toml`, `.stylua.toml`), resolved by poly.
//!
//! StyLua keeps its options in `stylua.toml`; the `stylua` engine takes the same keys in `poly.toml`
//! (`[engines.stylua]`). The file nearest the formatted file (see [`native_config`]) is laid *under* the
//! `poly.toml` table, so the layering is tool default, poly's opinionated globals, the native file, then
//! `poly.toml`. The file's digest salts the format cache key, so editing it invalidates cached output.

use std::borrow::Cow;
use std::path::Path;

use super::native_config::{self, Found};
use crate::config::EngineConfig;

/// Config file names, in the order StyLua tries them within one directory.
pub const FILE_NAMES: &[&str] = &["stylua.toml", ".stylua.toml"];

/// The `stylua.toml` keys the engine reads; anything else in the file is ignored.
pub const KEYS: &[&str] = &[
    "syntax",
    "column_width",
    "line_endings",
    "indent_type",
    "indent_width",
    "quote_style",
    "call_parentheses",
    "collapse_simple_statement",
    "space_after_function_names",
    "block_newline_gaps",
    "sort_requires",
];

fn governing(path: &Path) -> Option<std::sync::Arc<Found>> {
    native_config::nearest("stylua", FILE_NAMES, path)
}

/// The cache-key salt for formatting `path`: a digest of the config that governs it, if any.
#[must_use]
pub fn salt(path: &Path) -> Option<String> {
    governing(path).map(|found| format!("stylua:{}", found.digest))
}

/// `cfg` with the governing `stylua.toml` laid under its options; a key `poly.toml` already sets wins.
#[must_use]
pub fn apply<'a>(cfg: &'a EngineConfig, path: &Path) -> Cow<'a, EngineConfig> {
    let Some(found) = governing(path) else {
        return Cow::Borrowed(cfg);
    };
    if let Some(error) = &found.parse_error {
        let key = format!("stylua-config:{}", found.path.display());
        super::warn_once(key, || {
            tracing::warn!(file = %found.path.display(), %error, "stylua config could not be parsed; ignoring it");
        });
        return Cow::Borrowed(cfg);
    }
    let missing: Vec<_> = KEYS
        .iter()
        .filter(|key| !cfg.options.contains_key(**key))
        .filter_map(|key| found.table.get(*key).map(|value| ((*key).to_owned(), value.clone())))
        .collect();
    if missing.is_empty() {
        return Cow::Borrowed(cfg);
    }
    let mut merged = cfg.clone();
    merged.options.extend(missing);
    Cow::Owned(merged)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::GlobalDefaults;

    fn cfg(options: &str) -> EngineConfig {
        EngineConfig {
            globals: GlobalDefaults::default(),
            indent_width: 4,
            options: options.parse().unwrap(),
        }
    }

    #[test]
    fn the_native_file_sits_under_poly_toml_and_salts_the_cache() {
        let repo = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(repo.path().join(".git")).unwrap();
        std::fs::write(
            repo.path().join(".stylua.toml"),
            "column_width = 80\nquote_style = \"ForceSingle\"\nunknown = 1\n",
        )
        .unwrap();
        native_config::forget();
        let path = repo.path().join("a.lua");

        let base = cfg("quote_style = \"ForceDouble\"\n");
        let merged = apply(&base, &path);
        assert_eq!(merged.options["column_width"].as_integer(), Some(80));
        assert_eq!(merged.options["quote_style"].as_str(), Some("ForceDouble"));
        assert!(!merged.options.contains_key("unknown"));
        assert!(salt(&path).is_some_and(|salt| salt.starts_with("stylua:")));
    }

    #[test]
    fn without_a_native_file_the_config_is_untouched() {
        let repo = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(repo.path().join(".git")).unwrap();
        native_config::forget();
        let path = repo.path().join("a.lua");
        assert!(matches!(apply(&cfg(""), &path), Cow::Borrowed(_)));
        assert_eq!(salt(&path), None);
    }
}
