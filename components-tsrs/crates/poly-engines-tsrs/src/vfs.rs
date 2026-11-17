//! The file system tsrs reads: exactly the files the host handed over, wrapped as tsrs wraps a real one
//! (bundled `lib.*.d.ts` files and cached lookups).
//!
//! Adapted from `skif-js-types` (`src/vfs.rs`, MIT, same author; see `ATTRIBUTIONS.md`), with the backing store
//! replaced by an in-memory tree of the request's files.

use std::collections::{BTreeSet, HashMap};
use std::path::Path;
use std::sync::Arc;
use std::time::SystemTime;

use tsrs_core::tspath;
use tsrs_vfs::{Entries, FS, FileInfo, FileMode, bundled, cachedvfs};

const BOM: char = '\u{feff}';

/// `path` as tsrs names files: absolute, forward slashes, normalized.
pub(crate) fn tsrs_path(path: &Path) -> String {
    let absolute = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    tspath::normalize_path(&absolute.to_string_lossy())
}

/// The file `path` of a request rooted at `root`, as tsrs names files.
///
/// A component is handed paths relative to the root, and the root is not the working directory of a run started
/// below it or naming it from elsewhere; a path the host spells from the root or absolutely is taken as it is.
pub(crate) fn tsrs_name(root: &Path, path: &Path) -> String {
    if path.is_absolute() || path.starts_with(root) {
        tsrs_path(path)
    } else {
        tsrs_path(&root.join(path))
    }
}

/// The directory of the `lib.*.d.ts` files embedded in tsrs. Passed explicitly: `bundled::default_lib_path`
/// reads `TSRS_LIB_PATH`.
pub(crate) fn lib_path() -> String {
    bundled::lib_path()
}

#[derive(Default)]
struct Dir {
    files: BTreeSet<String>,
    directories: BTreeSet<String>,
}

/// A read-only tree of the request's files.
///
/// File names are case-sensitive on every platform: the result must not depend on the machine the check runs
/// on.
#[derive(Default)]
pub(crate) struct MemoryFs {
    files: HashMap<String, Arc<str>>,
    directories: HashMap<String, Dir>,
}

impl MemoryFs {
    /// Adds the file at `path` (as tsrs names it) and every directory above it.
    pub(crate) fn insert(&mut self, path: String, content: Arc<str>) {
        let mut child = path.clone();
        let mut is_file = true;
        loop {
            let parent = tspath::get_directory_path(&child);
            if parent.is_empty() || parent == child {
                break;
            }
            let name = child.rsplit('/').next().unwrap_or_default().to_owned();
            let dir = self.directories.entry(parent.clone()).or_default();
            if is_file {
                dir.files.insert(name);
            } else {
                dir.directories.insert(name);
            }
            is_file = false;
            child = parent;
        }
        self.files.insert(path, content);
    }

    pub(crate) fn contains_file(&self, path: &str) -> bool {
        self.files.contains_key(path)
    }

    /// The file system tsrs reads through.
    pub(crate) fn into_tsrs(self) -> Arc<dyn FS> {
        Arc::new(bundled::wrap_fs(cachedvfs::from(self)))
    }
}

fn read_only(path: &str) -> String {
    format!("cannot write {path}: the type check reads files only")
}

impl FS for MemoryFs {
    fn use_case_sensitive_file_names(&self) -> bool {
        true
    }

    fn file_exists(&self, path: &str) -> bool {
        self.files.contains_key(path)
    }

    fn read_file(&self, path: &str) -> Option<String> {
        let text = self.files.get(path)?;
        Some(text.strip_prefix(BOM).unwrap_or(text).to_owned())
    }

    fn write_file(&self, path: &str, _data: &str) -> Result<(), String> {
        Err(read_only(path))
    }

    fn append_file(&self, path: &str, _data: &str) -> Result<(), String> {
        Err(read_only(path))
    }

    fn remove(&self, path: &str) -> Result<(), String> {
        Err(read_only(path))
    }

    fn chtimes(&self, path: &str, _a_time: SystemTime, _m_time: SystemTime) -> Result<(), String> {
        Err(read_only(path))
    }

    fn directory_exists(&self, path: &str) -> bool {
        self.directories.contains_key(path)
    }

    fn get_accessible_entries(&self, path: &str) -> Entries {
        let Some(dir) = self.directories.get(path) else {
            return Entries::default();
        };
        Entries {
            files: dir.files.iter().cloned().collect(),
            directories: dir.directories.iter().cloned().collect(),
            symlinks: Some(Default::default()),
        }
    }

    fn stat(&self, path: &str) -> Option<FileInfo> {
        let name = path.rsplit('/').next().unwrap_or_default().to_owned();
        if let Some(text) = self.files.get(path) {
            return Some(FileInfo {
                name,
                size: i64::try_from(text.len()).unwrap_or(i64::MAX),
                mode: FileMode::None,
                mod_time: None,
            });
        }
        self.directories.contains_key(path).then_some(FileInfo {
            name,
            size: 0,
            mode: FileMode::Dir,
            mod_time: None,
        })
    }

    fn realpath(&self, path: &str) -> String {
        path.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fs() -> MemoryFs {
        let mut fs = MemoryFs::default();
        fs.insert("/ws/a.ts".into(), "\u{feff}export class A {}".into());
        fs.insert("/ws/b.ts".into(), "export class B {}".into());
        fs.insert("/ws/src/c.ts".into(), "".into());
        fs
    }

    #[test]
    fn reads_drop_the_byte_order_mark() {
        let fs = fs();
        assert_eq!(fs.read_file("/ws/a.ts").as_deref(), Some("export class A {}"));
        assert_eq!(fs.read_file("/ws/b.ts").as_deref(), Some("export class B {}"));
        assert_eq!(fs.read_file("/ws/missing.ts"), None);
    }

    #[test]
    fn lists_files_and_directories_by_name() {
        let entries = fs().get_accessible_entries("/ws");
        assert_eq!(entries.files, ["a.ts", "b.ts"]);
        assert_eq!(entries.directories, ["src"]);
        assert_eq!(fs().get_accessible_entries("/missing"), Entries::default());
    }

    #[test]
    fn answers_existence_and_stat_from_the_request_files() {
        let fs = fs();
        assert!(fs.file_exists("/ws/a.ts") && !fs.file_exists("/ws/src"));
        assert!(fs.directory_exists("/ws/src") && !fs.directory_exists("/ws/a.ts"));
        assert!(fs.stat("/ws/src").is_some_and(|info| info.is_dir()));
        assert!(
            fs.stat("/ws/b.ts")
                .is_some_and(|info| !info.is_dir() && info.name() == "b.ts")
        );
        assert!(fs.stat("/ws/none").is_none());
        assert!(fs.write_file("/ws/b.ts", "x").is_err());
    }

    #[test]
    fn names_are_case_sensitive() {
        assert!(!fs().file_exists("/ws/B.ts"));
    }

    #[test]
    fn serves_the_bundled_libraries() {
        let fs = fs().into_tsrs();
        let lib = format!("{}/lib.es5.d.ts", lib_path());
        assert!(fs.file_exists(&lib));
        assert!(
            fs.read_file(&lib)
                .is_some_and(|text| text.contains("interface Array<T>"))
        );
    }
}
