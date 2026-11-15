//! Atomic file writes for formatted and autofixed output.
//!
//! Split out of `runner.rs` to keep that file under the module-size cap; the
//! runner calls [`write_atomic_if_unchanged`] wherever `--fix` or `fmt --fix` lands content.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, SystemTime};

/// Suffix of the sibling temp files; a kill between create and rename leaves one behind.
const TMP_SUFFIX: &str = ".poly.tmp";

/// Temp files older than this are leftovers of a killed run, not a concurrent writer.
const STALE_TMP_AGE: Duration = Duration::from_secs(24 * 60 * 60);

/// Directories already swept this process: one `read_dir` per directory, not one per written file.
static SWEPT: std::sync::LazyLock<Mutex<HashSet<PathBuf>>> = std::sync::LazyLock::new(Default::default);

/// Distinct temp names tried before giving up when each is already taken.
const TMP_ATTEMPTS: u32 = 8;

/// Replace `path` with `contents` via a sibling temp file and a rename, so a
/// crash or a concurrent reader never sees a half-written file, preserving the
/// original file's permissions (an executable script keeps its exec bit).
///
/// A symlink is resolved first and its target written, so the link survives. A hard-linked target is
/// de-linked: the rename gives the path a new inode while other links keep the old one.
pub(super) fn write_atomic(path: &Path, contents: &str) -> anyhow::Result<()> {
    write_resolved(&resolve_target(path), contents, None)
}

/// [`write_atomic`] that refuses to overwrite a file edited since it was read.
///
/// `expected` is the content the caller read before processing. The on-disk bytes are compared with
/// it immediately before the rename; on a mismatch nothing is replaced and the error says so.
pub(super) fn write_atomic_if_unchanged(path: &Path, expected: &str, contents: &str) -> anyhow::Result<()> {
    write_resolved(&resolve_target(path), contents, Some(expected))
}

/// The real file behind `path`: an explicitly named symlink must not be replaced by a regular file.
fn resolve_target(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

fn write_resolved(path: &Path, contents: &str, expected: Option<&str>) -> anyhow::Result<()> {
    use std::io::Write as _;

    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let file_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("poly");
    sweep_stale_tmp(parent);
    let original_permissions = std::fs::metadata(path).ok().map(|m| m.permissions());
    let (mut file, tmp) = create_tmp(parent, file_name, original_permissions.as_ref())?;
    let _registered = crate::tempreg::Registration::new(&tmp);
    let written = (|| -> anyhow::Result<()> {
        file.write_all(contents.as_bytes())?;
        if let Some(permissions) = original_permissions
            && file.metadata()?.permissions() != permissions
        {
            file.set_permissions(permissions)?;
        }
        if let Some(expected) = expected
            && std::fs::read(path)? != expected.as_bytes()
        {
            anyhow::bail!("file changed while it was being processed, not overwritten");
        }
        std::fs::rename(&tmp, path)?;
        Ok(())
    })();
    if written.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    written
}

/// Exclusively create the sibling temp file. The name is predictable (pid), so `create_new` keeps a
/// pre-planted file or symlink at that name from being followed or truncated; a taken name moves on to
/// the next counter suffix.
fn create_tmp(
    parent: &Path,
    file_name: &str,
    original_permissions: Option<&std::fs::Permissions>,
) -> anyhow::Result<(std::fs::File, PathBuf)> {
    let pid = std::process::id();
    for attempt in 0..TMP_ATTEMPTS {
        let tmp = tmp_candidate(parent, file_name, pid, attempt);
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        // ~keep The rename replaces the original inode with a freshly created temp file, whose mode is
        // `0666 & !umask` and has no relationship to the file being formatted. Without this, formatting
        // an executable script silently clears its exec bit. Creating the file with the original mode
        // makes the follow-up `fchmod` a no-op in the common case, so it is skipped then.
        #[cfg(unix)]
        {
            use std::os::unix::fs::{OpenOptionsExt as _, PermissionsExt as _};
            if let Some(permissions) = original_permissions {
                options.mode(permissions.mode() & 0o7777);
            }
        }
        #[cfg(not(unix))]
        let _ = original_permissions;
        match options.open(&tmp) {
            Ok(file) => return Ok((file, tmp)),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error.into()),
        }
    }
    anyhow::bail!("could not create a temp file next to {file_name}: every candidate name is taken")
}

fn tmp_candidate(parent: &Path, file_name: &str, pid: u32, attempt: u32) -> PathBuf {
    if attempt == 0 {
        parent.join(format!(".{file_name}.{pid}{TMP_SUFFIX}"))
    } else {
        parent.join(format!(".{file_name}.{pid}.{attempt}{TMP_SUFFIX}"))
    }
}

/// Best-effort removal of `.*.poly.tmp` files in `dir` older than [`STALE_TMP_AGE`], at most once per
/// directory per process.
fn sweep_stale_tmp(dir: &Path) {
    let first = SWEPT
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .insert(dir.to_path_buf());
    if !first {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        if !(name.starts_with('.') && name.ends_with(TMP_SUFFIX)) {
            continue;
        }
        let stale = entry
            .metadata()
            .ok()
            .filter(|meta| meta.is_file())
            .and_then(|meta| meta.modified().ok())
            .and_then(|modified| SystemTime::now().duration_since(modified).ok())
            .is_some_and(|age| age > STALE_TMP_AGE);
        if stale {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn leftovers(dir: &Path) -> Vec<String> {
        std::fs::read_dir(dir)
            .unwrap()
            .filter_map(Result::ok)
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.ends_with(TMP_SUFFIX))
            .collect()
    }

    #[test]
    fn replaces_content_when_the_file_is_unchanged() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.txt");
        std::fs::write(&file, "old\n").unwrap();
        write_atomic_if_unchanged(&file, "old\n", "new\n").unwrap();
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "new\n");
        assert!(leftovers(dir.path()).is_empty());
    }

    #[test]
    fn refuses_a_file_changed_since_it_was_read_and_removes_the_temp_file() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.txt");
        std::fs::write(&file, "edited by someone else\n").unwrap();
        let error = write_atomic_if_unchanged(&file, "old\n", "new\n").unwrap_err();
        assert!(
            error.to_string().contains("changed while it was being processed"),
            "{error}"
        );
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "edited by someone else\n");
        assert!(leftovers(dir.path()).is_empty(), "temp file must be cleaned up");
    }

    #[test]
    fn sweeps_only_stale_temp_files() {
        let dir = tempfile::tempdir().unwrap();
        let stale = dir.path().join(".x.1.poly.tmp");
        let fresh = dir.path().join(".y.2.poly.tmp");
        std::fs::write(&stale, "").unwrap();
        std::fs::write(&fresh, "").unwrap();
        let old = SystemTime::now() - Duration::from_secs(2 * 24 * 60 * 60);
        std::fs::File::options()
            .write(true)
            .open(&stale)
            .unwrap()
            .set_modified(old)
            .unwrap();
        let file = dir.path().join("a.txt");
        std::fs::write(&file, "old\n").unwrap();
        write_atomic(&file, "new\n").unwrap();
        assert!(!stale.exists());
        assert!(fresh.exists());
    }

    #[cfg(unix)]
    #[test]
    fn writes_through_a_symlink_and_keeps_the_link() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("real.txt");
        let link = dir.path().join("link.txt");
        std::fs::write(&target, "old\n").unwrap();
        std::os::unix::fs::symlink(&target, &link).unwrap();
        write_atomic_if_unchanged(&link, "old\n", "new\n").unwrap();
        assert!(std::fs::symlink_metadata(&link).unwrap().file_type().is_symlink());
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "new\n");
    }

    fn age(path: &Path) {
        let old = SystemTime::now() - Duration::from_secs(2 * 24 * 60 * 60);
        std::fs::File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_modified(old)
            .unwrap();
    }

    #[test]
    fn sweeps_each_directory_once() {
        let dir = tempfile::tempdir().unwrap();
        sweep_stale_tmp(dir.path());
        let stale = dir.path().join(".late.1.poly.tmp");
        std::fs::write(&stale, "").unwrap();
        age(&stale);
        sweep_stale_tmp(dir.path());
        assert!(stale.exists(), "the second sweep of a directory must not run");
    }

    #[test]
    fn a_planted_file_at_the_first_temp_name_is_not_overwritten() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.txt");
        std::fs::write(&file, "old\n").unwrap();
        let planted = tmp_candidate(dir.path(), "a.txt", std::process::id(), 0);
        std::fs::write(&planted, "planted").unwrap();
        write_atomic(&file, "new\n").unwrap();
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "new\n");
        assert_eq!(std::fs::read_to_string(&planted).unwrap(), "planted");
    }

    #[cfg(unix)]
    #[test]
    fn a_planted_symlink_at_the_first_temp_name_is_not_followed() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.txt");
        let victim = dir.path().join("victim.txt");
        std::fs::write(&file, "old\n").unwrap();
        std::fs::write(&victim, "victim\n").unwrap();
        let planted = tmp_candidate(dir.path(), "a.txt", std::process::id(), 0);
        std::os::unix::fs::symlink(&victim, &planted).unwrap();
        write_atomic(&file, "new\n").unwrap();
        assert_eq!(std::fs::read_to_string(&victim).unwrap(), "victim\n");
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "new\n");
        assert!(std::fs::symlink_metadata(&planted).unwrap().file_type().is_symlink());
    }

    #[test]
    fn gives_up_when_every_temp_name_is_taken() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.txt");
        std::fs::write(&file, "old\n").unwrap();
        for attempt in 0..TMP_ATTEMPTS {
            std::fs::write(tmp_candidate(dir.path(), "a.txt", std::process::id(), attempt), "").unwrap();
        }
        assert!(write_atomic(&file, "new\n").is_err());
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "old\n");
    }
}
