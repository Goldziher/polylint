//! A read-only tree of files in memory as a host's file system: exactly the files it was given, for a host that
//! checks a request's files and for tests.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::SystemTime;

use crate::vfs::{BomFiles, wrap_fs};
use tsrs_vfs::{Entries, FS, FileInfo, FileMode};

const BOM: char = '\u{feff}';

/// Files held in memory, by their names as tsrs names files ([`tsrs_path`](crate::tsrs_path)): a file system of
/// exactly these files and the directories above them. Names are case-sensitive on every platform, nothing is a
/// symbolic link and nothing can be written, so what a program built over it finds does not depend on the machine.
#[derive(Debug, Clone, Default)]
pub struct MemoryFs {
    files: BTreeMap<String, Arc<str>>,
}

struct Reading {
    files: BTreeMap<String, Arc<str>>,
    directories: BTreeSet<String>,
    boms: Arc<BomFiles>,
}

impl MemoryFs {
    /// No files.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// These files and the file at `path` with `text`, replacing an earlier one.
    #[must_use]
    pub fn with_file(mut self, path: &str, text: impl Into<Arc<str>>) -> Self {
        self.insert(path, text);
        self
    }

    /// Adds the file at `path` with `text`, replacing an earlier one. A text the host already holds as an `Arc<str>`
    /// is shared, not copied.
    pub fn insert(&mut self, path: &str, text: impl Into<Arc<str>>) {
        self.files.insert(path.to_owned(), text.into());
    }

    /// Whether a file is at `path`.
    #[must_use]
    pub fn contains_file(&self, path: &str) -> bool {
        self.files.contains_key(path)
    }

    /// The file system programs read the files through ([`wrap_fs`]), with its record of the files that start with
    /// a byte order mark: a read drops the mark, as tsrs expects, and records the file.
    #[must_use]
    pub fn into_fs(self) -> (Arc<dyn FS>, Arc<BomFiles>) {
        let mut directories = BTreeSet::new();
        for path in self.files.keys() {
            let mut dir = path.as_str();
            while let Some(slash) = dir.rfind('/') {
                dir = &dir[..slash];
                directories.insert(if dir.is_empty() { "/".to_owned() } else { dir.to_owned() });
            }
        }
        let boms = Arc::new(BomFiles::default());
        let reading = Reading {
            files: self.files,
            directories,
            boms: Arc::clone(&boms),
        };
        (wrap_fs(reading), boms)
    }
}

fn read_only(path: &str) -> String {
    format!("cannot write {path}: the files are read only")
}

/// The name of the entry `path` is directly below `dir`, if it is.
fn child<'a>(dir: &str, path: &'a str) -> Option<&'a str> {
    let rest = path.strip_prefix(dir)?;
    let name = if dir == "/" { rest } else { rest.strip_prefix('/')? };
    (!name.is_empty() && !name.contains('/')).then_some(name)
}

impl FS for Reading {
    fn use_case_sensitive_file_names(&self) -> bool {
        true
    }

    fn file_exists(&self, path: &str) -> bool {
        self.files.contains_key(path)
    }

    fn read_file(&self, path: &str) -> Option<String> {
        let text = self.files.get(path)?;
        Some(match text.strip_prefix(BOM) {
            Some(rest) => {
                self.boms.record(path);
                rest.to_owned()
            }
            None => text.as_ref().to_owned(),
        })
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
        self.directories.contains(path)
    }

    fn get_accessible_entries(&self, path: &str) -> Entries {
        let mut entries = Entries::default();
        if !self.directories.contains(path) {
            return entries;
        }
        entries.files = self
            .files
            .keys()
            .filter_map(|file| child(path, file))
            .map(str::to_owned)
            .collect();
        entries.directories = self
            .directories
            .iter()
            .filter_map(|dir| child(path, dir))
            .map(str::to_owned)
            .collect();
        entries.symlinks = Some(entries.symlinks.take().unwrap_or_default());
        entries
    }

    fn stat(&self, path: &str) -> Option<FileInfo> {
        let mode = if self.files.contains_key(path) {
            FileMode::None
        } else if self.directories.contains(path) {
            FileMode::Dir
        } else {
            return None;
        };
        Some(FileInfo {
            name: path.rsplit('/').next().unwrap_or_default().to_owned(),
            size: 0,
            mode,
            mod_time: None,
        })
    }

    fn realpath(&self, path: &str) -> String {
        path.to_owned()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn files() -> MemoryFs {
        MemoryFs::new()
            .with_file("/ws/a.ts", "\u{feff}export class A {}")
            .with_file("/ws/b.ts", "export class B {}")
            .with_file("/ws/src/c.ts", "")
    }

    #[test]
    fn reads_drop_the_byte_order_mark_and_record_it() {
        let (fs, boms) = files().into_fs();
        assert_eq!(boms.offset("/ws/a.ts"), 0);
        assert_eq!(fs.read_file("/ws/a.ts").as_deref(), Some("export class A {}"));
        assert_eq!(fs.read_file("/ws/b.ts").as_deref(), Some("export class B {}"));
        assert_eq!(boms.offset("/ws/a.ts"), 3);
        assert_eq!(boms.offset("/ws/b.ts"), 0);
        assert_eq!(fs.read_file("/ws/missing.ts"), None);
    }

    #[test]
    fn lists_files_and_directories_by_name() {
        let (fs, _) = files().into_fs();
        let entries = fs.get_accessible_entries("/ws");
        assert_eq!(entries.files, ["a.ts", "b.ts"]);
        assert_eq!(entries.directories, ["src"]);
        assert!(entries.symlinks.is_some_and(|links| links.is_empty()));
        assert_eq!(fs.get_accessible_entries("/").directories, ["ws"]);
        assert_eq!(fs.get_accessible_entries("/missing"), Entries::default());
    }

    #[test]
    fn answers_existence_and_stat_from_the_files_alone() {
        let memory = files();
        assert!(memory.contains_file("/ws/a.ts") && !memory.contains_file("/ws/src"));
        let (fs, _) = memory.into_fs();
        assert!(fs.file_exists("/ws/a.ts") && !fs.file_exists("/ws/src"));
        assert!(fs.directory_exists("/ws/src") && !fs.directory_exists("/ws/a.ts"));
        assert!(!fs.file_exists("/ws/B.ts"), "names are case-sensitive");
        assert!(fs.stat("/ws/src").is_some_and(|info| info.is_dir()));
        assert!(
            fs.stat("/ws/b.ts")
                .is_some_and(|info| !info.is_dir() && info.name() == "b.ts")
        );
        assert!(fs.stat("/ws/none").is_none());
        assert_eq!(fs.realpath("/ws/b.ts"), "/ws/b.ts");
        assert!(fs.write_file("/ws/b.ts", "x").is_err());
    }
}
