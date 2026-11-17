//! The programs the analyses of a run over several roots share ([`SharedPrograms`]).
//!
//! A root nested in another often has no `tsconfig.json` of its own, so TypeScript's discovery places its judged
//! files in the program of a config above it: the outer root's, which the outer root and every other root below that
//! config would each build again. With shared programs a program is built once: it stays, after the walk of the
//! analysis that built it, while a root of the run that has not finished holds a file the config includes, and every
//! analysis that plans files into it walks them on that build. An analysis that needs a program another one is
//! building waits for that build instead of starting its own.
//!
//! The analyses also share the files of their programs ([`SharedFiles`]): each is parsed and bound once for the run,
//! whichever program reads it first. A lib or `node_modules` declaration file stays until the run's last program and
//! the run's [`SharedPrograms`] are gone; a project file stays while a program that read it lives, and is freed with
//! the last one. They share the configs their plannings and programs read the same way ([`SharedConfigs`]).

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::time::Duration;

use tsrs_compiler::{Program, ProgramRegions};
use tsrs_core::arena::{Region, RegionScope};

use crate::ParseCounts;
use crate::build::BuildSplit;
use crate::owner::ProgramOwner;
use crate::shared_configs::SharedConfigs;
use crate::shared_files::{ProgramFiles, SharedFiles};
use crate::vfs::{BomFiles, tsrs_path};

/// The programs the analyses of one run share, so that a program the judged files of several roots go to is built
/// once instead of once per root: an outer root's `tsconfig.json` that also includes the files of the roots nested
/// below it without a config of their own.
///
/// Give the same value (or a clone, which shares it) to every analysis of the run, and only to analyses that read
/// the same files: an
/// analysis walks a shared program as the analysis that built it read it. A built program stays while a root of the
/// run that has not finished holds a file its config includes, and is freed once none does; an analysis marks its
/// root finished when its facts are returned or it is given up, and a host marks a root it analyses without types
/// with [`SharedPrograms::done`]. The programs of the run also share the files they read, and the analyses the configs
/// their plannings and programs read: each is parsed once for the run. A bundled lib file, a declaration file below
/// `node_modules` or a config is kept until these programs (every clone) and the last program that read it are
/// dropped; a project file until the last program that read it is dropped. The facts do not depend on it.
#[derive(Clone)]
pub struct SharedPrograms {
    registry: Arc<Registry>,
    files: Arc<SharedFiles>,
    configs: Arc<SharedConfigs>,
}

impl SharedPrograms {
    /// The programs shared by the analyses of `roots`, the roots of the run's analyses, absolute.
    #[must_use]
    pub fn new(roots: impl IntoIterator<Item = PathBuf>) -> Self {
        Self {
            registry: Arc::new(Registry {
                roots: roots.into_iter().map(|root| tsrs_path(&root)).collect(),
                state: Mutex::default(),
                published: Condvar::new(),
            }),
            files: Arc::default(),
            configs: Arc::default(),
        }
    }

    /// How the programs of the run got the lib and `node_modules` declaration files they read, so far: the files
    /// parsed for the run, those taken parsed from it, and those parsed for one program alone.
    #[must_use]
    pub fn shared_files(&self) -> ParseCounts {
        self.files.counts()
    }

    /// How many project files the run's programs hold now, for tests that check each is freed with the last program
    /// that read it.
    #[must_use]
    pub fn project_files_held(&self) -> usize {
        self.files.project_files_held()
    }

    /// The roots of the run's analyses, as tsrs names files.
    #[must_use]
    pub fn roots(&self) -> &BTreeSet<String> {
        &self.registry.roots
    }

    /// The files the programs of the run share.
    #[must_use]
    pub fn files(&self) -> Arc<SharedFiles> {
        Arc::clone(&self.files)
    }

    /// How the plannings and programs of the run got the configs they read, so far: the configs, then the configs
    /// they extend.
    #[must_use]
    pub fn shared_configs(&self) -> (ParseCounts, ParseCounts) {
        self.configs.counts()
    }

    /// The configs the plannings and programs of the run share.
    #[must_use]
    pub fn configs(&self) -> Arc<SharedConfigs> {
        Arc::clone(&self.configs)
    }

    /// Marks the analysis of `root` finished: a program kept only for it is freed.
    pub fn done(&self, root: &Path) {
        let root = tsrs_path(root);
        let freed: Vec<Arc<Resident>> = {
            let mut state = lock(&self.registry.state);
            state.done.insert(root.clone());
            for entry in state.entries.values_mut() {
                if let Entry::Ready { waiting, .. } = entry {
                    waiting.remove(&root);
                }
            }
            let unwanted: Vec<ProgramKey> = state
                .entries
                .iter()
                .filter(|(_, entry)| matches!(entry, Entry::Ready { waiting, .. } if waiting.is_empty()))
                .map(|(key, _)| key.clone())
                .collect();
            unwanted
                .iter()
                .filter_map(|key| match state.entries.remove(key) {
                    Some(Entry::Ready { resident, .. }) => Some(resident),
                    _ => None,
                })
                .collect()
        };
        // Freed outside the lock: freeing a large program takes a while.
        drop(freed);
    }

    /// How many programs were built through these programs, for tests that check each is built once.
    #[must_use]
    pub fn builds(&self) -> usize {
        lock(&self.registry.state).builds
    }

    /// How many built programs are kept for roots that have not finished, for tests that check none outlives them.
    #[must_use]
    pub fn kept(&self) -> usize {
        lock(&self.registry.state)
            .entries
            .values()
            .filter(|entry| matches!(entry, Entry::Ready { .. }))
            .count()
    }

    /// The program of `key`: built already, or to be built by the caller, who holds the claim until it publishes the
    /// program. Waits while another analysis builds it; that build holds its cores already and waits for no host, so
    /// the wait ends.
    #[must_use]
    pub fn claim(&self, key: &ProgramKey) -> Claim<'_> {
        let registry = &*self.registry;
        let mut state = lock(&registry.state);
        loop {
            let found = match state.entries.get(key) {
                Some(Entry::Building) => Found::Building,
                Some(Entry::Ready { resident, .. }) if !resident.owner.poisoned() => Found::Ready(Arc::clone(resident)),
                Some(Entry::Ready { .. }) | None => Found::Absent,
            };
            match found {
                Found::Building => state = registry.published.wait(state).unwrap_or_else(PoisonError::into_inner),
                Found::Ready(resident) => return Claim::Ready(resident),
                Found::Absent => {
                    state.entries.insert(key.clone(), Entry::Building);
                    state.builds += 1;
                    return Claim::Build(Ticket {
                        registry,
                        key: key.clone(),
                        published: false,
                    });
                }
            }
        }
    }

    /// Notes that the analysis of `root` walked `resident`, and frees it once no other root may need it.
    pub fn walked(&self, resident: &Arc<Resident>, root: &str) {
        let freed = {
            let mut state = lock(&self.registry.state);
            let key = &resident.key;
            let Some(Entry::Ready {
                resident: kept,
                waiting,
            }) = state.entries.get_mut(key)
            else {
                return;
            };
            if !Arc::ptr_eq(kept, resident) {
                return;
            }
            waiting.remove(root);
            if !waiting.is_empty() {
                return;
            }
            state.entries.remove(key)
        };
        drop(freed);
    }
}

impl std::fmt::Debug for SharedPrograms {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SharedPrograms")
            .field("roots", &self.registry.roots)
            .finish_non_exhaustive()
    }
}

/// What the analyses sharing the programs share.
struct Registry {
    /// The roots of the run, as tsrs names files.
    roots: BTreeSet<String>,
    state: Mutex<State>,
    /// Notified when a build is published or given up.
    published: Condvar,
}

impl Registry {
    /// The roots other than `builder` that hold one of `files`, as tsrs names them.
    fn holding(&self, files: &[String], builder: &str) -> BTreeSet<String> {
        let mut holding = BTreeSet::new();
        let mut seen: BTreeSet<&str> = BTreeSet::new();
        for file in files {
            let mut dir = file.as_str();
            while let Some(slash) = dir.rfind('/') {
                dir = &dir[..slash];
                // The directories above one already seen were seen with it.
                if !seen.insert(dir) {
                    break;
                }
                if dir != builder && self.roots.contains(dir) {
                    holding.insert(dir.to_owned());
                }
            }
        }
        holding
    }
}

#[derive(Default)]
struct State {
    /// The roots whose analyses finished.
    done: BTreeSet<String>,
    entries: BTreeMap<ProgramKey, Entry>,
    builds: usize,
}

enum Entry {
    /// An analysis is building the program.
    Building,
    /// Built, and kept for the roots of `waiting`.
    Ready {
        resident: Arc<Resident>,
        /// The roots that have not finished and hold a file the config includes, other than those that walked it.
        waiting: BTreeSet<String>,
    },
}

/// What [`SharedPrograms::claim`] found under the lock.
enum Found {
    Building,
    Ready(Arc<Resident>),
    Absent,
}

/// A program as the analyses that share it ask for it: its config, by its canonical path as tsrs names files, and
/// whether tsrs runs it single-threaded.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct ProgramKey {
    config: String,
    single_threaded: bool,
}

impl ProgramKey {
    /// The key of the program of `config`, by its canonical path as tsrs names files.
    #[must_use]
    pub fn new(config: &str, single_threaded: bool) -> Self {
        Self {
            config: config.to_owned(),
            single_threaded,
        }
    }
}

/// How an analysis gets a program it needs.
pub enum Claim<'a> {
    /// Another analysis built it.
    Ready(Arc<Resident>),
    /// The caller builds it and publishes it with the ticket.
    Build(Ticket<'a>),
}

/// The claim of the analysis that builds a program. Dropped unpublished (the config cannot be read, a panic, a
/// cancelled run), it gives the build up, and the next analysis that needs the program builds it.
pub struct Ticket<'a> {
    registry: &'a Registry,
    key: ProgramKey,
    published: bool,
}

impl Ticket<'_> {
    /// Publishes `resident`, the program built from the config that includes `files`, for the roots of the run other
    /// than `builder` that hold one of them and have not finished; with none, nothing keeps it beyond the builder's
    /// walk.
    pub fn publish(mut self, resident: &Arc<Resident>, files: &[String], builder: &str) {
        let holding = self.registry.holding(files, builder);
        let mut state = lock(&self.registry.state);
        let waiting: BTreeSet<String> = holding.into_iter().filter(|root| !state.done.contains(root)).collect();
        if waiting.is_empty() {
            state.entries.remove(&self.key);
        } else {
            state.entries.insert(
                self.key.clone(),
                Entry::Ready {
                    resident: Arc::clone(resident),
                    waiting,
                },
            );
        }
        self.published = true;
        drop(state);
        self.registry.published.notify_all();
    }
}

impl Drop for Ticket<'_> {
    fn drop(&mut self) {
        if self.published {
            return;
        }
        let mut state = lock(&self.registry.state);
        if matches!(state.entries.get(&self.key), Some(Entry::Building)) {
            state.entries.remove(&self.key);
        }
        drop(state);
        self.registry.published.notify_all();
    }
}

/// A built and bound program with what its walks need, freed when the last analysis walking it lets it go. Its fields
/// drop in order: the program, then the loader regions and the scratch region its config and host were allocated in,
/// which the program points into, and last the shared files and configs the program read, which everything before
/// points into.
pub struct Resident {
    owner: ProgramOwner,
    _loader_regions: Arc<ProgramRegions>,
    region: Region,
    boms: Arc<BomFiles>,
    key: ProgramKey,
    checkers: usize,
    build: Duration,
    split: BuildSplit,
    project_files: Vec<String>,
    /// The project files the program read, also held by its owner: this hold lets them go after the loader regions
    /// and the region of the program's config and host, and before the run's shared files.
    _project_files: Arc<ProgramFiles>,
    _files: Arc<SharedFiles>,
    _configs: Arc<SharedConfigs>,
}

impl Resident {
    /// The program of `owner`, built in `region` with `loader_regions` on `checkers` checkers in `build`, reading
    /// through the file system that recorded `boms`, the shared files of `files` (holding the project files it read
    /// until the resident is dropped) and the shared configs of `configs`.
    #[must_use]
    pub fn new(
        (owner, loader_regions, region): (ProgramOwner, Arc<ProgramRegions>, Region),
        (boms, files, configs): (Arc<BomFiles>, Arc<SharedFiles>, Arc<SharedConfigs>),
        key: ProgramKey,
        checkers: usize,
        (build, split, project_files): (Duration, BuildSplit, Vec<String>),
    ) -> Self {
        let held = owner.files();
        Self {
            owner,
            _loader_regions: loader_regions,
            region,
            boms,
            key,
            checkers,
            build,
            split,
            project_files,
            _project_files: held,
            _files: files,
            _configs: configs,
        }
    }

    /// The program, for as long as the resident lives.
    #[must_use]
    pub fn program(&self) -> &Program {
        self.owner.program()
    }

    /// The files of the program whose text starts with a byte order mark.
    #[must_use]
    pub fn boms(&self) -> &BomFiles {
        &self.boms
    }

    /// The checkers the program was built on.
    #[must_use]
    pub const fn checkers(&self) -> usize {
        self.checkers
    }

    /// Reading the config, building and binding the program.
    #[must_use]
    pub const fn build(&self) -> Duration {
        self.build
    }

    /// Where [`Resident::build`] went.
    #[must_use]
    pub const fn split(&self) -> BuildSplit {
        self.split
    }

    /// The names of the project files the program parsed; empty unless timings were on.
    #[must_use]
    pub fn project_files(&self) -> &[String] {
        &self.project_files
    }

    /// Enters the program's region for a walk, waiting while another analysis walks it: what the walking thread
    /// allocates lives as long as the program. A walk that panics poisons the program.
    #[must_use]
    pub fn walking(&self) -> Walking<'_> {
        Walking {
            resident: self,
            _scope: self.region.enter(),
        }
    }
}

/// A walk of a [`Resident`]; see [`Resident::walking`].
pub struct Walking<'a> {
    resident: &'a Resident,
    _scope: RegionScope,
}

impl Drop for Walking<'_> {
    fn drop(&mut self) {
        if std::thread::panicking() {
            self.resident.owner.poison();
        }
    }
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn registry(roots: &[&str]) -> Registry {
        Registry {
            roots: roots.iter().map(|root| (*root).to_owned()).collect(),
            state: Mutex::default(),
            published: Condvar::new(),
        }
    }

    fn files(names: &[&str]) -> Vec<String> {
        names.iter().map(|name| (*name).to_owned()).collect()
    }

    #[test]
    fn the_roots_holding_a_file_of_a_config_are_every_root_above_one_of_its_files_but_the_builder() {
        let registry = registry(&["/ws", "/ws/test/a", "/ws/test/a/inner", "/ws/test/b", "/ws/examples/c"]);
        let cases = [
            (
                "nested roots below the builder",
                files(&["/ws/src/x.ts", "/ws/test/a/inner/y.ts", "/ws/test/b/z.ts"]),
                "/ws",
                vec!["/ws/test/a", "/ws/test/a/inner", "/ws/test/b"],
            ),
            (
                "the outer root of a nested builder",
                files(&["/ws/src/x.ts", "/ws/test/b/z.ts"]),
                "/ws/test/b",
                vec!["/ws"],
            ),
            ("no other root", files(&["/ws/src/x.ts"]), "/ws", vec![]),
            (
                "a directory named like a root's prefix",
                files(&["/ws/test/ab/x.ts"]),
                "/ws",
                vec![],
            ),
        ];
        for (case, files, builder, expected) in cases {
            let expected: BTreeSet<String> = expected.into_iter().map(str::to_owned).collect();
            assert_eq!(registry.holding(&files, builder), expected, "{case}");
        }
    }
}
