//! The in-memory inventory of result files that lets a lookup reject a missing key without touching
//! the filesystem.

use std::collections::HashSet;
use std::path::Path;
use std::sync::RwLock;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::{CacheKey, Namespace};

/// Open-time inventory of result files, plus entries written by this process.
///
/// Missing keys are rejected in memory instead of issuing a failing filesystem
/// read for every file and engine. The inventory is deliberately a snapshot:
/// writes from another process become visible on the next cache open. A stale
/// positive is harmless because [`ResultCache::get`] still reads the file and
/// treats deletion or corruption as a miss.
#[derive(Debug)]
pub(crate) struct PresenceIndex {
    snapshot: [HashSet<String>; 3],
    added: [RwLock<HashSet<String>>; 3],
    any_added: AtomicBool,
}

impl PresenceIndex {
    pub(crate) fn empty() -> Self {
        Self {
            snapshot: std::array::from_fn(|_| HashSet::new()),
            added: std::array::from_fn(|_| RwLock::new(HashSet::new())),
            any_added: AtomicBool::new(false),
        }
    }

    pub(crate) fn scan(root: &Path) -> Self {
        let mut index = Self::empty();
        for namespace in Namespace::ALL {
            let directory = root.join("results").join(namespace.as_dir());
            let Ok(entries) = std::fs::read_dir(directory) else {
                continue;
            };
            let present = &mut index.snapshot[namespace.slot()];
            for entry in entries.flatten() {
                let Ok(file_type) = entry.file_type() else {
                    continue;
                };
                let name = entry.file_name();
                let Some(name) = name.to_str() else {
                    continue;
                };
                if file_type.is_file() && !name.starts_with('.') {
                    present.insert(name.to_owned());
                }
            }
        }
        index
    }

    pub(crate) fn contains(&self, namespace: Namespace, key: &CacheKey) -> bool {
        if self.snapshot[namespace.slot()].contains(key.as_str()) {
            return true;
        }
        if !self.any_added.load(Ordering::Acquire) {
            return false;
        }
        self.added[namespace.slot()]
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .contains(key.as_str())
    }

    pub(crate) fn record(&self, namespace: Namespace, key: &CacheKey) {
        self.added[namespace.slot()]
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(key.as_str().to_owned());
        self.any_added.store(true, Ordering::Release);
    }
}
