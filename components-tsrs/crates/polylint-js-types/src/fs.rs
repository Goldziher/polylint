//! The read-only file system the analysis reads every config and source through: the host's own view, so an
//! embedder's unsaved buffers are what the checker sees. The trait holds only what the analysis reads, with the
//! signatures of the trait it was taken from (polylint ADR 0081), so a host of that trait implements this one by
//! forwarding.

use std::collections::BTreeMap;
use std::io;
use std::ops::Bound;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

/// A directory entry returned by [`FileSystem::read_dir`].
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct DirEntry {
    /// Full path of the entry (the listed directory joined with the entry name).
    pub path: PathBuf,
    /// Whether the entry is a directory (symlinks are followed).
    pub is_dir: bool,
    /// Whether the entry is a regular file (symlinks are followed).
    pub is_file: bool,
    /// Whether the entry itself is a symbolic link; `is_dir` and `is_file` describe its target.
    pub is_symlink: bool,
}

impl DirEntry {
    /// An entry that is not a symbolic link.
    #[must_use]
    pub const fn new(path: PathBuf, is_dir: bool, is_file: bool) -> Self {
        Self {
            path,
            is_dir,
            is_file,
            is_symlink: false,
        }
    }

    /// Marks the entry as a symbolic link.
    #[must_use]
    pub const fn symlink(mut self) -> Self {
        self.is_symlink = true;
        self
    }
}

/// What a path is, with symbolic links followed. A path that does not exist is neither.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[non_exhaustive]
pub struct PathKind {
    /// The path is a regular file.
    pub is_file: bool,
    /// The path is a directory.
    pub is_dir: bool,
}

impl PathKind {
    /// A path that is neither a file nor a directory.
    pub const NONE: Self = Self {
        is_file: false,
        is_dir: false,
    };
    /// A regular file.
    pub const FILE: Self = Self {
        is_file: true,
        is_dir: false,
    };
    /// A directory.
    pub const DIR: Self = Self {
        is_file: false,
        is_dir: true,
    };
}

/// Read-only file system access for the analysis.
pub trait FileSystem: Send + Sync {
    /// Reads a UTF-8 file.
    ///
    /// # Errors
    /// Any IO error from reading the file; [`io::ErrorKind::InvalidData`] for text that is not UTF-8, which the
    /// analysis then reads with [`FileSystem::read_bytes`] and decodes as TypeScript does (UTF-16).
    fn read_to_string(&self, path: &Path) -> io::Result<Arc<str>>;
    /// Lists a directory, sorted by path.
    ///
    /// # Errors
    /// Any IO error from listing the directory.
    fn read_dir(&self, path: &Path) -> io::Result<Vec<DirEntry>>;
    /// Lists a directory like [`FileSystem::read_dir`], as a shared slice.
    ///
    /// Caching implementations hand out the listing they hold instead of copying it.
    ///
    /// # Errors
    /// Any IO error from listing the directory.
    fn read_dir_shared(&self, path: &Path) -> io::Result<Arc<[DirEntry]>> {
        self.read_dir(path).map(Arc::from)
    }
    /// Whether `path` is a regular file.
    fn is_file(&self, path: &Path) -> bool;
    /// Whether `path` is a directory.
    fn is_dir(&self, path: &Path) -> bool;
    /// What `path` is, in one query.
    ///
    /// The default asks [`FileSystem::is_file`] and then [`FileSystem::is_dir`]; an implementation that can answer both
    /// with one lookup (a `stat`) overrides it. An implementation that answers `is_file` or `is_dir` through `kind`
    /// must override `kind` as well.
    fn kind(&self, path: &Path) -> PathKind {
        let is_file = self.is_file(path);
        PathKind {
            is_file,
            is_dir: !is_file && self.is_dir(path),
        }
    }
    /// `path` as the file system resolves it: absolute, with every symbolic link followed.
    ///
    /// The default returns `path` unchanged, which is right for a tree that has no links (such as [`MemoryFs`]).
    ///
    /// # Errors
    /// Any IO error from resolving the path, such as it not existing.
    fn canonicalize(&self, path: &Path) -> io::Result<PathBuf> {
        Ok(path.to_path_buf())
    }
    /// Reads a file as raw bytes, for files that are not UTF-8.
    ///
    /// The default reads the file as text, so it fails for text that is not valid UTF-8.
    ///
    /// # Errors
    /// Any IO error from reading the file.
    fn read_bytes(&self, path: &Path) -> io::Result<Vec<u8>> {
        self.read_to_string(path).map(|text| text.as_bytes().to_vec())
    }
    /// When the file at `path` was last modified; `None` when the file system cannot tell (the default).
    fn modified(&self, path: &Path) -> Option<SystemTime> {
        let _ = path;
        None
    }
}

/// Forwards every [`FileSystem`] method to the target of a pointer type, so `&F`, `Box<F>` and `Arc<F>` can be used
/// wherever a file system is expected.
macro_rules! forward_file_system {
    ($($pointer:ty),+ $(,)?) => {$(
        impl<F: FileSystem + ?Sized> FileSystem for $pointer {
            fn read_to_string(&self, path: &Path) -> io::Result<Arc<str>> {
                (**self).read_to_string(path)
            }

            fn read_dir(&self, path: &Path) -> io::Result<Vec<DirEntry>> {
                (**self).read_dir(path)
            }

            fn read_dir_shared(&self, path: &Path) -> io::Result<Arc<[DirEntry]>> {
                (**self).read_dir_shared(path)
            }

            fn is_file(&self, path: &Path) -> bool {
                (**self).is_file(path)
            }

            fn is_dir(&self, path: &Path) -> bool {
                (**self).is_dir(path)
            }

            fn kind(&self, path: &Path) -> PathKind {
                (**self).kind(path)
            }

            fn canonicalize(&self, path: &Path) -> io::Result<PathBuf> {
                (**self).canonicalize(path)
            }

            fn read_bytes(&self, path: &Path) -> io::Result<Vec<u8>> {
                (**self).read_bytes(path)
            }

            fn modified(&self, path: &Path) -> Option<SystemTime> {
                (**self).modified(path)
            }
        }
    )+};
}

forward_file_system!(&F, Box<F>, Arc<F>);

/// The real file system.
#[derive(Debug, Clone, Copy, Default)]
pub struct OsFs;

impl FileSystem for OsFs {
    fn read_to_string(&self, path: &Path) -> io::Result<Arc<str>> {
        std::fs::read_to_string(path).map(Arc::from)
    }

    fn read_dir(&self, path: &Path) -> io::Result<Vec<DirEntry>> {
        let mut entries = Vec::new();
        for entry in std::fs::read_dir(path)? {
            // An entry that vanished between the listing and the lookup, or cannot be read, is skipped; it must not
            // hide its siblings.
            let Ok(entry) = entry else { continue };
            let path = entry.path();
            // Only a symlink needs a `stat` to learn its target; when the record has no type, a `stat` answers.
            let (is_dir, is_file, is_symlink) = match entry.file_type() {
                Ok(file_type) if file_type.is_symlink() => {
                    let (is_dir, is_file) = std::fs::metadata(&path)
                        .map_or((false, false), |metadata| (metadata.is_dir(), metadata.is_file()));
                    (is_dir, is_file, true)
                }
                Ok(file_type) => (file_type.is_dir(), file_type.is_file(), false),
                Err(_) => match std::fs::metadata(&path) {
                    Ok(metadata) => (metadata.is_dir(), metadata.is_file(), false),
                    Err(_) => continue,
                },
            };
            let entry = DirEntry::new(path, is_dir, is_file);
            entries.push(if is_symlink { entry.symlink() } else { entry });
        }
        entries.sort_unstable_by(|a, b| a.path.as_os_str().cmp(b.path.as_os_str()));
        Ok(entries)
    }

    fn is_file(&self, path: &Path) -> bool {
        path.is_file()
    }

    fn is_dir(&self, path: &Path) -> bool {
        path.is_dir()
    }

    fn kind(&self, path: &Path) -> PathKind {
        std::fs::metadata(path).map_or(PathKind::NONE, |metadata| PathKind {
            is_file: metadata.is_file(),
            is_dir: metadata.is_dir(),
        })
    }

    fn canonicalize(&self, path: &Path) -> io::Result<PathBuf> {
        std::fs::canonicalize(path)
    }

    fn read_bytes(&self, path: &Path) -> io::Result<Vec<u8>> {
        std::fs::read(path)
    }

    fn modified(&self, path: &Path) -> Option<SystemTime> {
        std::fs::metadata(path).and_then(|metadata| metadata.modified()).ok()
    }
}

/// An in-memory file tree for tests and embedders. Directories exist implicitly as parents of files.
///
/// Paths order component-wise, so everything below a directory is one contiguous range of the map; listing and
/// directory checks are range queries rather than scans of the whole tree.
#[derive(Debug, Clone, Default)]
pub struct MemoryFs {
    files: BTreeMap<PathBuf, Arc<str>>,
}

impl MemoryFs {
    /// An empty tree.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a file, builder style.
    #[must_use]
    pub fn with_file(mut self, path: impl Into<PathBuf>, content: impl AsRef<str>) -> Self {
        self.insert(path, content);
        self
    }

    /// Adds or replaces a file.
    pub fn insert(&mut self, path: impl Into<PathBuf>, content: impl AsRef<str>) {
        self.files.insert(path.into(), Arc::from(content.as_ref()));
    }

    /// The files strictly below `dir`.
    fn below<'a>(&'a self, dir: &'a Path) -> impl Iterator<Item = &'a PathBuf> + 'a {
        self.files
            .range::<Path, _>((Bound::Included(dir), Bound::Unbounded))
            .map(|(path, _)| path)
            .take_while(move |path| path.starts_with(dir))
            .filter(move |path| path.as_path() != dir)
    }
}

fn not_found(path: &Path) -> io::Error {
    io::Error::new(io::ErrorKind::NotFound, format!("{} not found", path.display()))
}

impl FileSystem for MemoryFs {
    fn read_to_string(&self, path: &Path) -> io::Result<Arc<str>> {
        self.files.get(path).cloned().ok_or_else(|| not_found(path))
    }

    fn read_dir(&self, path: &Path) -> io::Result<Vec<DirEntry>> {
        let mut children: BTreeMap<PathBuf, (bool, bool)> = BTreeMap::new();
        for file in self.below(path) {
            let Ok(rest) = file.strip_prefix(path) else { continue };
            let mut components = rest.components();
            let Some(first) = components.next() else { continue };
            let is_file = components.next().is_none();
            let flags = children.entry(path.join(first.as_os_str())).or_default();
            if is_file {
                flags.1 = true;
            } else {
                flags.0 = true;
            }
        }
        if children.is_empty() {
            return Err(not_found(path));
        }
        Ok(children
            .into_iter()
            .map(|(path, (is_dir, is_file))| DirEntry::new(path, is_dir, is_file))
            .collect())
    }

    fn is_file(&self, path: &Path) -> bool {
        self.files.contains_key(path)
    }

    fn is_dir(&self, path: &Path) -> bool {
        self.below(path).next().is_some()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn tree() -> MemoryFs {
        MemoryFs::new()
            .with_file("/ws/a/package.json", "{}")
            .with_file("/ws/a/src/index.js", "")
            .with_file("/ws/a-b/package.json", "{}")
            .with_file("/ws/ab", "file")
            .with_file("/ws/z/package.json", "{}")
    }

    fn names(entries: &[DirEntry]) -> Vec<(String, bool, bool)> {
        entries
            .iter()
            .map(|e| {
                (
                    e.path.file_name().unwrap().to_string_lossy().into_owned(),
                    e.is_dir,
                    e.is_file,
                )
            })
            .collect()
    }

    #[test]
    fn memory_listing_is_a_sorted_range_that_ignores_prefix_siblings() {
        let fs = tree();
        assert_eq!(
            names(&fs.read_dir(Path::new("/ws")).unwrap()),
            [
                ("a".into(), true, false),
                ("a-b".into(), true, false),
                ("ab".into(), false, true),
                ("z".into(), true, false)
            ]
        );
        assert_eq!(
            names(&fs.read_dir(Path::new("/ws/a")).unwrap()),
            [("package.json".into(), false, true), ("src".into(), true, false)]
        );
    }

    #[test]
    fn memory_missing_and_file_paths_are_not_directories() {
        let fs = tree();
        assert!(fs.read_dir(Path::new("/ws/missing")).is_err());
        assert!(fs.read_dir(Path::new("/ws/ab")).is_err());
        assert!(fs.is_dir(Path::new("/ws/a")) && fs.is_dir(Path::new("/ws/a/src")) && fs.is_dir(Path::new("/ws")));
        assert!(!fs.is_dir(Path::new("/ws/ab")) && !fs.is_dir(Path::new("/ws/a/sr")) && !fs.is_dir(Path::new("/x")));
        assert!(fs.is_file(Path::new("/ws/ab")) && !fs.is_file(Path::new("/ws/a")));
        assert_eq!(fs.kind(Path::new("/ws/a")), PathKind::DIR);
        assert_eq!(fs.kind(Path::new("/ws/ab")), PathKind::FILE);
        assert_eq!(fs.kind(Path::new("/x")), PathKind::NONE);
    }

    #[test]
    fn memory_reads_share_one_allocation() {
        let fs = tree();
        let first = fs.read_to_string(Path::new("/ws/ab")).unwrap();
        let second = fs.read_to_string(Path::new("/ws/ab")).unwrap();
        assert_eq!(&*first, "file");
        assert!(Arc::ptr_eq(&first, &second));
        assert_eq!(
            fs.read_to_string(Path::new("/nope")).unwrap_err().kind(),
            io::ErrorKind::NotFound
        );
        assert_eq!(fs.read_bytes(Path::new("/ws/ab")).unwrap(), b"file");
        assert_eq!(fs.modified(Path::new("/ws/ab")), None);
    }

    #[test]
    fn os_listing_reports_kinds_sorted() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("sub")).unwrap();
        std::fs::write(dir.path().join("b.txt"), "hello").unwrap();
        std::fs::write(dir.path().join("a.txt"), "").unwrap();
        let entries = OsFs.read_dir(dir.path()).unwrap();
        assert_eq!(
            names(&entries),
            [
                ("a.txt".into(), false, true),
                ("b.txt".into(), false, true),
                ("sub".into(), true, false)
            ]
        );
        assert_eq!(&*OsFs.read_to_string(&dir.path().join("b.txt")).unwrap(), "hello");
        assert_eq!(OsFs.kind(dir.path()), PathKind::DIR);
        assert_eq!(OsFs.kind(&dir.path().join("a.txt")), PathKind::FILE);
        assert_eq!(OsFs.kind(&dir.path().join("missing")), PathKind::NONE);
        assert!(OsFs.modified(&dir.path().join("a.txt")).is_some());
        std::fs::write(dir.path().join("utf16.ts"), [0xff, 0xfe, b'a', 0]).unwrap();
        let invalid = OsFs.read_to_string(&dir.path().join("utf16.ts")).unwrap_err();
        assert_eq!(invalid.kind(), io::ErrorKind::InvalidData);
        assert_eq!(
            OsFs.read_bytes(&dir.path().join("utf16.ts")).unwrap(),
            [0xff, 0xfe, b'a', 0]
        );
    }

    #[cfg(unix)]
    #[test]
    fn os_listing_follows_and_marks_symlinks() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("real")).unwrap();
        std::os::unix::fs::symlink(dir.path().join("real"), dir.path().join("link")).unwrap();
        std::os::unix::fs::symlink(dir.path().join("gone"), dir.path().join("dangling")).unwrap();
        let entries = OsFs.read_dir(dir.path()).unwrap();
        assert_eq!(
            names(&entries),
            [
                ("dangling".into(), false, false),
                ("link".into(), true, false),
                ("real".into(), true, false)
            ]
        );
        let links: Vec<bool> = entries.iter().map(|entry| entry.is_symlink).collect();
        assert_eq!(links, [true, true, false]);
    }

    #[test]
    fn pointers_to_a_file_system_are_file_systems() {
        fn is_file(fs: impl FileSystem, path: &str) -> bool {
            fs.is_file(Path::new(path))
        }
        let fs = tree();
        assert!(is_file(&fs, "/ws/ab"));
        assert!(is_file(Box::new(fs.clone()), "/ws/ab"));
        let dynamic: Arc<dyn FileSystem> = Arc::new(fs);
        assert!(is_file(Arc::clone(&dynamic), "/ws/ab"));
        assert!(!is_file(dynamic, "/ws/a"));
    }
}
