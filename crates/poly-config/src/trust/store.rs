//! The per-user trust store: which repositories may run their exec-capable keys.
//!
//! One JSON file, `trust.json`, in poly's per-user config directory
//! (`POLY_CONFIG_HOME`, else `$XDG_CONFIG_HOME/poly` or `~/.config/poly` on
//! Linux, `~/Library/Preferences/poly` on macOS, `%APPDATA%\poly` on Windows). Anyone who can write it
//! can make poly run any repository's programs, so the directory is refused when
//! another user owns it or group or others can write it, and the file is replaced
//! atomically and kept owner-only.

use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, bail};
use serde::{Deserialize, Serialize};

/// Environment variable overriding the per-user config directory the store lives in.
pub const CONFIG_HOME_ENV: &str = "POLY_CONFIG_HOME";

const STORE_FILE: &str = "trust.json";
const STORE_VERSION: u32 = 1;

/// What one trusted repository's record allows.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepoTrust {
    /// Digests of the exec-capable keys of every config in the repository, as
    /// they were when the user trusted it. A config whose keys hash to anything
    /// else is untrusted again.
    pub digests: BTreeSet<String>,
    /// Local `extends` bases outside the repository that its configs read.
    #[serde(default)]
    pub extends: BTreeSet<String>,
    /// When the record was written, in seconds since the Unix epoch.
    #[serde(default)]
    pub trusted_at: u64,
}

/// Every trust record, keyed by canonical repository root.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrustStore {
    version: u32,
    repos: BTreeMap<String, RepoTrust>,
}

impl TrustStore {
    /// The record for `root`, if it is trusted.
    #[must_use]
    pub fn get(&self, root: &Path) -> Option<&RepoTrust> {
        self.repos.get(&key(root))
    }

    /// Record `trust` for `root`, replacing any earlier record.
    pub fn insert(&mut self, root: &Path, trust: RepoTrust) {
        self.repos.insert(key(root), trust);
    }

    /// Drop the record for `root`; whether there was one.
    pub fn remove(&mut self, root: &Path) -> bool {
        self.repos.remove(&key(root)).is_some()
    }

    /// Every trusted root and its record.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &RepoTrust)> {
        self.repos.iter().map(|(root, trust)| (root.as_str(), trust))
    }

    /// Whether any record allows reading the out-of-repository `extends` base `path`.
    #[must_use]
    pub fn allows_extends(&self, path: &Path) -> bool {
        let wanted = path.to_string_lossy();
        self.repos.values().any(|trust| trust.extends.contains(wanted.as_ref()))
    }

    /// Read the store at its default location; an absent file is an empty store.
    ///
    /// # Errors
    ///
    /// When the config directory cannot be resolved or is not private, or the file
    /// cannot be read or parsed.
    pub fn load() -> anyhow::Result<TrustStore> {
        TrustStore::load_from(&store_dir()?)
    }

    /// [`TrustStore::load`] from an explicit directory.
    ///
    /// # Errors
    ///
    /// As [`TrustStore::load`].
    pub fn load_from(dir: &Path) -> anyhow::Result<TrustStore> {
        let path = dir.join(STORE_FILE);
        if !path.exists() {
            return Ok(TrustStore::default());
        }
        require_private(dir)?;
        let text = std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
        let store: TrustStore =
            serde_json::from_str(&text).with_context(|| format!("parsing the trust store {}", path.display()))?;
        if store.version != STORE_VERSION {
            bail!(
                "the trust store {} has version {}, this poly reads version {STORE_VERSION}",
                path.display(),
                store.version
            );
        }
        Ok(store)
    }

    /// Write the store to its default location.
    ///
    /// # Errors
    ///
    /// When the directory cannot be created or is not private, or the write fails.
    pub fn save(&self) -> anyhow::Result<PathBuf> {
        let dir = store_dir()?;
        self.save_to(&dir)?;
        Ok(dir.join(STORE_FILE))
    }

    /// [`TrustStore::save`] into an explicit directory: a private temporary file
    /// beside the store, synced, then renamed over it.
    ///
    /// # Errors
    ///
    /// As [`TrustStore::save`].
    pub fn save_to(&self, dir: &Path) -> anyhow::Result<()> {
        create_private(dir).with_context(|| format!("creating {}", dir.display()))?;
        require_private(dir)?;
        let store = TrustStore {
            version: STORE_VERSION,
            repos: self.repos.clone(),
        };
        let mut text = serde_json::to_string_pretty(&store)?;
        text.push('\n');
        let target = dir.join(STORE_FILE);
        let temporary = dir.join(format!(".{STORE_FILE}.{}.tmp", std::process::id()));
        let written = write_private(&temporary, text.as_bytes())
            .and_then(|()| std::fs::rename(&temporary, &target))
            .with_context(|| format!("writing {}", target.display()));
        if written.is_err() {
            let _ = std::fs::remove_file(&temporary);
        }
        written
    }
}

fn key(root: &Path) -> String {
    root.to_string_lossy().into_owned()
}

/// The directory holding `trust.json`.
///
/// # Errors
///
/// When neither `POLY_CONFIG_HOME` nor the platform config directory resolves.
pub fn store_dir() -> anyhow::Result<PathBuf> {
    if let Some(dir) = std::env::var_os(CONFIG_HOME_ENV).filter(|dir| !dir.is_empty()) {
        return Ok(PathBuf::from(dir));
    }
    use etcetera::BaseStrategy;
    let strategy = etcetera::choose_base_strategy()
        .map_err(|error| anyhow::anyhow!("could not resolve the platform config directory: {error}"))?;
    Ok(strategy.config_dir().join("poly"))
}

#[cfg(unix)]
fn create_private(dir: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    std::fs::DirBuilder::new().recursive(true).mode(0o700).create(dir)
}

#[cfg(not(unix))]
fn create_private(dir: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)
}

#[cfg(unix)]
fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::os::unix::fs::OpenOptionsExt;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)?;
    file.write_all(bytes)?;
    file.sync_all()
}

#[cfg(not(unix))]
fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut file = std::fs::OpenOptions::new().write(true).create_new(true).open(path)?;
    file.write_all(bytes)?;
    file.sync_all()
}

/// Why a directory with this `mode` and `owner` may not hold `user`'s trust store, if it may not.
#[cfg(unix)]
pub(crate) fn violation(mode: u32, owner: u32, user: u32) -> Option<&'static str> {
    if owner != user {
        Some("is not owned by the current user")
    } else if mode & 0o022 != 0 {
        Some("is writable by group or others")
    } else {
        None
    }
}

/// Refuse a store directory another account could write into.
///
/// Unlike a cache, a loose directory is never tightened here: whoever could
/// write it may already have planted a record, so the store is not read.
#[cfg(unix)]
fn require_private(dir: &Path) -> anyhow::Result<()> {
    use std::os::unix::fs::MetadataExt;
    let meta = std::fs::metadata(dir).with_context(|| format!("inspecting {}", dir.display()))?;
    if let Some(reason) = violation(meta.mode(), meta.uid(), rustix::process::geteuid().as_raw()) {
        bail!(
            "refusing the trust store directory {}: it {reason}, so another user could mark a repository \
             trusted. Make it yours and private (`chmod 700 {}`)",
            dir.display(),
            dir.display()
        );
    }
    Ok(())
}

#[cfg(not(unix))]
fn require_private(_dir: &Path) -> anyhow::Result<()> {
    Ok(())
}
