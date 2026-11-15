//! Choosing and vetting the directory components are cached and loaded from.
//!
//! A cached library is verified and later opened by path, and alef's verified stamp trusts a
//! library whose file identity (device, inode, times) is unchanged. Anyone who can write the
//! directory can forge a library together with a matching stamp, so every root, default or
//! overridden, is refused when it belongs to someone else and tightened to 0700 when it is the
//! user's own; a root that cannot be tightened is refused too.

use std::path::{Path, PathBuf};

use anyhow::Context;

/// The cache root a host uses and whether alef's verified stamp may be trusted under it.
pub(crate) struct CacheRoot {
    pub(crate) path: PathBuf,
    pub(crate) stamp: bool,
}

/// Resolve the cache root: `override_root` (from `POLY_COMPONENT_CACHE`) when given, else
/// `default_root`. Both get the same policy; only the override runs without the verified stamp.
///
/// # Errors
///
/// On unix, when the root is not owned by the current user, cannot be created or resolved, or is
/// writable by group or others and cannot be tightened to 0700.
pub(crate) fn resolve(default_root: PathBuf, override_root: Option<PathBuf>) -> anyhow::Result<CacheRoot> {
    let (path, stamp) = match override_root {
        Some(path) => (path, false),
        None => (default_root, true),
    };
    create_private(&path).with_context(|| format!("creating the component cache {}", path.display()))?;
    // A symlinked root is resolved once, so the directory vetted here is the one used.
    let path = path
        .canonicalize()
        .with_context(|| format!("resolving the component cache {}", path.display()))?;
    require_private(&path)?;
    Ok(CacheRoot { path, stamp })
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

/// Why a component cache directory is not private to the current user.
#[cfg(unix)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Violation {
    ForeignOwner,
    SharedWrite,
}

#[cfg(unix)]
impl Violation {
    fn describe(self) -> &'static str {
        match self {
            Self::ForeignOwner => "is not owned by the current user",
            Self::SharedWrite => "is writable by group or others",
        }
    }
}

/// Why a directory with this `mode` and `owner` is not private to `user`, if it is not.
#[cfg(unix)]
fn violation(mode: u32, owner: u32, user: u32) -> Option<Violation> {
    if owner != user {
        Some(Violation::ForeignOwner)
    } else if mode & 0o022 != 0 {
        Some(Violation::SharedWrite)
    } else {
        None
    }
}

#[cfg(unix)]
fn check(path: &Path) -> anyhow::Result<Option<Violation>> {
    use std::os::unix::fs::MetadataExt;
    let meta = std::fs::metadata(path).with_context(|| format!("inspecting the component cache {}", path.display()))?;
    Ok(violation(meta.mode(), meta.uid(), rustix::process::geteuid().as_raw()))
}

#[cfg(unix)]
fn require_private(path: &Path) -> anyhow::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let refuse = |reason: &str| {
        anyhow::anyhow!(
            "refusing the component cache {}: it {reason}, so another user could replace a cached component. \
             Use a directory only you can write (`chmod 700 {}`)",
            path.display(),
            path.display()
        )
    };
    match check(path)? {
        None => Ok(()),
        Some(Violation::ForeignOwner) => Err(refuse(Violation::ForeignOwner.describe())),
        Some(Violation::SharedWrite) => {
            // Our own directory that is merely shared (a 0775 CI mount): tighten it, and refuse it if
            // that fails, since a component cached there could be replaced by another user.
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).map_err(|error| {
                refuse(Violation::SharedWrite.describe()).context(format!("tightening failed: {error}"))
            })?;
            match check(path)? {
                None => Ok(()),
                Some(left) => Err(refuse(left.describe())),
            }
        }
    }
}

#[cfg(not(unix))]
fn require_private(_path: &Path) -> anyhow::Result<()> {
    Ok(())
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
        assert_eq!(violation(0o700, 0, 1000), Some(Violation::ForeignOwner));
        assert_eq!(violation(0o770, 1000, 1000), Some(Violation::SharedWrite));
        assert_eq!(violation(0o707, 1000, 1000), Some(Violation::SharedWrite));
    }

    #[test]
    fn a_foreign_owned_override_is_refused() {
        // Root-owned and not writable by us: only meaningful when not running as root.
        if rustix::process::geteuid().is_root() || !Path::new("/usr").is_dir() {
            return;
        }
        let error = resolve(PathBuf::from("/unused"), Some(PathBuf::from("/usr")))
            .err()
            .expect("refused");
        let message = format!("{error:#}");
        assert!(message.contains("not owned"), "{message}");
        assert!(message.contains("chmod 700"), "{message}");
    }

    #[test]
    fn an_own_world_writable_override_is_tightened_and_used() {
        let dir = dir_with_mode(0o777);
        let root = resolve(PathBuf::from("/unused"), Some(dir.path().to_owned())).unwrap();
        assert!(!root.stamp);
        let mode = std::fs::metadata(dir.path()).unwrap().permissions().mode();
        assert_eq!(mode & 0o022, 0, "{mode:o}");
    }

    #[test]
    fn an_own_group_writable_override_is_tightened_and_used() {
        let dir = dir_with_mode(0o775);
        resolve(PathBuf::from("/unused"), Some(dir.path().to_owned())).unwrap();
        let mode = std::fs::metadata(dir.path()).unwrap().permissions().mode();
        assert_eq!(mode & 0o022, 0, "{mode:o}");
    }

    #[test]
    fn a_symlinked_override_resolves_to_its_target() {
        let parent = dir_with_mode(0o700);
        let target = parent.path().join("real");
        std::fs::create_dir(&target).unwrap();
        let link = parent.path().join("link");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        let root = resolve(PathBuf::from("/unused"), Some(link)).unwrap();
        assert_eq!(root.path, target.canonicalize().unwrap());
    }

    #[test]
    fn a_private_override_is_accepted_without_the_stamp() {
        let dir = dir_with_mode(0o700);
        let root = resolve(PathBuf::from("/unused"), Some(dir.path().to_owned())).unwrap();
        assert_eq!(root.path, dir.path().canonicalize().unwrap());
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
    fn a_shared_default_root_is_tightened_and_keeps_the_stamp() {
        let dir = dir_with_mode(0o777);
        let root = resolve(dir.path().to_owned(), None).unwrap();
        assert!(root.stamp);
        let mode = std::fs::metadata(dir.path()).unwrap().permissions().mode();
        assert_eq!(mode & 0o022, 0, "{mode:o}");
    }

    #[test]
    fn a_foreign_owned_default_root_is_refused() {
        if rustix::process::geteuid().is_root() || !Path::new("/usr").is_dir() {
            return;
        }
        let error = resolve(PathBuf::from("/usr"), None).err().expect("refused");
        assert!(format!("{error:#}").contains("not owned"), "{error:#}");
    }

    #[test]
    fn an_untightenable_shared_root_is_refused() {
        // Not owned by us, so chmod cannot succeed and the foreign-owner policy applies.
        if rustix::process::geteuid().is_root() || !Path::new("/tmp").is_dir() {
            return;
        }
        let error = resolve(PathBuf::from("/tmp"), None).err().expect("refused");
        assert!(
            format!("{error:#}").contains("refusing the component cache"),
            "{error:#}"
        );
    }
}
