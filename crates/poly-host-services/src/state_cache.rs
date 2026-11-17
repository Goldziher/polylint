//! State kept across MCP requests: parsed configs, and the scope of the process-global tool lock.
//!
//! # Config cache
//!
//! Resolving a config reads and parses `poly.toml` (cascade, `poly.local.toml`, native tool
//! configs). [`StateCache`] keeps the parsed result per request root and revalidates it with a
//! `stat` of every file the resolution reads, so an edit is seen on the next request.
//!
//! Only parsing is cached. The `ConfigSet`, file discovery, engine plan and result-cache handle
//! depend on the file tree and the request's paths, so they are rebuilt per request.
//!
//! The recorded files are, for each ancestor directory of the root, `poly.toml`,
//! `poly.local.toml`, `poly.lock`, the typos, oxfmt and oxlint native configs, `pyproject.toml`
//! and `.git`, plus the per-user trust store (a config's exec-capable keys are kept or withheld
//! by it), with absent files recorded as absent so one appearing invalidates the entry. A
//! config that uses `extends` reads files this stamp cannot enumerate, so it is never cached.
//! An mtime-and-length stamp can miss a same-length rewrite within one mtime tick.
//!
//! # Tool-lock scope
//!
//! [`poly_config::tool_lock`] holds one process-global set of pins that the tool provisioner
//! reads mid-run. Two concurrent requests for repositories with different `poly.lock` files must
//! not see each other's pins, and the provisioner has no run root to key a lookup by. [`enter`]
//! therefore admits concurrent runs only while they share the same pins, and makes a run with
//! different pins wait until the earlier ones finish.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::time::UNIX_EPOCH;

use poly_config::tool_lock::{self, LockedTool};

/// Entries kept per cache; the oldest is dropped beyond this.
const MAX_ENTRIES: usize = 16;

/// Config files read from each directory while resolving a config.
const WATCHED_NAMES: [&str; 11] = [
    "poly.toml",
    "poly.local.toml",
    "poly.lock",
    "_typos.toml",
    ".typos.toml",
    "pyproject.toml",
    ".oxfmtrc.json",
    ".oxfmtrc.jsonc",
    ".oxlintrc.json",
    ".oxlintrc.jsonc",
    ".git",
];

type FileState = Option<(u128, u64)>;
type Stamp = Vec<(PathBuf, FileState)>;

fn file_state(path: &Path) -> FileState {
    let meta = std::fs::metadata(path).ok()?;
    let mtime = meta.modified().ok()?.duration_since(UNIX_EPOCH).ok()?.as_nanos();
    Some((mtime, meta.len()))
}

/// The stamp for a request root: `explicit` (a config file) or a directory.
fn stamp_for(root: &Path, explicit: bool) -> Stamp {
    let mut stamp = Stamp::new();
    let dir = if explicit { root.parent().unwrap_or(root) } else { root };
    if explicit {
        stamp.push((root.to_path_buf(), file_state(root)));
    }
    for ancestor in dir.ancestors() {
        for name in WATCHED_NAMES {
            let path = ancestor.join(name);
            let state = file_state(&path);
            stamp.push((path, state));
        }
    }
    // What a parsed config keeps of its exec-capable keys depends on the trust store, so a
    // `poly trust` run beside the server invalidates the entry like an edit to the config.
    if let Ok(store) = poly_config::trust::store::store_file() {
        let state = file_state(&store);
        stamp.push((store, state));
    }
    stamp
}

fn stamp_current(stamp: &Stamp) -> bool {
    stamp.iter().all(|(path, state)| file_state(path) == *state)
}

/// Whether a watched `poly.toml` pulls in other files through `extends`.
fn uses_extends(stamp: &Stamp) -> bool {
    stamp
        .iter()
        .filter(|(path, state)| {
            state.is_some()
                && path
                    .file_name()
                    .is_some_and(|n| n == "poly.toml" || n == "poly.local.toml")
        })
        .any(|(path, _)| std::fs::read_to_string(path).map_or(true, |text| text.contains("extends")))
}

struct Entry<T> {
    key: (bool, PathBuf),
    stamp: Stamp,
    value: Arc<T>,
}

/// Parsed values keyed by request root, revalidated by file stamps (see the module docs).
pub(crate) struct StateCache<T> {
    entries: Mutex<Vec<Entry<T>>>,
}

impl<T> StateCache<T> {
    pub(crate) const fn new() -> Self {
        Self {
            entries: Mutex::new(Vec::new()),
        }
    }

    /// The value cached for `root` while its stamp holds, else `load()`'s result.
    ///
    /// `explicit` marks `root` as a config file rather than a directory. Errors are not cached.
    pub(crate) fn get_or_load(
        &self,
        root: &Path,
        explicit: bool,
        load: impl FnOnce() -> anyhow::Result<T>,
    ) -> anyhow::Result<Arc<T>> {
        let root = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
        let key = (explicit, root);
        {
            let mut entries = self.entries.lock().unwrap_or_else(PoisonError::into_inner);
            if let Some(i) = entries.iter().position(|e| e.key == key) {
                if stamp_current(&entries[i].stamp) {
                    return Ok(Arc::clone(&entries[i].value));
                }
                entries.remove(i);
            }
        }
        // Stamped before loading so a file changing mid-load fails the next validation.
        let stamp = stamp_for(&key.1, explicit);
        let value = Arc::new(load()?);
        if !uses_extends(&stamp) {
            let mut entries = self.entries.lock().unwrap_or_else(PoisonError::into_inner);
            entries.retain(|e| e.key != key);
            if entries.len() >= MAX_ENTRIES {
                entries.remove(0);
            }
            entries.push(Entry {
                key,
                stamp,
                value: Arc::clone(&value),
            });
        }
        Ok(value)
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.entries.lock().unwrap_or_else(PoisonError::into_inner).len()
    }
}

struct Scope {
    tools: Vec<LockedTool>,
    active: usize,
}

static SCOPE: Mutex<Scope> = Mutex::new(Scope {
    tools: Vec::new(),
    active: 0,
});
static IDLE: Condvar = Condvar::new();

/// Keeps the process's tool pins at the entered set until dropped.
pub struct ToolLockGuard(());

/// Install `tools` as the process's pins for the lifetime of the guard, waiting while runs
/// holding different pins are in flight. Runs with identical pins share the scope.
pub fn enter(tools: Vec<LockedTool>) -> ToolLockGuard {
    let mut scope = SCOPE.lock().unwrap_or_else(PoisonError::into_inner);
    while scope.active > 0 && scope.tools != tools {
        scope = IDLE.wait(scope).unwrap_or_else(PoisonError::into_inner);
    }
    if scope.active == 0 {
        tool_lock::install(tools.clone());
        scope.tools = tools;
    }
    scope.active += 1;
    ToolLockGuard(())
}

/// [`enter`] with the pins of the repository holding `start` (see
/// [`tool_lock::read_from_repo`]).
///
/// # Errors
///
/// When `poly.lock` exists but cannot be read or parsed.
pub fn enter_repo(start: &Path) -> anyhow::Result<ToolLockGuard> {
    Ok(enter(tool_lock::read_from_repo(start)?))
}

impl Drop for ToolLockGuard {
    fn drop(&mut self) {
        let mut scope = SCOPE.lock().unwrap_or_else(PoisonError::into_inner);
        scope.active -= 1;
        if scope.active == 0 {
            IDLE.notify_all();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use poly_config::tool_lock::LockedAsset;

    fn pinned(version: &str) -> Vec<LockedTool> {
        vec![LockedTool {
            name: "scope-test-tool".into(),
            version: version.into(),
            assets: vec![LockedAsset {
                platform: "any".into(),
                url: "https://example.invalid/t".into(),
                sha256: "0".repeat(64),
                member: None,
            }],
        }]
    }

    fn repo_with_config(text: &str) -> tempfile::TempDir {
        let repo = tempfile::tempdir().unwrap();
        std::fs::create_dir(repo.path().join(".git")).unwrap();
        std::fs::write(repo.path().join("poly.toml"), text).unwrap();
        repo
    }

    #[test]
    fn an_unchanged_config_returns_the_same_arc_and_an_edit_reparses() {
        let repo = repo_with_config("");
        let cache = StateCache::<String>::new();
        let loads = std::cell::Cell::new(0);
        let load = || {
            loads.set(loads.get() + 1);
            Ok(format!("load {}", loads.get()))
        };
        let first = cache.get_or_load(repo.path(), false, load).unwrap();
        let second = cache.get_or_load(repo.path(), false, load).unwrap();
        assert!(Arc::ptr_eq(&first, &second));
        assert_eq!(loads.get(), 1);
        std::fs::write(repo.path().join("poly.toml"), "# changed, and longer\n").unwrap();
        let third = cache.get_or_load(repo.path(), false, load).unwrap();
        assert!(!Arc::ptr_eq(&first, &third));
        assert_eq!(*third, "load 2");
    }

    #[test]
    fn a_watched_file_appearing_invalidates_the_entry() {
        let repo = repo_with_config("");
        let cache = StateCache::<u32>::new();
        let loads = std::cell::Cell::new(0);
        let load = || {
            loads.set(loads.get() + 1);
            Ok(loads.get())
        };
        cache.get_or_load(repo.path(), false, load).unwrap();
        std::fs::write(repo.path().join("poly.local.toml"), "").unwrap();
        cache.get_or_load(repo.path(), false, load).unwrap();
        assert_eq!(loads.get(), 2);
    }

    #[test]
    fn a_config_with_extends_is_never_cached() {
        let repo = repo_with_config("extends = [\"base.toml\"]\n");
        let cache = StateCache::<u32>::new();
        cache.get_or_load(repo.path(), false, || Ok(1)).unwrap();
        assert_eq!(cache.len(), 0);
    }

    #[test]
    fn errors_are_not_cached() {
        let repo = repo_with_config("");
        let cache = StateCache::<u32>::new();
        assert!(cache.get_or_load(repo.path(), false, || anyhow::bail!("bad")).is_err());
        assert_eq!(*cache.get_or_load(repo.path(), false, || Ok(7)).unwrap(), 7);
    }

    #[test]
    fn the_cache_is_bounded_and_drops_the_oldest() {
        let cache = StateCache::<usize>::new();
        let repos: Vec<_> = (0..MAX_ENTRIES + 3).map(|_| repo_with_config("")).collect();
        for (i, repo) in repos.iter().enumerate() {
            cache.get_or_load(repo.path(), false, || Ok(i)).unwrap();
        }
        assert_eq!(cache.len(), MAX_ENTRIES);
        let reloaded = std::cell::Cell::new(false);
        cache
            .get_or_load(repos[0].path(), false, || {
                reloaded.set(true);
                Ok(0)
            })
            .unwrap();
        assert!(reloaded.get(), "the oldest entry was evicted");
    }

    #[test]
    fn runs_with_different_pins_never_overlap() {
        let _serial = crate::CONFIG_LOADING.lock().unwrap_or_else(PoisonError::into_inner);
        let first = enter(pinned("1.0.0"));
        let (tx, rx) = std::sync::mpsc::channel();
        let other = std::thread::spawn(move || {
            let _guard = enter(pinned("2.0.0"));
            tx.send(tool_lock::get("scope-test-tool").map(|t| t.version)).unwrap();
        });
        assert_eq!(tool_lock::get("scope-test-tool").unwrap().version, "1.0.0");
        assert!(rx.try_recv().is_err(), "the other root waits while pins differ");
        drop(first);
        assert_eq!(rx.recv().unwrap().as_deref(), Some("2.0.0"));
        other.join().unwrap();
        tool_lock::install(Vec::new());
    }

    #[test]
    fn runs_with_identical_pins_share_the_scope() {
        let _serial = crate::CONFIG_LOADING.lock().unwrap_or_else(PoisonError::into_inner);
        let first = enter(pinned("1.0.0"));
        let second = enter(pinned("1.0.0"));
        drop(first);
        assert_eq!(tool_lock::get("scope-test-tool").unwrap().version, "1.0.0");
        drop(second);
        tool_lock::install(Vec::new());
    }
}
