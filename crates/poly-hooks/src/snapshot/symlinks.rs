//! Removing materialized symlinks that leave the snapshot.

use std::path::Path;

use tracing::warn;

use crate::git;

/// Remove every materialized symlink whose target does not resolve inside the
/// snapshot (see the module docs). Runs on every refresh, not just on the run
/// that materialized the entry, so a link that survived an earlier poly is
/// cleaned up too.
pub(super) fn sanitize_symlinks(dir: &Path, staged: &[git::StagedEntry]) {
    let Ok(canonical_dir) = std::fs::canonicalize(dir) else {
        return;
    };
    for entry in staged.iter().filter(|entry| entry.is_symlink) {
        let link = dir.join(&entry.path);
        let Ok(target) = std::fs::read_link(&link) else {
            continue;
        };
        if link_stays_inside(dir, &canonical_dir, &entry.path, &target) {
            continue;
        }
        warn!(
            path = %entry.path.display(),
            target = %target.display(),
            "removing a staged symlink whose target escapes the snapshot"
        );
        // `remove_file` unlinks the symlink itself, never its target.
        let _ = std::fs::remove_file(&link);
    }
}

/// Whether the materialized link at `dir/link_path` stays inside the snapshot.
///
/// The kernel resolves `..` physically, after following any symlink earlier in
/// the path, so a lexical answer alone is bypassable by a chain of in-tree links
/// (`d/e -> ..`, then `t -> d/e/d/e/../../../..`). The resolved location is
/// therefore checked physically. A link that does not resolve (dangling) cannot
/// be judged that way, so it is kept only when its lexical reading is
/// trustworthy: no symlinked parent directory and no `..` after a named
/// component.
pub(super) fn link_stays_inside(dir: &Path, canonical_dir: &Path, link_path: &Path, target: &Path) -> bool {
    use std::path::Component;

    if !resolves_inside_tree(link_path, target) {
        return false;
    }
    if let Ok(resolved) = std::fs::canonicalize(dir.join(link_path)) {
        return resolved.starts_with(canonical_dir);
    }
    let mut seen_normal = false;
    for component in target.components() {
        match component {
            Component::Normal(_) => seen_normal = true,
            Component::ParentDir if seen_normal => return false,
            _ => {}
        }
    }
    !link_path
        .ancestors()
        .skip(1)
        .filter(|ancestor| !ancestor.as_os_str().is_empty())
        .any(|ancestor| std::fs::symlink_metadata(dir.join(ancestor)).is_ok_and(|meta| meta.file_type().is_symlink()))
}

/// Whether a symlink staged at `link_path` (repo-relative) with content
/// `target` resolves to somewhere inside the tree it was materialized into.
///
/// Purely lexical, so only a cheap first filter: it cannot see a symlink earlier
/// in the path, which [`link_stays_inside`] checks physically. An absolute
/// target — or one whose `..` components climb past the root — escapes.
pub(super) fn resolves_inside_tree(link_path: &Path, target: &Path) -> bool {
    use std::path::Component;

    let mut depth = link_path.components().count().saturating_sub(1);
    for component in target.components() {
        match component {
            Component::Normal(_) => depth += 1,
            Component::CurDir => {}
            Component::ParentDir => {
                if depth == 0 {
                    return false;
                }
                depth -= 1;
            }
            Component::RootDir | Component::Prefix(_) => return false,
        }
    }
    true
}

#[cfg(all(test, unix))]
mod tests;
