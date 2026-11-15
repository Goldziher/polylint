//! A scratch copy of the files a run is about to hand to a tool.
//!
//! Two situations need one: a formatter rewrites its inputs in place, and poly
//! must not touch the real tree (the default is a dry run, and the bytes poly
//! holds are authoritative even when they differ from disk); and a linter asked
//! about content that is not on disk (the re-lint after an autofix).
//!
//! The mirror keeps each file at its *relative path*, not under a flat name:
//! tools judge a file by its name (`BUILD` versus `defs.bzl`, `Dockerfile`),
//! its extension, and the directories above it. Alongside the files it copies
//! the config and marker files the tool would have found by walking up from the
//! original location, so the tool sees the same project from inside the mirror.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

/// A scratch tree, removed on drop.
pub(super) struct Mirror {
    dir: crate::tempreg::RegisteredTempDir,
}

impl Mirror {
    pub(super) fn new() -> Result<Self> {
        let dir = crate::tempreg::RegisteredTempDir::new("poly-recipe-")
            .context("creating a scratch directory for a tool run")?;
        Ok(Mirror { dir })
    }

    pub(super) fn root(&self) -> &Path {
        self.dir.path()
    }

    /// Write `content` at `rel` inside the mirror.
    pub(super) fn write(&self, rel: &Path, content: &str) -> Result<PathBuf> {
        let path = self.dir.path().join(rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
        }
        std::fs::write(&path, content).with_context(|| format!("writing {}", path.display()))?;
        Ok(path)
    }

    /// Copy every file named in `names` found in `rel`'s directory or any
    /// ancestor under `cwd`, to the same relative place in the mirror.
    pub(super) fn copy_context(&self, cwd: &Path, rel: &Path, names: &[String]) -> Result<()> {
        let mut directory = rel.parent().map(Path::to_path_buf);
        while let Some(current) = directory {
            for name in names {
                let source = cwd.join(&current).join(name);
                let target = self.dir.path().join(&current).join(name);
                if source.is_file() && !target.exists() {
                    if let Some(parent) = target.parent() {
                        std::fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
                    }
                    std::fs::copy(&source, &target)
                        .with_context(|| format!("copying project config {} for the tool run", source.display()))?;
                }
            }
            directory = if current.as_os_str().is_empty() {
                None
            } else {
                current.parent().map(Path::to_path_buf)
            };
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn files_keep_their_relative_path_and_context_files_follow() {
        let repo = tempfile::tempdir().expect("repo");
        std::fs::create_dir_all(repo.path().join("a/b")).expect("mkdir");
        std::fs::write(repo.path().join("buf.yaml"), "version: v2\n").expect("buf.yaml");
        std::fs::write(repo.path().join("a/other.txt"), "x").expect("other");

        let mirror = Mirror::new().expect("mirror");
        let written = mirror
            .write(Path::new("a/b/x.proto"), "syntax = \"proto3\";\n")
            .expect("write");
        mirror
            .copy_context(repo.path(), Path::new("a/b/x.proto"), &["buf.yaml".to_owned()])
            .expect("copy context");

        assert_eq!(written, mirror.root().join("a/b/x.proto"));
        assert!(
            mirror.root().join("buf.yaml").is_file(),
            "the config above the file is copied"
        );
        assert!(
            !mirror.root().join("a/other.txt").exists(),
            "only named context is copied"
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_failed_context_copy_is_an_error_naming_the_file() {
        use std::os::unix::fs::PermissionsExt;

        let repo = tempfile::tempdir().expect("repo");
        let config = repo.path().join("buf.yaml");
        std::fs::write(&config, "version: v2\n").expect("buf.yaml");
        std::fs::set_permissions(&config, std::fs::Permissions::from_mode(0o000)).expect("chmod");
        if std::fs::File::open(&config).is_ok() {
            return;
        }

        let mirror = Mirror::new().expect("mirror");
        let error = mirror
            .copy_context(repo.path(), Path::new("x.proto"), &["buf.yaml".to_owned()])
            .expect_err("unreadable config must not be skipped");
        assert!(format!("{error:#}").contains("buf.yaml"));
    }
}
