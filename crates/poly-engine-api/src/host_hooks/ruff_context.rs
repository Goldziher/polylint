//! The filesystem inputs ruff's lint output depends on beyond a file's own
//! bytes, so the result cache can key on them instead of refusing to cache.
//!
//! Two things outside the file change what ruff reports for it:
//!
//! - **The package root.** `detect_package_root` walks up from the file's
//!   directory while each directory holds an `__init__.py`; the root it stops
//!   at feeds module-name rules (`N999`, `INP001`) and import sorting. Editing
//!   a sibling `__init__.py` therefore changes this file's diagnostics without
//!   touching the file: `package_root` is the per-file answer.
//! - **Which modules exist.** isort classifies `import a.b` as first-party only
//!   when `<src-root>/a/b` is a directory or `<src-root>/a/b.py[i]` is a file,
//!   so adding, removing or renaming a Python module can move an import
//!   between sections in files that never changed: [`module_tree`] is the
//!   per-run answer, a digest of every importable path under the source roots.
//!
//! `namespace-packages` is not a poly option, so the package walk passes none;
//! `namespace_packages_are_not_configurable` (in the ruff engine's tests) fails if that ever changes.
//!
//! This module deliberately does not depend on `ruff_linter`: the cache lookup must run without the
//! ruff component loaded, so the package walk is reimplemented here (it is a loop over `__init__.py`).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex, PoisonError};

/// The outermost directory at or above `dir` reached by walking up while each directory holds an
/// `__init__.py`: ruff's `detect_package_root` for a project with no `namespace-packages`, which
/// poly does not expose. `None` when `dir` itself is not a package.
fn detect_package_root(dir: &Path) -> Option<PathBuf> {
    let mut root = None;
    for ancestor in dir.ancestors() {
        if !ancestor.join("__init__.py").is_file() {
            return root;
        }
        root = Some(ancestor.to_path_buf());
    }
    root
}

/// Directories never descended into: they hold vendored or generated trees
/// that a project does not import as first-party packages, and walking them
/// would dominate the cost of the digest on a JavaScript or Rust checkout.
const SKIPPED_DIRS: [&str; 3] = ["node_modules", "target", "__pycache__"];

/// `path` as the text folded into a cache key: relative to `base` (the checkout the run started
/// in) when it lies beneath it, so two worktrees of one repository key alike; the full path
/// otherwise, which can never collide with a relative one since it starts at the filesystem root.
fn checkout_relative(path: &Path, base: &Path) -> String {
    match path.strip_prefix(base) {
        Ok(rest) if rest.as_os_str().is_empty() => ".".to_owned(),
        Ok(rest) => rest.to_string_lossy().into_owned(),
        Err(_) => path.to_string_lossy().into_owned(),
    }
}

/// The package-root text folded into the cache key: `-` when `root` is `None`.
fn package_root_text(root: Option<&Path>, base: &Path) -> String {
    root.map_or_else(|| "-".to_owned(), |root| checkout_relative(root, base))
}

/// The package root `detect_package_root` finds for `path`'s directory, as the
/// text folded into the cache key.
#[cfg(test)]
pub(super) fn package_root(path: &Path, base: &Path) -> String {
    package_root_text(path.parent().and_then(detect_package_root).as_deref(), base)
}

/// Package roots already found, keyed by the directory they were found for. A
/// run asks for the same few directories once per file, twice over (cache key,
/// then lint), and every ask is a walk up the tree.
#[derive(Default)]
struct PackageRoots(Mutex<HashMap<PathBuf, Option<PathBuf>>>);

impl PackageRoots {
    fn clear(&self) {
        self.0.lock().unwrap_or_else(PoisonError::into_inner).clear();
    }

    fn get(&self, dir: &Path) -> Option<PathBuf> {
        if let Some(hit) = self.0.lock().unwrap_or_else(PoisonError::into_inner).get(dir) {
            return hit.clone();
        }
        let found = detect_package_root(dir);
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(dir.to_path_buf(), found.clone());
        found
    }
}

static PACKAGE_ROOTS: LazyLock<PackageRoots> = LazyLock::new(PackageRoots::default);

/// The run's checkout root (the current directory), read once per plan rather than per file.
static RUN_BASE: Mutex<Option<PathBuf>> = Mutex::new(None);

/// Drop every remembered package root and the remembered checkout root. Called when a run is
/// planned, so an `__init__.py` added or removed, or a changed current directory, between runs of
/// a long-lived process is seen.
pub fn forget_package_roots() {
    PACKAGE_ROOTS.clear();
    *RUN_BASE.lock().unwrap_or_else(PoisonError::into_inner) = None;
}

/// The package root of `dir`, remembered until the next plan.
pub fn cached_package_root(dir: &Path) -> Option<PathBuf> {
    PACKAGE_ROOTS.get(dir)
}

/// The package root of `path`, answered from the run's remembered roots and keyed relative to the
/// checkout so another worktree of the repository shares the result.
pub fn run_package_root(path: &Path) -> String {
    let root = path.parent().and_then(cached_package_root);
    let mut base = RUN_BASE.lock().unwrap_or_else(PoisonError::into_inner);
    let base = base.get_or_insert_with(|| std::env::current_dir().unwrap_or_default());
    package_root_text(root.as_deref(), base)
}

/// A digest of the module tree isort probes: the resolved source roots (ruff
/// defaults them to the current directory and its `src`) and every directory and
/// `.py`/`.pyi` path beneath them. Roots are keyed relative to the current
/// directory so identical trees in two worktrees digest alike.
pub fn module_tree(src_roots: &[PathBuf]) -> String {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    module_tree_at(&cwd, src_roots)
}

fn module_tree_at(cwd: &Path, src_roots: &[PathBuf]) -> String {
    let roots: Vec<PathBuf> = if src_roots.is_empty() {
        vec![cwd.to_path_buf(), cwd.join("src")]
    } else {
        src_roots.iter().map(|root| cwd.join(root)).collect()
    };
    let mut hasher = blake3::Hasher::new();
    for root in &roots {
        hasher.update(b"\0root\0");
        hasher.update(checkout_relative(root, cwd).as_bytes());
        let mut entries = walk(root);
        entries.sort_unstable();
        for entry in entries {
            hasher.update(entry.as_bytes());
            hasher.update(b"\n");
        }
    }
    hasher.finalize().to_hex().to_string()
}

fn is_identifier(name: &str) -> bool {
    let mut chars = name.chars();
    chars.next().is_some_and(|first| first == '_' || first.is_alphabetic())
        && chars.all(|c| c == '_' || c.is_alphanumeric())
}

fn walk(root: &Path) -> Vec<String> {
    let mut found = Vec::new();
    let mut pending = vec![(root.to_path_buf(), String::new())];
    while let Some((dir, prefix)) = pending.pop() {
        let Ok(read) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in read.flatten() {
            let name = entry.file_name();
            let Some(name) = name.to_str() else {
                continue;
            };
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            let is_dir = file_type.is_dir() || (file_type.is_symlink() && entry.path().is_dir());
            let relative = format!("{prefix}{name}");
            if is_dir {
                if is_identifier(name) && !SKIPPED_DIRS.contains(&name) {
                    found.push(format!("d:{relative}"));
                    if file_type.is_dir() {
                        pending.push((entry.path(), format!("{relative}/")));
                    }
                }
            } else if name.ends_with(".py") || name.ends_with(".pyi") {
                found.push(format!("f:{relative}"));
            }
        }
    }
    found
}

#[cfg(test)]
#[path = "ruff_context_tests.rs"]
mod tests;
