//! Synchronous git helper functions.
//!
//! Ported from `polyhooks/src/git.rs`. All `async`/`.await` removed; every
//! call uses the blocking `Cmd::output()` / `Cmd::status()` methods.
//!
//! This file owns the shared plumbing — the error type, the resolved `git`
//! binary and repository-root statics, the [`Cmd`] builder every helper starts
//! from, the byte/path decoding of git's `-z` output, and the repository-layout
//! queries. The helpers that share a distinct input are in submodules:
//! `index` for the staging area (and the path-safety guard its results
//! require), `revision` for anything that names a commit.
//!
//! The following upstream helpers are intentionally **not ported** (clone /
//! fetch helpers, install shims, workspace helpers) — they belong to later
//! phases or the polyhooks crate:
//!
//! `init_repo`, `shallow_clone`, `full_clone`, `clone_repo_attempt`,
//! `clone_repo`, `get_lfs_files`, `write_tree`, `ls_files`, `has_diff`,
//! `files_not_staged`, `has_unmerged_paths`, `is_in_merge_conflict`,
//! `get_conflicted_files`.

use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use tracing::instrument;

use crate::fs::PathClean as _;
use crate::process::{Cmd, Error as ProcessError, StatusError};

mod index;
mod revision;
#[cfg(test)]
mod testutil;

pub(crate) use self::index::{
    StagedEntry, add, checkout_index_paths, is_safe_relative_path, list_staged_entries, list_submodule_gitlinks,
    stage_contents_from, staged_blobs, worktree_digest,
};
pub use self::index::{get_staged_files, list_files};
pub use self::revision::get_changed_files;
pub(crate) use self::revision::{
    get_ancestors_not_in_remote, get_parent_commit, get_root_commits, is_ancestor, rev_exists,
};

/// Errors returned by git helpers in this module.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Propagated command error.
    #[error(transparent)]
    Command(#[from] ProcessError),

    /// `git` binary not found on `PATH`.
    #[error("Failed to find git: {0}")]
    GitNotFound(#[from] which::Error),

    /// Underlying I/O error (from status / output calls).
    #[error(transparent)]
    Io(#[from] std::io::Error),

    /// Git output was not valid UTF-8.
    #[error(transparent)]
    Utf8(#[from] std::str::Utf8Error),

    /// A revision argument began with `-`, which git would parse as an option.
    #[error("invalid git revision (must not begin with `-`): old={old:?}, new={new:?}")]
    InvalidRevision {
        /// The `old` side of the requested range.
        old: String,
        /// The `new` side of the requested range.
        new: String,
    },

    /// An index path that would escape the tree it is joined onto.
    #[error("refusing an index path that escapes its tree: {path}")]
    UnsafePath {
        /// The offending repo-relative path, as git reported it.
        path: String,
    },
}

/// Resolved path to the `git` binary (or the error from `which::which`).
pub(crate) static GIT: LazyLock<Result<PathBuf, which::Error>> = LazyLock::new(|| which::which("git"));

/// `GIT_*` environment variables that should be removed when running
/// subprocess git commands to avoid inheriting hook-injected state.
///
/// `GIT_INDEX_FILE` is intentionally kept so that `git write-tree` works
/// correctly when called from inside a `git commit -a` / `-p` hook.
///
/// This is deliberately the **opposite** of the `remote` module's
/// `git_env_to_remove` (`crates/poly-cli/src/remote/git.rs`), which *drops*
/// `GIT_INDEX_FILE`. These commands act on the **consumer** repository and must
/// see the index the in-flight commit is building; those act on **foreign**
/// cached repositories, where the consumer's index corrupts an unrelated
/// checkout. Same variable, opposing requirements: do not unify the two lists.
pub(crate) static GIT_ENV_TO_REMOVE: LazyLock<Vec<String>> = LazyLock::new(|| git_env_to_remove(std::env::vars()));

/// The names among `vars` that [`GIT_ENV_TO_REMOVE`] strips from a git subprocess.
fn git_env_to_remove(vars: impl Iterator<Item = (String, String)>) -> Vec<String> {
    const KEEP: &[&str] = &[
        "GIT_INDEX_FILE",
        "GIT_EXEC_PATH",
        "GIT_SSH",
        "GIT_SSH_COMMAND",
        "GIT_SSL_CAINFO",
        "GIT_SSL_NO_VERIFY",
        "GIT_CONFIG_COUNT",
        "GIT_CONFIG_PARAMETERS",
        "GIT_HTTP_PROXY_AUTHMETHOD",
        "GIT_ALLOW_PROTOCOL",
        "GIT_ASKPASS",
    ];

    vars.filter(|(k, _)| {
        k.starts_with("GIT_")
            && !k.starts_with("GIT_CONFIG_KEY_")
            && !k.starts_with("GIT_CONFIG_VALUE_")
            && !KEEP.contains(&k.as_str())
    })
    .map(|(key, _)| key)
    .collect()
}

/// Build a base `Cmd` pointing at the `git` binary, with
/// `core.useBuiltinFSMonitor=false` injected to avoid monitor overhead and the inherited
/// hook-injected `GIT_*` state (work tree, object directories, namespace, ...) removed, so the
/// command resolves the repository from its own working directory.
pub(crate) fn git_cmd(summary: &str) -> Result<Cmd, Error> {
    let mut cmd = Cmd::new(GIT.as_ref().map_err(|&e| Error::GitNotFound(e))?, summary);
    cmd.remove_git_envs();
    cmd.arg("-c").arg("core.useBuiltinFSMonitor=false");
    Ok(cmd)
}

fn zsplit(s: &[u8]) -> Result<Vec<PathBuf>, std::str::Utf8Error> {
    s.split(|&b| b == b'\0')
        .filter(|slice| !slice.is_empty())
        .map(path_from_git_bytes)
        .collect()
}

#[cfg(unix)]
// The Result keeps the signature identical to the non-unix variant, which can fail.
#[expect(clippy::unnecessary_wraps)]
pub(crate) fn path_from_git_bytes(bytes: &[u8]) -> Result<PathBuf, std::str::Utf8Error> {
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt as _;

    Ok(PathBuf::from(OsStr::from_bytes(bytes)))
}

#[cfg(not(unix))]
pub(crate) fn path_from_git_bytes(bytes: &[u8]) -> Result<PathBuf, std::str::Utf8Error> {
    std::str::from_utf8(bytes).map(PathBuf::from)
}

/// Return the top-level directory of the current working tree.
///
/// Unlike most helpers here, this uses `std::process::Command` directly so it
/// can be called before `Cmd` is fully initialised.
#[instrument(level = "trace")]
pub fn get_root() -> Result<PathBuf, Error> {
    let git = GIT.as_ref().map_err(|&e| Error::GitNotFound(e))?;
    let output = std::process::Command::new(git)
        .arg("rev-parse")
        .arg("--show-toplevel")
        .output()?;

    if !output.status.success() {
        return Err(Error::Command(ProcessError::Status {
            summary: "get git root".to_string(),
            error: StatusError {
                status: output.status,
                output: Some(output),
            },
        }));
    }

    Ok(PathBuf::from(String::from_utf8_lossy(&output.stdout).trim_ascii()))
}

/// Return the path to the `.git` directory (or git-dir for worktrees).
pub(crate) fn get_git_dir() -> Result<PathBuf, Error> {
    let output = git_cmd("get git dir")?
        .arg("rev-parse")
        .arg("--git-dir")
        .check(true)
        .output()?;
    Ok(PathBuf::from(String::from_utf8_lossy(&output.stdout).trim_ascii()))
}

/// Return the git common dir (the primary `.git` even in a linked worktree).
pub fn get_git_common_dir() -> Result<PathBuf, Error> {
    let output = git_cmd("get git common dir")?
        .arg("rev-parse")
        .arg("--git-common-dir")
        .check(true)
        .output()?;
    let trimmed = output.stdout.trim_ascii();
    if trimmed.is_empty() {
        get_git_dir()
    } else {
        Ok(PathBuf::from(String::from_utf8_lossy(trimmed).as_ref()))
    }
}

/// Return the effective git hooks directory (respects `core.hooksPath`).
pub fn get_git_hooks_dir() -> Result<PathBuf, Error> {
    let output = git_cmd("get git hooks dir")?
        .arg("rev-parse")
        .arg("--git-path")
        .arg("hooks")
        .check(true)
        .output()?;

    let hooks_dir = if output.stdout.trim_ascii().is_empty() {
        get_git_common_dir()?.join("hooks")
    } else {
        PathBuf::from(String::from_utf8_lossy(output.stdout.trim_ascii()).as_ref())
    };
    Ok(hooks_dir.clean())
}

/// The effective `core.hooksPath` (local, worktree or global scope), if set.
pub fn get_config_hooks_path(root: &Path) -> Result<Option<String>, Error> {
    let output = git_cmd("get core.hooksPath")?
        .current_dir(root)
        .arg("config")
        .arg("--get")
        .arg("core.hooksPath")
        .check(false)
        .output()?;
    let value = String::from_utf8_lossy(output.stdout.trim_ascii()).into_owned();
    Ok((output.status.success() && !value.is_empty()).then_some(value))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zsplit_splits_on_nul_and_filters_empty() {
        let input = b"a/b.rs\0c/d.rs\0\0";
        let result = zsplit(input).expect("valid utf-8");
        assert_eq!(result, vec![PathBuf::from("a/b.rs"), PathBuf::from("c/d.rs")]);
    }

    fn vars(names: &[&str]) -> impl Iterator<Item = (String, String)> {
        names
            .iter()
            .map(|name| ((*name).to_owned(), "x".to_owned()))
            .collect::<Vec<_>>()
            .into_iter()
    }

    #[test]
    fn hook_injected_git_state_is_scrubbed_but_the_index_and_transport_settings_stay() {
        let removed = git_env_to_remove(vars(&[
            "GIT_DIR",
            "GIT_WORK_TREE",
            "GIT_OBJECT_DIRECTORY",
            "GIT_ALTERNATE_OBJECT_DIRECTORIES",
            "GIT_NAMESPACE",
            "GIT_INDEX_FILE",
            "GIT_SSH_COMMAND",
            "GIT_CONFIG_KEY_0",
            "PATH",
        ]));
        assert_eq!(
            removed,
            [
                "GIT_DIR",
                "GIT_WORK_TREE",
                "GIT_OBJECT_DIRECTORY",
                "GIT_ALTERNATE_OBJECT_DIRECTORIES",
                "GIT_NAMESPACE"
            ]
        );
    }

    #[test]
    fn git_cmd_removes_every_scrubbed_variable() {
        let cmd = git_cmd("scrub").expect("git");
        let removed: Vec<String> = cmd
            .get_envs()
            .filter(|(_, value)| value.is_none())
            .map(|(key, _)| key.to_string_lossy().into_owned())
            .collect();
        for key in GIT_ENV_TO_REMOVE.iter() {
            assert!(removed.contains(key), "{key} reaches git: {removed:?}");
        }
    }

    #[test]
    fn git_static_resolves() {
        assert!(GIT.is_ok(), "git not found: {:?}", *GIT);
    }

    #[test]
    fn git_root_is_directory() {
        let root = get_root().expect("git root");
        assert!(root.is_dir(), "root is not a directory: {}", root.display());
    }
}
