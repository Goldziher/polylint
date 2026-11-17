//! Programs of the TypeScript checker of tsrs built over a host's file system: the parses the programs of a run
//! share, and the owners that free them.
//!
//! - [`wrap_fs`] makes the file system tsrs reads through from the host's, with the bundled `lib.*.d.ts` files
//!   ([`lib_path`]); [`MemoryFs`] is such a file system over files held in memory.
//! - [`SharedFiles`] and [`SharedConfigs`] keep each file and config the programs of a run read, parsed once for the
//!   run.
//! - A [`ProgramHost`] builds programs in two steps, the host changing the parsed config in between, and a
//!   [`ProgramOwner`] lends the built program and its diagnostics and frees it when dropped.
//! - [`SharedPrograms`] builds once a program several analyses of a run need, and [`Parallelism`], [`Schedule`] and
//!   [`Cores`] share the machine among the programs built at once, within the host's [`Limits`].
//! - An [`Observer`] is told what a program reads, and a [`ModuleResolver`] resolves modules with no program.
//! - A [`CacheStore`] is the seam of a host's cache, with the envelope its entries are checked by ([`cache`]).
//!
//! The crate depends on the tsrs crates alone, reads no environment variable and does no I/O beyond the file system
//! it is given. tsrs types appear in its API: depend on the tsrs crates at the exact version this crate pins.
//!
//! The `plain-ptrs` feature, off by default, turns on `tsrs_core/plain-ptrs`. The crate is correct either way; the
//! arena statistics and the retiring of freed regions ([`retire_freed_sources`]) exist only without it.
//!
//! What the crate owns, and the measured constants it carries, are recorded in ADR 0078 of the polylint repository
//! (`adrs/0078-polylint-tsrs-published-program-layer.md`).

mod build;
pub mod cache;
mod memory;
mod observe;
mod owner;
mod program;
mod resident;
mod resolve;
mod schedule;
mod shared_configs;
mod shared_files;
mod vfs;

pub use build::{BuildRequest, BuildSplit, BuiltProgram};
pub use cache::{CacheNamespace, CacheStore};
pub use memory::MemoryFs;
pub use observe::Observer;
pub use owner::{Executor, ProgramOwner};
pub use program::ProgramHost;
pub use resident::{Claim, ProgramKey, Resident, SharedPrograms, Ticket, Walking};
pub use resolve::{ModuleResolver, ResolvedModule};
pub use schedule::{
    Admitted, CORES_PER_PROGRAM, Cores, Limits, MOST_SHARED_CHECKERS, Parallelism, Schedule, Taken, charge, cores_of,
};
pub use shared_configs::{ConfigCounts, ConfigHost, ConfigReader, Parsed, SharedConfigs};
pub use shared_files::{
    Counts, ProgramFiles, RETIRE_BELOW, SharedFiles, kept_for_run, retire_freed_sources, retire_freed_sources_below,
    retiring,
};
pub use vfs::{BomFiles, lib_path, tsrs_path, wrap_fs};

/// The example of the README, compiled and run with the documentation tests.
#[cfg(doctest)]
#[doc = include_str!("../README.md")]
struct ReadmeExample;

/// How the requests for the files or configs a run shares got them, one count per request: each is parsed once for
/// the run, the first time it is asked for, and reused by every later request.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct ParseCounts {
    /// Requests that read and parsed for the run.
    pub parsed: usize,
    /// Requests that took the run's parse.
    pub reused: usize,
    /// Requests that parsed for themselves alone: the thread was in a checker's memory scope, or (a config) another
    /// thread was parsing it.
    pub unshared: usize,
}

/// The bytes of tsrs's arenas in use now, for tests that check freed programs give their memory back and for a
/// host's timings.
///
/// With compressed pointers (the default on unix) these are the chunks of the process-wide reservation that are
/// handed out, on every thread; with `plain-ptrs` only the calling thread's arena is counted.
#[must_use]
pub fn arena_bytes_in_use() -> usize {
    tsrs_core::ptr::reserve_stats().map_or_else(tsrs_core::ptr::arena_used_bytes, |(in_use, _)| in_use)
}

/// The most bytes of tsrs's arenas in use at once in the process so far; `None` with `plain-ptrs`, where only the
/// calling thread's arena is counted.
#[must_use]
pub fn arena_peak_bytes() -> Option<usize> {
    tsrs_core::ptr::reserve_stats().map(|(_, peak)| peak)
}
