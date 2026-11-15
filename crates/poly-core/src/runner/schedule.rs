//! The order and granularity the per-file loop hands work to the pool in.
//!
//! An indexed `par_iter` over the file list lets rayon cut it into contiguous
//! runs of files, and a worker folds its run sequentially: nothing in a run that
//! has started can be stolen. Discovery sorts by path, so the costly files of a
//! repository (generated bindings, vendored sources) sit next to each other and
//! land in one run, and the other workers go idle while that one finishes.
//! Per-file cost spans four orders of magnitude, so instead every file is its own
//! unit, pulled from a shared queue in descending size order: the biggest files
//! start first, the small ones fill in behind them, and the loop ends close to
//! `total work / workers` (greedy longest-first list scheduling).

use rayon::prelude::*;

use crate::discover::DiscoveredFile;

/// Pair each file with its loop input, largest file first. Size is a proxy
/// for cost that needs one `stat`, not a read; a file that cannot be stat'ed
/// sorts last and fails where it is read. The sort is stable, so equal sizes
/// keep discovery order.
pub(super) fn largest_first<T: Send>(files: &[DiscoveredFile], inputs: Vec<T>) -> Vec<(&DiscoveredFile, T)> {
    let sizes: Vec<u64> = files
        .par_iter()
        .map(|f| std::fs::metadata(&f.path).map_or(0, |m| m.len()))
        .collect();
    let mut work: Vec<(u64, (&DiscoveredFile, T))> = sizes.into_iter().zip(files.iter().zip(inputs)).collect();
    work.sort_by_key(|(size, _)| std::cmp::Reverse(*size));
    work.into_iter().map(|(_, item)| item).collect()
}

/// Run `op` over `work` with every item an independent unit, an idle worker
/// taking the next item in order. Results come back in completion order; the
/// callers sort them by path before anything reads them.
pub(super) fn run_each<T, R, F>(work: Vec<T>, op: F) -> Vec<R>
where
    T: Send,
    R: Send,
    F: Fn(T) -> R + Sync + Send,
{
    work.into_iter().par_bridge().map(op).collect()
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;
    use std::time::Duration;

    use super::*;
    use crate::language::Language;

    fn discovered(path: &std::path::Path) -> DiscoveredFile {
        DiscoveredFile {
            path: path.to_path_buf(),
            language: Language::Python,
            config_id: 0,
        }
    }

    #[test]
    fn the_largest_file_comes_first_and_ties_keep_discovery_order() {
        let dir = tempfile::tempdir().expect("tempdir");
        let sizes = [("a", 10), ("b", 300), ("c", 10), ("d", 0), ("e", 50)];
        let files: Vec<DiscoveredFile> = sizes
            .iter()
            .map(|(name, len)| {
                let path = dir.path().join(name);
                std::fs::write(&path, "x".repeat(*len)).expect("write");
                discovered(&path)
            })
            .collect();
        let missing = discovered(&dir.path().join("missing"));
        let mut all = files.clone();
        all.insert(0, missing);
        let inputs: Vec<usize> = (0..all.len()).collect();
        let order: Vec<usize> = largest_first(&all, inputs).into_iter().map(|(_, i)| i).collect();
        // b(300) e(50) a(10) c(10), then the unreadable file and the empty one in discovery order.
        assert_eq!(order, vec![2, 5, 1, 3, 0, 4]);
    }

    #[test]
    fn every_item_runs_exactly_once() {
        let mut seen = run_each((0..1000).collect(), |i: u32| i);
        seen.sort_unstable();
        assert_eq!(seen, (0..1000).collect::<Vec<_>>());
    }

    /// The regression: a contiguous run of costly items at the front of the
    /// list used to be folded by one worker while the rest of the pool idled.
    /// Each costly item is its own unit now, so more than one worker takes them.
    #[test]
    fn a_cluster_of_costly_items_is_shared_across_workers() {
        let pool = rayon::ThreadPoolBuilder::new().num_threads(4).build().expect("pool");
        let workers: Vec<Option<usize>> = pool.install(|| {
            run_each((0..400).collect(), |i: u32| {
                if i < 24 {
                    std::thread::sleep(Duration::from_millis(15));
                    Some(rayon::current_thread_index().expect("on a pool worker"))
                } else {
                    None
                }
            })
        });
        let distinct: HashSet<usize> = workers.into_iter().flatten().collect();
        assert!(distinct.len() >= 3, "costly items ran on {distinct:?}");
    }
}
