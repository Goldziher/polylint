//! The directory a run is rooted at, for the code below the runner that would
//! otherwise read the process working directory.
//!
//! A `poly lint` / `poly fmt` child resolves everything relative to the directory
//! it was started in. A run inside a long-lived process (the `poly hooks` runner
//! judging a staged snapshot, ADR 0073) cannot change the process working
//! directory, because hooks run concurrently. The runner therefore enters the run's
//! root on every thread that executes the run, and the working-directory reads on
//! the per-file path ask [`current_dir`] instead of [`std::env::current_dir`].
//!
//! The root travels as a thread-local rather than an argument because the
//! [`Engine`](crate::engine::Engine) methods carry no run context, and adding one
//! would change every engine, every test that builds a `SourceFile` or an
//! `EngineConfig`, and the component wire protocol. Outside a rooted run nothing is
//! entered and every helper answers exactly what it did before: the process working
//! directory, and paths unchanged.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::sync::Arc;

thread_local! {
    static ROOT: RefCell<Option<Arc<Path>>> = const { RefCell::new(None) };
}

/// Restores the previously entered root when dropped; see [`enter`].
#[must_use = "the root is left as soon as the guard is dropped"]
pub struct Entered {
    previous: Option<Arc<Path>>,
}

impl Drop for Entered {
    fn drop(&mut self) {
        let previous = self.previous.take();
        ROOT.with(|root| *root.borrow_mut() = previous);
    }
}

/// Make `root` the run root of the calling thread until the returned guard is
/// dropped. `None` leaves the thread unrooted for that span.
pub fn enter(root: Option<Arc<Path>>) -> Entered {
    let previous = ROOT.with(|slot| slot.replace(root));
    Entered { previous }
}

/// Make `root` the run root of the calling thread for the rest of its life: for
/// the start handler of a pool that runs nothing but one rooted run.
pub fn pin(root: Arc<Path>) {
    ROOT.with(|slot| *slot.borrow_mut() = Some(root));
}

/// The run root entered on the calling thread, if any.
#[must_use]
pub fn get() -> Option<Arc<Path>> {
    ROOT.with(|root| root.borrow().clone())
}

/// Call `f` with the run root entered on the calling thread, without cloning it.
pub fn with<R>(f: impl FnOnce(Option<&Path>) -> R) -> R {
    ROOT.with(|root| f(root.borrow().as_deref()))
}

/// The directory a run's relative paths resolve against: the entered run root,
/// else the process working directory.
///
/// # Errors
///
/// When no root is entered and the process working directory cannot be read.
pub fn current_dir() -> std::io::Result<PathBuf> {
    match get() {
        Some(root) => Ok(root.to_path_buf()),
        None => std::env::current_dir(),
    }
}

/// `path` as the child run would have spelled it: relative to the entered run
/// root when it lies under it, else unchanged. Borrows, so it costs no allocation
/// on the per-file path; outside a rooted run it is the identity.
#[must_use]
pub fn relative(path: &Path) -> &Path {
    ROOT.with(|root| match root.borrow().as_deref() {
        Some(root) => path.strip_prefix(root).unwrap_or(path),
        None => path,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unrooted_thread_answers_the_process_directory_and_leaves_paths_alone() {
        assert!(get().is_none());
        assert_eq!(current_dir().unwrap(), std::env::current_dir().unwrap());
        assert_eq!(relative(Path::new("/a/b.py")), Path::new("/a/b.py"));
    }

    #[test]
    fn an_entered_root_anchors_and_relativizes_until_the_guard_drops() {
        let root: Arc<Path> = Arc::from(Path::new("/snapshot"));
        {
            let _entered = enter(Some(Arc::clone(&root)));
            assert_eq!(current_dir().unwrap(), PathBuf::from("/snapshot"));
            assert_eq!(relative(Path::new("/snapshot/src/a.py")), Path::new("src/a.py"));
            assert_eq!(relative(Path::new("/elsewhere/a.py")), Path::new("/elsewhere/a.py"));
            {
                let _inner = enter(None);
                assert!(get().is_none());
            }
            assert_eq!(get().as_deref(), Some(Path::new("/snapshot")));
        }
        assert!(get().is_none());
    }
}
