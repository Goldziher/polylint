//! Lexical path cleaning.
//!
//! Ported from `polyhooks/src/fs.rs` (MIT, © 2023 Astral Software Inc.).

use std::path::{Component, Path, PathBuf};

/// Lexical path cleaning (no filesystem access).
pub(crate) trait PathClean {
    /// Return a clean version of this path with `.` and `..` resolved.
    fn clean(&self) -> PathBuf;
}

impl PathClean for Path {
    fn clean(&self) -> PathBuf {
        clean_path(self)
    }
}

impl PathClean for PathBuf {
    fn clean(&self) -> PathBuf {
        clean_path(self.as_path())
    }
}

fn clean_path(path: &Path) -> PathBuf {
    let mut out: Vec<Component<'_>> = Vec::new();

    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => match out.last() {
                Some(Component::RootDir) => {}
                Some(Component::Normal(_)) => {
                    out.pop();
                }
                None | Some(Component::CurDir | Component::ParentDir | Component::Prefix(_)) => {
                    out.push(component);
                }
            },
            c => out.push(c),
        }
    }

    if out.is_empty() {
        PathBuf::from(".")
    } else {
        out.iter().collect()
    }
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    #[test]
    fn path_clean_various() {
        use super::PathClean as _;

        let cases = [
            ("", "."),
            (".", "."),
            ("./test/./path", "test/path"),
            ("test/path/..", "test"),
            ("test/path/../../..", ".."),
            ("test/path/../../../..", "../.."),
            ("../test/..", ".."),
            ("/../test", "/test"),
            ("/test/path/../../../..", "/"),
        ];

        for (input, expected) in cases {
            assert_eq!(Path::new(input).clean(), PathBuf::from(expected), "{input}");
        }
    }
}
