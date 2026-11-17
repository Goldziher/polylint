//! The owner of one tsrs program, which gives the program's memory back when it is dropped (polylint ADR 0078).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use tsrs_ast::{Diagnostic, SourceFile};
use tsrs_compiler::{Context, Program, ProgramOptions};
use tsrs_core::P;

use crate::shared_files::ProgramFiles;

/// Where a program is freed: the threads the parallel work of freeing it runs on. A host that builds its programs on
/// a thread pool of its own frees them there, so that the work never runs on the threads of whoever drops the last
/// handle to a shared program.
pub trait Executor: Send + Sync {
    /// Runs `work` once, to its end, before returning.
    fn run(&self, work: &mut (dyn FnMut() + Send));
}

/// A program built for one analysis, or for the analyses of a run that share it. Dropping it frees the program with
/// its checkers (built with `checker_regions`) and its processed-file data; every tsrs handle read from it (a source
/// file, a node, a diagnostic) dangles afterwards, so only owned data may outlive it.
pub struct ProgramOwner {
    program: &'static Program,
    /// The project files the program read from the run (`shared_files`), let go once the program is freed.
    files: Arc<ProgramFiles>,
    /// Where the program is freed; on the dropping thread without one.
    executor: Option<Arc<dyn Executor>>,
    /// Set when a walk of the program panicked inside tsrs.
    poisoned: AtomicBool,
}

impl ProgramOwner {
    /// Builds the program `options` describe ([`ProgramHost::program_options`](crate::ProgramHost::program_options)),
    /// whose compiler host holds the project files it reads in `files`; it is freed on `executor`, or on the
    /// dropping thread without one.
    #[must_use]
    pub fn new(options: ProgramOptions, files: Arc<ProgramFiles>, executor: Option<Arc<dyn Executor>>) -> Self {
        Self {
            program: tsrs_compiler::new_program(options),
            files,
            executor,
            poisoned: AtomicBool::new(false),
        }
    }

    /// The program, for as long as the owner lives.
    #[must_use]
    pub fn program(&self) -> &Program {
        self.program
    }

    /// The diagnostics of the program in the order `tsc` reports them (the config's, the syntax errors', then the
    /// program's, global and semantic ones, stopping at the first group that has any), sorted and deduplicated as
    /// `tsc` does. With `files`, the per-file diagnostics are those of the named files only (as tsrs names files; a
    /// name the program does not hold is skipped), while the config's and the program's own are always given.
    ///
    /// The diagnostics point into the program: copy what is needed out of them before the owner is dropped. tsrs
    /// allocates them in the checkers' regions when the program was built with `checker_diagnostic_regions`, else in
    /// the arena of the thread that made them, which is never freed.
    #[must_use]
    pub fn diagnostics(&self, files: Option<&[&str]>) -> Vec<P<Diagnostic>> {
        let program = self.program;
        let files: Option<Vec<P<SourceFile>>> =
            files.map(|names| names.iter().filter_map(|name| program.get_source_file(name)).collect());
        let found = tsrs_compiler::get_diagnostics_of_any_program(
            &Context::default(),
            program,
            files.as_deref(),
            false,
            &mut |ctx, file| program.get_bind_diagnostics(ctx, file),
            &mut |ctx, file| program.get_semantic_diagnostics(ctx, file),
        );
        tsrs_compiler::sort_and_deduplicate_diagnostics(&found)
    }

    /// The project files the program read, for whatever must keep them until after the memory the program points
    /// into from beside it (its loader regions, its config) is gone.
    #[must_use]
    pub fn files(&self) -> Arc<ProgramFiles> {
        Arc::clone(&self.files)
    }

    /// Leaves the program's memory to the process when the owner drops: a walk that panicked inside tsrs may have
    /// left it half updated, and a panic while freeing it would abort the host.
    pub fn poison(&self) {
        self.poisoned.store(true, Ordering::Release);
    }

    /// Whether a walk of the program panicked.
    #[must_use]
    pub fn poisoned(&self) -> bool {
        self.poisoned.load(Ordering::Acquire)
    }
}

impl Drop for ProgramOwner {
    #[expect(
        unsafe_code,
        reason = "tsrs leaks each program from a box; only `free_unshared_program` gives its arenas back (ADR 0078)"
    )]
    fn drop(&mut self) {
        // A program dropped while a panic inside tsrs unwinds may be half updated, and a second panic while freeing
        // it would abort the host: its memory is left to the process instead.
        if std::thread::panicking() || self.poisoned() {
            return;
        }
        let program = self.program;
        // SAFETY: `new_program` made the program, which never went through `update_program`, so no other program
        // shares its processed files or mapper. The owner lends the program for its own lifetime only (`program`,
        // `diagnostics`), and this is its drop: every checker handle was released when `for_each_checker_group` or
        // the diagnostics call returned, the analyses that share the program hold the owner through one `Arc` and
        // drop it after their walks, and each copies its facts into owned data before then. Freeing drops the
        // program, its processed files and its mapper, none of which owns a source file: every file it read through
        // its compiler host lives in a region of the run's shared files (`shared_files`), kept alive past this call
        // by the run (the lib and `node_modules` declaration files) or by this owner's and its holder's hold on the
        // program's project files, and the files it parsed for itself alone live in its loader regions, dropped
        // after it.
        let free = move || unsafe { tsrs_compiler::free_unshared_program(program) };
        match &self.executor {
            Some(executor) => {
                let mut free = Some(free);
                executor.run(&mut || {
                    if let Some(free) = free.take() {
                        free();
                    }
                });
            }
            None => free(),
        }
    }
}
