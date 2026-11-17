//! The file system tsrs reads through: the host's own, so configs and sources come from the same view as the rest of
//! its analysis (an embedder's buffers included), wrapped as tsrs wraps a real one (bundled `lib.*.d.ts` files and cached
//! lookups).

use std::io;
use std::path::Path;
use std::sync::Arc;
use std::time::SystemTime;

use crate::FileSystem;
use polylint_tsrs::BomFiles;
use tsrs_core::tspath;
use tsrs_vfs::{Entries, FS, FileInfo, FileMode};

const BOM: char = '\u{feff}';

/// A read-only [`FS`] over the host's [`FileSystem`].
struct HostVfs {
    fs: Arc<dyn FileSystem>,
    boms: Arc<BomFiles>,
}

/// The file system every config and program of one analysis reads through, with the record of the files that start
/// with a byte order mark.
pub(crate) fn tsrs_fs(fs: Arc<dyn FileSystem>) -> (Arc<dyn FS>, Arc<BomFiles>) {
    let boms = Arc::new(BomFiles::default());
    let vfs = HostVfs {
        fs,
        boms: Arc::clone(&boms),
    };
    (polylint_tsrs::wrap_fs(vfs), boms)
}

fn read_only(path: &str) -> String {
    format!("cannot write {path}: the type-aware analysis reads files only")
}

impl FS for HostVfs {
    fn use_case_sensitive_file_names(&self) -> bool {
        // The platform's default, decided when the crate is built: a [`FileSystem`] has no case-sensitivity query, and
        // probing the disk would make the answer depend on the machine (skif ADR 0002).
        !cfg!(any(target_os = "macos", target_os = "windows"))
    }

    fn file_exists(&self, path: &str) -> bool {
        self.fs.kind(Path::new(path)).is_file
    }

    fn read_file(&self, path: &str) -> Option<String> {
        let native = Path::new(path);
        match self.fs.read_to_string(native) {
            Ok(text) => Some(match text.strip_prefix(BOM) {
                Some(rest) => {
                    self.boms.record(path);
                    rest.to_owned()
                }
                None => text.to_string(),
            }),
            // UTF-16 text, which tsrs decodes as TypeScript does; oxc parses no such file, so no position joins.
            Err(error) if error.kind() == io::ErrorKind::InvalidData => {
                self.fs.read_bytes(native).ok().map(tsrs_vfs::internal::decode_bytes)
            }
            Err(_) => None,
        }
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
        self.fs.kind(Path::new(path)).is_dir
    }

    fn get_accessible_entries(&self, path: &str) -> Entries {
        let Ok(listing) = self.fs.read_dir_shared(Path::new(path)) else {
            return Entries::default();
        };
        let mut entries = Entries::default();
        let mut symlinks = entries.symlinks.take().unwrap_or_default();
        for entry in listing.iter() {
            let Some(name) = entry.path.file_name().and_then(|name| name.to_str()) else {
                continue;
            };
            if entry.is_dir {
                entries.directories.push(name.to_owned());
            } else if entry.is_file {
                entries.files.push(name.to_owned());
            } else {
                continue;
            }
            if entry.is_symlink {
                symlinks.insert(name.to_owned());
            }
        }
        entries.symlinks = Some(symlinks);
        entries
    }

    fn stat(&self, path: &str) -> Option<FileInfo> {
        let native = Path::new(path);
        let kind = self.fs.kind(native);
        if !kind.is_file && !kind.is_dir {
            return None;
        }
        Some(FileInfo {
            name: native
                .file_name()
                .map_or_else(String::new, |name| name.to_string_lossy().into_owned()),
            // Unknown without reading the file; building a program does not use it.
            size: 0,
            mode: if kind.is_dir { FileMode::Dir } else { FileMode::None },
            mod_time: self.fs.modified(native),
        })
    }

    fn realpath(&self, path: &str) -> String {
        self.fs.canonicalize(Path::new(path)).map_or_else(
            |_| path.to_owned(),
            |real| tspath::normalize_path(&real.to_string_lossy()),
        )
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::MemoryFs;

    fn fs() -> Arc<dyn FS> {
        let memory = MemoryFs::new()
            .with_file("/ws/a.ts", "\u{feff}export class A {}")
            .with_file("/ws/b.ts", "export class B {}")
            .with_file("/ws/src/c.ts", "");
        tsrs_fs(Arc::new(memory)).0
    }

    #[test]
    fn reads_drop_the_byte_order_mark_and_record_it() {
        let memory = MemoryFs::new()
            .with_file("/ws/a.ts", "\u{feff}export class A {}")
            .with_file("/ws/b.ts", "export class B {}");
        let (fs, boms) = tsrs_fs(Arc::new(memory));
        assert_eq!(boms.offset("/ws/a.ts"), 0);
        assert_eq!(fs.read_file("/ws/a.ts").as_deref(), Some("export class A {}"));
        assert_eq!(fs.read_file("/ws/b.ts").as_deref(), Some("export class B {}"));
        assert_eq!(boms.offset("/ws/a.ts"), 3);
        assert_eq!(boms.offset("/ws/b.ts"), 0);
        assert_eq!(fs.read_file("/ws/missing.ts"), None);
    }

    #[test]
    fn lists_files_and_directories_by_name() {
        let entries = fs().get_accessible_entries("/ws");
        assert_eq!(entries.files, ["a.ts", "b.ts"]);
        assert_eq!(entries.directories, ["src"]);
        assert!(entries.symlinks.is_some_and(|links| links.is_empty()));
        assert_eq!(fs().get_accessible_entries("/missing"), Entries::default());
    }

    #[test]
    fn answers_existence_and_stat_from_the_host_file_system() {
        let fs = fs();
        assert!(fs.file_exists("/ws/a.ts") && !fs.file_exists("/ws/src"));
        assert!(fs.directory_exists("/ws/src") && !fs.directory_exists("/ws/a.ts"));
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
