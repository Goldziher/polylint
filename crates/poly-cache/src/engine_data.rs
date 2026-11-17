//! The store an engine caches its own work in, below the granularity of a poly result (ADR 0080).
//!
//! A project check is cached as a whole: one edit anywhere in a JavaScript workspace re-runs skif over all of it. skif
//! can keep the part of that work an edit leaves valid (each module's parse) when the host gives it a store, and this is
//! that store. Its entries live in the result cache's [`Namespace::Engine`] directory, so `poly cache stats`, `clean`,
//! `gc` and the automatic sweep cover them like any other entry, and it is the same per-repository directory
//! `[cache] dir` and `POLY_CACHE_HOME` move.
//!
//! The engine computes its keys and checks every value it reads back (skif seals each one); the store only moves bytes.
//! Each key is folded with [`CACHE_FORMAT_VERSION`](crate::CACHE_FORMAT_VERSION) and the build identity of the binary
//! that runs the engine, the preamble of every result-cache key, so another layout or another build never reads an
//! entry. The host's own identity reaches the engine as a salt ([`engine_data_salt`]).

use std::fs::{File, OpenOptions};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::time::{Duration, SystemTime};

use crate::{CacheKey, InputDigest, Namespace, ResultCache, SerializedArgs, TMP_COUNTER};

/// The salt a host hands an engine for its keys: the cache format version and the host's build identity, so entries
/// written under another layout or by another build of the host are misses.
#[must_use]
pub fn engine_data_salt() -> String {
    format!("{}\0{}", crate::CACHE_FORMAT_VERSION, crate::build_identity())
}

/// How old an entry's mtime must be before a hit refreshes it. Well inside the sweep's age limit, and coarse enough
/// that a run reads most entries without writing any metadata.
const REFRESH_AFTER: Duration = Duration::from_secs(24 * 60 * 60);

/// Moves the mtime of the entry at `path` to now. A failure only leaves the entry to age out sooner.
fn refresh(path: &Path) {
    // Reopened for writing, since setting a file's times needs write access on Windows; nothing is written.
    let result = OpenOptions::new()
        .write(true)
        .open(path)
        .and_then(|file| file.set_modified(SystemTime::now()));
    if let Err(error) = result {
        tracing::debug!("engine cache entry {} not refreshed: {error}", path.display());
    }
}

/// An engine's view of the result cache's `results/engine/` directory.
#[derive(Debug, Clone)]
pub struct EngineDataStore {
    dir: PathBuf,
}

impl EngineDataStore {
    /// The store of the result cache rooted at `root` ([`ResultCache::root`]), creating its directory owner-only.
    ///
    /// # Errors
    ///
    /// When `root` is not absolute (a store must not depend on the working directory of the process that opens it) or
    /// the directory cannot be created.
    pub fn open(root: &Path) -> anyhow::Result<Self> {
        anyhow::ensure!(
            root.is_absolute(),
            "the engine cache root `{}` is not absolute",
            root.display()
        );
        let dir = root.join("results").join(Namespace::Engine.as_dir());
        crate::permissions::create_dir_all_private(&dir)
            .map_err(|e| anyhow::anyhow!("failed to create cache dir {}: {e}", dir.display()))?;
        Ok(Self { dir })
    }

    /// The result-cache key of the engine key `key` in the engine's namespace `id` (`skif.modules`).
    #[must_use]
    pub fn key(id: &str, key: &[u8; 32]) -> CacheKey {
        let digest = InputDigest(blake3::Hash::from_bytes(*key).to_hex().to_string());
        ResultCache::key_with_args(Namespace::Engine, id, "", &SerializedArgs(String::new()), &digest)
    }

    /// The bytes stored under `key` in `id`, or `None` when there are none or they cannot be read.
    ///
    /// A hit on an entry last written or refreshed more than a day ago moves its mtime to now, best effort: the sweep evicts by mtime, and an unchanged module's entry is read on every run but never rewritten,
    /// so without it the entries in steady use would be the first to age out.
    #[must_use]
    pub fn get(&self, id: &str, key: &[u8; 32]) -> Option<Vec<u8>> {
        let path = self.dir.join(Self::key(id, key).as_str());
        let mut file = File::open(&path).ok()?;
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes).ok()?;
        let stale = file
            .metadata()
            .and_then(|meta| meta.modified())
            .is_ok_and(|modified| modified.elapsed().is_ok_and(|age| age >= REFRESH_AFTER));
        if stale {
            refresh(&path);
        }
        Some(bytes)
    }

    /// Stores `bytes` under `key` in `id` with an atomic sibling-tmp + rename, so a concurrent reader sees the old
    /// entry, the new one or none, never a torn one.
    ///
    /// # Errors
    ///
    /// When the entry cannot be written.
    pub fn put(&self, id: &str, key: &[u8; 32], bytes: &[u8]) -> anyhow::Result<()> {
        let key = Self::key(id, key);
        let dest = self.dir.join(key.as_str());
        let n = TMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        let tmp = self
            .dir
            .join(format!(".{}.{}.{}.tmp", key.as_str(), std::process::id(), n));
        std::fs::write(&tmp, bytes).map_err(|e| anyhow::anyhow!("cache write {}: {e}", tmp.display()))?;
        if let Err(error) = std::fs::rename(&tmp, &dest) {
            let _ = std::fs::remove_file(&tmp);
            anyhow::bail!("cache rename to {}: {error}", dest.display());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_entry_is_read_back_from_the_engine_namespace_and_counted_by_stats_and_clean() {
        let dir = tempfile::tempdir().unwrap();
        let cache = ResultCache::open(dir.path().to_path_buf(), true).unwrap();
        let store = EngineDataStore::open(cache.root()).unwrap();
        let key = [7u8; 32];
        assert!(store.get("skif.modules", &key).is_none());
        store.put("skif.modules", &key, b"sealed").unwrap();
        assert_eq!(store.get("skif.modules", &key).as_deref(), Some(&b"sealed"[..]));
        assert!(store.get("skif.other", &key).is_none(), "the id is part of the key");
        assert!(store.get("skif.modules", &[8u8; 32]).is_none());

        let stats = cache.stats().unwrap();
        let engine = stats
            .per_namespace
            .iter()
            .find(|entry| entry.namespace == Namespace::Engine)
            .expect("the engine namespace is reported");
        assert_eq!((engine.entries, engine.bytes), (1, 6));
        assert_eq!(cache.clean().unwrap(), 6);
        assert!(store.get("skif.modules", &key).is_none(), "clean removes it");
    }

    #[test]
    fn a_hit_refreshes_the_mtime_of_a_stale_entry_so_the_age_sweep_keeps_it() {
        use std::fs::FileTimes;

        let dir = tempfile::tempdir().unwrap();
        let cache = ResultCache::open(dir.path().to_path_buf(), true).unwrap();
        let store = EngineDataStore::open(cache.root()).unwrap();
        let (hot, cold) = ([1u8; 32], [2u8; 32]);
        store.put("skif.modules", &hot, b"hot").unwrap();
        store.put("skif.modules", &cold, b"cold").unwrap();
        let month_ago = SystemTime::now() - Duration::from_secs(30 * 86_400);
        for key in [&hot, &cold] {
            std::fs::OpenOptions::new()
                .write(true)
                .open(store.dir.join(EngineDataStore::key("skif.modules", key).as_str()))
                .unwrap()
                .set_times(FileTimes::new().set_modified(month_ago))
                .unwrap();
        }

        assert_eq!(store.get("skif.modules", &hot).as_deref(), Some(&b"hot"[..]));
        assert_eq!(
            cache.gc(Some(Duration::from_secs(14 * 86_400)), None).unwrap(),
            4,
            "only the entry nothing read is evicted"
        );
        assert_eq!(store.get("skif.modules", &hot).as_deref(), Some(&b"hot"[..]));
        assert!(store.get("skif.modules", &cold).is_none());
    }

    #[test]
    fn a_relative_root_is_refused() {
        assert!(EngineDataStore::open(Path::new("relative/cache")).is_err());
    }

    #[test]
    fn the_salt_names_the_layout_and_the_build() {
        let salt = engine_data_salt();
        assert!(
            salt.starts_with(&format!("{}\0", crate::CACHE_FORMAT_VERSION)),
            "{salt:?}"
        );
        assert!(salt.ends_with(crate::build_identity()), "{salt:?}");
    }
}
