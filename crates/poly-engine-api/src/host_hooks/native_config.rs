//! Finding the nearest native tool config for a file, shared by the Lua hooks.
//!
//! StyLua and selene each read a config file next to the code (`stylua.toml`, `selene.toml`). Both are
//! resolved by poly rather than by the engine, so the file that governs a path is the same one the cache
//! key was salted with, whichever process serves the engine. The search starts in the file's directory
//! and climbs to the repository root (the first directory holding `.git`), nearest file wins.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock, RwLock};

use super::file_bytes;

/// A native config file as found on disk.
#[derive(Debug)]
pub struct Found {
    /// The config file.
    pub path: PathBuf,
    /// The file's contents parsed as TOML; empty when it did not parse (see [`Found::parse_error`]).
    pub table: toml::Table,
    /// Why the file did not parse, when it did not.
    pub parse_error: Option<String>,
    /// A digest of the file's bytes, for cache keys.
    pub digest: String,
}

type Memo = HashMap<(&'static str, PathBuf), Option<Arc<Found>>>;

fn memo() -> &'static RwLock<Memo> {
    static MEMO: OnceLock<RwLock<Memo>> = OnceLock::new();
    MEMO.get_or_init(RwLock::default)
}

/// Drops every memoised lookup so the next run rereads configs from disk.
pub fn forget() {
    memo()
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clear();
}

fn load(path: PathBuf) -> Arc<Found> {
    let bytes = file_bytes::for_digest(&path);
    let digest = blake3::hash(&bytes).to_hex().to_string();
    let (table, parse_error) = match std::str::from_utf8(&bytes)
        .map_err(|error| error.to_string())
        .and_then(|text| text.parse::<toml::Table>().map_err(|error| error.to_string()))
    {
        Ok(table) => (table, None),
        Err(error) => (toml::Table::new(), Some(error)),
    };
    Arc::new(Found {
        path,
        table,
        parse_error,
        digest,
    })
}

/// The nearest of `names` (tried in order within one directory) at or above `path`'s directory.
///
/// `tool` keys the memo so two tools searching the same directory do not share an answer.
#[must_use]
pub fn nearest(tool: &'static str, names: &[&str], path: &Path) -> Option<Arc<Found>> {
    let start = path.parent().unwrap_or(path);
    let key = (tool, start.to_path_buf());
    if let Some(hit) = memo()
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get(&key)
    {
        return hit.clone();
    }
    let mut found = None;
    for dir in start.ancestors() {
        if let Some(candidate) = names
            .iter()
            .map(|name| dir.join(name))
            .find(|candidate| candidate.is_file())
        {
            found = Some(load(candidate));
            break;
        }
        if dir.join(".git").exists() {
            break;
        }
    }
    memo()
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .insert(key, found.clone());
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_nearest_file_wins_and_the_repository_root_ends_the_search() {
        let repo = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(repo.path().join(".git")).unwrap();
        std::fs::create_dir_all(repo.path().join("a/b")).unwrap();
        std::fs::write(repo.path().join("t.toml"), "x = 1\n").unwrap();
        std::fs::write(repo.path().join("a/t.toml"), "x = 2\n").unwrap();
        forget();
        let near = nearest("test", &["t.toml"], &repo.path().join("a/b/f.lua")).unwrap();
        assert_eq!(near.table["x"].as_integer(), Some(2));
        let far = nearest("test", &["t.toml"], &repo.path().join("f.lua")).unwrap();
        assert_eq!(far.table["x"].as_integer(), Some(1));
        assert_ne!(near.digest, far.digest);
        assert!(nearest("test", &["missing.toml"], &repo.path().join("a/f.lua")).is_none());
    }

    #[test]
    fn a_file_that_does_not_parse_is_found_with_its_error() {
        let repo = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(repo.path().join(".git")).unwrap();
        std::fs::write(repo.path().join("t.toml"), "x = = 1\n").unwrap();
        forget();
        let found = nearest("test-bad", &["t.toml"], &repo.path().join("f.lua")).unwrap();
        assert!(found.parse_error.is_some());
        assert!(found.table.is_empty());
    }
}
