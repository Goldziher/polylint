//! The config a plain (recipe-less) catalog tool may read, as a per-file cache input.
//!
//! A recipe names the config files its tool reads (`Recipe::config_files`, hashed by `recipe::salt`). A plain
//! catalog tool comes from the mdsf catalog, which names none, and these tools read their own config from disk:
//! `yamllint` finds `.yamllint` in its working directory, a path-mode tool such as `prettier` searches upward from the
//! file. Without that config in the key, editing it served the previous verdict.
//!
//! Lacking a list of names per tool, the salt takes every file *named like a config* (see [`is_config_like`]: a
//! dotfile, an `*rc` file, a `*.config.*` file, or one of the shared manifests tools keep a section in) in the
//! directories a tool can search: the file's own directory and its ancestors, and the tool's working directory and its
//! ancestors. Matching is by name, never by extension alone, so the files a tool lints are not each other's inputs:
//! editing one YAML file leaves its siblings' keys alone. It still over-invalidates (an edit to `package.json` re-runs
//! `yamllint` below it), which is the safe direction for an opt-in tier.
//!
//! Each walk ends at the checkout root (the first directory holding `.git`). Outside a checkout it ends at the run
//! root, and for a file that is not under the run root either it takes that one directory, so it never climbs into
//! a home directory's dotfiles ([`limit`]).
//!
//! Files are named relative to where the walk starts (by depth), never by absolute path, so two checkouts of a
//! repository key alike. A salt is computed once per `(directory, working directory)` for the run; a file whose
//! directory was seen costs one map lookup.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock, PoisonError, RwLock};

/// A config file larger than this is keyed by its size alone: a tool's own config never is that large.
const MAX_HASHED_BYTES: u64 = 256 * 1024;

/// Manifests that tools keep their own section in (`[tool.x]`, a `"prettier"` key, `[flake8]`).
const SHARED_MANIFESTS: &[&str] = &[
    "pyproject.toml",
    "setup.cfg",
    "tox.ini",
    "package.json",
    "deno.json",
    "deno.jsonc",
    "composer.json",
];

/// Dotfiles that configure git or the desktop, not a tool poly runs. `.gitignore` is not among them: `prettier`
/// declines a file it matches.
const NOT_CONFIG: &[&str] = &[
    ".git",
    ".DS_Store",
    ".gitattributes",
    ".gitmodules",
    ".gitkeep",
    ".keep",
];

/// Whether a file named `name` may be a tool's config: a dotfile (`.yamllint`, `.prettierrc.json`), an `*rc` file, a
/// `<tool>.config.<ext>` file, or a shared manifest.
fn is_config_like(name: &str) -> bool {
    if NOT_CONFIG.contains(&name) {
        return false;
    }
    name.starts_with('.') || name.ends_with("rc") || name.contains(".config.") || SHARED_MANIFESTS.contains(&name)
}

/// The salts computed under one run root for one tool working directory, by the directory of the file.
struct Scope {
    run_root: PathBuf,
    anchor: Option<PathBuf>,
    by_dir: HashMap<PathBuf, Arc<str>>,
}

#[derive(Default)]
struct Memo {
    scopes: Vec<Scope>,
    dirs: HashMap<PathBuf, Arc<str>>,
}

fn memo() -> &'static RwLock<Memo> {
    static MEMO: OnceLock<RwLock<Memo>> = OnceLock::new();
    MEMO.get_or_init(RwLock::default)
}

/// Forget every salt, so a long-lived host re-reads config edited since the last run.
pub(super) fn forget() {
    let mut memo = memo().write().unwrap_or_else(PoisonError::into_inner);
    memo.scopes.clear();
    memo.dirs.clear();
}

/// The cache input for a plain catalog tool linting or formatting `path` from the working directory `anchor`.
pub(super) fn salt(path: &Path, anchor: Option<&Path>) -> String {
    // The tool runs in `anchor` resolved under the run root (ADR 0073), else in the run root itself.
    let Ok(run_root) = poly_engine_api::run_root::current_dir() else {
        return "unanchored".to_owned();
    };
    let dir = path.parent().unwrap_or(path);
    {
        let memo = memo().read().unwrap_or_else(PoisonError::into_inner);
        let known = memo
            .scopes
            .iter()
            .find(|scope| scope.run_root == run_root && scope.anchor.as_deref() == anchor)
            .and_then(|scope| scope.by_dir.get(dir));
        if let Some(known) = known {
            return known.to_string();
        }
    }
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"file\0");
    walk(&mut hasher, &run_root.join(dir), &run_root);
    hasher.update(b"\0cwd\0");
    walk(
        &mut hasher,
        &anchor.map_or_else(|| run_root.clone(), |anchor| run_root.join(anchor)),
        &run_root,
    );
    let salt: Arc<str> = Arc::from(hasher.finalize().to_hex().as_str());
    let mut memo = memo().write().unwrap_or_else(PoisonError::into_inner);
    let at = memo
        .scopes
        .iter()
        .position(|scope| scope.run_root == run_root && scope.anchor.as_deref() == anchor)
        .unwrap_or_else(|| {
            memo.scopes.push(Scope {
                run_root,
                anchor: anchor.map(Path::to_path_buf),
                by_dir: HashMap::new(),
            });
            memo.scopes.len() - 1
        });
    memo.scopes[at].by_dir.insert(dir.to_path_buf(), Arc::clone(&salt));
    salt.to_string()
}

/// The last directory a walk from `start` reads: the nearest ancestor holding `.git`; without one, `run_root` when
/// `start` is under it, else `start` itself.
fn limit<'a>(start: &'a Path, run_root: &'a Path) -> &'a Path {
    start
        .ancestors()
        .find(|dir| dir.join(".git").exists())
        .unwrap_or(if start.starts_with(run_root) { run_root } else { start })
}

/// Fold in the config files of `start` and its ancestors up to [`limit`].
fn walk(hasher: &mut blake3::Hasher, start: &Path, run_root: &Path) {
    let start = std::path::absolute(start).unwrap_or_else(|_| start.to_path_buf());
    let last = limit(&start, run_root);
    for (depth, dir) in start.ancestors().enumerate() {
        hasher.update(&(depth as u64).to_le_bytes());
        hasher.update(dir_digest(dir).as_bytes());
        if dir == last {
            break;
        }
    }
}

fn dir_digest(dir: &Path) -> Arc<str> {
    if let Some(known) = memo().read().unwrap_or_else(PoisonError::into_inner).dirs.get(dir) {
        return Arc::clone(known);
    }
    let digest: Arc<str> = Arc::from(compute_dir_digest(dir));
    memo()
        .write()
        .unwrap_or_else(PoisonError::into_inner)
        .dirs
        .entry(dir.to_path_buf())
        .or_insert(digest)
        .clone()
}

fn compute_dir_digest(dir: &Path) -> String {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return String::new();
    };
    let mut files: Vec<(String, PathBuf)> = entries
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_ok_and(|kind| !kind.is_dir()))
        .filter_map(|entry| {
            let name = entry.file_name().into_string().ok()?;
            is_config_like(&name).then(|| (name, entry.path()))
        })
        .collect();
    files.sort();
    let mut hasher = blake3::Hasher::new();
    for (name, path) in &files {
        hasher.update(name.as_bytes());
        hasher.update(b"\0");
        match std::fs::metadata(path) {
            Ok(metadata) if metadata.len() > MAX_HASHED_BYTES => {
                hasher.update(b"size:");
                hasher.update(&metadata.len().to_le_bytes());
            }
            _ => {
                let bytes = std::fs::read(path).unwrap_or_else(|_| b"<unreadable>".to_vec());
                hasher.update(&bytes);
            }
        }
        hasher.update(b"\0");
    }
    hasher.finalize().to_hex().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_like_names() {
        for name in [
            ".yamllint",
            ".yamllint.yaml",
            ".prettierrc",
            "prettier.config.js",
            "pyproject.toml",
            "setup.cfg",
            "package.json",
            ".gitignore",
        ] {
            assert!(is_config_like(name), "{name}");
        }
        for name in [
            "main.py",
            "README.md",
            ".git",
            "Makefile",
            "values.yaml",
            "data.json",
            "Cargo.toml",
            "poly.toml",
        ] {
            assert!(!is_config_like(name), "{name}");
        }
    }

    /// A config edit in the file's directory or an ancestor changes the salt; the same tree elsewhere keys alike.
    #[test]
    fn the_salt_follows_config_files_and_not_the_checkout_location() {
        let tree = |yamllint: &str| {
            let dir = tempfile::tempdir().expect("tempdir");
            std::fs::create_dir(dir.path().join(".git")).expect("git");
            std::fs::create_dir(dir.path().join("sub")).expect("sub");
            std::fs::write(dir.path().join(".yamllint"), yamllint).expect("config");
            std::fs::write(dir.path().join("sub/c.yaml"), "a: 1\n").expect("file");
            let salt = salt(&dir.path().join("sub/c.yaml"), Some(dir.path()));
            (dir, salt)
        };
        let (_one, first) = tree("extends: default\n");
        let (_two, same) = tree("extends: default\n");
        let (_three, edited) = tree("extends: relaxed\n");
        assert_eq!(first, same);
        assert_ne!(first, edited);
    }

    /// The files a tool lints are not inputs of one another: only a file named like a config is.
    #[test]
    fn a_linted_sibling_and_an_unrelated_manifest_are_not_inputs() {
        let tree = |sibling: &str, manifest: &str| {
            let dir = tempfile::tempdir().expect("tempdir");
            std::fs::create_dir(dir.path().join(".git")).expect("git");
            std::fs::write(dir.path().join("a.yaml"), "a: 1\n").expect("a");
            std::fs::write(dir.path().join("b.yaml"), sibling).expect("b");
            std::fs::write(dir.path().join("Cargo.toml"), manifest).expect("manifest");
            salt(&dir.path().join("a.yaml"), Some(dir.path()))
        };
        assert_eq!(tree("b: 1\n", "[package]\n"), tree("b: 2\n", "[workspace]\n"));
    }

    /// Outside a checkout the walk stops at the run root, and a file outside both is keyed by its own directory.
    #[test]
    fn a_walk_without_git_does_not_climb_past_the_run_root() {
        let dir = tempfile::tempdir().expect("tempdir");
        let run_root = dir.path().join("run");
        let nested = run_root.join("a/b");
        let elsewhere = dir.path().join("elsewhere/x");
        std::fs::create_dir_all(&nested).expect("nested");
        std::fs::create_dir_all(&elsewhere).expect("elsewhere");
        assert_eq!(limit(&nested, &run_root), run_root);
        assert_eq!(limit(&elsewhere, &run_root), elsewhere);
        std::fs::create_dir(dir.path().join(".git")).expect("git");
        assert_eq!(limit(&nested, &run_root), dir.path());
        assert_eq!(limit(&elsewhere, &run_root), dir.path());
    }
}
