//! The config a plain (recipe-less) catalog tool may read, as a per-file cache input.
//!
//! A recipe names the config files its tool reads (`Recipe::config_files`, hashed by `recipe::salt`). A plain
//! catalog tool comes from the mdsf catalog, which names none, and these tools read their own config from disk:
//! `yamllint` finds `.yamllint` in its working directory, a path-mode tool such as `prettier` searches upward from the
//! file. Without that config in the key, editing it served the previous verdict.
//!
//! Lacking a list of names, the salt takes every *config-like* file (see [`is_config_like`]) in the directories a tool
//! can search: the file's own directory and its ancestors, and the tool's working directory and its ancestors, each
//! walk ending at the first directory holding `.git`. That over-invalidates (an edit to `package.json` re-runs
//! `yamllint`), which is the safe direction for an opt-in tier, and it is cheap: one directory listing and the read of
//! its small config-like files per directory, memoised per directory for the run.
//!
//! Files are named relative to where the walk starts (`../.yamllint`), never by absolute path, so two checkouts of a
//! repository key alike.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock, PoisonError, RwLock};

/// A config-like file larger than this is keyed by its size alone: lockfiles and generated JSON can be large, and a
/// tool's own config never is.
const MAX_HASHED_BYTES: u64 = 256 * 1024;

/// Extensions of files that are plausibly a tool's config.
const CONFIG_EXTENSIONS: &[&str] = &[
    "toml",
    "yaml",
    "yml",
    "json",
    "jsonc",
    "json5",
    "cfg",
    "ini",
    "conf",
    "config",
    "xml",
    "properties",
];

/// Whether a file named `name` may be a tool's config: a dotfile, a `*rc` file, or a config-format extension.
fn is_config_like(name: &str) -> bool {
    if name == ".git" || name == ".DS_Store" {
        return false;
    }
    name.starts_with('.')
        || name.ends_with("rc")
        || Path::new(name)
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| CONFIG_EXTENSIONS.contains(&extension))
}

type DirDigests = RwLock<HashMap<PathBuf, Arc<str>>>;

fn memo() -> &'static DirDigests {
    static DIGESTS: OnceLock<DirDigests> = OnceLock::new();
    DIGESTS.get_or_init(DirDigests::default)
}

/// Forget every directory digest, so a long-lived host re-reads config edited since the last run.
pub(super) fn forget() {
    memo().write().unwrap_or_else(PoisonError::into_inner).clear();
}

/// The cache input for a plain catalog tool linting or formatting `path` from the working directory `anchor`.
pub(super) fn salt(path: &Path, anchor: Option<&Path>) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"file\0");
    walk(&mut hasher, path.parent().unwrap_or(path));
    hasher.update(b"\0cwd\0");
    // The tool runs in `anchor` resolved under the run root (ADR 0073), else in the run root itself.
    match poly_engine_api::run_root::current_dir() {
        Ok(cwd) => walk(
            &mut hasher,
            &anchor.map_or_else(|| cwd.clone(), |anchor| cwd.join(anchor)),
        ),
        Err(_) => {
            hasher.update(b"unanchored");
        }
    }
    hasher.finalize().to_hex().to_string()
}

/// Fold in the config-like files of `start` and its ancestors, up to and including the first directory holding `.git`.
fn walk(hasher: &mut blake3::Hasher, start: &Path) {
    let start = std::path::absolute(start).unwrap_or_else(|_| start.to_path_buf());
    for (depth, dir) in start.ancestors().enumerate() {
        hasher.update(&(depth as u64).to_le_bytes());
        hasher.update(dir_digest(dir).as_bytes());
        if dir.join(".git").exists() {
            break;
        }
    }
}

fn dir_digest(dir: &Path) -> Arc<str> {
    if let Some(known) = memo().read().unwrap_or_else(PoisonError::into_inner).get(dir) {
        return Arc::clone(known);
    }
    let digest: Arc<str> = Arc::from(compute_dir_digest(dir));
    memo()
        .write()
        .unwrap_or_else(PoisonError::into_inner)
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
            ".prettierrc",
            "pyproject.toml",
            "setup.cfg",
            "package.json",
            "tslint.json",
        ] {
            assert!(is_config_like(name), "{name}");
        }
        for name in ["main.py", "README.md", ".git", "Makefile"] {
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
}
