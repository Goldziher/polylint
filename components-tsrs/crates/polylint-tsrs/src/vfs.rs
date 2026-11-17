//! The file system tsrs reads through: the host's own, so configs and sources come from the same view as the rest of
//! its analysis (an embedder's buffers included), wrapped as tsrs wraps a real one (bundled `lib.*.d.ts` files and
//! cached lookups), with the record of the files that start with a byte order mark.

use std::collections::HashSet;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, PoisonError, RwLock};

use tsrs_core::tspath;
use tsrs_vfs::{FS, bundled, cachedvfs};

/// The UTF-8 length of a byte order mark.
const BOM_LEN: u32 = 3;

/// The files whose text starts with a byte order mark. tsrs drops the mark when it reads a file, as TypeScript does;
/// a host whose own positions count the mark finds a position of tsrs in such a file [`BomFiles::offset`] bytes
/// further in its text. A host's file system records the files as it reads them.
#[derive(Debug, Default)]
pub struct BomFiles {
    any: AtomicBool,
    files: RwLock<HashSet<String>>,
}

impl BomFiles {
    /// Records that the text of `file_name` starts with a byte order mark: read through the host's file system, or
    /// read through another analysis's and shared with this one.
    pub fn record(&self, file_name: &str) {
        self.files
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(file_name.to_owned());
        self.any.store(true, Ordering::Release);
    }

    /// The bytes the text of `file_name` has before tsrs's position 0.
    #[must_use]
    pub fn offset(&self, file_name: &str) -> u32 {
        if !self.any.load(Ordering::Acquire) {
            return 0;
        }
        let files = self.files.read().unwrap_or_else(PoisonError::into_inner);
        if files.contains(file_name) { BOM_LEN } else { 0 }
    }
}

/// A host's file system as programs read through it: with the bundled `lib.*.d.ts` files and cached lookups, as tsrs
/// wraps a real one. The host's `FS` answers from whatever holds its files (a disk, an editor's buffers, a request's
/// files in memory), drops a leading byte order mark from the text it returns and records the file in a [`BomFiles`]
/// if its own positions count the mark.
#[must_use]
pub fn wrap_fs(fs: impl FS + 'static) -> Arc<dyn FS> {
    Arc::new(bundled::wrap_fs(cachedvfs::from(fs)))
}

/// The directory of the `lib.*.d.ts` files embedded in tsrs. Passed explicitly: `bundled::default_lib_path` reads
/// `TSRS_LIB_PATH` (polylint ADR 0078).
#[must_use]
pub fn lib_path() -> String {
    bundled::lib_path()
}

/// `path` as tsrs names files: forward slashes, normalized.
#[must_use]
pub fn tsrs_path(path: &Path) -> String {
    tspath::normalize_path(&path.to_string_lossy())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::memory::MemoryFs;

    #[test]
    fn serves_the_bundled_libraries_beside_the_host_files() {
        let (fs, _) = MemoryFs::new().with_file("/ws/b.ts", "export class B {}").into_fs();
        let lib = format!("{}/lib.es5.d.ts", lib_path());
        assert!(fs.file_exists(&lib));
        assert!(
            fs.read_file(&lib)
                .is_some_and(|text| text.contains("interface Array<T>"))
        );
        assert_eq!(fs.read_file("/ws/b.ts").as_deref(), Some("export class B {}"));
    }

    #[test]
    fn a_recorded_file_is_offset_by_its_byte_order_mark() {
        let boms = BomFiles::default();
        assert_eq!(boms.offset("/ws/a.ts"), 0);
        boms.record("/ws/a.ts");
        assert_eq!(boms.offset("/ws/a.ts"), BOM_LEN);
        assert_eq!(boms.offset("/ws/b.ts"), 0);
    }

    #[test]
    fn tsrs_paths_use_forward_slashes() {
        assert_eq!(tsrs_path(Path::new("/ws/a/../b.ts")), "/ws/b.ts");
        assert_eq!(tsrs_path(Path::new("/ws/src/x.ts")), "/ws/src/x.ts");
    }
}
