//! Memos scoped to one run.
//!
//! A recipe tool's answers depend on files that change between runs, so a
//! long-lived host (`poly mcp`) must not replay them: every memo is emptied at
//! the start of a run ([`forget_run_memos`]), and is bounded so a run that
//! touches many roots (each mirrored run has a fresh temp root) cannot grow it
//! without limit.

use std::collections::HashMap;
use std::hash::Hash;
use std::sync::{Mutex, MutexGuard, OnceLock};

/// Entries a memo holds before it starts over.
const MAX_ENTRIES: usize = 4096;

pub(super) struct Memo<K, V> {
    entries: OnceLock<Mutex<HashMap<K, V>>>,
}

impl<K: Hash + Eq, V: Clone> Memo<K, V> {
    pub(super) const fn new() -> Self {
        Memo {
            entries: OnceLock::new(),
        }
    }

    fn lock(&self) -> MutexGuard<'_, HashMap<K, V>> {
        self.entries
            .get_or_init(Default::default)
            .lock()
            .expect("recipe memo poisoned")
    }

    pub(super) fn get(&self, key: &K) -> Option<V> {
        self.lock().get(key).cloned()
    }

    pub(super) fn insert(&self, key: K, value: V) {
        let mut entries = self.lock();
        if entries.len() >= MAX_ENTRIES {
            entries.clear();
        }
        entries.insert(key, value);
    }

    /// The value for `key`, made by `make` (outside the lock) when absent.
    pub(super) fn get_or_insert_with(&self, key: K, make: impl FnOnce() -> V) -> V {
        if let Some(known) = self.get(&key) {
            return known;
        }
        let value = make();
        self.insert(key, value.clone());
        value
    }

    pub(super) fn clear(&self) {
        self.lock().clear();
    }

    #[cfg(test)]
    pub(super) fn len(&self) -> usize {
        self.lock().len()
    }
}

/// Empty every recipe memo; called when a run is planned.
pub(super) fn forget_run_memos() {
    super::lint::INVOCATIONS.clear();
    super::unit::PREPARED.clear();
    super::paths::ROOTS.clear();
    super::salt::DIGESTS.clear();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clearing_forgets_answers_and_the_next_ask_recomputes() {
        let memo: Memo<String, u32> = Memo::new();
        assert_eq!(memo.get_or_insert_with("k".to_owned(), || 1), 1);
        assert_eq!(memo.get_or_insert_with("k".to_owned(), || 2), 1);
        memo.clear();
        assert_eq!(memo.get_or_insert_with("k".to_owned(), || 2), 2);
    }

    #[test]
    fn a_memo_never_outgrows_its_bound() {
        let memo: Memo<usize, usize> = Memo::new();
        for key in 0..MAX_ENTRIES * 2 + 3 {
            memo.insert(key, key);
        }
        assert!(memo.len() <= MAX_ENTRIES);
    }

    #[test]
    fn the_path_and_digest_memos_are_emptied_by_clear() {
        let root = std::path::Path::new("/memo-test-root");
        let key = (root.to_path_buf(), vec!["**".to_owned()]);
        super::super::salt::DIGESTS.insert(key.clone(), "old".to_owned());
        super::super::paths::ROOTS.insert(key.clone(), None);
        super::super::salt::DIGESTS.clear();
        super::super::paths::ROOTS.clear();
        assert_eq!(super::super::salt::DIGESTS.get(&key), None);
        assert_eq!(super::super::paths::ROOTS.get(&key), None);
    }

    #[test]
    fn a_digest_is_replayed_within_a_run_and_recomputed_after_the_memo_is_cleared() {
        let dir = tempfile::tempdir().expect("tempdir");
        let file = dir.path().join("a.proto");
        std::fs::write(&file, "one").expect("write");
        let globs = ["*.proto".to_owned()];
        let first = super::super::salt::dependency_digest(dir.path(), &globs);
        std::fs::write(&file, "two").expect("write");
        assert_eq!(super::super::salt::dependency_digest(dir.path(), &globs), first);
        super::super::salt::DIGESTS.clear();
        assert_ne!(super::super::salt::dependency_digest(dir.path(), &globs), first);
    }
}
