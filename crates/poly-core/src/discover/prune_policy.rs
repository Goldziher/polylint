//! The built-in pruned-directory set and the lazily loaded git-tracked evidence behind it.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// Tool-cache and environment directory names pruned from every walk, whether
/// or not git tracks anything in them (`node_modules`, `.git`, virtualenvs,
/// linter and test caches, `.polylint`).
///
/// None of it is source the user authored, so no linter or formatter should
/// touch it, and pruning it at the walk boundary avoids descending into large
/// generated subtrees. `[discovery] no_prune` is the explicit opt-out.
const ALWAYS_PRUNED_DIRECTORIES: &[&str] = &[
    "node_modules",
    ".git",
    ".venv",
    "venv",
    "__pycache__",
    ".mypy_cache",
    ".ruff_cache",
    ".pytest_cache",
    ".tox",
    ".gradle",
    ".next",
    ".nuxt",
    ".polylint",
];

/// Directory names that are build output in most ecosystems and ordinary source
/// in others (`deploy/build/`, a committed `vendor/` or Hex `deps/`).
///
/// These are pruned only when git tracks nothing inside them: a tracked
/// directory is source by definition, an untracked one is output. Outside a git
/// repository there is no such evidence, so they are pruned unconditionally.
const CONDITIONALLY_PRUNED_DIRECTORIES: &[&str] = &["build", "dist", "target", "deps", "vendor", "coverage"];

/// The git-tracked directories among [`CONDITIONALLY_PRUNED_DIRECTORIES`] under one root, loaded
/// at most once. `None` inside means the root is not in a git repository (or git cannot run).
pub(crate) type TrackedDirs = Arc<std::sync::OnceLock<Option<std::collections::HashSet<PathBuf>>>>;

/// One [`TrackedDirs`] cell per walk root, shared by the config scan, discovery and file listings
/// of a run so the whole run spawns at most one `git ls-files` per root.
#[derive(Debug, Default)]
pub(crate) struct TrackedDirsCache(Mutex<HashMap<PathBuf, TrackedDirs>>);

impl TrackedDirsCache {
    /// The shared cell for `root`, created empty on first request.
    pub(crate) fn for_root(&self, root: &Path) -> TrackedDirs {
        let mut cells = self.0.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        Arc::clone(cells.entry(root.to_path_buf()).or_default())
    }
}

/// The decision rule for [`super::keep_walk_entry`]: which nested directories a walk
/// drops, built once per walk root.
///
/// The set of git-tracked directories among [`CONDITIONALLY_PRUNED_DIRECTORIES`]
/// is loaded lazily, by one `git ls-files` per root on the first such directory
/// the walk meets, so a tree without any pays nothing and no git process is
/// ever spawned per directory. The cell is shared across the run's walks of the
/// same root.
#[derive(Debug)]
pub(crate) struct PrunePolicy {
    root: PathBuf,
    no_prune: Vec<String>,
    tracked: TrackedDirs,
}

impl PrunePolicy {
    pub(crate) fn new(root: &Path, no_prune: &[String], tracked: TrackedDirs) -> Self {
        Self {
            root: root.to_path_buf(),
            no_prune: no_prune.to_vec(),
            tracked,
        }
    }

    /// Whether the walk keeps the directory `name` at `path`.
    pub(super) fn keeps_directory(&self, name: &std::ffi::OsStr, path: &Path) -> bool {
        if self.no_prune.iter().any(|kept| name == kept.as_str()) {
            return true;
        }
        if ALWAYS_PRUNED_DIRECTORIES.iter().any(|pruned| name == *pruned) {
            return false;
        }
        if CONDITIONALLY_PRUNED_DIRECTORIES.iter().any(|pruned| name == *pruned) {
            return self
                .tracked
                .get_or_init(|| tracked_conditional_directories(&self.root))
                .as_ref()
                .is_some_and(|tracked| tracked.contains(path));
        }
        true
    }
}

/// Every directory under `root` that is named like one of
/// [`CONDITIONALLY_PRUNED_DIRECTORIES`] and holds at least one git-tracked file,
/// as `root`-joined paths. `None` when `root` is not in a git repository (or git
/// cannot run), in which case the caller has no evidence to keep anything.
///
/// Git is asked only for paths under such a directory; if that pathspec call
/// fails the full listing is used instead.
fn tracked_conditional_directories(root: &Path) -> Option<std::collections::HashSet<PathBuf>> {
    let mut narrowed = std::process::Command::new("git");
    narrowed.arg("-C").arg(root).args(["ls-files", "-z", "--"]);
    for name in CONDITIONALLY_PRUNED_DIRECTORIES {
        narrowed.arg(format!(":(glob)**/{name}/**"));
    }
    let output = match narrowed.output() {
        Ok(output) if output.status.success() => output,
        _ => std::process::Command::new("git")
            .arg("-C")
            .arg(root)
            .args(["ls-files", "-z"])
            .output()
            .ok()
            .filter(|output| output.status.success())?,
    };
    Some(conditional_directories_in(root, &output.stdout))
}

/// The [`CONDITIONALLY_PRUNED_DIRECTORIES`]-named directories that appear as a parent segment of a
/// path in NUL-separated `git ls-files -z` output, as `root`-joined paths.
fn conditional_directories_in(root: &Path, listing: &[u8]) -> std::collections::HashSet<PathBuf> {
    let mut directories = std::collections::HashSet::new();
    for raw in listing.split(|byte| *byte == 0).filter(|path| !path.is_empty()) {
        let Ok(relative) = std::str::from_utf8(raw) else {
            continue;
        };
        let mut end = 0;
        let mut segments = relative.split('/').peekable();
        while let Some(segment) = segments.next() {
            if segments.peek().is_none() {
                break;
            }
            end += segment.len();
            if CONDITIONALLY_PRUNED_DIRECTORIES.contains(&segment) {
                directories.insert(root.join(&relative[..end]));
            }
            end += 1;
        }
    }
    directories
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collects_conditional_segments_from_listing() {
        let listing = b"src/main.rs\0deploy/build/app.yaml\0vendor/a/b.go\0pkg/target/x/y.rs\0build\0\0";
        let found = conditional_directories_in(Path::new("/r"), listing);
        let expected: std::collections::HashSet<PathBuf> = ["/r/deploy/build", "/r/vendor", "/r/pkg/target"]
            .into_iter()
            .map(PathBuf::from)
            .collect();
        assert_eq!(found, expected);
    }

    #[test]
    fn nested_conditional_segments_are_each_recorded() {
        let found = conditional_directories_in(Path::new("/r"), b"build/dist/f.txt\0");
        assert!(found.contains(Path::new("/r/build")));
        assert!(found.contains(Path::new("/r/build/dist")));
        assert_eq!(found.len(), 2);
    }

    #[test]
    fn file_named_like_conditional_directory_is_not_recorded() {
        assert!(conditional_directories_in(Path::new("/r"), b"src/build\0dist\0").is_empty());
    }

    #[test]
    fn cache_hands_out_one_cell_per_root() {
        let cache = TrackedDirsCache::default();
        let first = cache.for_root(Path::new("/a"));
        assert!(Arc::ptr_eq(&first, &cache.for_root(Path::new("/a"))));
        assert!(!Arc::ptr_eq(&first, &cache.for_root(Path::new("/b"))));
    }

    #[test]
    fn policy_uses_shared_cell_without_spawning_git() {
        let cache = TrackedDirsCache::default();
        let cell = cache.for_root(Path::new("/r"));
        let _ = cell.set(Some(std::iter::once(PathBuf::from("/r/vendor")).collect()));
        let policy = PrunePolicy::new(Path::new("/r"), &[], cache.for_root(Path::new("/r")));
        assert!(policy.keeps_directory("vendor".as_ref(), Path::new("/r/vendor")));
        assert!(!policy.keeps_directory("build".as_ref(), Path::new("/r/build")));
        assert!(!policy.keeps_directory("node_modules".as_ref(), Path::new("/r/node_modules")));
        assert!(policy.keeps_directory("src".as_ref(), Path::new("/r/src")));
    }
}
