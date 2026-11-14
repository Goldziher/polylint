//! The owner of one tsrs program, which gives the program's memory back when it is dropped.
//!
//! Adapted from `skif-js-types` (`src/owner.rs`, MIT, same author; see `ATTRIBUTIONS.md`).

use tsrs_compiler::{Program, ProgramOptions};

/// A program built for one check. Dropping it frees the program with its checkers (built with `checker_regions`)
/// and its processed-file data: every tsrs handle read from it dangles afterwards, so only owned findings may
/// outlive it.
pub(crate) struct ProgramOwner {
    program: &'static Program,
}

impl ProgramOwner {
    /// Builds the program `options` describe.
    pub(crate) fn new(options: ProgramOptions) -> Self {
        Self {
            program: tsrs_compiler::new_program(options),
        }
    }

    /// The program, valid until the owner drops. tsrs leaks each program from a box, so its checks take
    /// `&'static self`; the lifetime is that of the owner.
    pub(crate) fn program(&self) -> &'static Program {
        self.program
    }
}

impl Drop for ProgramOwner {
    #[expect(
        unsafe_code,
        reason = "tsrs leaks each program from a box; only `free_unshared_program` gives its arenas back (ADR 0051)"
    )]
    fn drop(&mut self) {
        // A program dropped while a panic inside tsrs unwinds may be half updated, and a second panic while freeing
        // it would abort the host: its memory is left to the process instead.
        if std::thread::panicking() {
            return;
        }
        // SAFETY: `new_program` made the program, which never went through `update_program`, so no other program
        // shares its processed files or mapper. The checks that used its checkers have returned, and the findings
        // were copied into owned data before the owner is dropped.
        unsafe { tsrs_compiler::free_unshared_program(self.program) };
    }
}
