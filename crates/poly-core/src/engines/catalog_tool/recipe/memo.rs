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
    /// Whether a value is final. Clearing and the size bound keep unsettled
    /// values: a cell another thread is still filling must stay the one every
    /// asker shares, or the work it guards would run twice.
    settled: fn(&V) -> bool,
}

fn always_settled<V>(_: &V) -> bool {
    true
}

impl<K: Hash + Eq, V: Clone> Memo<K, V> {
    /// A memo whose values are plain answers, always final.
    pub(super) const fn new() -> Self {
        Self::with_settled(always_settled)
    }

    /// A memo of shared cells; `settled` says whether a cell has been filled.
    pub(super) const fn with_settled(settled: fn(&V) -> bool) -> Self {
        Memo {
            entries: OnceLock::new(),
            settled,
        }
    }

    /// Recovers a poisoned lock: each operation leaves the map valid, so one engine's panic must not
    /// fail every later file.
    fn lock(&self) -> MutexGuard<'_, HashMap<K, V>> {
        self.entries
            .get_or_init(Default::default)
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub(super) fn get(&self, key: &K) -> Option<V> {
        self.lock().get(key).cloned()
    }

    pub(super) fn insert(&self, key: K, value: V) {
        let mut entries = self.lock();
        if entries.len() >= MAX_ENTRIES {
            entries.retain(|_, value| !(self.settled)(value));
        }
        entries.insert(key, value);
    }

    /// The value for `key`, made by `make` when absent. Lookup and insertion
    /// share one lock hold so concurrent askers all receive the same value;
    /// `make` therefore runs under the lock and must be cheap (build an empty
    /// cell, then fill it outside).
    pub(super) fn get_or_insert_with(&self, key: K, make: impl FnOnce() -> V) -> V {
        let mut entries = self.lock();
        if let Some(known) = entries.get(&key) {
            return known.clone();
        }
        if entries.len() >= MAX_ENTRIES {
            entries.retain(|_, value| !(self.settled)(value));
        }
        let value = make();
        entries.insert(key, value.clone());
        value
    }

    /// Forget every settled value.
    pub(super) fn clear(&self) {
        self.lock().retain(|_, value| !(self.settled)(value));
    }

    #[cfg(test)]
    pub(super) fn len(&self) -> usize {
        self.lock().len()
    }
}

/// Empty every recipe memo; called when a run is planned.
pub(in crate::engines::catalog_tool) fn forget_run_memos() {
    super::lint::INVOCATIONS.clear();
    super::unit::PREPARED.clear();
    super::paths::ROOTS.clear();
    super::salt::DIGESTS.clear();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_panic_while_the_memo_is_locked_does_not_fail_later_askers() {
        use std::sync::Arc;
        let memo: Arc<Memo<String, u32>> = Arc::new(Memo::new());
        memo.insert("a".to_owned(), 1);
        let poisoner = Arc::clone(&memo);
        let crashed = std::thread::spawn(move || {
            let _held = poisoner.lock();
            panic!("engine panic while the memo is held");
        })
        .join();
        assert!(crashed.is_err());
        // Every map operation leaves the table valid, so the entries survive the poison.
        assert_eq!(memo.get(&"a".to_owned()), Some(1));
        memo.insert("b".to_owned(), 2);
        assert_eq!(memo.get(&"b".to_owned()), Some(2));
    }

    #[test]
    fn clearing_forgets_answers_and_the_next_ask_recomputes() {
        let memo: Memo<String, u32> = Memo::new();
        assert_eq!(memo.get_or_insert_with("k".to_owned(), || 1), 1);
        assert_eq!(memo.get_or_insert_with("k".to_owned(), || 2), 1);
        memo.clear();
        assert_eq!(memo.get_or_insert_with("k".to_owned(), || 2), 2);
    }

    #[test]
    fn concurrent_askers_share_one_cell_and_the_work_runs_once() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicUsize, Ordering};
        type Cell = Arc<OnceLock<usize>>;
        fn filled(cell: &Cell) -> bool {
            cell.get().is_some()
        }
        let memo: Arc<Memo<String, Cell>> = Arc::new(Memo::with_settled(filled));
        let runs = Arc::new(AtomicUsize::new(0));
        let barrier = Arc::new(std::sync::Barrier::new(16));
        let workers: Vec<_> = (0..16)
            .map(|_| {
                let (memo, runs, barrier) = (memo.clone(), runs.clone(), barrier.clone());
                std::thread::spawn(move || {
                    barrier.wait();
                    let cell = memo.get_or_insert_with("k".to_owned(), Default::default);
                    *cell.get_or_init(|| {
                        std::thread::sleep(std::time::Duration::from_millis(50));
                        runs.fetch_add(1, Ordering::SeqCst)
                    })
                })
            })
            .collect();
        let seen: Vec<usize> = workers.into_iter().map(|w| w.join().expect("join")).collect();
        assert_eq!(runs.load(Ordering::SeqCst), 1);
        assert!(seen.iter().all(|value| *value == 0));
    }

    #[test]
    fn clearing_keeps_a_cell_still_being_filled() {
        use std::sync::Arc;
        type Cell = Arc<OnceLock<u8>>;
        fn filled(cell: &Cell) -> bool {
            cell.get().is_some()
        }
        let memo: Memo<u8, Cell> = Memo::with_settled(filled);
        let in_flight = memo.get_or_insert_with(1, Default::default);
        let done = memo.get_or_insert_with(2, Default::default);
        done.set(7).expect("fill");
        memo.clear();
        assert!(Arc::ptr_eq(&in_flight, &memo.get_or_insert_with(1, Default::default)));
        assert_eq!(memo.len(), 1);
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
