//! The files the programs of a run share (polylint ADR 0078): every file a program reads through its compiler host
//! is read, parsed and bound once for the run instead of once per program. The bundled `lib.*.d.ts` files and the
//! declaration files below `node_modules` stay for the run; a project's own files (its sources, its declaration files,
//! and the sources of the projects it references) stay while a program that read them lives.
//!
//! # Why sharing is sound
//!
//! - **Ownership.** A shared file is parsed and bound in a registered region of its own ([`Region::new`]). A file kept
//!   for the run is held by the run's [`SharedFiles`]; every program that reads it holds the cache through its
//!   compiler host and through whatever holds the program for its walks (its resident). A project file is held by
//!   the [`ProgramFiles`] of each program that read it, which the program's [`ProgramOwner`](crate::ProgramOwner) and
//!   that holder hold
//!   until the program, its loader regions and the scratch region of its config and host are dropped; the cache keeps
//!   only a weak handle. So a file's region outlives every program that points into it, and is freed with the last of
//!   them. Freeing a program (`free_unshared_program`) frees the program, its processed-file data and its mapper,
//!   which are heap boxes; its loader regions are dropped after it. None of them owns a shared file, so freeing one
//!   program never frees or touches a file another program reads. Nothing that outlives the programs points into a
//!   project file: the cache is keyed on the parse options, and the parser registers the file's text for compact
//!   identifiers with an `on_free` hook of the file's region that unregisters it.
//! - **Identical parse.** In tsrs a parse is a function of the file's text and its [`SourceFileParseOptions`] (name,
//!   path and the external-module-indicator options); the script kind and language variant come from the name, and
//!   no compiler option (target, JSX, module detection, `isolatedModules`, `allowJs`, `checkJs`, strictness) reaches
//!   the parser or the binder otherwise. The cache is keyed on the whole parse options, so two requests share a file
//!   only when tsrs would parse it the same way for both. For a declaration file the external-module-indicator
//!   options are always the default (`get_external_module_indicator_options`), so every program that reads one shares
//!   it. For a project file they come from the JSX and module detection options of the project that holds the file
//!   (`get_compiler_options_for_file`: the referenced project's, for a file reached through a project reference) and
//!   from the module format its `package.json` implies, so two programs share it only when those agree; the implied
//!   format itself is program data (`SourceFileMetaData`), not a field of the file. Content-mapped files do not go
//!   through the host's `get_source_file` and are never shared. The programs of a run read one file system (the
//!   condition for sharing a [`SharedFiles`]).
//! - **Binding.** The binder reads only the file (`Binder::new` takes the file alone, no compiler option) and binds
//!   it once (`SourceFile::bind_once`); it runs here, on insertion, inside the file's region, so the file is bound
//!   before any program sees it and every program's `bind_source_files` skips it.
//! - **Checkers.** A tsrs program already checks its files on several checkers at once, so no checker owns a file; to
//!   a file, a checker of another program is one more such checker. Each program's checkers keep what they work out
//!   in link stores of their own (node links, symbol links and merged symbols are tables of the checker keyed by node
//!   or symbol, in the checker's region) and write into symbols and nodes they create (transient symbols, since
//!   `merge_symbol` clones a bound target before merging into it, and the synthetic nodes of flow analysis and JSX),
//!   never into a parsed file or a bound symbol: in the bodies of a project's sources as in declarations. The writes
//!   into a file a checker does make are those it already makes concurrently with the other checkers of its program:
//!   the lazily computed caches of the file (`JSDoc`, line map, name table), each set once from the file's text
//!   alone, and a symbol's or node's id, set once by compare-and-swap from a process-wide counter. tsrs allocates
//!   those caches in the region that owns the file (`SourceFile::owner_region`, `enter_owner`), found through the
//!   region registry: registering the file's region keeps them there, whichever checker fills them, instead of in the
//!   region that happens to be current on the filling thread, which a program frees. This is how tsrs itself shares
//!   declaration files between the projects of `tsc -b` (`bind_in_file_regions`) and frees a file's region on its
//!   own in watch mode.
//! - **Threads.** The cache is `Send + Sync`. Concurrent requests for one file wait on its slot while one of them
//!   parses; parsing and binding run on the calling thread and need no thread pool, so a pool thread that waits
//!   blocks nothing the parse needs. A thread inside a checker's scratch region (one that took a loader job while it
//!   waited) parses the file for its program alone, as before: the parser moves the `SourceFile` out of a scratch
//!   region, which would put it outside the file's region.

use std::collections::HashMap;
use std::fmt::Display;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock, PoisonError, Weak};

use tsrs_ast::{SourceFile, SourceFileParseOptions};
use tsrs_compiler::CompilerHost;
use tsrs_contentmapper::{self as contentmapper, Project};
use tsrs_core::arena::Region;
use tsrs_core::{P, tspath};
use tsrs_diagnostics::Message;
use tsrs_tsoptions::{Mapper, ParsedCommandLine};
use tsrs_vfs::FS;

use crate::ParseCounts;
use crate::observe::Observer;
use crate::shared_configs::ConfigReader;
use crate::vfs::BomFiles;

/// The first chunk of a shared file's region; it grows as the parse needs and is trimmed to what parse and bind used.
const FIRST_CHUNK: usize = 64 << 10;

/// How much of tsrs's arena address range (32 GiB with compressed pointers) may be in use, retired ranges included,
/// for a freed project file region to be retired (`Region::retire_on_free`) once the process opted in with
/// [`retire_freed_sources`]; [`retire_freed_sources_below`] takes another bound. A file's region is carved from its
/// thread's slabs beside the regions of the files kept for the run and the programs' loader regions, and a freed
/// region stays resident until its whole slab is freed; retired, its pages go back to the system at once, but its
/// range, and that of every slab it shares, is never handed out again. On a large monorepo (n8n) that keeps the peak
/// resident set below that of programs that parse every file for themselves, for about 1.2 GiB of the range. Past
/// this much, regions are freed as others are, so a large run cannot exhaust the range.
pub const RETIRE_BELOW: usize = 8 << 30;

/// The bytes of the arena range below which freed project file regions are retired; zero while the process has not
/// opted in ([`retire_freed_sources`]).
static RETIRE: AtomicUsize = AtomicUsize::new(0);

/// Retires the regions of project files freed from now on, while under [`RETIRE_BELOW`] (8 GiB) of the arena range
/// is in use: for a process that exits after one run. A host that keeps analysing, such as an addon or an editor,
/// must not: every run would give up more of the address range for good, and tsrs aborts once it is spent.
/// Process-wide and off by default.
pub fn retire_freed_sources() {
    retire_freed_sources_below(RETIRE_BELOW);
}

/// [`retire_freed_sources`] with a bound of the host's own: regions are retired while under `bytes` of the arena
/// range are in use, and never with zero, which turns retiring off again.
pub fn retire_freed_sources_below(bytes: usize) {
    RETIRE.store(bytes, Ordering::Relaxed);
}

/// Whether the process opted in to retiring freed project file regions ([`retire_freed_sources`]).
#[must_use]
pub fn retiring() -> bool {
    retire_below() != 0
}

/// The bound the process opted in with; zero without one.
fn retire_below() -> usize {
    RETIRE.load(Ordering::Relaxed)
}

/// The shared files of a run, by the parse options tsrs read each with: every file a program reads through its
/// compiler host is read, parsed and bound once for the run. The bundled lib files and the declaration files below
/// `node_modules` stay until the value is dropped; a project file stays while a program that read it lives. Only
/// programs that read one file system may share a value.
#[derive(Default)]
pub struct SharedFiles {
    /// The lib and `node_modules` declaration files, kept for the run.
    slots: Mutex<HashMap<SourceFileParseOptions, Arc<Slot>>>,
    /// The project files, kept by the programs that read them ([`ProgramFiles`]): an entry whose programs are all
    /// freed is dead, and the next request parses the file again.
    project: Mutex<HashMap<SourceFileParseOptions, Weak<Slot>>>,
    counts: Counts,
}

/// One file: empty while no request has parsed it; `None` once a request found it unreadable.
type Slot = OnceLock<Option<SharedFile>>;

/// The project files one program read from the run's [`SharedFiles`], which it keeps until the program is freed: the
/// file's region is freed when the last program holding it lets it go.
#[derive(Default)]
pub struct ProgramFiles {
    files: Mutex<Vec<Arc<Slot>>>,
}

impl ProgramFiles {
    fn hold(&self, slot: Arc<Slot>) {
        lock(&self.files).push(slot);
    }
}

/// A parsed and bound file with the region that holds it, freed with the cache or, for a project file, with the last
/// program that read it.
struct SharedFile {
    file: P<SourceFile>,
    /// Whether its text starts with a byte order mark, which the file system that read it dropped.
    bom: bool,
    _region: Region,
}

impl SharedFiles {
    /// The counts of the files kept for the run, so far.
    #[must_use]
    pub fn counts(&self) -> ParseCounts {
        self.counts.snapshot()
    }

    /// How many project files the run's programs hold now, for tests that check each is freed with its programs.
    #[must_use]
    pub fn project_files_held(&self) -> usize {
        lock(&self.project)
            .values()
            .filter(|slot| slot.strong_count() > 0)
            .count()
    }

    /// The slot of `options`: the run's, or, for a project file, the one a living program holds, else a new one.
    fn slot(&self, options: &SourceFileParseOptions, project: bool) -> Arc<Slot> {
        if !project {
            return Arc::clone(
                lock(&self.slots)
                    .entry(options.clone())
                    .or_insert_with(|| Arc::new(OnceLock::new())),
            );
        }
        let mut slots = lock(&self.project);
        if let Some(slot) = slots.get(options).and_then(Weak::upgrade) {
            return slot;
        }
        let slot = Arc::new(OnceLock::new());
        slots.insert(options.clone(), Arc::downgrade(&slot));
        slot
    }

    /// The file `options` name for a program reading through `host`, whose file system records the files with a byte
    /// order mark in `boms`: the shared one, parsed by this request or an earlier one, or, on a thread inside a
    /// scratch region, one parsed for the program alone. A project file is held for the program in `program`; the
    /// others are kept for the run. The request counts in `counts` (the analysis's) and in the cache's own.
    fn source_file(
        &self,
        options: SourceFileParseOptions,
        host: &dyn CompilerHost,
        boms: &BomFiles,
        counts: &Counts,
        program: Option<&ProgramFiles>,
    ) -> Option<P<SourceFile>> {
        let project = program.is_some();
        let slot = self.slot(&options, project);
        let (shared, parsed) = match slot.get() {
            Some(shared) => (shared, false),
            None if tsrs_core::arena::scratch_active() => {
                self.counts.tally(project).unshared.fetch_add(1, Ordering::Relaxed);
                counts.tally(project).unshared.fetch_add(1, Ordering::Relaxed);
                return host.get_source_file(options);
            }
            None => {
                let mut parsed = false;
                let shared = slot.get_or_init(|| {
                    parsed = true;
                    parse(options, host, boms, if project { retire_below() } else { 0 })
                });
                (shared, parsed)
            }
        };
        let count = |counts: &Counts| {
            let tally = counts.tally(project);
            let counter = if parsed { &tally.parsed } else { &tally.reused };
            counter.fetch_add(1, Ordering::Relaxed);
        };
        count(&self.counts);
        count(counts);
        let file = shared.as_ref().map(|shared| {
            if shared.bom {
                boms.record(shared.file.file_name());
            }
            shared.file
        });
        if let Some(program) = program {
            program.hold(slot);
        }
        file
    }
}

/// Reads, parses and binds the file `options` name through `host` in a region of its own, retired when freed while
/// under `retire_below` bytes of the arena's address range are in use (never with zero).
fn parse(
    options: SourceFileParseOptions,
    host: &dyn CompilerHost,
    boms: &BomFiles,
    retire_below: usize,
) -> Option<SharedFile> {
    let region = Region::new(FIRST_CHUNK);
    // Without compressed pointers there are no stats, and a retired slab is never given back at all.
    if tsrs_core::ptr::reserve_stats().is_some_and(|(in_use, _)| in_use < retire_below) {
        region.retire_on_free();
    }
    let file = {
        let _scope = region.enter();
        let file = host.get_source_file(options)?;
        tsrs_binder::bind_source_file(file);
        file
    };
    region.trim();
    Some(SharedFile {
        file,
        bom: boms.offset(file.file_name()) != 0,
        _region: region,
    })
}

/// Whether the file `file_name` is kept for the run: a bundled lib file or a declaration file below `node_modules`.
/// A project's own files, declaration files included, are kept by the programs that read them ([`ProgramFiles`]).
#[must_use]
pub fn kept_for_run(file_name: &str) -> bool {
    tsrs_vfs::bundled::embedded_file(file_name).is_some()
        || (file_name.contains("/node_modules/") && tspath::is_declaration_file_name(file_name))
}

/// Files parsed, reused and parsed unshared, counted with relaxed atomics: each count is read after the programs that
/// add to it are done.
#[derive(Debug, Default)]
struct Tally {
    parsed: AtomicUsize,
    reused: AtomicUsize,
    unshared: AtomicUsize,
}

impl Tally {
    fn snapshot(&self) -> ParseCounts {
        ParseCounts {
            parsed: self.parsed.load(Ordering::Relaxed),
            reused: self.reused.load(Ordering::Relaxed),
            unshared: self.unshared.load(Ordering::Relaxed),
        }
    }
}

/// How the requests of one analysis got their files: the files kept for the run (or the configs, `shared_configs`)
/// and, apart, the project files.
#[derive(Debug, Default)]
pub struct Counts {
    kept: Tally,
    project: Tally,
}

impl Counts {
    /// The counts of the files kept for the run.
    #[must_use]
    pub fn snapshot(&self) -> ParseCounts {
        self.kept.snapshot()
    }

    /// The counts of the project files.
    #[must_use]
    pub fn project_snapshot(&self) -> ParseCounts {
        self.project.snapshot()
    }

    fn tally(&self, project: bool) -> &Tally {
        if project { &self.project } else { &self.kept }
    }

    pub(crate) fn count_parsed(&self) {
        self.kept.parsed.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn count_reused(&self) {
        self.kept.reused.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn count_unshared(&self) {
        self.kept.unshared.fetch_add(1, Ordering::Relaxed);
    }
}

/// The compiler host of one program: `inner`'s, except that every file comes from the run's cache and the config of a
/// project reference from the run's configs (`shared_configs`).
pub(crate) struct SharingHost {
    inner: Arc<dyn CompilerHost>,
    files: Arc<SharedFiles>,
    boms: Arc<BomFiles>,
    counts: Arc<Counts>,
    configs: Arc<ConfigReader>,
    /// The project files the program read, which it keeps until it is freed.
    program: Arc<ProgramFiles>,
    /// What is told of the files and configs the program asks for.
    observer: Option<Arc<dyn Observer>>,
}

impl SharingHost {
    /// `inner`, reading the shared files from `files` and the configs of project references with `configs`; `boms`
    /// is the record of `inner`'s file system, `counts` those of the analysis that builds the program, and `program`
    /// the program's hold on the project files it reads, to be dropped after the program. `observer` is told of
    /// every file and referenced config the program asks for.
    pub(crate) fn new(
        inner: Arc<dyn CompilerHost>,
        (files, boms, counts): (Arc<SharedFiles>, Arc<BomFiles>, Arc<Counts>),
        configs: Arc<ConfigReader>,
        program: Arc<ProgramFiles>,
        observer: Option<Arc<dyn Observer>>,
    ) -> Self {
        Self {
            inner,
            files,
            boms,
            counts,
            configs,
            program,
            observer,
        }
    }
}

impl CompilerHost for SharingHost {
    fn fs(&self) -> &dyn FS {
        self.inner.fs()
    }

    fn default_library_path(&self) -> &str {
        self.inner.default_library_path()
    }

    fn get_current_directory(&self) -> &str {
        self.inner.get_current_directory()
    }

    fn trace(&self, msg: &'static Message, args: &[&dyn Display]) {
        self.inner.trace(msg, args);
    }

    fn get_source_file(&self, opts: SourceFileParseOptions) -> Option<P<SourceFile>> {
        let program = (!kept_for_run(&opts.file_name)).then_some(&*self.program);
        let Some(observer) = &self.observer else {
            return self
                .files
                .source_file(opts, self.inner.as_ref(), &self.boms, &self.counts, program);
        };
        let file_name = opts.file_name.clone();
        let file = self
            .files
            .source_file(opts, self.inner.as_ref(), &self.boms, &self.counts, program);
        observer.source_file(&file_name, file.map(|file| file.text()));
        file
    }

    fn get_content_mapped_source_files(
        &self,
        parse_options: SourceFileParseOptions,
        mapper: &Mapper,
    ) -> Result<contentmapper::SourceFiles, contentmapper::Error> {
        self.inner.get_content_mapped_source_files(parse_options, mapper)
    }

    fn content_mapper_project(&self) -> Option<Arc<dyn Project>> {
        self.inner.content_mapper_project()
    }

    /// The config of a project reference, read once for the run (`shared_configs`) as `inner` reads it: the names asked
    /// for are absolute, so the current directory of the parse changes nothing; `None` when it cannot be read.
    fn get_resolved_project_reference(&self, file_name: &str, _path: tspath::Path) -> Option<P<ParsedCommandLine>> {
        let config = self.configs.config(file_name).config;
        if let Some(observer) = &self.observer {
            observer.config(
                file_name,
                config.map(|config| config.extended_source_files()).as_deref(),
            );
        }
        config
    }
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

const _: () = {
    const fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<SharedFiles>();
    assert_send_sync::<SharingHost>();
};

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use std::sync::Barrier;

    use tsrs_ast::ExternalModuleIndicatorOptions;

    use super::*;
    use crate::memory::MemoryFs;
    use crate::vfs;

    /// The bound is one byte, under which the arena never is, so no region of the tests beside this one is retired.
    #[test]
    fn a_host_opts_in_to_retiring_with_a_bound_of_its_own() {
        assert!(!retiring());
        retire_freed_sources_below(1);
        assert!(retiring());
        assert_eq!(retire_below(), 1);
        retire_freed_sources_below(0);
        assert!(!retiring());
    }

    const SHAPES: &str = "/ws/node_modules/shapes/index.d.ts";
    const SHAPES_TEXT: &str = "export interface Shape {\n  area(): number;\n}\n";
    const WIDGET: &str = "/ws/src/widget.ts";
    const WIDGET_TEXT: &str = "export class Widget {\n  paint(): void {}\n}\n";

    /// A compiler host over `memory` as a program of one analysis reads through it, with the record of its file
    /// system.
    struct Reader {
        host: Arc<dyn CompilerHost>,
        boms: Arc<BomFiles>,
    }

    impl Reader {
        fn new(memory: &MemoryFs) -> Self {
            let (fs, boms) = memory.clone().into_fs();
            let host = tsrs_compiler::new_compiler_host("/ws", fs, &vfs::lib_path(), None, None, None);
            Self { host, boms }
        }

        /// `SHAPES` from `files`, with the external-module-indicator options `indicator`.
        fn read_with(
            &self,
            files: &SharedFiles,
            counts: &Counts,
            indicator: ExternalModuleIndicatorOptions,
        ) -> Option<P<SourceFile>> {
            let options = SourceFileParseOptions {
                file_name: SHAPES.to_owned(),
                path: tspath::to_path(SHAPES, "/ws", true),
                external_module_indicator_options: indicator,
            };
            files.source_file(options, self.host.as_ref(), &self.boms, counts, None)
        }

        /// `WIDGET`, a project file, from `files` for the program that holds what it reads in `program`.
        fn read_project(&self, files: &SharedFiles, counts: &Counts, program: &ProgramFiles) -> Option<P<SourceFile>> {
            let options = SourceFileParseOptions {
                file_name: WIDGET.to_owned(),
                path: tspath::to_path(WIDGET, "/ws", true),
                external_module_indicator_options: ExternalModuleIndicatorOptions::default(),
            };
            files.source_file(options, self.host.as_ref(), &self.boms, counts, Some(program))
        }

        /// `SHAPES` from `files`, with the options of a declaration file.
        fn read(&self, files: &SharedFiles, counts: &Counts) -> Option<P<SourceFile>> {
            self.read_with(files, counts, ExternalModuleIndicatorOptions::default())
        }
    }

    fn shapes() -> MemoryFs {
        MemoryFs::new().with_file(SHAPES, SHAPES_TEXT)
    }

    const fn counts(parsed: usize, reused: usize, unshared: usize) -> ParseCounts {
        ParseCounts {
            parsed,
            reused,
            unshared,
        }
    }

    #[test]
    fn lib_files_and_declaration_files_below_node_modules_are_kept_for_the_run_and_project_files_are_not() {
        let lib = format!("{}/lib.es5.d.ts", vfs::lib_path());
        let cases = [
            (lib.as_str(), true),
            ("/ws/node_modules/shapes/index.d.ts", true),
            ("/ws/node_modules/@types/node/fs.d.ts", true),
            ("/ws/packages/a/node_modules/shapes/index.d.mts", true),
            ("/ws/node_modules/shapes/index.js", false),
            ("/ws/node_modules/shapes/src/index.ts", false),
            ("/ws/src/globals.d.ts", false),
            ("/ws/src/widget.ts", false),
            ("/ws/my_node_modules/shapes/index.d.ts", false),
        ];
        for (file_name, expected) in cases {
            assert_eq!(kept_for_run(file_name), expected, "{file_name}");
        }
    }

    #[test]
    fn a_second_request_with_the_same_options_reuses_the_bound_file() {
        let memory = shapes();
        let files = SharedFiles::default();
        let analysis = Counts::default();
        let first = Reader::new(&memory).read(&files, &analysis).expect("the file is read");
        let second = Reader::new(&memory).read(&files, &analysis).expect("the file is read");
        assert_eq!(first, second, "one parse for both requests");
        assert!(first.is_bound(), "bound on insertion");
        assert_eq!(first.text(), SHAPES_TEXT);
        assert_eq!(files.counts(), counts(1, 1, 0), "the run's counts");
        assert_eq!(analysis.snapshot(), counts(1, 1, 0), "the analysis's counts");
    }

    #[test]
    fn requests_with_other_parse_options_do_not_share_the_file() {
        let reader = Reader::new(&shapes());
        let files = SharedFiles::default();
        let analysis = Counts::default();
        let indicators = [
            ExternalModuleIndicatorOptions::default(),
            ExternalModuleIndicatorOptions {
                jsx: false,
                force: true,
            },
            ExternalModuleIndicatorOptions {
                jsx: true,
                force: false,
            },
        ];
        let parsed: Vec<P<SourceFile>> = indicators
            .into_iter()
            .map(|indicator| reader.read_with(&files, &analysis, indicator).unwrap())
            .collect();
        assert!(parsed[0] != parsed[1] && parsed[0] != parsed[2] && parsed[1] != parsed[2]);
        assert_eq!(files.counts(), counts(3, 0, 0));
    }

    #[test]
    fn a_reused_file_with_a_byte_order_mark_is_recorded_for_the_analysis_that_reuses_it() {
        let memory = MemoryFs::new().with_file(SHAPES, format!("\u{feff}{SHAPES_TEXT}"));
        let (first, second) = (Reader::new(&memory), Reader::new(&memory));
        let files = SharedFiles::default();
        let analysis = Counts::default();
        first.read(&files, &analysis).unwrap();
        assert_eq!(first.boms.offset(SHAPES), 3, "the parsing analysis read the mark");
        assert_eq!(
            second.boms.offset(SHAPES),
            0,
            "the other analysis has not read the file"
        );
        second.read(&files, &analysis).unwrap();
        assert_eq!(second.boms.offset(SHAPES), 3, "the reusing analysis knows of the mark");
        assert_eq!(files.counts(), counts(1, 1, 0));
    }

    #[test]
    fn an_unreadable_file_is_none_for_every_request() {
        let reader = Reader::new(&MemoryFs::new());
        let files = SharedFiles::default();
        let analysis = Counts::default();
        assert_eq!(reader.read(&files, &analysis), None);
        assert_eq!(reader.read(&files, &analysis), None);
        assert_eq!(files.counts(), counts(1, 1, 0));
    }

    #[test]
    fn concurrent_requests_for_one_file_parse_it_once() {
        const THREADS: usize = 8;
        let memory = shapes();
        let files = SharedFiles::default();
        let analysis = Counts::default();
        let start = Barrier::new(THREADS);
        let parsed: Vec<P<SourceFile>> = std::thread::scope(|scope| {
            let handles: Vec<_> = (0..THREADS)
                .map(|_| {
                    scope.spawn(|| {
                        let reader = Reader::new(&memory);
                        start.wait();
                        reader.read(&files, &analysis).unwrap()
                    })
                })
                .collect();
            handles.into_iter().map(|handle| handle.join().unwrap()).collect()
        });
        assert!(
            parsed.iter().all(|&file| file == parsed[0]),
            "every request gets the one parse"
        );
        assert_eq!(files.counts(), counts(1, THREADS - 1, 0));
    }

    #[test]
    fn a_request_inside_a_scratch_region_parses_the_file_for_its_program_alone() {
        let reader = Reader::new(&shapes());
        let files = SharedFiles::default();
        let analysis = Counts::default();
        let scratch = Region::new_scratch(FIRST_CHUNK);
        let alone = {
            let _scratch = scratch.enter_scratch();
            reader.read(&files, &analysis).unwrap()
        };
        let shared = reader.read(&files, &analysis).unwrap();
        assert_ne!(
            alone, shared,
            "the request inside the scratch region left the cache empty"
        );
        assert_eq!(files.counts(), counts(1, 0, 1));
    }

    #[test]
    fn a_project_file_two_programs_read_is_parsed_once_and_freed_with_the_last_of_them() {
        let memory = MemoryFs::new().with_file(WIDGET, WIDGET_TEXT);
        let files = SharedFiles::default();
        let analysis = Counts::default();
        let (first, second) = (ProgramFiles::default(), ProgramFiles::default());
        let parsed = Reader::new(&memory).read_project(&files, &analysis, &first).unwrap();
        let reused = Reader::new(&memory).read_project(&files, &analysis, &second).unwrap();
        assert_eq!(parsed, reused, "one parse for both programs");
        assert!(parsed.is_bound(), "bound on insertion");
        assert_eq!(parsed.text(), WIDGET_TEXT);
        assert_eq!(files.counts.project_snapshot(), counts(1, 1, 0), "the run's counts");
        assert_eq!(analysis.project_snapshot(), counts(1, 1, 0), "the analysis's counts");
        assert_eq!(files.counts(), counts(0, 0, 0), "no file kept for the run");
        assert_eq!(files.project_files_held(), 1);
        drop(first);
        assert_eq!(files.project_files_held(), 1, "the second program still holds the file");
        drop(second);
        assert_eq!(files.project_files_held(), 0, "freed with the last program");
        let third = ProgramFiles::default();
        Reader::new(&memory).read_project(&files, &analysis, &third).unwrap();
        assert_eq!(
            files.counts.project_snapshot(),
            counts(2, 1, 0),
            "a program after the file was freed parses it again"
        );
    }

    #[test]
    fn a_project_file_held_by_no_program_is_freed_while_the_files_kept_for_the_run_stay() {
        let memory = shapes().with_file(WIDGET, WIDGET_TEXT);
        let reader = Reader::new(&memory);
        let files = SharedFiles::default();
        let analysis = Counts::default();
        let program = ProgramFiles::default();
        reader.read(&files, &analysis).unwrap();
        reader.read_project(&files, &analysis, &program).unwrap();
        drop(program);
        assert_eq!(files.project_files_held(), 0);
        reader.read(&files, &analysis).unwrap();
        assert_eq!(
            files.counts(),
            counts(1, 1, 0),
            "the declaration file stays for the run"
        );
    }
}
