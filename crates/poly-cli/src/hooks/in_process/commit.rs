//! The `commit` builtin in process: `poly commit <message file>` as the re-exec
//! child runs it, from the hook's directory, into a buffer.

use std::io;
use std::path::Path;

use anyhow::Result;
use clap::Parser;
use poly_hooks::in_process::{Call, Outcome};

use crate::commit::{CommitArgs, lint_with_io};
use crate::config_sources::{RemoteExtendsResolver, repo_root_from};
use crate::global::ColorChoice;

/// `poly commit`'s argument grammar, for parsing the call's argv the way the
/// child parses its command line.
#[derive(Parser)]
#[command(name = "poly commit")]
struct CommitCommand {
    #[command(flatten)]
    args: CommitArgs,
}

/// The exit code of a message that could not be checked, as `poly commit`
/// returns it.
const UNVERIFIED: i32 = 2;

/// Run a `commit` call as `poly commit <args> <files>` would from `call.dir`.
///
/// Declines `--stdin`, since the child's stdin is the runner's, which this does
/// not reproduce, and global flags the child would not have been given.
pub(super) fn run(call: &Call<'_>) -> Option<Result<Outcome>> {
    if !super::flags_match_child() {
        return None;
    }
    let mut args = match CommitCommand::try_parse_from(super::argv("commit", call)) {
        Ok(command) => command.args,
        Err(error) => {
            return Some(Ok(Outcome {
                code: error.exit_code(),
                output: error.render().to_string().into_bytes(),
            }));
        }
    };
    if args.stdin {
        return None;
    }
    // The child's stderr is a pipe the runner captures, so gitfluff's `auto`
    // resolves to no colour there, whatever this process's stderr is.
    args.color = ColorChoice::Never;
    // The child read relative paths from `call.dir`; this process reads them
    // from its own working directory, so they are joined unless the two agree.
    if !super::is_working_dir(call.dir) {
        for path in [&mut args.commit_file, &mut args.from_file, &mut args.config]
            .into_iter()
            .flatten()
        {
            if path.is_relative() {
                *path = call.dir.join(&*path);
            }
        }
    }
    Some(Ok(lint(&args, call.dir)))
}

/// Lint the message as the child does, collecting what it writes to stdout and
/// then to stderr, and mapping a failure to `poly commit: <error>` and exit 2.
fn lint(args: &CommitArgs, dir: &Path) -> Outcome {
    let mut output = Vec::new();
    let resolver = match RemoteExtendsResolver::new(&repo_root_from(dir)) {
        Ok(resolver) => resolver,
        Err(error) => return unverified(output, &error),
    };
    let mut diagnostics = Vec::new();
    match lint_with_io(
        args,
        &resolver,
        Some(dir),
        &mut io::empty(),
        &mut output,
        &mut diagnostics,
    ) {
        Ok(code) => {
            output.extend_from_slice(&diagnostics);
            // The child exits with the low byte, as `ExitCode::from(code as u8)` does.
            Outcome {
                code: i32::from(code as u8),
                output,
            }
        }
        Err(error) => unverified(output, &error),
    }
}

/// `output` followed by the line `poly commit` prints for a run it could not
/// complete.
fn unverified(mut output: Vec<u8>, error: &anyhow::Error) -> Outcome {
    output.extend_from_slice(format!("poly commit: {error:#}\n").as_bytes());
    Outcome {
        code: UNVERIFIED,
        output,
    }
}
