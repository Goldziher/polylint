//! Whether a `.clang-format` governs a file, so the generic reindenter can stand aside.
//!
//! The tier-2 reindenter knows nothing about clang-format's style options, so on a project that
//! carries one it rewrites C-family files into a layout the project's real formatter then
//! rejects. clang-format finds its style by walking up from the file, so this does the same;
//! the answer is memoised per directory so siblings share one walk.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{OnceLock, RwLock};

use crate::language::Language;

/// Reported for each file the generic formatter leaves to clang-format.
pub(super) const SKIP: &str =
    "formatted by clang-format (.clang-format present); enable [tools.clang-format] to format it";

/// Languages clang-format takes by default (its catalog recipe's `default_languages`), by [`Language::id`] of the
/// tier-2 routing. Java, JavaScript and protobuf reach it only through `[tools.clang-format] files`, so a
/// `.clang-format` alone does not take them from this formatter.
pub(super) fn is_clang_language(language: &Language) -> bool {
    matches!(language, Language::C | Language::Cpp)
        || matches!(language, Language::Other(id) if matches!(id.as_str(), "objc" | "objectivec" | "objective-c"))
}

fn memo() -> &'static RwLock<HashMap<PathBuf, bool>> {
    static MEMO: OnceLock<RwLock<HashMap<PathBuf, bool>>> = OnceLock::new();
    MEMO.get_or_init(RwLock::default)
}

/// Drops every memoised answer so the next run rereads the filesystem.
pub(crate) fn forget() {
    memo()
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clear();
}

fn has_own(dir: &Path) -> bool {
    dir.join(".clang-format").is_file() || dir.join("_clang-format").is_file()
}

fn is_repo_root(dir: &Path) -> bool {
    dir.join(".git").exists()
}

/// Whether a `.clang-format` or `_clang-format` applies to files in `dir`: one in `dir` itself or
/// in an ancestor, the search ending at the repository root.
fn governed_dir(dir: &Path) -> bool {
    let mut walked: Vec<&Path> = Vec::new();
    let mut answer = false;
    for ancestor in dir.ancestors() {
        if let Some(&known) = memo()
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(ancestor)
        {
            answer = known;
            break;
        }
        walked.push(ancestor);
        if has_own(ancestor) {
            answer = true;
            break;
        }
        if is_repo_root(ancestor) {
            break;
        }
    }
    let mut guard = memo().write().unwrap_or_else(std::sync::PoisonError::into_inner);
    for dir in walked {
        guard.insert(dir.to_path_buf(), answer);
    }
    answer
}

/// Whether a clang-format style file governs `path`.
pub(super) fn governs(path: &Path) -> bool {
    let Ok(absolute) = std::path::absolute(path) else {
        return false;
    };
    absolute.parent().is_some_and(governed_dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo(files: &[&str]) -> tempfile::TempDir {
        let repo = tempfile::tempdir().unwrap();
        for name in files {
            let path = repo.path().join(name);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, "").unwrap();
        }
        repo
    }

    #[test]
    fn a_style_file_above_governs_nested_sources() {
        let repo = repo(&[".clang-format", "src/deep/a.c"]);
        assert!(governs(&repo.path().join("src/deep/a.c")));
    }

    #[test]
    fn the_underscore_spelling_governs_too() {
        let repo = repo(&["_clang-format", "a.h"]);
        assert!(governs(&repo.path().join("a.h")));
    }

    #[test]
    fn no_style_file_governs_nothing() {
        let repo = repo(&["src/a.c"]);
        assert!(!governs(&repo.path().join("src/a.c")));
    }

    #[test]
    fn the_search_stops_at_the_repository_root() {
        let repo = repo(&[".clang-format", "inner/.git/HEAD", "inner/a.c"]);
        assert!(!governs(&repo.path().join("inner/a.c")));
    }
}
