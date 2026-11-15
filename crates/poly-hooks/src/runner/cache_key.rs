//! Tier-1 result-cache key derivation for a hook, plus the worktree-mutation
//! probe that decides whether a passing run may be stored.

use std::collections::HashMap;
use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use poly_cache::{CacheKey, InputDigest, Namespace, ResultCache};
use rayon::prelude::*;

use crate::filter::FilePattern;
use crate::git;
use crate::model::{Hook, HookCache, HookCommand};

use super::fixes;

/// The subset of `matched` whose worktree content differs from the index.
///
/// Compared byte-for-byte rather than via `git diff-files`: a stat-based "clean"
/// for a file that really did change would let a passing run be cached under a
/// key derived from content that never passed, and the next hit would replay
/// that pass. Anything unreadable or not a regular file counts as modified — the
/// conservative direction, which only ever suppresses a store.
pub(super) fn modified_matched(root: &Path, matched: &[PathBuf]) -> anyhow::Result<Vec<PathBuf>> {
    fixes::differing_from_staged(root, matched)
}

/// Read buffer for streaming a file into the hasher.
const HASH_BUFFER_BYTES: usize = 64 * 1024;

/// Salt folded into the version of every `DeclaredInputs` key, so entries made
/// under the previous whole-tree digest layout are unreachable.
const DECLARED_DIGEST_LAYOUT: &str = "declared-digest=v2";

/// Per-file content hash.
type ContentHash = [u8; 32];

/// The files a `DeclaredInputs` pattern selected, each with its content hash,
/// sorted by path, plus the digest folded from them.
pub(super) struct DeclaredSet {
    files: Vec<(String, ContentHash)>,
    digest: InputDigest,
}

/// Run-scoped memo of declared-input digests and the tracked-file listing.
///
/// Every cargo hook declares the same inputs, so without this each hook relisted
/// the tree and rehashed every file. One memo lives per priority group: a hook
/// that rewrites files in an earlier group cannot leave a later group keyed on
/// stale content.
type SetMap = HashMap<(String, PathBuf), Option<Arc<DeclaredSet>>>;

#[derive(Default)]
pub(super) struct DeclaredMemo {
    listings: Mutex<HashMap<PathBuf, Arc<Vec<PathBuf>>>>,
    sets: Mutex<SetMap>,
    list_calls: AtomicUsize,
    hash_passes: AtomicUsize,
}

impl DeclaredMemo {
    /// How many times `git ls-files` ran through this memo.
    #[cfg(test)]
    pub(super) fn list_calls(&self) -> usize {
        self.list_calls.load(Ordering::Relaxed)
    }

    /// How many full hashing passes ran through this memo.
    #[cfg(test)]
    pub(super) fn hash_passes(&self) -> usize {
        self.hash_passes.load(Ordering::Relaxed)
    }

    fn listing(&self, git_root: &Path) -> anyhow::Result<Arc<Vec<PathBuf>>> {
        let mut listings = self.listings.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(found) = listings.get(git_root) {
            return Ok(Arc::clone(found));
        }
        self.list_calls.fetch_add(1, Ordering::Relaxed);
        let listed = Arc::new(git::list_files(git_root)?);
        listings.insert(git_root.to_path_buf(), Arc::clone(&listed));
        Ok(listed)
    }

    fn selected(&self, git_root: &Path, pattern: &FilePattern) -> anyhow::Result<Vec<PathBuf>> {
        Ok(self
            .listing(git_root)?
            .iter()
            .filter(|path| pattern.is_match(path))
            .cloned()
            .collect())
    }

    /// The digest set for `pattern`, hashed once per `(pattern, content_root)`.
    /// The lock is held while hashing so concurrent hooks wait for the result
    /// rather than repeating the pass.
    fn declared(&self, git_root: &Path, content_root: &Path, pattern: &FilePattern) -> Option<Arc<DeclaredSet>> {
        let key = (format!("{pattern:?}"), content_root.to_path_buf());
        let mut sets = self.sets.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(found) = sets.get(&key) {
            return found.clone();
        }
        let computed = self.selected(git_root, pattern).ok().and_then(|paths| {
            self.hash_passes.fetch_add(1, Ordering::Relaxed);
            let files = hash_files(content_root, &paths)?;
            let digest = fold(&files);
            Some(Arc::new(DeclaredSet { files, digest }))
        });
        sets.insert(key, computed.clone());
        computed
    }

    fn peek(&self, content_root: &Path, pattern: &FilePattern) -> Option<Arc<DeclaredSet>> {
        let key = (format!("{pattern:?}"), content_root.to_path_buf());
        self.sets
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&key)
            .cloned()
            .flatten()
    }
}

/// The subset of the declared inputs whose worktree content changed since the
/// hook's key was derived, for the cache-store mutation guard.
///
/// When the key was derived from the worktree itself, the per-file hashes taken
/// then are compared with a fresh parallel pass; the index is not consulted.
/// Under a staged run the key came from the snapshot, so the worktree is
/// compared with the index as before, reusing the memoised file listing.
pub(super) fn declared_modified(
    memo: &DeclaredMemo,
    git_root: &Path,
    content_root: &Path,
    pattern: &FilePattern,
) -> anyhow::Result<Vec<PathBuf>> {
    if content_root == git_root {
        if let Some(before) = memo.peek(content_root, pattern) {
            return Ok(before
                .files
                .par_iter()
                .map_init(
                    || vec![0u8; HASH_BUFFER_BYTES],
                    |buffer, (path, hash)| {
                        (hash_path(&content_root.join(path), buffer) != Some(*hash)).then(|| PathBuf::from(path))
                    },
                )
                .flatten()
                .collect());
        }
    }
    let selected = memo.selected(git_root, pattern)?;
    modified_matched(git_root, &selected)
}

/// Derive the [`Namespace::Hook`] cache key for `hook`, or `None` when the hook
/// is not cacheable or its inputs cannot be read.
///
/// The key folds in the hook id, a command-identity `version`, the declared
/// environment (as the `args` table), and a content digest of the relevant
/// input files — so a changed command, env, or input invalidates the entry.
///
/// `git_root` (the real repository) resolves the input *file set* — via
/// `git ls-files` / the matched paths — while `content_root` supplies the
/// *bytes* that are digested. They differ for a workspace hook under isolation,
/// where the list is the tracked tree but the content is the staged snapshot.
pub(super) fn cache_key(
    memo: &DeclaredMemo,
    git_root: &Path,
    content_root: &Path,
    hook: &Hook,
    matched: &[PathBuf],
) -> Option<CacheKey> {
    let digest = match &hook.cache {
        HookCache::Disabled => return None,
        HookCache::MatchedFiles => matched_files_digest(content_root, matched)?,
        HookCache::DeclaredInputs(pattern) => memo.declared(git_root, content_root, pattern)?.digest.clone(),
    };
    let version = hook_version(hook);
    let args = hook_env_table(hook);
    Some(ResultCache::key(Namespace::Hook, &hook.id, &version, &args, &digest))
}

/// Digest the hook's matched files (each as `(relative_path, bytes)`), reading
/// bytes from `content_root`.
///
/// Returns `None` if any matched file cannot be read, which skips caching this
/// hook rather than risk a key derived from partial inputs.
fn matched_files_digest(content_root: &Path, matched: &[PathBuf]) -> Option<InputDigest> {
    let mut files: Vec<(String, Vec<u8>)> = Vec::new();
    for path in matched {
        let bytes = std::fs::read(content_root.join(path)).ok()?;
        files.push((path.to_string_lossy().into_owned(), bytes));
    }
    files.sort_by(|a, b| a.0.cmp(&b.0));
    Some(ResultCache::file_set_digest(
        files.iter().map(|(path, bytes)| (path.as_str(), bytes.as_slice())),
    ))
}

/// Stream `path` through blake3. `None` if it cannot be opened or read.
fn hash_path(path: &Path, buffer: &mut [u8]) -> Option<ContentHash> {
    let mut file = std::fs::File::open(path).ok()?;
    let mut hasher = blake3::Hasher::new();
    loop {
        let read = file.read(buffer).ok()?;
        if read == 0 {
            return Some(*hasher.finalize().as_bytes());
        }
        hasher.update(&buffer[..read]);
    }
}

/// Hash every path under `content_root` in parallel, keeping only the 32-byte
/// hashes, sorted by path for a deterministic digest. `None` if any read fails.
fn hash_files(content_root: &Path, paths: &[PathBuf]) -> Option<Vec<(String, ContentHash)>> {
    let mut files = paths
        .par_iter()
        .map_init(
            || vec![0u8; HASH_BUFFER_BYTES],
            |buffer, path| {
                let hash = hash_path(&content_root.join(path), buffer)?;
                Some((path.to_string_lossy().into_owned(), hash))
            },
        )
        .collect::<Option<Vec<_>>>()?;
    files.sort_by(|a, b| a.0.cmp(&b.0));
    Some(files)
}

/// Fold sorted per-file hashes into one [`InputDigest`].
fn fold(files: &[(String, ContentHash)]) -> InputDigest {
    ResultCache::file_set_digest(files.iter().map(|(path, hash)| (path.as_str(), hash.as_slice())))
}

/// A string capturing the hook's command identity, so a changed command, script
/// target, argument list, or file-passing mode invalidates the cache key.
fn hook_version(hook: &Hook) -> String {
    use std::fmt::Write as _;
    let mut version = String::new();
    match &hook.command {
        HookCommand::Run(line) => version.push_str(line),
        HookCommand::Script { path, runner } => {
            let _ = write!(version, "script\0{runner:?}\0{path}");
        }
    }
    version.push('\0');
    for (index, arg) in hook.args.iter().enumerate() {
        if index > 0 {
            version.push('\0');
        }
        version.push_str(arg);
    }
    let _ = write!(version, "\0pass_filenames={}", hook.pass_filenames);
    if matches!(hook.cache, HookCache::DeclaredInputs(_)) {
        let _ = write!(version, "\0{DECLARED_DIGEST_LAYOUT}");
    }
    version
}

/// The hook's declared environment as a TOML table, so an env change invalidates
/// the cache key. The `BTreeMap` is already ordered, giving a stable table.
fn hook_env_table(hook: &Hook) -> toml::Table {
    hook.env
        .iter()
        .map(|(key, value)| (key.clone(), toml::Value::String(value.clone())))
        .collect()
}

#[cfg(test)]
#[path = "cache_key_tests.rs"]
mod tests;
