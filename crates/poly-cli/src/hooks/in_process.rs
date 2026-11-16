//! The handler `poly hooks` registers for its in-process builtins (ADR 0067,
//! ADR 0071).
//!
//! Each builtin lowers to a [`Call`] naming the `poly` subcommand its re-exec
//! line runs. [`handle`] runs that subcommand in this process and returns the
//! exit code and the bytes the child would have printed, or declines (`None`)
//! whenever it cannot promise that, so the runner starts the child after all.

mod commit;
mod tools;

use std::ffi::OsString;
use std::path::Path;

use anyhow::Result;
use poly_hooks::in_process::{COMMIT, Call, FILE_SAFETY, FMT, LINT, Outcome};

/// The handler registered with [`poly_hooks::in_process::register`]: dispatch
/// `call` to the builtin it names, and decline every other name.
#[must_use]
pub fn handle(call: &Call<'_>) -> Option<Result<Outcome>> {
    match call.name {
        FILE_SAFETY => super::checks::in_process(call),
        COMMIT => commit::run(call),
        LINT | FMT => tools::run(call),
        _ => None,
    }
}

/// The argv the re-exec child's subcommand parser would have seen: the
/// subcommand, the check's own arguments, then the matched files.
fn argv<'a>(subcommand: &'a str, call: &'a Call<'_>) -> impl Iterator<Item = OsString> + 'a {
    std::iter::once(OsString::from(subcommand))
        .chain(call.args.iter().map(OsString::from))
        .chain(call.files.iter().map(|file| file.as_os_str().to_owned()))
}

/// Whether `dir` is this process's working directory, where the re-exec child
/// would have started too.
fn is_working_dir(dir: &Path) -> bool {
    match (
        std::env::current_dir().and_then(|cwd| cwd.canonicalize()),
        dir.canonicalize(),
    ) {
        (Ok(cwd), Ok(dir)) => cwd == dir,
        _ => false,
    }
}

/// Whether this process runs with the global flags a child of the runner gets:
/// none. `--offline` and `--trust` act process-wide here but never reach the
/// child's command line; `POLY_OFFLINE` and `POLY_TRUST` reach both.
fn flags_match_child() -> bool {
    let flags = crate::global();
    !flags.offline && !flags.trust
}
