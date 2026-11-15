//! Confinement of request paths to the server's workspace.
//!
//! A frontend serves a model, and a model can be steered by text it read. Without a boundary a
//! `paths` or `config` argument names any file the process can reach, so one injected instruction
//! can rewrite another project or load an attacker's `poly.toml`. [`WorkspaceGuard`] canonicalizes
//! each requested path, which resolves symlinks and `..`, and refuses one outside the root.

use std::path::{Path, PathBuf};

use anyhow::Context;

/// The directory a frontend may operate in, plus the operator's own trusted config file.
#[derive(Debug, Clone)]
pub struct WorkspaceGuard {
    root: PathBuf,
    trusted_config: Option<PathBuf>,
    allow_outside: bool,
}

impl WorkspaceGuard {
    /// Confine requests to `root`, canonicalized here so later symlink swaps cannot move it.
    ///
    /// `trusted_config` is a config file the operator passed on the command line; it is accepted
    /// wherever it lives. `allow_outside` turns the check off.
    ///
    /// # Errors
    ///
    /// When `root` or `trusted_config` cannot be resolved.
    pub fn new(root: &Path, trusted_config: Option<&Path>, allow_outside: bool) -> anyhow::Result<Self> {
        let root = root
            .canonicalize()
            .with_context(|| format!("cannot resolve the workspace root `{}`", root.display()))?;
        let trusted_config = trusted_config
            .map(Path::canonicalize)
            .transpose()
            .with_context(|| "cannot resolve the `--config` file")?;
        Ok(Self {
            root,
            trusted_config,
            allow_outside,
        })
    }

    /// A guard rooted at the current working directory.
    ///
    /// # Errors
    ///
    /// When the working directory or `trusted_config` cannot be resolved.
    pub fn for_cwd(trusted_config: Option<&Path>, allow_outside: bool) -> anyhow::Result<Self> {
        let cwd = std::env::current_dir().context("cannot resolve the working directory")?;
        Self::new(&cwd, trusted_config, allow_outside)
    }

    /// Check every requested path.
    ///
    /// # Errors
    ///
    /// When a path does not resolve or resolves outside the workspace.
    pub fn check_paths<S: AsRef<str>>(&self, paths: &[S]) -> anyhow::Result<()> {
        paths.iter().try_for_each(|path| self.check_path(path.as_ref(), "path"))
    }

    /// Check a requested config file.
    ///
    /// # Errors
    ///
    /// When the file does not resolve or resolves outside the workspace.
    pub fn check_config(&self, config: Option<&str>) -> anyhow::Result<()> {
        let Some(config) = config else { return Ok(()) };
        if self.allow_outside {
            return Ok(());
        }
        if let Ok(resolved) = Path::new(config).canonicalize()
            && self.trusted_config.as_ref() == Some(&resolved)
        {
            return Ok(());
        }
        self.check_path(config, "config")
    }

    fn check_path(&self, requested: &str, what: &str) -> anyhow::Result<()> {
        if self.allow_outside {
            return Ok(());
        }
        let resolved = Path::new(requested)
            .canonicalize()
            .with_context(|| format!("cannot resolve {what} `{requested}`"))?;
        anyhow::ensure!(
            resolved.starts_with(&self.root),
            "{what} `{requested}` is outside the workspace `{}`; start `poly mcp` with \
             `--allow-outside-workspace` to permit it",
            self.root.display()
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn guard(allow: bool) -> (tempfile::TempDir, tempfile::TempDir, WorkspaceGuard) {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join("src")).unwrap();
        let guard = WorkspaceGuard::new(root.path(), None, allow).unwrap();
        (root, outside, guard)
    }

    #[test]
    fn inside_path_is_accepted() {
        let (root, _outside, guard) = guard(false);
        let inside = root.path().join("src");
        guard.check_paths(&[inside.to_str().unwrap()]).unwrap();
        guard.check_paths(&[root.path().to_str().unwrap()]).unwrap();
    }

    #[test]
    fn outside_path_is_rejected() {
        let (_root, outside, guard) = guard(false);
        let error = guard.check_paths(&[outside.path().to_str().unwrap()]).unwrap_err();
        assert!(error.to_string().contains("outside the workspace"), "{error}");
    }

    #[test]
    fn dotdot_traversal_is_rejected() {
        let (root, outside, guard) = guard(false);
        let name = outside.path().file_name().unwrap().to_str().unwrap();
        let sneaky = root.path().join("src").join("..").join("..").join(name);
        assert!(guard.check_paths(&[sneaky.to_str().unwrap()]).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn symlink_escaping_the_root_is_rejected() {
        let (root, outside, guard) = guard(false);
        let link = root.path().join("escape");
        std::os::unix::fs::symlink(outside.path(), &link).unwrap();
        assert!(guard.check_paths(&[link.to_str().unwrap()]).is_err());
    }

    #[test]
    fn outside_config_is_rejected_and_inside_accepted() {
        let (root, outside, guard) = guard(false);
        let evil = outside.path().join("poly.toml");
        std::fs::write(&evil, "").unwrap();
        let good = root.path().join("poly.toml");
        std::fs::write(&good, "").unwrap();
        assert!(guard.check_config(Some(evil.to_str().unwrap())).is_err());
        guard.check_config(Some(good.to_str().unwrap())).unwrap();
        guard.check_config(None).unwrap();
    }

    #[test]
    fn operator_config_outside_the_root_is_trusted() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let shared = outside.path().join("poly.toml");
        std::fs::write(&shared, "").unwrap();
        let guard = WorkspaceGuard::new(root.path(), Some(&shared), false).unwrap();
        guard.check_config(Some(shared.to_str().unwrap())).unwrap();
    }

    #[test]
    fn opt_out_accepts_outside_paths() {
        let (_root, outside, guard) = guard(true);
        guard.check_paths(&[outside.path().to_str().unwrap()]).unwrap();
        guard.check_config(Some("/nonexistent/poly.toml")).unwrap();
    }
}
