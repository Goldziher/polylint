//! Type-aware facts about JavaScript and TypeScript class, enum and namespace members, from the TypeScript checker of
//! tsrs.
//!
//! A host's module graph and syntax facts come from its own parser (oxc, for one). Where a project has types,
//! [`analyze`] answers what syntax alone cannot: which member a read reaches through a typed receiver, an override or
//! an implemented interface. The answer is owned data ([`TypeFacts`]). The programs are built, shared and freed
//! through `polylint-tsrs`.
//!
//! polylint ADR 0081 records where the crate came from and why it is published on its own. The known false findings
//! of the typed run are listed in `KNOWN-DEFECTS.md`.

mod coverage;
mod error;
mod facts;
mod fs;
mod hash;
mod members;
mod programs;
mod query;
mod resident;
mod session;
mod timing;
mod vfs;

use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::thread::Scope;

pub use error::TypeError;
pub use facts::{
    CoverageReason, CoverageState, DeclId, DeclKind, DeclQuery, PackageCoverage, ProgramTiming, SharedConfigCounts,
    SharedFileCounts, TypeFacts, TypeQuery, TypeTimings,
};
pub use fs::{DirEntry, FileSystem, MemoryFs, OsFs, PathKind};
pub use resident::TypePrograms;
pub use session::{PendingTypes, TypeBudget};

/// The engine and version recorded in [`TypeFacts::engine`].
pub const ENGINE: &str = "tsrs 7.1.0-beta.tsrs2";

/// A package whose judged files the analysis covers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypePackage {
    /// The package directory, absolute.
    pub dir: PathBuf,
    /// The package name, when it has one.
    pub name: Option<String>,
    /// The absolute paths of the package's judged files.
    pub files: Vec<PathBuf>,
}

/// What [`analyze`] works on.
#[derive(Clone)]
#[non_exhaustive]
pub struct TypeInput<'a> {
    /// The file system every file and config is read through.
    pub fs: Arc<dyn FileSystem>,
    /// The workspace root, absolute.
    pub root: &'a Path,
    /// The packages with judged files.
    pub packages: &'a [TypePackage],
    /// A `tsconfig` that replaces discovery for the root package (`[js] tsconfig`).
    pub tsconfig_override: Option<&'a Path>,
    /// The declarations to answer for.
    pub query: &'a TypeQuery,
    /// The threads the checker may use; `None` for all cores.
    pub threads: Option<NonZeroUsize>,
    /// The cores the analyses of the run share; `None` for cores of the analysis's own.
    pub budget: Option<&'a TypeBudget>,
    /// The programs the analyses of the run share, with their lib and `node_modules` declaration files; `None` to
    /// build every program for this analysis alone, whose programs then share those files among themselves.
    pub programs: Option<&'a TypePrograms>,
    /// Set by the host to stop the analysis.
    pub cancel: Option<&'a AtomicBool>,
}

impl<'a> TypeInput<'a> {
    /// An input over `packages` for `query`, with discovered `tsconfig` files, all cores and no cancellation.
    #[must_use]
    pub fn new(fs: Arc<dyn FileSystem>, root: &'a Path, packages: &'a [TypePackage], query: &'a TypeQuery) -> Self {
        Self {
            fs,
            root,
            packages,
            tsconfig_override: None,
            query,
            threads: None,
            budget: None,
            programs: None,
            cancel: None,
        }
    }

    /// Uses `tsconfig` for the root package instead of discovering one.
    #[must_use]
    pub fn with_tsconfig_override(mut self, tsconfig: &'a Path) -> Self {
        self.tsconfig_override = Some(tsconfig);
        self
    }

    /// Limits the checker to `threads` threads.
    #[must_use]
    pub fn with_threads(mut self, threads: NonZeroUsize) -> Self {
        self.threads = Some(threads);
        self
    }

    /// Takes the programs' cores from `budget`, which the other analyses of the run share.
    #[must_use]
    pub fn with_budget(mut self, budget: &'a TypeBudget) -> Self {
        self.budget = Some(budget);
        self
    }

    /// Walks the programs `programs` holds built already and shares those it builds with the other analyses of the
    /// run.
    #[must_use]
    pub fn with_programs(mut self, programs: &'a TypePrograms) -> Self {
        self.programs = Some(programs);
        self
    }

    /// Stops the analysis once `cancel` is set.
    #[must_use]
    pub fn with_cancel(mut self, cancel: &'a AtomicBool) -> Self {
        self.cancel = Some(cancel);
        self
    }
}

/// What building the programs needs before the query is known: a [`TypeInput`] without its query, owned, so the
/// programs can build while the host still works out what to ask ([`start`]).
#[derive(Clone)]
#[non_exhaustive]
pub struct TypeBuild {
    /// The file system every file and config is read through.
    pub fs: Arc<dyn FileSystem>,
    /// The workspace root, absolute.
    pub root: PathBuf,
    /// The packages with the files that may be judged: the programs of their configs start building. Only the
    /// judged files handed to [`PendingTypes::finish`] are walked; a program none of them needs is freed unwalked,
    /// and a program only they need is built then.
    pub packages: Vec<TypePackage>,
    /// A `tsconfig` that replaces discovery for the root package (`[js] tsconfig`).
    pub tsconfig_override: Option<PathBuf>,
    /// The threads the checker may use; `None` for all cores.
    pub threads: Option<NonZeroUsize>,
    /// The cores the analyses of the run share; `None` for cores of the analysis's own.
    pub budget: Option<TypeBudget>,
    /// The programs the analyses of the run share, with their lib and `node_modules` declaration files; `None` to
    /// build every program for this analysis alone, whose programs then share those files among themselves.
    pub programs: Option<TypePrograms>,
}

impl TypeBuild {
    /// A build of the programs of `packages`, with discovered `tsconfig` files and all cores.
    #[must_use]
    pub fn new(fs: Arc<dyn FileSystem>, root: PathBuf, packages: Vec<TypePackage>) -> Self {
        Self {
            fs,
            root,
            packages,
            tsconfig_override: None,
            threads: None,
            budget: None,
            programs: None,
        }
    }

    /// Uses `tsconfig` for the root package instead of discovering one.
    #[must_use]
    pub fn with_tsconfig_override(mut self, tsconfig: PathBuf) -> Self {
        self.tsconfig_override = Some(tsconfig);
        self
    }

    /// Limits the checker to `threads` threads.
    #[must_use]
    pub fn with_threads(mut self, threads: NonZeroUsize) -> Self {
        self.threads = Some(threads);
        self
    }

    /// Takes the programs' cores from `budget`, which the other analyses of the run share.
    #[must_use]
    pub fn with_budget(mut self, budget: TypeBudget) -> Self {
        self.budget = Some(budget);
        self
    }

    /// Walks the programs `programs` holds built already and shares those it builds with the other analyses of the
    /// run.
    #[must_use]
    pub fn with_programs(mut self, programs: TypePrograms) -> Self {
        self.programs = Some(programs);
        self
    }

    /// The input over the packages of the build for `query`, without cancellation.
    fn input<'a>(&'a self, query: &'a TypeQuery) -> TypeInput<'a> {
        TypeInput {
            fs: Arc::clone(&self.fs),
            root: &self.root,
            packages: &self.packages,
            tsconfig_override: self.tsconfig_override.as_deref(),
            query,
            threads: self.threads,
            budget: self.budget.as_ref(),
            programs: self.programs.as_ref(),
            cancel: None,
        }
    }
}

/// Starts building the programs of `build` on threads of `scope`, so they build while the host works out its query;
/// [`PendingTypes::finish`] then walks them. The facts are those [`analyze`] gives for the input `finish` describes,
/// whichever programs were built before it.
///
/// `cancel` stops the analysis once set, as [`TypeInput::cancel`] does.
pub fn start<'scope, 'env>(
    scope: &'scope Scope<'scope, 'env>,
    build: TypeBuild,
    cancel: Option<&'env AtomicBool>,
) -> PendingTypes<'scope> {
    session::start(scope, build, cancel)
}

/// Builds the programs of the packages in `input`, asks the checker about the declarations of its query and returns
/// the facts with the coverage of every package.
///
/// # Errors
/// [`TypeError::Config`] when an explicit `tsconfig` cannot be read, [`TypeError::Internal`] when the checker fails
/// unexpectedly and [`TypeError::Cancelled`] when the host cancels. A package without usable types is no error.
pub fn analyze(input: &TypeInput<'_>) -> Result<TypeFacts, TypeError> {
    session::analyze(input)
}

/// Retires the regions of the project files the programs of later analyses free: their pages go back to
/// the system at once instead of when the rest of their slabs is freed, which keeps the peak resident set of a large
/// run down, but their address range is never reused. Process-wide and off by default; only a process that exits
/// after one run (such as a command-line lint) turns it on. A host that keeps analysing, such as an addon or an
/// editor, must not: each run would give up more of the address range for good, and tsrs aborts once it is spent.
pub fn retire_freed_sources() {
    polylint_tsrs::retire_freed_sources();
}

/// Turns on, for the rest of the process, the timings that cost work to collect:
/// [`TypeTimings::project_files_distinct`] lists every program's files to count them. Off by default; the other
/// [`TypeTimings`] are always collected.
pub fn enable_timings() {
    timing::enable();
}

/// The bytes of tsrs's arenas in use now, for tests that check repeated analyses give their memory back and for the
/// host's timings.
///
/// With compressed pointers (the default on unix) these are the chunks of the process-wide reservation that are
/// handed out, on every thread; with `plain-ptrs` only the calling thread's arena is counted.
#[doc(hidden)]
#[must_use]
pub fn arena_bytes_in_use() -> usize {
    polylint_tsrs::arena_bytes_in_use()
}

/// The most bytes of tsrs's arenas in use at once in the process so far, for the host's timings; `None` with
/// `plain-ptrs`, where only the calling thread's arena is counted.
#[doc(hidden)]
#[must_use]
pub fn arena_peak_bytes() -> Option<usize> {
    polylint_tsrs::arena_peak_bytes()
}

/// The example of the README, compiled and run with the documentation tests.
#[cfg(doctest)]
#[doc = include_str!("../README.md")]
struct ReadmeExample;

const _: () = {
    const fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<TypeFacts>();
    assert_send_sync::<TypeQuery>();
    assert_send_sync::<TypeError>();
    assert_send_sync::<TypeBuild>();
    assert_send_sync::<TypeBudget>();
    assert_send_sync::<TypePrograms>();
};
