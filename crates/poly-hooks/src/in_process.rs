//! Hooks the running `poly` binary performs in its own process (ADR 0067).
//!
//! A [`HookCommand::InProcess`](crate::model::HookCommand::InProcess) hook names a
//! check rather than a command. The binary that embeds this crate registers one
//! [`Handler`] at startup; the runner hands it the hook's working directory and
//! matched files instead of spawning `poly` again through the shell. Without a
//! registered handler, or when the handler declines the name, the hook runs its
//! equivalent command line exactly as before, so a library consumer that never
//! registers one keeps the re-exec behaviour.

use std::path::Path;
use std::sync::OnceLock;

/// One in-process hook invocation.
#[derive(Debug, Clone, Copy)]
pub struct Call<'a> {
    /// The check to run (the variant's `name`).
    pub name: &'a str,
    /// The check's own arguments.
    pub args: &'a [String],
    /// The directory the re-exec child would have run from: the execution root
    /// (the staged snapshot under isolation) plus the hook's `cwd`.
    pub dir: &'a Path,
    /// The matched files, relative to `dir`, as the child would have received them.
    pub files: &'a [&'a Path],
}

/// What an in-process check reported.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outcome {
    /// The exit code the re-exec child would have returned.
    pub code: i32,
    /// The bytes the child would have written to stdout and stderr.
    pub output: Vec<u8>,
}

/// Runs a [`Call`], or returns `None` to fall back to the command line.
pub type Handler = fn(&Call<'_>) -> Option<anyhow::Result<Outcome>>;

/// The [`Call::name`] of the `file_safety` builtin group: its arguments are
/// `poly hooks check` flags.
pub const FILE_SAFETY: &str = "file-safety";

static HANDLER: OnceLock<Handler> = OnceLock::new();

/// Register the process's in-process hook handler. Returns `false` when one was
/// already registered; the first registration stays.
pub fn register(handler: Handler) -> bool {
    HANDLER.set(handler).is_ok()
}

/// Run `call` through the registered handler, or `None` when there is none or
/// it declined.
///
/// A panicking handler is reported as an error for this hook alone, as a
/// crashed child would have been, rather than unwinding through the runner.
pub(crate) fn dispatch(call: &Call<'_>) -> Option<anyhow::Result<Outcome>> {
    let handler = HANDLER.get()?;
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| handler(call)))
        .unwrap_or_else(|_| Some(Err(anyhow::anyhow!("in-process check `{}` panicked", call.name))))
}
