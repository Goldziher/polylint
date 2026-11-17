//! How the programs of one analysis are built: the config first, which the host may change, then the program over
//! the files the run shares.

use std::sync::Arc;

use tsrs_ast::Diagnostic;
use tsrs_compiler::{CompilerHost, ProgramOptions, ProgramRegions};
use tsrs_core::{P, tspath};
use tsrs_tsoptions::ParsedCommandLine;
use tsrs_vfs::FS;

use crate::observe::{ObservedFs, Observer};
use crate::shared_configs::{ConfigCounts, ConfigHost, ConfigReader, SharedConfigs};
use crate::shared_files::{Counts, ProgramFiles, SharedFiles, SharingHost};
use crate::vfs::{self, BomFiles};

/// What the programs of one analysis are built from: its file system, the files and configs it shares with the other
/// analyses of the run, and its own counts of how it got them.
///
/// A program is built in two steps, with the host in between:
///
/// 1. [`ProgramHost::load_config`] reads the program's config as a command line the host owns: it may replace the
///    compiler options (the checker count, strictness, `noEmit`) and read or drop the config's `errors` (an `extends`
///    that cannot be resolved).
/// 2. [`ProgramHost::program_options`] turns the command line into the options of a program that reads its files
///    from the run's, and [`ProgramOwner::new`](crate::ProgramOwner::new) builds it.
///
/// tsrs allocates what the config and the compiler host need in the region current on the calling thread: a host
/// that frees its programs enters a scratch region of its own around both steps and drops it after the program.
pub struct ProgramHost {
    fs: Arc<dyn FS>,
    boms: Arc<BomFiles>,
    files: Arc<SharedFiles>,
    file_counts: Arc<Counts>,
    configs: Arc<ConfigReader>,
    observer: Option<Arc<dyn Observer>>,
}

impl ProgramHost {
    /// A host of programs reading through `fs` ([`wrap_fs`](crate::wrap_fs)), whose record of byte order marks is
    /// `boms` (an empty one if it keeps none), with `cwd` (as tsrs names files) as the directory relative config
    /// names are resolved against, sharing `files` and `configs` with every other host of the run that was given
    /// them.
    #[must_use]
    pub fn new(
        fs: Arc<dyn FS>,
        boms: Arc<BomFiles>,
        cwd: String,
        files: Arc<SharedFiles>,
        configs: Arc<SharedConfigs>,
    ) -> Self {
        let configs = Arc::new(ConfigReader::new(
            configs,
            Arc::<ConfigCounts>::default(),
            ConfigHost::new(Arc::clone(&fs), cwd),
        ));
        Self {
            fs,
            boms,
            files,
            file_counts: Arc::default(),
            configs,
            observer: None,
        }
    }

    /// Tells `observer` what the programs built from now on read. Without one nothing is told, at no cost.
    #[must_use]
    pub fn with_observer(mut self, observer: Arc<dyn Observer>) -> Self {
        self.observer = Some(observer);
        self
    }

    /// The file system the configs and programs read through.
    #[must_use]
    pub fn fs(&self) -> &Arc<dyn FS> {
        &self.fs
    }

    /// The files read so far whose text starts with a byte order mark.
    #[must_use]
    pub fn boms(&self) -> &Arc<BomFiles> {
        &self.boms
    }

    /// The files the programs share with the run.
    #[must_use]
    pub fn files(&self) -> &Arc<SharedFiles> {
        &self.files
    }

    /// How the programs built through this host got their files.
    #[must_use]
    pub fn file_counts(&self) -> &Counts {
        &self.file_counts
    }

    /// The reader of the configs, which counts how this host got them.
    #[must_use]
    pub fn configs(&self) -> &Arc<ConfigReader> {
        &self.configs
    }

    /// The config at `path` as a command line the caller owns and may change before
    /// [`ProgramHost::program_options`]: a copy of the run's parse ([`ConfigReader::load_own`]).
    ///
    /// # Errors
    /// The diagnostics of a config that cannot be read or parsed; empty when it cannot be read without tsrs saying
    /// why.
    pub fn load_config(&self, path: &str) -> Result<ParsedCommandLine, Vec<P<Diagnostic>>> {
        let loaded = self.configs.load_own(path);
        if let Some(observer) = &self.observer {
            let extended = loaded.as_ref().ok().map(ParsedCommandLine::extended_source_files);
            observer.config(path, extended.as_deref());
        }
        loaded
    }

    /// The options of the program of `config`, the command line of the config at `config_path`: its compiler host
    /// reads every file from the run's shared files, holding the project files in `files`, and the configs of its
    /// project references from the run's shared configs. Referenced projects contribute their sources, the loader
    /// regions are `regions` and the checkers get regions of their own, which go back with the program. The caller
    /// may change the options further before it builds the program ([`ProgramOwner::new`](crate::ProgramOwner::new)
    /// with the same `files`).
    ///
    /// Keep `regions` until after the program is dropped, then `files`, then the run's shared files and configs:
    /// each is pointed into by what comes before it.
    #[must_use]
    pub fn program_options(
        &self,
        config_path: &str,
        config: ParsedCommandLine,
        files: Arc<ProgramFiles>,
        regions: Arc<ProgramRegions>,
    ) -> ProgramOptions {
        let fs: Arc<dyn FS> = match &self.observer {
            Some(observer) => Arc::new(ObservedFs::new(Arc::clone(&self.fs), Arc::clone(observer))),
            None => Arc::clone(&self.fs),
        };
        let compiler_host = tsrs_compiler::new_compiler_host(
            &tspath::get_directory_path(config_path),
            fs,
            &vfs::lib_path(),
            None,
            None,
            None,
        );
        // The files come parsed and bound from the files the run shares, which the run (the lib and `node_modules`
        // declaration files) or the program's owner and holder (its project files) keep until after it is freed.
        let compiler_host: Arc<dyn CompilerHost> = Arc::new(SharingHost::new(
            compiler_host,
            (
                Arc::clone(&self.files),
                Arc::clone(&self.boms),
                Arc::clone(&self.file_counts),
            ),
            Arc::clone(&self.configs),
            files,
            self.observer.clone(),
        ));
        let mut options = ProgramOptions::new(P::new(config), compiler_host);
        // Referenced projects contribute their sources, as editors and tsrs-lint read them.
        options.use_source_of_project_reference = true;
        options.regions = Some(regions);
        // The checkers' memory goes back with the program.
        options.checker_regions = true;
        options
    }
}

const _: () = {
    const fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<ProgramHost>();
};
