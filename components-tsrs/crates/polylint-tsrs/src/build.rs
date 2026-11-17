//! One call that builds and binds a program: the config, the host's changes to it, the program over the run's files.

use std::sync::Arc;
use std::time::{Duration, Instant};

use tsrs_ast::Diagnostic;
use tsrs_compiler::ProgramRegions;
use tsrs_core::{CompilerOptions, P, Tristate};
use tsrs_tsoptions::ParsedCommandLine;

use crate::owner::{Executor, ProgramOwner};
use crate::program::ProgramHost;
use crate::shared_files::ProgramFiles;

/// The parts of one program's build.
#[derive(Debug, Clone, Copy, Default)]
pub struct BuildSplit {
    /// Reading and parsing the program's own config.
    pub config_parse: Duration,
    /// Creating the program: reading, parsing and resolving its files.
    pub program_creation: Duration,
    /// Binding its source files.
    pub bind: Duration,
}

/// What [`ProgramHost::build`] builds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BuildRequest<'a> {
    /// The config, as tsrs names files.
    pub config_path: &'a str,
    /// The checkers the program checks on.
    pub checkers: usize,
    /// Whether tsrs runs the program single-threaded.
    pub single_threaded: bool,
    /// Whether the files the config includes are returned ([`BuiltProgram::files`]).
    pub list_files: bool,
    /// Whether the diagnostics the checkers make go back with the program. Without it they stay in the arena of the
    /// thread that made them, which is never freed: for a host that reads no diagnostic, or copies them out before
    /// the program is dropped, set it.
    pub diagnostics_in_checker_regions: bool,
}

/// A built and bound program with what it points into.
pub struct BuiltProgram {
    /// The program. Drop it before `loader_regions`.
    pub owner: ProgramOwner,
    /// The regions of the files the program parsed for itself alone, which it points into.
    pub loader_regions: Arc<ProgramRegions>,
    /// The files the config includes, when asked for; else empty.
    pub files: Vec<String>,
    /// Where the build went.
    pub split: BuildSplit,
}

/// Runs `work` on `executor`, or on the calling thread without one.
fn on<R: Send>(executor: Option<&Arc<dyn Executor>>, work: impl FnOnce() -> R + Send) -> Option<R> {
    let Some(executor) = executor else {
        return Some(work());
    };
    let mut work = Some(work);
    let mut result = None;
    executor.run(&mut || {
        if let Some(work) = work.take() {
            result = Some(work());
        }
    });
    result
}

impl ProgramHost {
    /// Reads the config of `request`, lets `change` change its command line (after the checker count and the
    /// threading of the request are set in its compiler options), and builds and binds its program, which holds the
    /// project files it reads in `files`. The program is created, bound and later freed on `executor`, so the
    /// parallel work tsrs does stays on the host's threads; on the calling thread without one.
    ///
    /// tsrs allocates the config and the compiler host in the region current on the calling thread: enter a scratch
    /// region around the call and drop it after the program and its loader regions, then `files`.
    ///
    /// # Errors
    /// The diagnostics of a config that cannot be read or parsed; empty when tsrs does not say why, when the config
    /// has no compiler options or when `executor` did not run the work.
    pub fn build(
        &self,
        request: &BuildRequest<'_>,
        files: Arc<ProgramFiles>,
        executor: Option<&Arc<dyn Executor>>,
        change: impl FnOnce(&mut ParsedCommandLine),
    ) -> Result<BuiltProgram, Vec<P<Diagnostic>>> {
        let parse_start = Instant::now();
        let mut config = self.load_config(request.config_path)?;
        let config_parse = parse_start.elapsed();
        let base = config.compiler_options().ok_or_else(Vec::new)?;
        let listed = if request.list_files {
            config.file_names().to_vec()
        } else {
            Vec::new()
        };
        config.parsed_config.compiler_options = Some(P::new(CompilerOptions {
            checkers: Some(i64::try_from(request.checkers).unwrap_or(i64::MAX)),
            single_threaded: if request.single_threaded {
                Tristate::True
            } else {
                base.single_threaded
            },
            ..(*base).clone()
        }));
        change(&mut config);
        // Dropped after the program: nothing that outlives the program points into its loader regions.
        let loader_regions: Arc<ProgramRegions> = Arc::default();
        let mut options = self.program_options(
            request.config_path,
            config,
            Arc::clone(&files),
            Arc::clone(&loader_regions),
        );
        options.checker_diagnostic_regions = request.diagnostics_in_checker_regions;
        if request.single_threaded {
            options.single_threaded = Tristate::True;
        }
        let create_start = Instant::now();
        let freed_on = executor.cloned();
        let owner = on(executor, || ProgramOwner::new(options, files, freed_on)).ok_or_else(Vec::new)?;
        let program_creation = create_start.elapsed();
        let bind_start = Instant::now();
        on(executor, || owner.program().bind_source_files()).ok_or_else(Vec::new)?;
        let split = BuildSplit {
            config_parse,
            program_creation,
            bind: bind_start.elapsed(),
        };
        Ok(BuiltProgram {
            owner,
            loader_regions,
            files: listed,
            split,
        })
    }
}
