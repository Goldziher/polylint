//! Repository trust: whether a config may run programs (ADR 0059).
//!
//! A cloned repository's `poly.toml` can name executables, their argv and
//! environment, download URLs, whole-project commands and cache directories.
//! Honouring those on a plain `poly lint` turns "lint this checkout" into "run
//! this checkout's code". So a config's exec-capable keys ([`keys`]) are honoured
//! only when the user has trusted the repository with `poly trust`, which records
//! a digest of those keys' values in the per-user [`store`]. Any change to them
//! makes the repository untrusted again, as `direnv allow` does for `.envrc`.
//!
//! Untrusted, the keys are ignored rather than fatal: [`gate`] removes them from
//! the merged table before it is deserialized, keeps `[hooks]` but marks it
//! withheld ([`TrustState`]), and records a note the CLI prints once per run.
//!
//! Enforcement is process-wide and off until a front end turns it on
//! ([`set_mode`]): `poly` enforces for every command, `--trust`/`POLY_TRUST=1`
//! trust every repository for one run, and `poly mcp` enforces with no override.

mod keys;
pub mod store;

use std::cell::RefCell;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};

pub use keys::is_bare_program;
pub use store::{RepoTrust, TrustStore};

use crate::BaseConfigResolver;

/// Environment variable that trusts every repository for one run (CI).
pub const TRUST_ENV: &str = "POLY_TRUST";

/// How a process treats exec-capable config keys.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrustMode {
    /// Honour every key without consulting the store: a library caller or a test
    /// that never chose a mode.
    Unenforced,
    /// Honour a config's keys only when the store trusts their current values.
    Enforce,
    /// Honour every key for this run (`--trust`, `POLY_TRUST=1`).
    TrustAll,
}

static MODE: AtomicU8 = AtomicU8::new(0);

/// Set how this process treats exec-capable keys. The last call wins, so a front
/// end that must not be overridden (`poly mcp`) sets its mode after the flags.
pub fn set_mode(mode: TrustMode) {
    let value = match mode {
        TrustMode::Unenforced => 0,
        TrustMode::Enforce => 1,
        TrustMode::TrustAll => 2,
    };
    MODE.store(value, Ordering::Relaxed);
}

/// The process's [`TrustMode`].
#[must_use]
pub fn mode() -> TrustMode {
    match MODE.load(Ordering::Relaxed) {
        1 => TrustMode::Enforce,
        2 => TrustMode::TrustAll,
        _ => TrustMode::Unenforced,
    }
}

/// Whether `POLY_TRUST` asks to trust every repository for this run.
#[must_use]
pub fn env_requests_trust() -> bool {
    std::env::var(TRUST_ENV).is_ok_and(|value| crate::network::is_truthy(&value))
}

/// What trust left of one resolved config.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TrustState {
    /// The dotted exec-capable keys this config set and poly ignored because the
    /// repository is not trusted. Empty when nothing was withheld.
    pub withheld: Vec<String>,
    /// Whether `[hooks]` is among them: kept in the typed config, but its
    /// whole-project tools are not run and `poly hooks run` refuses it.
    pub hooks_withheld: bool,
    /// The repository root the decision was made for, when anything was withheld.
    pub root: Option<PathBuf>,
}

impl TrustState {
    /// Whether any key of this config was withheld.
    #[must_use]
    pub fn is_restricted(&self) -> bool {
        !self.withheld.is_empty()
    }
}

/// `path` canonicalized, or as given when it cannot be.
pub(crate) fn canonical(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// The repository a config in `dir` belongs to: its git root, else `dir` itself.
#[must_use]
pub fn trust_root(dir: &Path) -> PathBuf {
    let dir = canonical(dir);
    super::git_root(&dir).unwrap_or(dir)
}

/// Withhold the exec-capable keys of `table`, the merged config governing `dir`,
/// unless the process mode or the store allows them.
pub(crate) fn gate(table: &mut toml::Table, dir: &Path) -> TrustState {
    if mode() != TrustMode::Enforce {
        return TrustState::default();
    }
    let entries = keys::exec_entries(table);
    if entries.is_empty() {
        return TrustState::default();
    }
    let root = trust_root(dir);
    let digest = keys::digest(&root, dir, &entries);
    if cached_store().is_some_and(|store| store.get(&root).is_some_and(|trust| trust.digests.contains(&digest))) {
        return TrustState::default();
    }
    keys::strip(table, &entries);
    let withheld: Vec<String> = entries.iter().map(keys::Entry::display).collect();
    notes::record(&root, withheld.iter().cloned());
    TrustState {
        hooks_withheld: entries.iter().any(keys::Entry::is_hooks),
        withheld,
        root: Some(root),
    }
}

thread_local! {
    /// Out-of-repository `extends` bases met while `poly trust` assesses a repository.
    static COLLECTED: RefCell<Option<BTreeSet<String>>> = const { RefCell::new(None) };
}

/// Whether the local `extends` base at `base`, declared by a config in
/// `declaring_dir`, may be read. One inside the declaring config's repository
/// always may; one outside it only when a trust record names it, so an untrusted
/// config cannot read arbitrary files through `extends`.
pub(crate) fn admit_local_base(base: &Path, declaring_dir: &Path) -> bool {
    let base = canonical(base);
    let root = trust_root(declaring_dir);
    if base.starts_with(&root) {
        return true;
    }
    let collecting = COLLECTED.with_borrow_mut(|collected| match collected {
        Some(set) => {
            set.insert(base.to_string_lossy().into_owned());
            true
        }
        None => false,
    });
    if collecting || mode() != TrustMode::Enforce {
        return true;
    }
    if cached_store().is_some_and(|store| store.allows_extends(&base)) {
        return true;
    }
    notes::record(&root, [format!("extends ({})", base.display())]);
    false
}

fn store_slot() -> &'static Mutex<Option<Arc<TrustStore>>> {
    static STORE: OnceLock<Mutex<Option<Arc<TrustStore>>>> = OnceLock::new();
    STORE.get_or_init(|| Mutex::new(None))
}

/// Make `store`, just written, the one this process consults from now on.
pub fn use_store(store: TrustStore) {
    *store_slot().lock().unwrap_or_else(PoisonError::into_inner) = Some(Arc::new(store));
}

/// The store as this process first read it, or `None` (with a note) when it is unreadable.
fn cached_store() -> Option<Arc<TrustStore>> {
    let mut slot = store_slot().lock().unwrap_or_else(PoisonError::into_inner);
    if slot.is_none() {
        match TrustStore::load() {
            Ok(store) => *slot = Some(Arc::new(store)),
            Err(error) => {
                notes::store_error(format!("{error:#}"));
                return None;
            }
        }
    }
    slot.clone()
}

/// One config of a repository as `poly trust` sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigAssessment {
    /// The directory holding the `poly.toml`.
    pub dir: PathBuf,
    /// The exec-capable keys its merged config sets.
    pub keys: Vec<String>,
    /// The digests a trust record must hold for those keys to be honoured: one
    /// for the hierarchical config, one for the file loaded alone (`--config`).
    pub digests: BTreeSet<String>,
}

/// A repository's exec-capable configuration, as `poly trust` records it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoAssessment {
    /// The repository root the record is keyed by.
    pub root: PathBuf,
    /// Every config in the repository, in the order given.
    pub configs: Vec<ConfigAssessment>,
    /// Local `extends` bases outside the repository that the configs read.
    pub extends_outside: BTreeSet<String>,
}

impl RepoAssessment {
    /// The trust record that allows exactly this configuration.
    #[must_use]
    pub fn record(&self, trusted_at: u64) -> RepoTrust {
        RepoTrust {
            digests: self.configs.iter().flat_map(|config| config.digests.clone()).collect(),
            extends: self.extends_outside.clone(),
            trusted_at,
        }
    }

    /// Whether `record` allows every exec-capable key and `extends` base found.
    #[must_use]
    pub fn is_trusted_by(&self, record: Option<&RepoTrust>) -> bool {
        let configs_ok = self.configs.iter().all(|config| {
            config.keys.is_empty()
                || record.is_some_and(|record| config.digests.iter().any(|digest| record.digests.contains(digest)))
        });
        let extends_ok = self
            .extends_outside
            .iter()
            .all(|base| record.is_some_and(|record| record.extends.contains(base)));
        configs_ok && extends_ok
    }
}

/// Assess the repository at `root` whose `poly.toml` files sit in `config_dirs`:
/// each config's exec-capable keys and digests, and every `extends` base outside it.
///
/// # Errors
///
/// When a config cannot be read or resolved.
pub fn assess(
    root: &Path,
    config_dirs: &[PathBuf],
    resolver: &dyn BaseConfigResolver,
) -> anyhow::Result<RepoAssessment> {
    COLLECTED.with_borrow_mut(|collected| *collected = Some(BTreeSet::new()));
    let configs = config_dirs
        .iter()
        .map(|dir| assess_config(root, dir, resolver))
        .collect::<anyhow::Result<Vec<_>>>();
    let extends_outside = COLLECTED.with_borrow_mut(Option::take).unwrap_or_default();
    Ok(RepoAssessment {
        root: root.to_path_buf(),
        configs: configs?,
        extends_outside,
    })
}

fn assess_config(root: &Path, dir: &Path, resolver: &dyn BaseConfigResolver) -> anyhow::Result<ConfigAssessment> {
    let mut digests = BTreeSet::new();
    let mut names = BTreeSet::new();
    if let Some((table, nearest)) = super::cascade_table(dir, resolver)? {
        let entries = keys::exec_entries(&table);
        if !entries.is_empty() {
            digests.insert(keys::digest(root, &nearest, &entries));
            names.extend(entries.iter().map(keys::Entry::display));
        }
    }
    if let Some(file) = super::config_file_in(dir) {
        let table = super::read_config_table(&file, resolver, &mut Vec::new())?;
        let entries = keys::exec_entries(&table);
        if !entries.is_empty() {
            digests.insert(keys::digest(root, dir, &entries));
            names.extend(entries.iter().map(keys::Entry::display));
        }
    }
    Ok(ConfigAssessment {
        dir: dir.to_path_buf(),
        keys: names.into_iter().collect(),
        digests,
    })
}

/// The run's one note about what an untrusted repository's config could not do.
pub mod notes {
    use std::collections::{BTreeMap, BTreeSet};
    use std::path::{Path, PathBuf};
    use std::sync::{Mutex, OnceLock, PoisonError};

    use super::TRUST_ENV;

    #[derive(Default)]
    struct Notes {
        withheld: BTreeMap<PathBuf, BTreeSet<String>>,
        store_error: Option<String>,
    }

    fn notes() -> &'static Mutex<Notes> {
        static NOTES: OnceLock<Mutex<Notes>> = OnceLock::new();
        NOTES.get_or_init(|| Mutex::new(Notes::default()))
    }

    pub(super) fn record(root: &Path, keys: impl IntoIterator<Item = String>) {
        let mut notes = notes().lock().unwrap_or_else(PoisonError::into_inner);
        notes.withheld.entry(root.to_path_buf()).or_default().extend(keys);
    }

    pub(super) fn store_error(error: String) {
        notes().lock().unwrap_or_else(PoisonError::into_inner).store_error = Some(error);
    }

    /// Take the lines describing every key withheld so far, one per repository,
    /// plus one for an unreadable store. Empty when nothing was withheld.
    #[must_use]
    pub fn take() -> Vec<String> {
        let mut notes = notes().lock().unwrap_or_else(PoisonError::into_inner);
        let mut lines: Vec<String> = std::mem::take(&mut notes.withheld)
            .into_iter()
            .map(|(root, keys)| {
                format!(
                    "note: {} is not trusted, so poly ignored these config keys that can run programs: {}; \
                     run `poly trust` to allow them (or set {TRUST_ENV}=1 for one run)",
                    root.display(),
                    keys.into_iter().collect::<Vec<_>>().join(", ")
                )
            })
            .collect();
        if let Some(error) = notes.store_error.take() {
            lines.push(format!(
                "warning: cannot read the trust store, so no repository is trusted: {error}"
            ));
        }
        lines
    }
}

#[cfg(test)]
mod tests;
