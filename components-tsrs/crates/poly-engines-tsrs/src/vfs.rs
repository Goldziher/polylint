//! The names tsrs knows the request's files by. The file system itself, an in-memory tree of exactly the files the
//! host handed over, is `polylint_tsrs::MemoryFs`.

use std::path::Path;

use tsrs_core::tspath;

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
