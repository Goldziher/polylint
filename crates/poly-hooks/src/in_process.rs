//! Hooks the running `poly` binary performs in its own process (ADR 0067, ADR 0071).
//!
//! A [`HookCommand::InProcess`](crate::model::HookCommand::InProcess) hook names a
//! check rather than a command. The binary that embeds this crate registers one
//! [`Handler`] at startup; the runner hands it the hook's working directory and
//! matched files instead of spawning `poly` again through the shell. Without a
//! registered handler, or when the handler declines the name, the hook runs its
//! equivalent command line exactly as before, so a library consumer that never
//! registers one keeps the re-exec behaviour.
//!
//! The handler runs on a thread of its own, bounded by the hook's timeout budget
//! the way a child process is: past the limit the hook is reported as timed out
//! and the thread is abandoned, since a thread cannot be killed (ADR 0071).

use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::time::{Duration, Instant};

use crate::timeout::Budget;

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

/// The [`Call::name`] of the `lint` builtin: its arguments are `poly lint` flags.
pub const LINT: &str = "lint";

/// The [`Call::name`] of the `fmt` builtin: its arguments are `poly fmt` flags.
pub const FMT: &str = "fmt";

/// The [`Call::name`] of the `commit` builtin: its arguments are `poly commit`
/// flags, and its one file is the commit message.
pub const COMMIT: &str = "commit";

/// Stack for the thread a check runs on. A `lint` or `fmt` check loads config
/// and plans engines on it, so it gets what the lint runner gives its own
/// workers rather than the 2 MiB a spawned thread defaults to.
const CHECK_STACK_SIZE: usize = 16 * 1024 * 1024;

static HANDLER: OnceLock<Handler> = OnceLock::new();

/// Register the process's in-process hook handler. Returns `false` when one was
/// already registered; the first registration stays.
pub fn register(handler: Handler) -> bool {
    HANDLER.set(handler).is_ok()
}

/// How a [`dispatch`] ended.
pub(crate) enum Dispatched {
    /// The handler returned: `None` when there is none or it declined.
    Finished(Option<anyhow::Result<Outcome>>),
    /// The budget's limit passed first, after this long. The check's thread is
    /// left to finish on its own and its result is discarded.
    TimedOut(Duration),
}

/// [`Call`] with owned fields, so the check can run on a thread that outlives
/// the runner's borrow when it times out.
struct OwnedCall {
    name: String,
    args: Vec<String>,
    dir: PathBuf,
    files: Vec<PathBuf>,
}

impl OwnedCall {
    fn of(call: &Call<'_>) -> Self {
        Self {
            name: call.name.to_owned(),
            args: call.args.to_vec(),
            dir: call.dir.to_path_buf(),
            files: call.files.iter().map(|file| file.to_path_buf()).collect(),
        }
    }

    /// Run `handler` over this call. A panicking handler is reported as an
    /// error for this hook alone, as a crashed child would have been, rather
    /// than unwinding through the runner.
    fn run(&self, handler: Handler) -> Option<anyhow::Result<Outcome>> {
        let files: Vec<&Path> = self.files.iter().map(PathBuf::as_path).collect();
        let call = Call {
            name: &self.name,
            args: &self.args,
            dir: &self.dir,
            files: &files,
        };
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| handler(&call)))
            .unwrap_or_else(|_| Some(Err(anyhow::anyhow!("in-process check `{}` panicked", self.name))))
    }
}

/// Run `call` through the registered handler under `budget`.
///
/// The check runs on a thread of its own, never on the runner's worker: a
/// `lint` or `fmt` check fans out over rayon, and from a worker of the runner's
/// pool it would borrow that pool and its small stacks instead of the lint
/// runner's own. `announce` is called on the budget's still-running cadence,
/// as it is for a child process.
pub(crate) fn dispatch(call: &Call<'_>, budget: Budget, announce: &dyn Fn(Duration)) -> Dispatched {
    let Some(&handler) = HANDLER.get() else {
        return Dispatched::Finished(None);
    };
    let owned = OwnedCall::of(call);
    let (sender, receiver) = mpsc::channel();
    let spawned = std::thread::Builder::new()
        .name(format!("poly-hook-{}", call.name))
        .stack_size(CHECK_STACK_SIZE)
        .spawn(move || {
            // The receiver is gone only once the runner gave up on a timed-out check.
            let _ = sender.send(owned.run(handler));
        });
    if let Err(error) = spawned {
        let error = anyhow::Error::new(error).context(format!("starting in-process check `{}`", call.name));
        return Dispatched::Finished(Some(Err(error)));
    }
    wait(&receiver, budget, announce, call.name)
}

/// Wait for the check's result, announcing and timing out as `budget` says.
fn wait(
    receiver: &Receiver<Option<anyhow::Result<Outcome>>>,
    budget: Budget,
    announce: &dyn Fn(Duration),
    name: &str,
) -> Dispatched {
    let started = Instant::now();
    let mut next_notice = budget.announce_after;
    loop {
        let wake = match (budget.limit, next_notice) {
            (Some(limit), Some(notice)) => Some(limit.min(notice)),
            (limit, notice) => limit.or(notice),
        };
        let reply = match wake {
            Some(at) => receiver.recv_timeout(at.saturating_sub(started.elapsed())),
            None => receiver.recv().map_err(|_| RecvTimeoutError::Disconnected),
        };
        match reply {
            Ok(result) => return Dispatched::Finished(result),
            Err(RecvTimeoutError::Disconnected) => {
                let error = anyhow::anyhow!("in-process check `{name}` stopped without a result");
                return Dispatched::Finished(Some(Err(error)));
            }
            Err(RecvTimeoutError::Timeout) => {
                let elapsed = started.elapsed();
                if budget.limit.is_some_and(|limit| elapsed >= limit) {
                    return Dispatched::TimedOut(elapsed);
                }
                if let Some(notice) = next_notice
                    && elapsed >= notice
                {
                    announce(elapsed);
                    next_notice = Some(notice + budget.announce_every);
                }
            }
        }
    }
}
