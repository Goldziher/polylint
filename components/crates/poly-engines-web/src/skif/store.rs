//! skif's cache store over poly's result cache (ADR 0080).
//!
//! skif computes every key and seals every value; this store only moves bytes into and out of the result cache's
//! `results/engine/` directory, under an id per skif namespace and schema. A lookup that fails is a miss and a write
//! that fails is logged and dropped: a store may lose entries without changing a report.

use std::sync::Arc;

use poly_cache::EngineDataStore;
use poly_engine_api::engine::EngineCache;
use skif::cache::{CacheNamespace, CacheStore};

/// A [`CacheStore`] backed by the host's result cache.
pub(super) struct PolyStore {
    store: EngineDataStore,
}

impl PolyStore {
    /// The store in `cache`, or `None` (logged) when its directory cannot be opened: the run then caches nothing.
    pub(super) fn open(cache: &EngineCache) -> Option<Self> {
        match EngineDataStore::open(&cache.dir) {
            Ok(store) => Some(Self { store }),
            Err(error) => {
                tracing::debug!("skif: running without a cache store: {error:#}");
                None
            }
        }
    }
}

/// The result-cache id of a skif namespace: its name and schema, so a schema bump never reads an old entry.
fn id(namespace: CacheNamespace) -> String {
    format!("skif.{}.{}", namespace.name, namespace.schema)
}

impl CacheStore for PolyStore {
    fn get(&self, namespace: CacheNamespace, key: &[u8; 32]) -> Option<Arc<[u8]>> {
        self.store.get(&id(namespace), key).map(Arc::from)
    }

    fn put(&self, namespace: CacheNamespace, key: &[u8; 32], value: Arc<[u8]>) {
        if let Err(error) = self.store.put(&id(namespace), key, &value) {
            tracing::debug!("skif: cache entry not stored: {error:#}");
        }
    }
}

/// Engine options that change how a run executes, never what it finds: left out of the salt, so `-j` or a `threads`
/// change still reads every entry. `enabled` is always `true` when a store is used at all.
const OPERATIONAL: &[&str] = &["enabled", "threads"];

/// The salt of every key a run writes: the host's (cache format version and build identity), this backend's
/// [`VERSION`](super::VERSION) and the engine options that can change a result, so a different skif, mapping or
/// configuration never reads an entry another wrote.
pub(super) fn salt(cache: &EngineCache, options: &toml::Table) -> Vec<u8> {
    let mut options = options.clone();
    options.retain(|key, _| !OPERATIONAL.contains(&key));
    format!("{}\0{}\0{options}", cache.salt, super::VERSION).into_bytes()
}
