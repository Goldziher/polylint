//! The Go language version and module path gofumpt formats a file with, read from its `go.mod`.
//!
//! gofumpt's rules depend on the module's `go` directive (an octal literal becomes `0o777` only
//! from Go 1.13) and its path (which imports are standard library). The `go` sidecar sees only the
//! file's bytes, so the host finds the nearest `go.mod` and passes both as the `lang_version` and
//! `module_path` options, unless the options set them, and keys the format cache on them.
//!
//! It is a host hook ([`HostHook::GoMod`](super::HostHook::GoMod)).

use std::borrow::Cow;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use crate::config::EngineConfig;

const LANG_VERSION: &str = "lang_version";
const MODULE_PATH: &str = "module_path";

/// What a `go.mod` says that gofumpt needs.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GoMod {
    /// The `module` path.
    pub module: Option<String>,
    /// The `go` directive's version, such as `1.22` or `1.22.1`.
    pub go: Option<String>,
}

/// The first `module` and `go` directives of a `go.mod`. A `module (` block is not a path and is
/// skipped; the directives are single-line in every `go.mod` the go command writes.
fn parse(text: &str) -> GoMod {
    let mut parsed = GoMod::default();
    for line in text.lines() {
        let line = line.split("//").next().unwrap_or_default();
        let mut words = line.split_whitespace();
        match (words.next(), words.next()) {
            (Some("module"), Some(path)) if path != "(" && parsed.module.is_none() => {
                parsed.module = Some(path.trim_matches(['"', '`']).to_owned());
            }
            (Some("go"), Some(version)) if parsed.go.is_none() => parsed.go = Some(version.to_owned()),
            _ => {}
        }
    }
    parsed
}

/// The parsed `go.mod` in `dir`, once per directory.
fn load(dir: &Path) -> Option<Arc<GoMod>> {
    static MEMO: OnceLock<Mutex<HashMap<PathBuf, Option<Arc<GoMod>>>>> = OnceLock::new();
    let memo = MEMO.get_or_init(Mutex::default);
    if let Some(known) = memo.lock().unwrap_or_else(std::sync::PoisonError::into_inner).get(dir) {
        return known.clone();
    }
    let parsed = std::fs::read_to_string(dir.join("go.mod"))
        .ok()
        .map(|text| Arc::new(parse(&text)));
    memo.lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .insert(dir.to_path_buf(), parsed.clone());
    parsed
}

/// The `go.mod` governing `path`: the nearest one in its directory or above.
#[must_use]
pub fn nearest(path: &Path) -> Option<Arc<GoMod>> {
    nearest_root(path).map(|(_, module)| module)
}

/// The directory holding the `go.mod` governing `path`, with that `go.mod`: the module root a
/// file's module-relative path and its module-internal import paths are resolved against.
#[must_use]
pub fn nearest_root(path: &Path) -> Option<(PathBuf, Arc<GoMod>)> {
    let absolute = std::path::absolute(path).ok()?;
    absolute
        .parent()?
        .ancestors()
        .find_map(|dir| load(dir).map(|module| (dir.to_path_buf(), module)))
}

/// What the *lint* result of a Go file depends on beyond its bytes: the module root and path the
/// ast-grep Go architecture rules resolve imports and path globs against. `None` for any other
/// file, and for a Go file outside a module.
#[must_use]
pub fn lint_salt(path: &Path) -> Option<String> {
    if path.extension().is_none_or(|ext| ext != "go") {
        return None;
    }
    let (root, module) = nearest_root(path)?;
    // The file's path within its module, not the module's absolute root: what the rules resolve
    // against, and the same in every checkout of the tree.
    let absolute = std::path::absolute(path).ok()?;
    let within = absolute.strip_prefix(&root).unwrap_or(&absolute);
    Some(format!(
        "go-mod-lint:{}:{}",
        within.to_string_lossy().replace('\\', "/"),
        module.module.as_deref().unwrap_or_default()
    ))
}

/// The `(option key, go.mod value)` pairs the options table has not already set.
fn missing<'a>(cfg: &EngineConfig, module: &'a GoMod) -> impl Iterator<Item = (&'static str, &'a str)> {
    [
        (LANG_VERSION, module.go.as_deref()),
        (MODULE_PATH, module.module.as_deref()),
    ]
    .into_iter()
    .filter(|(key, _)| !cfg.options.contains_key(*key))
    .filter_map(|(key, value)| value.map(|value| (key, value)))
}

/// `cfg` with the `go.mod` version and module path of `path` filled in.
pub fn apply<'a>(cfg: &'a EngineConfig, path: &Path) -> Cow<'a, EngineConfig> {
    let Some(module) = nearest(path) else {
        return Cow::Borrowed(cfg);
    };
    let mut owned: Option<EngineConfig> = None;
    for (key, value) in missing(cfg, &module) {
        owned
            .get_or_insert_with(|| cfg.clone())
            .options
            .insert(key.to_owned(), toml::Value::String(value.to_owned()));
    }
    owned.map_or(Cow::Borrowed(cfg), Cow::Owned)
}

/// What the formatted output of `path` depends on beyond its bytes: the `go.mod` values `apply`
/// adds.
#[must_use]
pub fn salt(cfg: &EngineConfig, path: &Path) -> Option<String> {
    let module = nearest(path)?;
    let parts: Vec<String> = missing(cfg, &module)
        .map(|(key, value)| format!("{key}={value}"))
        .collect();
    (!parts.is_empty()).then(|| format!("go-mod:{}", parts.join(",")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::GlobalDefaults;

    fn repo(files: &[(&str, &str)]) -> tempfile::TempDir {
        let repo = tempfile::tempdir().unwrap();
        for (name, text) in files {
            let path = repo.path().join(name);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }
        repo
    }

    fn cfg(options: &str) -> EngineConfig {
        EngineConfig {
            globals: GlobalDefaults::default(),
            indent_width: 2,
            options: toml::from_str(options).unwrap(),
        }
    }

    fn option<'a>(cfg: &'a EngineConfig, key: &str) -> Option<&'a str> {
        cfg.options.get(key).and_then(toml::Value::as_str)
    }

    /// M4: the lint salt names the file within its module, so two checkouts of a module salt alike.
    #[test]
    fn the_lint_salt_does_not_depend_on_the_checkout_location() {
        let salt = || {
            let dir = repo(&[
                ("go.mod", "module example.com/m\n\ngo 1.22\n"),
                ("pkg/a.go", "package pkg\n"),
            ]);
            lint_salt(&dir.path().join("pkg/a.go"))
        };
        let first = salt();
        assert!(
            first.as_deref().is_some_and(|salt| salt.contains("pkg/a.go")),
            "{first:?}"
        );
        assert_eq!(first, salt());
    }

    #[test]
    fn the_module_and_go_directives_are_read() {
        let parsed =
            parse("// header\nmodule \"example.com/m\" // note\n\ngo 1.22.1\ntoolchain go1.23.0\nrequire x v1\n");
        assert_eq!(parsed.module.as_deref(), Some("example.com/m"));
        assert_eq!(parsed.go.as_deref(), Some("1.22.1"));
        assert_eq!(parse("module (\n)\n").module, None);
        assert_eq!(parse("").go, None);
    }

    #[test]
    fn the_nearest_go_mod_fills_the_options() {
        let repo = repo(&[
            ("go.mod", "module example.com/root\n\ngo 1.20\n"),
            ("sub/go.mod", "module example.com/sub\n\ngo 1.22\n"),
        ]);
        let inner = repo.path().join("sub/pkg/a.go");
        let bare = cfg("");
        let applied = apply(&bare, &inner);
        assert_eq!(option(&applied, LANG_VERSION), Some("1.22"));
        assert_eq!(option(&applied, MODULE_PATH), Some("example.com/sub"));
        let outer = repo.path().join("b.go");
        assert_eq!(option(&apply(&bare, &outer), LANG_VERSION), Some("1.20"));
    }

    #[test]
    fn options_the_user_sets_win_and_stay_out_of_the_salt() {
        let repo = repo(&[("go.mod", "module example.com/m\n\ngo 1.22\n")]);
        let path = repo.path().join("a.go");
        let set = cfg("lang_version = \"1.18\"\n");
        let applied = apply(&set, &path);
        assert_eq!(option(&applied, LANG_VERSION), Some("1.18"));
        assert_eq!(option(&applied, MODULE_PATH), Some("example.com/m"));
        assert_eq!(salt(&set, &path).as_deref(), Some("go-mod:module_path=example.com/m"));
        let all = cfg("lang_version = \"1.18\"\nmodule_path = \"x\"\n");
        assert!(matches!(apply(&all, &path), Cow::Borrowed(_)));
        assert_eq!(salt(&all, &path), None);
    }

    #[test]
    fn the_nearest_root_is_the_directory_of_the_nearest_go_mod() {
        let repo = repo(&[
            ("go.mod", "module example.com/root\n"),
            ("sub/go.mod", "module example.com/sub\n"),
            ("sub/pkg/a.go", "package pkg\n"),
            ("b.go", "package b\n"),
        ]);
        let (root, module) = nearest_root(&repo.path().join("sub/pkg/a.go")).unwrap();
        assert_eq!(root, std::path::absolute(repo.path().join("sub")).unwrap());
        assert_eq!(module.module.as_deref(), Some("example.com/sub"));
        let (root, _) = nearest_root(&repo.path().join("b.go")).unwrap();
        assert_eq!(root, std::path::absolute(repo.path()).unwrap());
    }

    #[test]
    fn the_lint_salt_names_the_module_for_go_files_only() {
        let repo = repo(&[("go.mod", "module example.com/m\n"), ("a.go", ""), ("a.rs", "")]);
        let salt = lint_salt(&repo.path().join("a.go")).unwrap();
        assert!(salt.ends_with(":example.com/m"), "{salt}");
        assert_eq!(lint_salt(&repo.path().join("a.rs")), None);
        let bare = tempfile::tempdir().unwrap();
        assert_eq!(lint_salt(&bare.path().join("a.go")), None);
    }

    #[test]
    fn a_file_outside_a_module_is_formatted_as_it_is() {
        let repo = repo(&[("a.go", "package a\n")]);
        let path = repo.path().join("a.go");
        assert!(matches!(apply(&cfg(""), &path), Cow::Borrowed(_)));
        assert_eq!(salt(&cfg(""), &path), None);
    }

    #[test]
    fn the_go_version_is_part_of_the_salt() {
        let one = repo(&[("go.mod", "module m\n\ngo 1.21\n")]);
        let two = repo(&[("go.mod", "module m\n\ngo 1.22\n")]);
        assert_ne!(
            salt(&cfg(""), &one.path().join("a.go")),
            salt(&cfg(""), &two.path().join("a.go"))
        );
    }
}
