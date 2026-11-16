//! The files outside a file that jarl's verdict on it depends on, for the cache key.
//!
//! Some of jarl's rules look beyond the file: `unused_function` and `duplicated_function_definition` read every R
//! file of the package (and the C and C++ sources under `src/`), the package's `DESCRIPTION` fixes the R version it
//! may rely on, `NAMESPACE` says what it imports, and `source("other.R")` makes another script part of the file's
//! own world. A cached verdict is only as current as those inputs, so the cache key folds in a digest of them.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

/// Directories no digest enters: version control, vendored libraries and build output.
const SKIPPED_DIRECTORIES: &[&str] = &[".git", "renv", "revdep", "node_modules", "target", "docs"];

/// Extensions of the files jarl reads besides `DESCRIPTION` and `NAMESPACE`.
const SOURCE_EXTENSIONS: &[&str] = &["r", "c", "cc", "cpp", "h", "hpp"];

/// How long a tree's digest is trusted. A run digests each package once instead of once per file; a server that
/// outlives the run recomputes it after this long.
const MEMO_LIFETIME: Duration = Duration::from_secs(2);

/// The nearest ancestor directory of `file` that holds a `DESCRIPTION`: the root of the R package it belongs to.
fn package_root(file: &Path) -> Option<PathBuf> {
    file.ancestors()
        .skip(1)
        .find(|directory| directory.join("DESCRIPTION").is_file())
        .map(Path::to_path_buf)
}

/// The digest of what lies outside `file` that its verdict depends on; `None` when nothing does.
///
/// A file in a package depends on the whole package. A script depends on its neighbours only when it calls
/// `source(`, in which case the directory it sits in (and below) stands for them.
pub(crate) fn salt(file: &Path, content: &str) -> Option<String> {
    let file = crate::jarl_config::absolute(file);
    if let Some(root) = package_root(&file) {
        return Some(tree_digest(&root));
    }
    if content.contains("source(") {
        return Some(tree_digest(file.parent()?));
    }
    None
}

/// A digest of the R and native sources and the package metadata under `root`, memoized for [`MEMO_LIFETIME`].
fn tree_digest(root: &Path) -> String {
    static MEMO: OnceLock<Mutex<HashMap<PathBuf, (Instant, String)>>> = OnceLock::new();
    let mut memo = MEMO
        .get_or_init(Mutex::default)
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some((computed, digest)) = memo.get(root)
        && computed.elapsed() < MEMO_LIFETIME
    {
        return digest.clone();
    }
    let mut hasher = blake3::Hasher::new();
    hash_tree(root, root, &mut hasher);
    let digest = hasher.finalize().to_hex().to_string();
    memo.insert(root.to_path_buf(), (Instant::now(), digest.clone()));
    digest
}

fn is_read_by_jarl(path: &Path) -> bool {
    let name = path.file_name().and_then(|name| name.to_str()).unwrap_or_default();
    if name == "DESCRIPTION" || name == "NAMESPACE" {
        return true;
    }
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| SOURCE_EXTENSIONS.contains(&extension.to_ascii_lowercase().as_str()))
}

/// Feed the relevant files below `directory` to `hasher` in name order, so the digest is stable.
fn hash_tree(root: &Path, directory: &Path, hasher: &mut blake3::Hasher) {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return;
    };
    let mut paths: Vec<PathBuf> = entries
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .collect();
    paths.sort();
    for path in paths {
        if path.is_dir() {
            let skipped = path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| SKIPPED_DIRECTORIES.contains(&name));
            if !skipped {
                hash_tree(root, &path, hasher);
            }
        } else if is_read_by_jarl(&path)
            && let Ok(bytes) = std::fs::read(&path)
        {
            hasher.update(path.strip_prefix(root).unwrap_or(&path).to_string_lossy().as_bytes());
            hasher.update(&[0]);
            hasher.update(&bytes);
            hasher.update(&[0]);
        }
    }
}
