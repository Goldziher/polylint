//! What a run was asked to look at, for an engine whose project check reads a directory tree of its
//! own (see [`ProjectAnalysis::project_inputs`](crate::engine::ProjectAnalysis::project_inputs)).
//!
//! Such an engine judges the whole tree whatever paths the run named, because a verdict over one
//! file of a log is not the verdict over the log. The run's paths then decide only which of its
//! findings are reported, so `poly lint docs/adr/0003-x.md` reports on that record and not on its
//! siblings, and a run that names nothing near the tree does not read it at all. The engine is told
//! which files those are ([`RunScope::targets`]) and may compute findings for them alone.

use std::path::{Path, PathBuf};

use rustc_hash::{FxHashMap, FxHashSet};

use crate::discover::DiscoveredFile;
use crate::engine::{Diagnostic, ProjectFile};

/// The paths a run named: directories (everything beneath them is in scope) and files.
pub(super) struct RunScope {
    /// Canonical path and the path as the run spelled it, in the order the run named them.
    dirs: Vec<(PathBuf, PathBuf)>,
    /// Canonical path to the path as the run spelled it, which is how the per-file results key it.
    files: FxHashMap<PathBuf, PathBuf>,
}

/// `path` with symlinks resolved. A path that does not exist (a record the lock names but the log
/// lost) resolves through its parent, so it still compares equal to the same path spelled another way.
pub(super) fn canonical(path: &Path) -> PathBuf {
    std::fs::canonicalize(path)
        .or_else(|error| match (path.parent(), path.file_name()) {
            (Some(parent), Some(name)) => std::fs::canonicalize(parent).map(|parent| parent.join(name)),
            _ => Err(error),
        })
        .unwrap_or_else(|_| path.to_path_buf())
}

/// The canonical paths among `files` that `reported` names: what discovery kept, out of the files
/// a whole-tree check was handed. Only files sharing a name with a finding are resolved, since
/// canonicalizing every discovered file to answer for a handful of findings would cost a syscall each.
pub(super) fn covered(files: &[DiscoveredFile], reported: &[(PathBuf, Vec<Diagnostic>)]) -> FxHashSet<PathBuf> {
    let names: FxHashSet<&std::ffi::OsStr> = reported.iter().filter_map(|(path, _)| path.file_name()).collect();
    files
        .iter()
        .filter(|file| file.path.file_name().is_some_and(|name| names.contains(name)))
        .map(|file| canonical(&file.path))
        .collect()
}

/// Whether any of `read` is one of the `rewritten` files (canonical paths). Only files sharing a name
/// with a rewritten one are resolved, for the reason [`covered`] gives.
pub(super) fn reads_any(read: &[PathBuf], rewritten: &FxHashSet<PathBuf>) -> bool {
    let names: FxHashSet<&std::ffi::OsStr> = rewritten.iter().filter_map(|path| path.file_name()).collect();
    read.iter()
        .filter(|path| path.file_name().is_some_and(|name| names.contains(name)))
        .any(|path| rewritten.contains(&canonical(path)))
}

impl RunScope {
    pub(super) fn new(paths: &[PathBuf]) -> Self {
        let mut scope = Self {
            dirs: Vec::new(),
            files: FxHashMap::default(),
        };
        for path in paths {
            let resolved = canonical(path);
            if path.is_dir() {
                scope.dirs.push((resolved, path.clone()));
            } else {
                scope.files.insert(resolved, path.clone());
            }
        }
        scope
    }

    /// Whether anything the run named is, or contains, something under `dir`.
    pub(super) fn reaches(&self, dir: &Path) -> bool {
        let dir = canonical(dir);
        self.dirs
            .iter()
            .any(|(named, _)| named.starts_with(&dir) || dir.starts_with(named))
            || self.files.keys().any(|file| file.starts_with(&dir))
    }

    /// The files among `loaded` whose findings the run reports, for a check over the directories `dirs`: the
    /// request's [`targets`](crate::engine::ProjectRequest::targets). `None` when the run reports on every one
    /// of them, because it named each directory or one above it (`poly lint .`) or named every file.
    ///
    /// A run naming only files, as a pre-commit hook does, resolves only the loaded files that share a name with
    /// one of them, for the reason [`covered`] gives.
    pub(super) fn targets(
        &self,
        mut dirs: impl Iterator<Item = PathBuf>,
        loaded: &[ProjectFile],
    ) -> Option<Vec<PathBuf>> {
        if dirs.all(|dir| {
            let dir = canonical(&dir);
            self.dirs.iter().any(|(named, _)| dir.starts_with(named))
        }) {
            return None;
        }
        let names: FxHashSet<&std::ffi::OsStr> = self.files.keys().filter_map(|path| path.file_name()).collect();
        // With a directory named every loaded file is a candidate. A loaded file is a regular file (the walk
        // yields no symlink), so it resolves as its directory does: one resolution per directory, not per file.
        let mut parents: FxHashMap<&Path, PathBuf> = FxHashMap::default();
        let targets: Vec<PathBuf> = loaded
            .iter()
            .filter(|file| {
                let Some(name) = file.path.file_name() else {
                    return false;
                };
                if self.dirs.is_empty() {
                    return names.contains(name) && self.admit(&file.path).is_some();
                }
                let resolved = match file.path.parent().filter(|parent| !parent.as_os_str().is_empty()) {
                    Some(parent) => parents.entry(parent).or_insert_with(|| canonical(parent)).join(name),
                    None => canonical(&file.path),
                };
                self.spelling(&resolved).is_some()
            })
            .map(|file| file.path.clone())
            .collect();
        (targets.len() < loaded.len()).then_some(targets)
    }

    /// The path to report `finding` under, or `None` when the run did not name it or a directory
    /// above it. It is spelled as the run spelled it: the named file, or the named directory joined
    /// with the rest, which is how discovery spells a file it walked to, so the finding joins that
    /// file's per-file result instead of standing beside it under another spelling.
    pub(super) fn admit(&self, finding: &Path) -> Option<PathBuf> {
        self.spelling(&canonical(finding))
    }

    /// [`admit`](Self::admit) for a path that is already canonical.
    fn spelling(&self, resolved: &Path) -> Option<PathBuf> {
        if let Some(spelled) = self.files.get(resolved) {
            return Some(spelled.clone());
        }
        self.dirs
            .iter()
            .find_map(|(named, spelled)| resolved.strip_prefix(named).ok().map(|rest| spelled.join(rest)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_named_file_is_admitted_under_the_spelling_the_run_used() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("log/0001.md");
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, "x").unwrap();
        let sibling = dir.path().join("log/0002.md");
        std::fs::write(&sibling, "x").unwrap();
        let roundabout = dir.path().join("log/../log/0001.md");
        let scope = RunScope::new(std::slice::from_ref(&roundabout));
        assert_eq!(scope.admit(&file), Some(roundabout));
        assert_eq!(scope.admit(&sibling), None);
    }

    #[test]
    fn a_named_directory_admits_everything_beneath_it_even_a_missing_file() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("log")).unwrap();
        let scope = RunScope::new(&[dir.path().join("log")]);
        let gone = dir.path().join("log/0009-gone.md");
        assert_eq!(scope.admit(&gone), Some(gone.clone()));
        assert_eq!(scope.admit(&dir.path().join("elsewhere/x.md")), None);
    }

    #[test]
    fn a_finding_under_a_named_directory_takes_the_spelling_the_run_gave_the_directory() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("log")).unwrap();
        std::fs::write(dir.path().join("log/0001.md"), "x").unwrap();
        let roundabout = dir.path().join("log/../log");
        let scope = RunScope::new(std::slice::from_ref(&roundabout));
        assert_eq!(
            scope.admit(&dir.path().join("./log/0001.md")),
            Some(roundabout.join("0001.md"))
        );
    }

    fn loaded(paths: &[PathBuf]) -> Vec<ProjectFile> {
        paths
            .iter()
            .map(|path| ProjectFile {
                path: path.clone(),
                language: crate::language::Language::TypeScript,
                content: "".into(),
            })
            .collect()
    }

    #[test]
    fn targets_are_the_named_files_and_none_when_the_run_covers_the_tree() {
        let dir = tempfile::tempdir().unwrap();
        let (src, other) = (dir.path().join("src"), dir.path().join("other"));
        for sub in [&src, &other] {
            std::fs::create_dir_all(sub).unwrap();
        }
        let (a, b, c) = (src.join("a.ts"), src.join("b.ts"), other.join("a.ts"));
        for file in [&a, &b, &c] {
            std::fs::write(file, "x").unwrap();
        }
        let files = loaded(&[a.clone(), b.clone(), c.clone()]);
        let tree = || std::iter::once(dir.path().to_path_buf());
        let targets = |paths: &[PathBuf]| RunScope::new(paths).targets(tree(), &files);

        assert_eq!(targets(&[dir.path().to_path_buf()]), None, "the run names the tree");
        assert_eq!(
            targets(std::slice::from_ref(&a)),
            Some(vec![a.clone()]),
            "a staged file, not its namesake"
        );
        assert_eq!(targets(std::slice::from_ref(&src)), Some(vec![a.clone(), b.clone()]));
        assert_eq!(
            targets(&[dir.path().join("other/../src"), c.clone()]),
            None,
            "a directory spelled another way and a file beside it name every file"
        );
        assert_eq!(
            targets(&[dir.path().join("src/../other")]),
            Some(vec![c.clone()]),
            "a directory spelled another way"
        );
        assert_eq!(targets(&[a.clone(), b.clone(), c.clone()]), None, "every file is named");
        let parent = dir.path().parent().unwrap().to_path_buf();
        assert_eq!(
            RunScope::new(&[parent]).targets(tree(), &files),
            None,
            "a directory above the tree"
        );
    }

    #[test]
    fn reaches_means_the_run_is_in_the_directory_or_above_it() {
        let dir = tempfile::tempdir().unwrap();
        for sub in ["log", "other"] {
            std::fs::create_dir_all(dir.path().join(sub)).unwrap();
        }
        let log = dir.path().join("log");
        assert!(RunScope::new(&[dir.path().to_path_buf()]).reaches(&log));
        assert!(RunScope::new(std::slice::from_ref(&log)).reaches(&log));
        assert!(!RunScope::new(&[dir.path().join("other")]).reaches(&log));
        std::fs::write(log.join("a.md"), "x").unwrap();
        assert!(RunScope::new(&[log.join("a.md")]).reaches(&log));
        assert!(!RunScope::new(&[]).reaches(&log));
    }
}
