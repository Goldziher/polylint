//! Registry of poly's own scratch directories, so a signal handler can remove
//! them: `Drop` never runs when the process is interrupted.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

static PATHS: Mutex<BTreeSet<PathBuf>> = Mutex::new(BTreeSet::new());

fn paths() -> MutexGuard<'static, BTreeSet<PathBuf>> {
    PATHS.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Record `path` for removal by [`cleanup_all`].
pub fn register(path: &Path) {
    paths().insert(path.to_path_buf());
}

/// Forget `path`; call when its owner removes it normally.
pub fn unregister(path: &Path) {
    paths().remove(path);
}

/// Remove every registered path, directory or file. Best effort.
pub fn cleanup_all() {
    remove_all(std::mem::take(&mut *paths()));
}

fn remove_all(taken: BTreeSet<PathBuf>) {
    for path in taken {
        if path.is_dir() {
            let _ = std::fs::remove_dir_all(&path);
        } else {
            let _ = std::fs::remove_file(&path);
        }
    }
}

/// Keeps a path registered until dropped; for a temp file whose owner removes it.
pub struct Registration(PathBuf);

impl Registration {
    /// Register `path` for the lifetime of the guard.
    pub fn new(path: &Path) -> Self {
        register(path);
        Self(path.to_path_buf())
    }
}

impl Drop for Registration {
    fn drop(&mut self) {
        unregister(&self.0);
    }
}

/// A [`tempfile::TempDir`] that is registered for signal cleanup while it lives.
pub struct RegisteredTempDir {
    dir: tempfile::TempDir,
}

impl RegisteredTempDir {
    /// Create a directory whose name starts with `prefix`.
    pub fn new(prefix: &str) -> std::io::Result<Self> {
        let dir = tempfile::Builder::new().prefix(prefix).tempdir()?;
        register(dir.path());
        Ok(Self { dir })
    }

    /// The directory's path.
    pub fn path(&self) -> &Path {
        self.dir.path()
    }
}

impl Drop for RegisteredTempDir {
    fn drop(&mut self) {
        unregister(self.dir.path());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn removal_deletes_directories_and_files_and_drop_unregisters() {
        let dir = tempfile::tempdir().expect("dir");
        let nested = dir.path().join("scratch");
        std::fs::create_dir(&nested).expect("mkdir");
        std::fs::write(nested.join("a"), "x").expect("write");
        let file = dir.path().join("loose");
        std::fs::write(&file, "x").expect("write");
        remove_all(BTreeSet::from([nested.clone(), file.clone()]));
        assert!(!nested.exists() && !file.exists());

        let kept = RegisteredTempDir::new("poly-tempreg-test-").expect("dir");
        let path = kept.path().to_path_buf();
        assert!(paths().contains(&path));
        drop(kept);
        assert!(!paths().contains(&path));
    }
}
