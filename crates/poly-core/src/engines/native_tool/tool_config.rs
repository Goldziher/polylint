//! Content fingerprints of the *tool's own* config files.
//!
//! A native-toolchain backend shells out to a first-party CLI, and several of
//! those CLIs read their own project config from disk before they format or
//! lint anything. That config is a real input to the tool's output, but poly's
//! result cache never saw it: the cache key folds in the engine id, its
//! [`Engine::version`], the TOML-serialized poly-side args and the file's
//! path + bytes — and a native tool takes *no* poly-side args (`enabled` is its
//! only key). Editing `rustfmt.toml` therefore changed nothing in the key, and
//! the next run served the previous run's formatting. This module computes a
//! blake3 fingerprint of those files so a config edit invalidates the cache,
//! mirroring what `astgrep`'s `rules_hash` does for user rule directories.
//!
//! ## Which tools read a config file
//!
//! Verified empirically against the installed binaries, invoked exactly the way
//! [`super::format::format_via_tool`] / [`super::lint::lint_via_shellcheck`]
//! invoke them (stdin → stdout, poly's argv):
//!
//! | Tool                 | Reads config | Files                             |
//! |----------------------|--------------|-----------------------------------|
//! | `rustfmt`            | yes          | `.rustfmt.toml`, `rustfmt.toml`   |
//! | `shellcheck`         | yes          | `.shellcheckrc`, `shellcheckrc`   |
//! | `swift-format`       | yes          | `.swift-format`                   |
//! | `gofmt`              | no           | — (flags only, no config file)    |
//! | `zig fmt`            | no           | —                                 |
//! | `shfmt`              | no           | — (see below)                     |
//! | `google-java-format` | no           | —                                 |
//! | `ktfmt`              | no           | —                                 |
//! | `Rscript` / styler   | no           | — (`--vanilla`, no `.Rprofile`)   |
//! | `dart format`        | no           | — (see below)                     |
//! | `gleam format`       | no           | —                                 |
//!
//! Two "no" entries depend on *how* poly invokes the tool and would flip if the
//! invocation changed:
//!
//! - **`shfmt`** consults `.editorconfig` only when it is given a filename.
//!   poly pipes stdin without `--filename` and always passes `-i <width>`, and
//!   under that argv `.editorconfig` is inert to shfmt itself. poly resolves it
//!   per file instead ([`poly_engine_api::host_hooks::editorconfig`]) and keys the cache on the
//!   resolved indentation, so it is not a fingerprinted config file here.
//!   Adding `--filename` to [`super::spec::SHFMT_SPEC`] would make
//!   `.editorconfig` an input of shfmt and require listing it here.
//! - **`dart format`** reads `formatter: page_width:` from
//!   `analysis_options.yaml` only when formatting a named file; over stdin the
//!   setting has no effect (the `--page-width` flag does, which is how the
//!   check was falsified).
//!
//! ## Bounded scope — read this before trusting the fingerprint
//!
//! The fingerprint is anchored at the **process's current directory** (a run
//! rooted elsewhere, ADR 0073, anchors it at that root) and walks *upward*
//! through its ancestors. It is **not** anchored per source file, even
//! though `rustfmt` and `swift-format` themselves search upward from the file's
//! own directory (poly runs those two with `current_dir` set to the file's
//! parent). Two structural facts force this:
//!
//! 1. [`Engine::version`] returns a borrowed `&str` and is called **once per
//!    file per engine** inside the runner's rayon `par_iter`. Anything computed
//!    there must be memoised, so it cannot vary per file.
//! 2. The format cache key hashes the file's **content only** — no path (see
//!    `ResultCache::single_file_digest`) — so a per-directory value could not be
//!    expressed in it even if it were free to compute.
//!
//! What follows from that:
//!
//! - **Covered:** a config file at or above the directory poly was invoked from
//!   — the overwhelmingly common `poly fmt .` at the repo root. For
//!   `shellcheck` this is *exactly* right: poly does not re-anchor the child, so
//!   shellcheck resolves `.shellcheckrc` from poly's own cwd.
//! - **Not covered by `version()`:** a config file in a subdirectory *below*
//!   the invocation directory that only governs part of a monorepo. For the
//!   tools poly runs in the file's own directory (`rustfmt`, `swift-format`)
//!   that gap is closed per file instead: [`file_salt`] fingerprints the config
//!   files from the file's directory upward — the files the tool itself
//!   resolves — plus, for `rustfmt`, the `--edition` poly passes and the
//!   `rust-toolchain[.toml]` that picks which `rustfmt` the rustup proxy runs
//!   there. It is the engine's `format_input_salt` and `file_context`, so both
//!   the content key and the stat fast path carry it, memoised per directory.
//!   `shellcheck` runs in poly's own directory and resolves `.shellcheckrc`
//!   from there, which `version()` covers exactly.
//! - **Not covered:** an edit made during the lifetime of one process. The
//!   fingerprint is computed once per process (it lives inside the `OnceLock`
//!   behind [`Engine::version`]; [`file_salt`] memoises per directory), so a
//!   long-lived host — `poly mcp` — keeps the fingerprint it read first. Every
//!   `poly` CLI invocation is a fresh process and so always reads fresh.
//!
//! [`Engine::version`]: crate::engine::Engine::version
//! [`Engine`]: crate::engine::Engine

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock, PoisonError, RwLock};

use super::spec::ToolSpec;

/// Config files `rustfmt` reads, in the order it probes them within one
/// directory (the dot form wins on a tie). Both names are hashed when both
/// exist, so which one rustfmt picks does not affect invalidation.
pub(crate) const RUSTFMT_CONFIG_FILES: &[&str] = &[".rustfmt.toml", "rustfmt.toml"];

/// Config files `shellcheck` reads. Both the dotted and undotted spellings are
/// honoured by shellcheck itself.
pub(crate) const SHELLCHECK_CONFIG_FILES: &[&str] = &[".shellcheckrc", "shellcheckrc"];

/// Config file `swift-format` reads.
pub(crate) const SWIFT_FORMAT_CONFIG_FILES: &[&str] = &[".swift-format"];

/// The rustup toolchain files that select which `rustfmt` the proxy runs in a directory, so a nested one can pick
/// another `rustfmt` than the one `--version` was probed from.
pub(crate) const RUST_TOOLCHAIN_FILES: &[&str] = &["rust-toolchain.toml", "rust-toolchain"];

/// The per-file cache input of a tool poly runs in the file's own directory: the fingerprint of the config files the
/// tool resolves from there, and for `rustfmt` the edition poly passes and the toolchain file. `None` for a tool run
/// in poly's own directory, whose config [`fingerprint`] already covers.
///
/// Memoised per directory, so it costs a map lookup per file once a directory has been seen.
pub(crate) fn file_salt(spec: &ToolSpec, path: &Path) -> Option<String> {
    let runs_in_file_dir = spec.rustfmt_config_flag || spec.run_in_file_dir;
    if !runs_in_file_dir || spec.config_files.is_empty() {
        return None;
    }
    // Absolute, as the tool sees its working directory: the ancestors of a relative `./a` end at the
    // run root, and count it twice (`.` and the empty path), so a relative and an absolute run of one
    // tree would otherwise key apart.
    let dir = absolute_dir(path.parent().unwrap_or(path));
    let config = dir_fingerprint(spec.engine_name, dir, |dir| {
        let mut fingerprint = fingerprint_at(dir, spec.config_files);
        if spec.edition_flag {
            fingerprint.push_str("|toolchain:");
            fingerprint.push_str(&fingerprint_at(dir, RUST_TOOLCHAIN_FILES));
        }
        fingerprint
    });
    Some(if spec.edition_flag {
        format!("edition:{}|cfg:{config}", super::edition::resolve_edition(path))
    } else {
        format!("cfg:{config}")
    })
}

/// `dir` under the run root (ADR 0073), else under the process directory; unchanged when it is absolute already or
/// the process directory cannot be read.
fn absolute_dir(dir: &Path) -> PathBuf {
    if dir.is_absolute() {
        return dir.to_path_buf();
    }
    poly_engine_api::run_root::current_dir().map_or_else(|_| dir.to_path_buf(), |base| base.join(dir))
}

type DirFingerprints = RwLock<HashMap<(&'static str, PathBuf), Arc<str>>>;

/// The fingerprint of `tool`'s config as seen from `dir`, built by `build` the first time this process sees the pair.
fn dir_fingerprint(tool: &'static str, dir: PathBuf, build: impl FnOnce(&Path) -> String) -> Arc<str> {
    static SEEN: OnceLock<DirFingerprints> = OnceLock::new();
    let seen = SEEN.get_or_init(DirFingerprints::default);
    let key = (tool, dir);
    if let Some(known) = seen.read().unwrap_or_else(PoisonError::into_inner).get(&key) {
        return Arc::clone(known);
    }
    let built: Arc<str> = Arc::from(build(&key.1));
    seen.write()
        .unwrap_or_else(PoisonError::into_inner)
        .entry(key)
        .or_insert(built)
        .clone()
}

/// Fingerprint returned when no candidate config file exists anywhere on the
/// search path.
///
/// It must be distinct from the fingerprint of an *empty* config file: an empty
/// file still contributes its own path to the hash, so it produces a blake3 hex
/// digest, never this sentinel. "config absent" and "config present but empty"
/// are different tool inputs and must key differently.
const NO_CONFIG: &str = "absent";

/// Fingerprint returned when the process has no readable current directory
/// (deleted cwd, or a permission error). Distinct from [`NO_CONFIG`] so the two
/// states never share a cache key.
const NO_ANCHOR: &str = "unanchored";

/// Fingerprint the config files named by `names`, searching upward from `root`,
/// the run root, or else from the process's current directory.
///
/// Returns [`NO_ANCHOR`] when the current directory cannot be read. See the
/// module docs for what this does and does not cover.
///
/// # Cost
///
/// One `open` attempt per (ancestor, name) pair plus a read of each file that
/// exists — tens of microseconds, paid **once per process** (once per run root,
/// see [`rooted_version`]) because the only caller stores the result behind
/// [`Engine::version`](crate::engine::Engine::version). It must never be called
/// on the per-file path.
pub(crate) fn fingerprint(names: &[&str], root: Option<&Path>) -> String {
    match root {
        Some(anchor) => fingerprint_at(anchor, names),
        None => match std::env::current_dir() {
            Ok(anchor) => fingerprint_at(&anchor, names),
            Err(_) => NO_ANCHOR.to_owned(),
        },
    }
}

/// The version keys of the roles that fingerprint a config file, by run root and
/// then by tool, for runs rooted somewhere other than the process directory (ADR 0073).
type RootedKeys = RwLock<HashMap<PathBuf, HashMap<&'static str, &'static str>>>;

/// The version key of `tool` for a run rooted at `root`, built by `build` the
/// first time this process sees that pair.
///
/// The process-wide `OnceLock` an unrooted run uses would hand a staged-snapshot
/// run the fingerprint of whichever root asked first, so a rooted run keys its own.
/// The lookup borrows `root` and allocates nothing once the key exists; each key
/// is leaked once per distinct (root, tool), which a process sees a handful of.
pub(crate) fn rooted_version(tool: &'static str, root: &Path, build: impl FnOnce() -> String) -> &'static str {
    static KEYS: OnceLock<RootedKeys> = OnceLock::new();
    let keys = KEYS.get_or_init(RootedKeys::default);
    let known = keys
        .read()
        .unwrap_or_else(PoisonError::into_inner)
        .get(root)
        .and_then(|by_tool| by_tool.get(tool).copied());
    if let Some(key) = known {
        return key;
    }
    let mut keys = keys.write().unwrap_or_else(PoisonError::into_inner);
    let by_tool = keys.entry(root.to_path_buf()).or_default();
    by_tool
        .entry(tool)
        .or_insert_with(|| Box::leak(build().into_boxed_str()));
    by_tool[tool]
}

/// [`fingerprint`] with an explicit anchor directory, so the search is testable
/// without mutating the process's current directory.
///
/// Hashes the path **and** the bytes of every existing candidate found in
/// `anchor` and each of its ancestors, nearest first. The path is taken relative
/// to `anchor` (`../rustfmt.toml` for a parent's), so worktrees of one
/// repository fingerprint alike while moving a config file up or down the tree
/// still changes the fingerprint even when its bytes do not — the tool would
/// resolve a different file, so the cache must too. Hashing *every* match rather than
/// stopping at the first only ever over-invalidates, which is the safe
/// direction.
pub(crate) fn fingerprint_at(anchor: &Path, names: &[&str]) -> String {
    let mut hasher = blake3::Hasher::new();
    let mut found_any = false;

    for (depth, dir) in anchor.ancestors().enumerate() {
        for name in names {
            let Ok(bytes) = std::fs::read(dir.join(name)) else {
                continue;
            };
            for _ in 0..depth {
                hasher.update(b"../");
            }
            hasher.update(name.as_bytes());
            hasher.update(b"\0");
            hasher.update(&bytes);
            hasher.update(b"\0");
            found_any = true;
        }
    }

    if found_any {
        hasher.finalize().to_hex().to_string()
    } else {
        NO_CONFIG.to_owned()
    }
}

/// The config file the tool would resolve from `anchor`: the nearest ancestor
/// (including `anchor`) holding any of `names`, with `names` listed in the
/// tool's own order of preference.
///
/// This is the single file a tool picks, where [`fingerprint_at`] deliberately
/// hashes every candidate; it is what has to be handed to a tool that is run
/// somewhere other than where it would have found the config itself.
pub(crate) fn nearest_config_at(anchor: &Path, names: &[&str]) -> Option<std::path::PathBuf> {
    anchor
        .ancestors()
        .flat_map(|dir| names.iter().map(move |name| dir.join(name)))
        .find(|candidate| candidate.is_file())
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    /// A directory with no candidate config file yields the absent sentinel,
    /// not a hash.
    #[test]
    fn missing_config_yields_the_absent_sentinel() {
        let dir = tempfile::tempdir().expect("tempdir");
        assert_eq!(fingerprint_at(dir.path(), RUSTFMT_CONFIG_FILES), NO_CONFIG);
    }

    /// An *empty* config file is a different tool input from no config file at
    /// all, so it must not collide with the absent sentinel.
    #[test]
    fn empty_config_hashes_distinctly_from_a_missing_one() {
        let dir = tempfile::tempdir().expect("tempdir");
        let missing = fingerprint_at(dir.path(), RUSTFMT_CONFIG_FILES);
        fs::write(dir.path().join("rustfmt.toml"), "").expect("write empty config");
        let empty = fingerprint_at(dir.path(), RUSTFMT_CONFIG_FILES);

        assert_eq!(missing, NO_CONFIG);
        assert_ne!(empty, NO_CONFIG, "an empty config file must still hash");
        assert_ne!(empty, missing);
    }

    /// The whole point: editing the config content changes the fingerprint.
    #[test]
    fn editing_the_config_changes_the_fingerprint() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("rustfmt.toml");
        fs::write(&path, "max_width = 120\n").expect("write config");
        let before = fingerprint_at(dir.path(), RUSTFMT_CONFIG_FILES);
        fs::write(&path, "max_width = 40\n").expect("rewrite config");
        let after = fingerprint_at(dir.path(), RUSTFMT_CONFIG_FILES);

        assert_ne!(before, after, "a config edit must change the fingerprint");
    }

    /// The same content in a different directory of the search path is a
    /// different resolution for the tool, so it must hash differently.
    #[test]
    fn the_same_content_at_a_different_depth_hashes_differently() {
        let dir = tempfile::tempdir().expect("tempdir");
        let nested = dir.path().join("nested");
        fs::create_dir(&nested).expect("mkdir");

        fs::write(dir.path().join("rustfmt.toml"), "max_width = 40\n").expect("write parent config");
        let parent_only = fingerprint_at(&nested, RUSTFMT_CONFIG_FILES);

        fs::write(nested.join("rustfmt.toml"), "max_width = 40\n").expect("write nested config");
        let both = fingerprint_at(&nested, RUSTFMT_CONFIG_FILES);

        assert_ne!(parent_only, both, "a nearer config file must change the fingerprint");
    }

    /// Two checkouts of one project, at different absolute paths, fingerprint
    /// alike; different config bytes still differ.
    #[test]
    fn the_fingerprint_is_relative_to_the_anchor() {
        let one = tempfile::tempdir().expect("tempdir");
        let two = tempfile::tempdir().expect("tempdir");
        let three = tempfile::tempdir().expect("tempdir");
        for (dir, width) in [(&one, 40), (&two, 40), (&three, 80)] {
            fs::write(dir.path().join("rustfmt.toml"), format!("max_width = {width}\n")).expect("write config");
        }

        let at = |dir: &tempfile::TempDir| fingerprint_at(dir.path(), RUSTFMT_CONFIG_FILES);
        assert_eq!(at(&one), at(&two));
        assert_ne!(at(&one), at(&three));
    }

    /// A config file above the anchor is found: the search walks ancestors, as
    /// `rustfmt` / `shellcheck` / `swift-format` all do.
    #[test]
    fn a_config_file_in_an_ancestor_is_found() {
        let dir = tempfile::tempdir().expect("tempdir");
        let deep = dir.path().join("a").join("b");
        fs::create_dir_all(&deep).expect("mkdir -p");
        fs::write(dir.path().join(".shellcheckrc"), "disable=SC2086\n").expect("write config");

        assert_ne!(
            fingerprint_at(&deep, SHELLCHECK_CONFIG_FILES),
            NO_CONFIG,
            "an ancestor's config file must be part of the fingerprint"
        );
    }

    /// Each tool only hashes the files it actually reads: a `rustfmt.toml`
    /// must not perturb shellcheck's fingerprint.
    #[test]
    fn an_unrelated_tools_config_is_ignored() {
        let dir = tempfile::tempdir().expect("tempdir");
        let before = fingerprint_at(dir.path(), SHELLCHECK_CONFIG_FILES);
        fs::write(dir.path().join("rustfmt.toml"), "max_width = 40\n").expect("write config");

        assert_eq!(fingerprint_at(dir.path(), SHELLCHECK_CONFIG_FILES), before);
    }

    #[test]
    fn the_nearest_config_wins_and_the_dotted_name_is_preferred_within_a_directory() {
        let dir = tempfile::tempdir().expect("tempdir");
        let nested = dir.path().join("a");
        fs::create_dir(&nested).expect("mkdir");
        fs::write(dir.path().join(".shellcheckrc"), "").expect("write parent config");
        assert_eq!(
            nearest_config_at(&nested, SHELLCHECK_CONFIG_FILES),
            Some(dir.path().join(".shellcheckrc"))
        );

        fs::write(nested.join("shellcheckrc"), "").expect("write undotted config");
        fs::write(nested.join(".shellcheckrc"), "").expect("write dotted config");
        assert_eq!(
            nearest_config_at(&nested, SHELLCHECK_CONFIG_FILES),
            Some(nested.join(".shellcheckrc"))
        );
    }

    #[test]
    fn no_config_anywhere_finds_nothing() {
        let dir = tempfile::tempdir().expect("tempdir");
        assert_eq!(nearest_config_at(dir.path(), &["poly-test-no-such-config-file"]), None);
    }

    /// What the stat fast path and the format content key fold in for a Rust file: the engine's `file_context`, which
    /// must equal its `format_input_salt`.
    fn rustfmt_file_context(path: &Path) -> Option<String> {
        use crate::engine::Engine;
        let engine = super::super::NativeToolEngine::for_language(crate::language::Language::Rust);
        let cfg = crate::config::EngineConfig {
            globals: crate::config::GlobalDefaults::default(),
            indent_width: 4,
            options: toml::Table::new(),
        };
        let context = engine.file_context(path, &cfg);
        assert_eq!(context, engine.format_input_salt(path, &cfg));
        context
    }

    /// S1: the `--edition` poly passes rustfmt is part of the file's key, so a crate moving to another edition does
    /// not replay the previous edition's formatting.
    #[test]
    fn the_rustfmt_file_salt_carries_the_crate_edition() {
        let salt_for = |edition: &str| {
            let dir = tempfile::tempdir().expect("tempdir");
            fs::create_dir(dir.path().join("src")).expect("mkdir");
            fs::write(
                dir.path().join("Cargo.toml"),
                format!("[package]\nname = \"x\"\nversion = \"0.1.0\"\nedition = \"{edition}\"\n"),
            )
            .expect("write manifest");
            rustfmt_file_context(&dir.path().join("src/lib.rs"))
        };
        let (old, new) = (salt_for("2021"), salt_for("2024"));
        assert!(
            old.as_deref().is_some_and(|salt| salt.contains("edition:2021")),
            "{old:?}"
        );
        assert_ne!(old, new);
    }

    /// S2: a `rustfmt.toml` below the invocation directory governs the files under it, and keys them; so does a
    /// nested `rust-toolchain.toml`, which picks another `rustfmt`.
    #[test]
    fn the_rustfmt_file_salt_carries_nested_config_and_toolchain_files() {
        let salt_with = |file: Option<(&str, &str)>| {
            let dir = tempfile::tempdir().expect("tempdir");
            let crate_dir = dir.path().join("rs");
            fs::create_dir_all(crate_dir.join("src")).expect("mkdir");
            if let Some((name, text)) = file {
                fs::write(crate_dir.join(name), text).expect("write config");
            }
            rustfmt_file_context(&crate_dir.join("src/main.rs"))
        };
        let plain = salt_with(None);
        let narrow = salt_with(Some(("rustfmt.toml", "max_width = 40\n")));
        let wide = salt_with(Some(("rustfmt.toml", "max_width = 100\n")));
        let toolchain = salt_with(Some(("rust-toolchain.toml", "[toolchain]\nchannel = \"nightly\"\n")));
        assert_ne!(plain, narrow);
        assert_ne!(narrow, wide);
        assert_ne!(plain, toolchain);
        assert_eq!(
            narrow,
            salt_with(Some(("rustfmt.toml", "max_width = 40\n"))),
            "checkout-independent"
        );
    }

    /// A relative and an absolute spelling of one file key alike, with a config file at the run root. (`swift-format`
    /// has the salt without an edition, which is resolved from the process directory.)
    #[test]
    fn the_file_salt_is_the_same_for_a_relative_and_an_absolute_path() {
        use std::sync::Arc;

        use super::super::spec::SWIFT_FORMAT_SPEC;
        let dir = tempfile::tempdir().expect("tempdir");
        fs::create_dir_all(dir.path().join("a")).expect("mkdir");
        fs::write(dir.path().join(".swift-format"), "{}\n").expect("config");
        let _entered = poly_engine_api::run_root::enter(Some(Arc::from(dir.path())));
        let salt = |path: &Path| file_salt(&SWIFT_FORMAT_SPEC, path);
        let absolute = salt(&dir.path().join("a/b.swift"));
        assert!(
            absolute.as_deref().is_some_and(|salt| !salt.contains(NO_CONFIG)),
            "{absolute:?}"
        );
        assert_eq!(salt(Path::new("./a/b.swift")), absolute);
        assert_eq!(salt(Path::new("a/b.swift")), absolute);
        assert_ne!(salt(Path::new("./c.swift")), absolute, "the config is one level nearer");
        assert_eq!(salt(Path::new("./c.swift")), salt(&dir.path().join("c.swift")));
        assert_eq!(salt(Path::new("c.swift")), salt(&dir.path().join("c.swift")));
    }

    /// `swift-format` runs in the file's directory too; `shellcheck` and the tools without config files have no
    /// per-file salt.
    #[test]
    fn only_tools_run_in_the_file_directory_have_a_file_salt() {
        use super::super::spec::{GOFMT_SPEC, SHELLCHECK_SPEC, SWIFT_FORMAT_SPEC};
        let dir = tempfile::tempdir().expect("tempdir");
        let file = dir.path().join("a");
        assert!(file_salt(&SWIFT_FORMAT_SPEC, &file).is_some());
        assert_eq!(file_salt(&SHELLCHECK_SPEC, &file), None);
        assert_eq!(file_salt(&GOFMT_SPEC, &file), None);
    }

    /// A tool with no config files is a constant: no filesystem access, always
    /// the absent sentinel.
    #[test]
    fn a_tool_with_no_config_files_is_always_absent() {
        let dir = tempfile::tempdir().expect("tempdir");
        fs::write(dir.path().join("rustfmt.toml"), "max_width = 40\n").expect("write config");
        assert_eq!(fingerprint_at(dir.path(), &[]), NO_CONFIG);
    }

    /// Two runs rooted at different directories in one process (a staged
    /// snapshot and its worktree, ADR 0073) each see their own root's config:
    /// neither is handed the fingerprint the other computed first.
    #[test]
    fn runs_rooted_at_different_directories_do_not_share_a_fingerprint() {
        use std::sync::Arc;

        use crate::engine::Engine;
        use crate::language::Language;

        let staged = tempfile::tempdir().expect("tempdir");
        let worktree = tempfile::tempdir().expect("tempdir");
        fs::write(staged.path().join("rustfmt.toml"), "max_width = 40\n").expect("write config");
        fs::write(worktree.path().join("rustfmt.toml"), "max_width = 120\n").expect("write config");
        let engine = super::super::NativeToolEngine::for_language(Language::Rust);
        let version_in = |root: &Path| {
            let _entered = poly_engine_api::run_root::enter(Some(Arc::from(root)));
            engine.version().to_owned()
        };

        let first = version_in(staged.path());
        let second = version_in(worktree.path());
        assert_ne!(first, second, "a second root must not reuse the first root's key");
        assert!(
            first.contains(&fingerprint_at(staged.path(), RUSTFMT_CONFIG_FILES)),
            "{first}"
        );
        assert!(
            second.contains(&fingerprint_at(worktree.path(), RUSTFMT_CONFIG_FILES)),
            "{second}"
        );
        assert_eq!(version_in(staged.path()), first, "a root keeps its own key");
    }
}
