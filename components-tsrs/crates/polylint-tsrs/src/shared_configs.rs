//! The configs the plannings and programs of a run share (polylint ADR 0078): each `tsconfig.json` or
//! `jsconfig.json` a planning (a host's search for the configs of its files) searches or a program references, and
//! each config one of those extends, is read and parsed once for the run instead of once by each planning and again
//! by every program whose project references reach it.
//!
//! # Why sharing is sound
//!
//! - **Ownership.** A shared config is parsed in a registered region of its own ([`Region::new`]), which the run's
//!   [`SharedConfigs`] holds. Every planning holds the cache through the analysis's [`ConfigReader`], and every program
//!   through its compiler host and whatever holds it for its walks (its resident), which drops it after the
//!   program, so the region outlives everything that points into it. Freeing a program (`free_unshared_program`)
//!   drops its project reference mapper, a heap box of pointers to the configs it references; it frees no config, so
//!   freeing one program never frees or touches a config another program reads.
//! - **Identical parse.** A config's parse is a function of the texts and directory listings of one file system (the
//!   condition for sharing a [`SharedConfigs`]) and of its absolute name: tsrs reads the current
//!   directory of the parse host only to make a name absolute, and the names asked for are absolute. A program takes
//!   the config it is built from as a copy of the run's parse ([`ConfigReader::parse_own`]), which it owns and may
//!   change, so the program's own config is never one of the shared configs it references. The configs a config
//!   extends come from the cache keyed by their path: tsrs itself bypasses the cache for a config that extends itself
//!   through the chain being parsed (the resolution stack), which is the one case where the parse depends on who asks.
//! - **Writes.** A program or a planning writes into a config only its lazily computed tables (`file_names_by_path`,
//!   the output-name maps, the common source directory), each set once behind a `OnceLock` from the config alone. tsrs
//!   allocates the arena data of those in the region that owns the config (`enter_owner`), found through the region
//!   registry, so whichever program fills them first leaves them in the config's region and not in its own. This is
//!   how the language server of tsrs shares parsed configs between its projects.
//! - **Threads.** The cache is `Send + Sync` and never makes a thread wait for another's parse: a request for a config
//!   another thread is parsing parses it for itself alone. Parsing expands `include` globs on rayon, where a waiting
//!   thread may take the job of another program's build that asks for the very config it is parsing; waiting there
//!   would never end. A request inside a checker's scratch region also parses the config for itself, as the shared
//!   declaration files do (`shared_files`).

use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::sync::{Arc, Mutex, PoisonError};

use tsrs_ast::Diagnostic;
use tsrs_core::arena::{self, Region};
use tsrs_core::{P, tspath};
use tsrs_tsoptions::{ExtendedConfigCache, ExtendedConfigCacheEntry, ParseConfigHost, ParsedCommandLine};
use tsrs_vfs::FS;

use crate::ParseCounts;
use crate::shared_files::Counts;

/// The host configs are parsed with: an analysis's file system and the directory relative config names are resolved
/// against (the workspace root).
#[derive(Clone)]
pub struct ConfigHost {
    fs: Arc<dyn FS>,
    cwd: String,
}

impl ConfigHost {
    /// A host reading through `fs` ([`wrap_fs`](crate::wrap_fs)) with the current directory `cwd`, as tsrs names
    /// files.
    #[must_use]
    pub fn new(fs: Arc<dyn FS>, cwd: String) -> Self {
        Self { fs, cwd }
    }

    /// `file` as tsrs keys files: absolute against the current directory, in the file system's case.
    #[must_use]
    pub fn to_path(&self, file: &str) -> tspath::Path {
        tspath::to_path(file, &self.cwd, self.fs.use_case_sensitive_file_names())
    }
}

impl ParseConfigHost for ConfigHost {
    fn fs(&self) -> &dyn FS {
        &*self.fs
    }

    fn get_current_directory(&self) -> &str {
        &self.cwd
    }
}

/// The first chunk of a shared config's region; it grows as the parse needs and is trimmed to what the parse used.
const FIRST_CHUNK: usize = 16 << 10;

/// The shared configs of a run: those of the plannings and project references by name, and the configs they extend
/// by path, each read and parsed once and kept until the value is dropped. Only analyses that read one file system
/// may share a value.
#[derive(Default)]
pub struct SharedConfigs {
    configs: Mutex<HashMap<String, Slot<Parsed>>>,
    extended: Mutex<HashMap<tspath::Path, Slot<P<ExtendedConfigCacheEntry>>>>,
    counts: ConfigCounts,
}

/// One config of the cache: claimed by the request that parses it, then parsed.
enum Slot<T> {
    Parsing,
    Parsed(Kept<T>),
}

/// A parse with the region that holds it, freed with the cache.
struct Kept<T> {
    value: T,
    _region: Region,
}

/// A config as a planning or a program reads it. The config and the diagnostics point into the run's
/// [`SharedConfigs`] or, for a config parsed for the request alone, into the region current at the request.
#[derive(Clone)]
#[non_exhaustive]
pub struct Parsed {
    /// The parsed config; `None` when it cannot be read.
    pub config: Option<P<ParsedCommandLine>>,
    /// Why a planning cannot use it: why it cannot be read, or its syntax errors.
    pub problem: Option<String>,
    /// The diagnostics `problem` words: those of reading the config, else the syntax errors of its file. Empty when
    /// the config is usable, and when it cannot be read without tsrs saying why.
    pub diagnostics: Vec<P<Diagnostic>>,
}

impl Parsed {
    /// The config, when a planning can use it.
    #[must_use]
    pub fn usable(&self) -> Option<P<ParsedCommandLine>> {
        if self.problem.is_some() { None } else { self.config }
    }
}

/// How the requests of one analysis for configs and for the configs they extend got them.
#[derive(Debug, Default)]
pub struct ConfigCounts {
    configs: Counts,
    extended: Counts,
}

impl ConfigCounts {
    /// The counts of the configs and of the configs they extend.
    #[must_use]
    pub fn snapshot(&self) -> (ParseCounts, ParseCounts) {
        (self.configs.snapshot(), self.extended.snapshot())
    }
}

impl SharedConfigs {
    /// The cache's counts for the run so far: the configs, then the configs they extend.
    #[must_use]
    pub fn counts(&self) -> (ParseCounts, ParseCounts) {
        self.counts.snapshot()
    }
}

/// How one analysis reads configs: from the run's cache, through its file system, counting in its own counts too.
pub struct ConfigReader {
    configs: Arc<SharedConfigs>,
    counts: Arc<ConfigCounts>,
    host: ConfigHost,
}

impl ConfigReader {
    /// A reader of `configs` through `host`, counting in `counts`.
    #[must_use]
    pub fn new(configs: Arc<SharedConfigs>, counts: Arc<ConfigCounts>, host: ConfigHost) -> Self {
        Self { configs, counts, host }
    }

    /// The host configs are parsed with.
    #[must_use]
    pub fn host(&self) -> &ConfigHost {
        &self.host
    }

    /// The shared configs, which a program that read one of them keeps.
    #[must_use]
    pub fn configs(&self) -> Arc<SharedConfigs> {
        Arc::clone(&self.configs)
    }

    /// The counts of the analysis: the configs, then the configs they extend.
    #[must_use]
    pub fn counts(&self) -> (ParseCounts, ParseCounts) {
        self.counts.snapshot()
    }

    /// The config named `file_name`, parsed once for the run: by this request, by an earlier one, or, while another
    /// thread parses it or inside a checker's scratch region, for this request alone.
    #[must_use]
    pub fn config(&self, file_name: &str) -> Parsed {
        let name = self.name(file_name);
        self.cached(&name).unwrap_or_else(|| self.unshared(&name))
    }

    /// The config named `file_name` as [`ConfigReader::config`] gives it, or `None` where that would parse it for this
    /// request alone. A planning reads its configs ahead so on the threads of its pool, where a request may come from a
    /// thread in another program's memory scope, which a config parsed for the request alone would point into.
    #[must_use]
    pub fn prefetch(&self, file_name: &str) -> Option<Parsed> {
        self.cached(&self.name(file_name))
    }

    /// A command line of the config at `file_name` that the caller owns and may change; `None` when it cannot be read
    /// or parsed. A program takes its own config so: a copy of the run's parse (the parsed options, which it replaces
    /// in part, and the file names, with the lazily computed tables left empty to be filled for the copy), and only
    /// where the run's cache cannot give the parse, the config is parsed into the current region.
    #[must_use]
    pub fn parse_own(&self, file_name: &str) -> Option<ParsedCommandLine> {
        self.load_own(file_name).ok()
    }

    /// The command line [`ConfigReader::parse_own`] gives, or the diagnostics that say why there is none: those of
    /// reading the config, else the syntax errors of its file; empty when it cannot be read without tsrs saying why.
    /// The errors tsrs keeps in a config that parses (an `extends` it cannot resolve, an unknown option) stay in the
    /// command line's `errors`, where the caller may read and change them before it builds the program.
    ///
    /// # Errors
    /// The diagnostics of a config that cannot be read or parsed.
    pub fn load_own(&self, file_name: &str) -> Result<ParsedCommandLine, Vec<P<Diagnostic>>> {
        if let Some(parsed) = self.cached(&self.name(file_name)) {
            return match parsed.usable() {
                Some(config) => Ok(config.with_file_names(config.file_names().to_vec())),
                None => Err(parsed.diagnostics),
            };
        }
        let (config, diagnostics) = self.parse(file_name);
        let (problem, diagnostics) = problem(file_name, config.as_ref(), &diagnostics);
        match config {
            Some(config) if problem.is_none() => Ok(config),
            _ => Err(diagnostics),
        }
    }

    fn name(&self, file_name: &str) -> String {
        tspath::get_normalized_absolute_path(file_name, self.host.get_current_directory())
    }

    /// The config `name` from the cache, parsed by this request or an earlier one; `None` while another thread parses
    /// it or inside a checker's scratch region.
    fn cached(&self, name: &str) -> Option<Parsed> {
        if arena::scratch_active() {
            return None;
        }
        match claim(&self.configs.configs, &name.to_owned(), Parsed::clone) {
            Claim::Kept(parsed) => {
                self.count(|counts| counts.configs.count_reused());
                Some(parsed)
            }
            Claim::Claimed => Some(self.parse_shared(name.to_owned())),
            Claim::Busy => None,
        }
    }

    /// Parses the config `name`, claimed by this request, into a region of its own and keeps it for the run.
    fn parse_shared(&self, name: String) -> Parsed {
        let region = Region::new(FIRST_CHUNK);
        let parsed = {
            let _scope = region.enter();
            self.parsed(&name)
        };
        region.trim();
        keep(&self.configs.configs, name, parsed.clone(), region);
        self.count(|counts| counts.configs.count_parsed());
        parsed
    }

    /// Parses the config `name` into the current region for this request alone.
    fn unshared(&self, name: &str) -> Parsed {
        self.count(|counts| counts.configs.count_unshared());
        self.parsed(name)
    }

    fn parsed(&self, name: &str) -> Parsed {
        let (config, diagnostics) = self.parse(name);
        let (problem, diagnostics) = problem(name, config.as_ref(), &diagnostics);
        Parsed {
            config: config.map(P::new),
            problem,
            diagnostics,
        }
    }

    fn parse(&self, file_name: &str) -> (Option<ParsedCommandLine>, Vec<P<Diagnostic>>) {
        tsrs_tsoptions::get_parsed_command_line_of_config_file(
            file_name,
            None,
            None,
            &self.host,
            Some(self as &dyn ExtendedConfigCache),
        )
    }

    /// Counts in the run's counts and in the analysis's.
    fn count(&self, add: impl Fn(&ConfigCounts)) {
        add(&self.configs.counts);
        add(&self.counts);
    }
}

impl ExtendedConfigCache for ConfigReader {
    fn get_extended_config(
        &self,
        file_name: &str,
        path: &tspath::Path,
        resolution_stack: &[tspath::Path],
        host: &dyn ParseConfigHost,
    ) -> P<ExtendedConfigCacheEntry> {
        let parse = || {
            tsrs_tsoptions::parse_extended_config(
                file_name,
                path.clone(),
                resolution_stack,
                host,
                Some(self as &dyn ExtendedConfigCache),
            )
        };
        if arena::scratch_active() {
            self.count(|counts| counts.extended.count_unshared());
            return parse();
        }
        match claim(&self.configs.extended, path, |entry| *entry) {
            Claim::Kept(entry) => {
                self.count(|counts| counts.extended.count_reused());
                entry
            }
            Claim::Claimed => {
                let region = Region::new(FIRST_CHUNK);
                let entry = {
                    let _scope = region.enter();
                    parse()
                };
                region.trim();
                keep(&self.configs.extended, path.clone(), entry, region);
                self.count(|counts| counts.extended.count_parsed());
                entry
            }
            // Another thread parses it, and may be waiting for a config this thread parses (configs that extend each
            // other): parse it for this request alone.
            Claim::Busy => {
                self.count(|counts| counts.extended.count_unshared());
                parse()
            }
        }
    }
}

/// What a request found in a cache of [`Slot`]s.
enum Claim<T> {
    /// The parse kept for the run.
    Kept(T),
    /// Nothing: the request claimed the slot and parses it for the run.
    Claimed,
    /// Another request is parsing it.
    Busy,
}

/// Looks `key` up in `slots`, claiming the slot when it is empty; `copy` copies a kept parse out.
fn claim<K: Eq + std::hash::Hash + Clone, T>(
    slots: &Mutex<HashMap<K, Slot<T>>>,
    key: &K,
    copy: impl FnOnce(&T) -> T,
) -> Claim<T> {
    let mut slots = lock(slots);
    match slots.entry(key.clone()) {
        Entry::Occupied(entry) => match entry.get() {
            Slot::Parsed(kept) => Claim::Kept(copy(&kept.value)),
            Slot::Parsing => Claim::Busy,
        },
        Entry::Vacant(entry) => {
            entry.insert(Slot::Parsing);
            Claim::Claimed
        }
    }
}

/// Keeps `value`, parsed in `region`, in the slot of `key` that this request claimed.
fn keep<K: Eq + std::hash::Hash, T>(slots: &Mutex<HashMap<K, Slot<T>>>, key: K, value: T, region: Region) {
    lock(slots).insert(key, Slot::Parsed(Kept { value, _region: region }));
}

/// Why a planning cannot use the config at `path`, parsed as `config` with `diagnostics`: what made it unreadable, or
/// its syntax errors, in words and as the diagnostics worded.
fn problem(
    path: &str,
    config: Option<&ParsedCommandLine>,
    diagnostics: &[P<Diagnostic>],
) -> (Option<String>, Vec<P<Diagnostic>>) {
    if !diagnostics.is_empty() {
        return (Some(messages(diagnostics)), diagnostics.to_vec());
    }
    let Some(config) = config else {
        return (Some(format!("cannot read {path}")), Vec::new());
    };
    let Some(file) = config.config_file else {
        return (None, Vec::new());
    };
    let syntax = file.source_file.diagnostics();
    if syntax.is_empty() {
        (None, Vec::new())
    } else {
        (Some(messages(syntax)), syntax.to_vec())
    }
}

fn messages(diagnostics: &[P<Diagnostic>]) -> String {
    diagnostics
        .iter()
        .map(|diagnostic| diagnostic.localize())
        .collect::<Vec<_>>()
        .join("; ")
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

const _: () = {
    const fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<SharedConfigs>();
    assert_send_sync::<ConfigReader>();
};

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use tsrs_core::CompilerOptions;

    use super::*;
    use crate::memory::MemoryFs;

    const BASE: &str = "/ws/tsconfig.base.json";
    const LOOSE: &str = "/ws/tsconfig.loose.json";
    const A: &str = "/ws/packages/a/tsconfig.json";
    const B: &str = "/ws/packages/b/tsconfig.json";

    /// A workspace of two packages whose configs extend `BASE` (strict), and a third, `c`, extending `LOOSE`.
    fn workspace() -> MemoryFs {
        let package = |base: &str| format!(r#"{{ "extends": "{base}", "include": ["src"] }}"#);
        MemoryFs::new()
            .with_file(BASE, r#"{ "compilerOptions": { "strict": true } }"#)
            .with_file(LOOSE, r#"{ "compilerOptions": { "strict": false } }"#)
            .with_file(A, package("../../tsconfig.base.json"))
            .with_file("/ws/packages/a/src/a.ts", "")
            .with_file(B, package("../../tsconfig.base.json"))
            .with_file("/ws/packages/b/src/b.ts", "")
            .with_file("/ws/packages/c/tsconfig.json", package("../../tsconfig.loose.json"))
            .with_file("/ws/packages/c/src/c.ts", "")
            .with_file("/ws/broken/tsconfig.json", r#"{ "include": ["src"" }"#)
    }

    /// A reader of `configs` over `memory`, as one analysis reads them.
    fn reader(memory: &MemoryFs, configs: &Arc<SharedConfigs>) -> ConfigReader {
        let (fs, _) = memory.clone().into_fs();
        ConfigReader::new(
            Arc::clone(configs),
            Arc::default(),
            ConfigHost::new(fs, "/ws".to_owned()),
        )
    }

    const fn counts(parsed: usize, reused: usize, unshared: usize) -> ParseCounts {
        ParseCounts {
            parsed,
            reused,
            unshared,
        }
    }

    fn strict(config: P<ParsedCommandLine>) -> bool {
        config.compiler_options().unwrap().strict.is_true()
    }

    #[test]
    fn requests_of_one_config_from_every_analysis_of_the_run_read_it_once() {
        const READERS: usize = 4;
        let memory = workspace();
        let configs = Arc::new(SharedConfigs::default());
        let readers: Vec<ConfigReader> = (0..READERS).map(|_| reader(&memory, &configs)).collect();
        let parsed: Vec<P<ParsedCommandLine>> = readers
            .iter()
            .map(|reader| reader.config(A).usable().expect("the config parses"))
            .collect();
        assert!(
            parsed.iter().all(|&config| config == parsed[0]),
            "every request gets the one parse"
        );
        assert_eq!(parsed[0].file_names(), ["/ws/packages/a/src/a.ts"]);
        assert_eq!(configs.counts().0, counts(1, READERS - 1, 0), "the run's counts");
        assert_eq!(readers[0].counts().0, counts(1, 0, 0), "the analysis that parsed it");
        assert_eq!(readers[1].counts().0, counts(0, 1, 0), "an analysis that reused it");
    }

    #[test]
    fn configs_that_extend_one_config_read_it_once_and_each_extended_config_is_its_own() {
        let memory = workspace();
        let configs = Arc::new(SharedConfigs::default());
        let reader = reader(&memory, &configs);
        let [a, b, c] = [A, B, "/ws/packages/c/tsconfig.json"].map(|name| reader.config(name).usable().unwrap());
        assert!(
            strict(a) && strict(b),
            "both take the options of the config they extend"
        );
        assert!(
            !strict(c),
            "a config that extends another config keeps that config's options"
        );
        assert_eq!(configs.counts(), (counts(3, 0, 0), counts(2, 1, 0)));
    }

    #[test]
    fn a_program_parses_its_own_config_and_reads_the_configs_it_extends_from_the_run() {
        let memory = workspace();
        let configs = Arc::new(SharedConfigs::default());
        let reader = reader(&memory, &configs);
        let shared = reader.config(A).usable().unwrap();
        let own = reader.parse_own(A).expect("the config parses");
        assert!(own.compiler_options().unwrap().strict.is_true());
        assert_eq!(own.file_names(), shared.file_names());
        assert!(
            own.file_names_by_path.get().is_none(),
            "the copy computes its own lazy tables"
        );
        assert_eq!(configs.counts(), (counts(1, 1, 0), counts(1, 0, 0)));
        assert!(reader.parse_own("/ws/broken/tsconfig.json").is_none());
    }

    #[test]
    fn programs_that_share_a_config_parse_it_once_and_change_only_their_copy() {
        let memory = workspace();
        let configs = Arc::new(SharedConfigs::default());
        let (first, second) = (reader(&memory, &configs), reader(&memory, &configs));
        let mut one = first.parse_own(A).expect("the config parses");
        let two = second.parse_own(A).expect("the config parses");
        assert_eq!(
            configs.counts(),
            (counts(1, 1, 0), counts(1, 0, 0)),
            "parsed once for the run"
        );
        one.parsed_config.compiler_options = Some(P::new(CompilerOptions::default()));
        assert!(!one.compiler_options().unwrap().strict.is_true());
        assert!(
            two.compiler_options().unwrap().strict.is_true(),
            "the other program's copy is unchanged"
        );
        assert!(strict(first.config(A).usable().unwrap()), "so is the shared parse");
    }

    #[test]
    fn a_config_that_cannot_be_read_or_parsed_has_a_problem_for_every_request() {
        let memory = workspace();
        let configs = Arc::new(SharedConfigs::default());
        let reader = reader(&memory, &configs);
        let missing = reader.config("/ws/missing/tsconfig.json");
        assert!(missing.config.is_none() && missing.problem.is_some());
        let broken = reader.config("/ws/broken/tsconfig.json");
        assert!(
            broken.config.is_some(),
            "a project reference still reads a config with syntax errors"
        );
        assert!(broken.problem.is_some(), "a planning does not");
        assert!(broken.usable().is_none());
        let again = reader.config("/ws/broken/tsconfig.json");
        assert!(
            again.config == broken.config,
            "the second request reads the first one's parse"
        );
        assert_eq!(again.problem, broken.problem);
        assert_eq!(configs.counts().0, counts(2, 1, 0));
    }

    #[test]
    fn a_request_while_another_parses_the_config_parses_it_alone_and_a_prefetch_leaves_it() {
        let memory = workspace();
        let configs = Arc::new(SharedConfigs::default());
        let reader = reader(&memory, &configs);
        lock(&configs.configs).insert(A.to_owned(), Slot::Parsing);
        lock(&configs.extended).insert(tspath::to_path(BASE, "/ws", false), Slot::Parsing);
        assert!(reader.prefetch(A).is_none(), "a prefetch never parses alone");
        let alone = reader.config(A).usable().expect("the config parses");
        assert!(strict(alone), "the extended config is parsed alone too");
        assert!(
            matches!(lock(&configs.configs).get(A), Some(Slot::Parsing)),
            "the request left the slot to the one parsing it"
        );
        assert_eq!(configs.counts(), (counts(0, 0, 1), counts(0, 0, 1)));
    }

    #[test]
    fn a_request_inside_a_scratch_region_parses_the_config_alone() {
        let memory = workspace();
        let configs = Arc::new(SharedConfigs::default());
        let reader = reader(&memory, &configs);
        let scratch = Region::new_scratch(FIRST_CHUNK);
        let alone = {
            let _scratch = scratch.enter_scratch();
            reader.config(A).usable().unwrap()
        };
        let shared = reader.config(A).usable().unwrap();
        assert!(
            alone != shared,
            "the request inside the scratch region left the cache empty"
        );
        assert_eq!(configs.counts(), (counts(1, 0, 1), counts(1, 0, 1)));
    }
}
