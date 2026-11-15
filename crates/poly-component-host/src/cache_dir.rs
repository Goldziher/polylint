//! Choosing and vetting the directory components are cached and loaded from.
//!
//! A cached library is verified and later opened by path, and alef's verified stamp trusts a
//! library whose file identity (device, inode, times) is unchanged. Anyone who can write the
//! directory can forge a library together with a matching stamp, so a root that others can write
//! is refused (an explicit override) or run without the stamp (the default root).

use std::path::{Path, PathBuf};

use anyhow::Context;

/// The cache root a host uses and whether alef's verified stamp may be trusted under it.
pub(crate) struct CacheRoot {
    pub(crate) path: PathBuf,
    pub(crate) stamp: bool,
}

/// Resolve the cache root: `override_root` (from `POLY_COMPONENT_CACHE`) when given, else
/// `default_root`.
///
/// # Errors
///
/// On unix, when `override_root` exists but is not owned by the current user or is writable by
/// group or others, or cannot be created.
pub(crate) fn resolve(default_root: PathBuf, override_root: Option<PathBuf>) -> anyhow::Result<CacheRoot> {
    match override_root {
        Some(path) => {
            create_private(&path).with_context(|| format!("creating the component cache {}", path.display()))?;
            require_private(&path)?;
            Ok(CacheRoot { path, stamp: false })
        }
        None => {
            // Best effort: the manager creates the directory itself on first install.
            let _ = create_private(&default_root);
            let stamp = is_private(&default_root);
            Ok(CacheRoot {
                path: default_root,
                stamp,
            })
        }
    }
}

#[cfg(unix)]
fn create_private(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    std::fs::DirBuilder::new().recursive(true).mode(0o700).create(path)
}

#[cfg(not(unix))]
fn create_private(path: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(path)
}

/// Why a directory with this `mode` and `owner` is not private to `user`, if it is not.
#[cfg(unix)]
fn violation(mode: u32, owner: u32, user: u32) -> Option<&'static str> {
    if owner != user {
        Some("is not owned by the current user")
    } else if mode & 0o022 != 0 {
        Some("is writable by group or others")
    } else {
        None
    }
}

#[cfg(unix)]
fn check(path: &Path) -> anyhow::Result<Option<&'static str>> {
    use std::os::unix::fs::MetadataExt;
    let meta = std::fs::metadata(path).with_context(|| format!("inspecting the component cache {}", path.display()))?;
    Ok(violation(meta.mode(), meta.uid(), rustix::process::geteuid().as_raw()))
}

#[cfg(unix)]
fn require_private(path: &Path) -> anyhow::Result<()> {
    if let Some(reason) = check(path)? {
        anyhow::bail!(
            "refusing the component cache {}: it {reason}, so another user could replace a cached component. \
             Use a directory only you can write (`chmod 700 {}`)",
            path.display(),
            path.display()
        );
    }
    Ok(())
}

#[cfg(not(unix))]
fn require_private(_path: &Path) -> anyhow::Result<()> {
    Ok(())
}

#[cfg(unix)]
fn is_private(path: &Path) -> bool {
    matches!(check(path), Ok(None))
}

#[cfg(not(unix))]
fn is_private(_path: &Path) -> bool {
    true
}

#[cfg(all(test, unix))]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use super::*;

    fn dir_with_mode(mode: u32) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(mode)).unwrap();
        dir
    }

    #[test]
    fn violation_names_foreign_owner_and_shared_write() {
        assert_eq!(violation(0o700, 1000, 1000), None);
        assert_eq!(violation(0o755, 1000, 1000), None);
        assert!(violation(0o700, 0, 1000).unwrap().contains("not owned"));
        assert!(violation(0o770, 1000, 1000).unwrap().contains("writable"));
        assert!(violation(0o707, 1000, 1000).unwrap().contains("writable"));
    }

    #[test]
    fn a_world_writable_override_is_refused() {
        let dir = dir_with_mode(0o777);
        let error = resolve(PathBuf::from("/unused"), Some(dir.path().to_owned()))
            .err()
            .expect("refused");
        let message = format!("{error:#}");
        assert!(message.contains(&dir.path().display().to_string()), "{message}");
        assert!(message.contains("chmod 700"), "{message}");
    }

    #[test]
    fn a_group_writable_override_is_refused() {
        let dir = dir_with_mode(0o770);
        assert!(resolve(PathBuf::from("/unused"), Some(dir.path().to_owned())).is_err());
    }

    #[test]
    fn a_private_override_is_accepted_without_the_stamp() {
        let dir = dir_with_mode(0o700);
        let root = resolve(PathBuf::from("/unused"), Some(dir.path().to_owned())).unwrap();
        assert_eq!(root.path, dir.path());
        assert!(!root.stamp);
    }

    #[test]
    fn a_missing_override_is_created_private() {
        let parent = dir_with_mode(0o700);
        let path = parent.path().join("a").join("cache");
        resolve(PathBuf::from("/unused"), Some(path.clone())).unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o077, 0, "{mode:o}");
    }

    #[test]
    fn a_missing_default_root_is_created_private_and_keeps_the_stamp() {
        let parent = dir_with_mode(0o700);
        let path = parent.path().join("components");
        let root = resolve(path.clone(), None).unwrap();
        assert!(root.stamp);
        assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o077, 0);
    }

    #[test]
    fn a_shared_default_root_is_used_without_the_stamp() {
        let dir = dir_with_mode(0o777);
        let root = resolve(dir.path().to_owned(), None).unwrap();
        assert_eq!(root.path, dir.path());
        assert!(!root.stamp);
    }
}
