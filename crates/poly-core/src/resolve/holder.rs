//! Which registered config directory holds a path, whatever spelling either one was given in.
//!
//! A run names paths as its caller typed them (`docs/adr`, `./docs/adr`, an absolute path), and a
//! config directory is spelled after the path whose ancestor chain found it: `.` for the working
//! directory, reached from a relative path. Compared as written, `docs/adr/x.md` is not under `.`,
//! so the file fell back to config 0 while `./docs/adr/x.md` took the registered config, and the
//! project root of a project check moved with the spelling. The directories are therefore matched by
//! identity: relative to the working directory when both sides sit under it, which costs no syscall,
//! and by canonical path otherwise.

use std::cmp::Reverse;
use std::path::{Component, Path, PathBuf};

use super::canon::Canon;

/// The registered config directories, nearest (deepest) first.
#[derive(Default)]
pub(super) struct Holders {
    entries: Vec<Holder>,
}

struct Holder {
    id: usize,
    /// The directory as registered, which is how paths found by walking from it are spelled.
    dir: PathBuf,
    canonical: Option<PathBuf>,
    /// The directory relative to the working directory, empty for the working directory itself;
    /// `None` when it is not under it.
    below_cwd: Option<PathBuf>,
}

impl Holders {
    pub(super) fn new(lookup: &[(PathBuf, usize)], canon: &Canon) -> Self {
        let cwd = canon.canonicalize(Path::new("."));
        let mut entries: Vec<Holder> = lookup
            .iter()
            .map(|(dir, id)| {
                let canonical = canon.canonicalize(dir);
                let below_cwd = canonical
                    .as_deref()
                    .zip(cwd.as_deref())
                    .and_then(|(dir, cwd)| dir.strip_prefix(cwd).ok())
                    .map(Path::to_path_buf);
                Holder {
                    id: *id,
                    dir: dir.clone(),
                    canonical,
                    below_cwd,
                }
            })
            .collect();
        entries.sort_by_key(|entry| Reverse(entry.canonical.as_deref().unwrap_or(&entry.dir).components().count()));
        Self { entries }
    }

    /// The id of the nearest registered config directory holding `dir`.
    pub(super) fn holding(&self, dir: &Path, canon: &Canon) -> Option<usize> {
        if self.entries.is_empty() {
            return None;
        }
        let relative = below_cwd(dir);
        let lexical = self.entries.iter().find(|entry| {
            dir.starts_with(&entry.dir)
                || match relative {
                    Some(relative) => entry
                        .below_cwd
                        .as_deref()
                        .is_some_and(|held| relative.starts_with(held)),
                    None => entry.canonical.as_deref().is_some_and(|held| dir.starts_with(held)),
                }
        });
        if let Some(entry) = lexical {
            return Some(entry.id);
        }
        // Spelled through a symlink, or relative to a working directory below the config: one
        // memoized `realpath` per directory nothing above matched.
        let resolved = canon.canonicalize(dir)?;
        self.entries
            .iter()
            .find(|entry| {
                entry
                    .canonical
                    .as_deref()
                    .is_some_and(|held| resolved.starts_with(held))
            })
            .map(|entry| entry.id)
    }
}

/// `dir` without a leading `.`, when it is a relative path that does not climb out of the working
/// directory: the form [`Holder::below_cwd`] is compared with.
fn below_cwd(dir: &Path) -> Option<&Path> {
    let dir = dir.strip_prefix(".").unwrap_or(dir);
    match dir.components().next() {
        None | Some(Component::Normal(_)) => Some(dir),
        Some(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_relative_path_is_recognised_with_or_without_the_dot() {
        assert_eq!(below_cwd(Path::new("docs/adr")), Some(Path::new("docs/adr")));
        assert_eq!(below_cwd(Path::new("./docs/adr")), Some(Path::new("docs/adr")));
        assert_eq!(below_cwd(Path::new(".")), Some(Path::new("")));
        assert_eq!(below_cwd(Path::new("")), Some(Path::new("")));
        assert_eq!(below_cwd(Path::new("../docs")), None);
        assert_eq!(below_cwd(Path::new("/docs")), None);
    }

    #[test]
    fn the_working_directory_holds_every_spelling_of_a_path_under_it() {
        let canon = Canon::default();
        let cwd = std::env::current_dir().unwrap();
        let holders = Holders::new(&[(PathBuf::from("."), 1)], &canon);
        for spelled in ["src", "./src", "src/deep/er", ""] {
            assert_eq!(holders.holding(Path::new(spelled), &canon), Some(1), "{spelled:?}");
        }
        assert_eq!(holders.holding(&cwd.join("src"), &canon), Some(1), "absolute");
        let outside = tempfile::tempdir().unwrap();
        assert_eq!(holders.holding(outside.path(), &canon), None);
    }

    #[test]
    fn the_nearest_directory_wins_across_spellings() {
        let canon = Canon::default();
        // `src` exists in the crate directory, where cargo runs the tests.
        let cwd = std::env::current_dir().unwrap();
        let holders = Holders::new(&[(PathBuf::from("."), 1), (PathBuf::from("src"), 2)], &canon);
        assert_eq!(holders.holding(Path::new("./src/runner"), &canon), Some(2));
        assert_eq!(holders.holding(&cwd.join("src/runner"), &canon), Some(2));
        assert_eq!(holders.holding(Path::new("tests"), &canon), Some(1));
        let absolute = Holders::new(&[(cwd.clone(), 1), (cwd.join("src"), 2)], &canon);
        assert_eq!(absolute.holding(Path::new("src/runner"), &canon), Some(2));
        assert_eq!(absolute.holding(Path::new("./tests"), &canon), Some(1));
    }
}
