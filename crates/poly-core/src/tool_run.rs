//! Run a subprocess engine under a deadline with bounded output capture.
//!
//! `Command::output` waits forever and buffers without limit, so one hung
//! `gofmt` or `golangci-lint` would pin a rayon worker for good. [`run`] bounds
//! both, and on expiry kills the child's whole process group so a wrapper script
//! cannot leave the real tool running.

use std::io::{Read, Write};
use std::process::{Command, ExitStatus, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Context;

/// Deadline for one formatter or linter invocation.
pub(crate) const DEFAULT_TIMEOUT: Duration = Duration::from_secs(60);
/// Deadline for a `--version` probe.
pub(crate) const PROBE_TIMEOUT: Duration = Duration::from_secs(10);
/// Captured stdout is a formatted file or a report; beyond this the run is refused.
pub(crate) const STDOUT_CAP: usize = 256 * 1024 * 1024;
/// stderr is only ever quoted in an error message.
pub(crate) const STDERR_CAP: usize = 64 * 1024;

/// The tool outlived its deadline and was killed.
#[derive(Debug)]
pub(crate) struct TimedOut {
    tool: String,
    after: Duration,
}

impl std::fmt::Display for TimedOut {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "'{}' timed out after {}s and was killed (set POLY_TOOL_TIMEOUT_SECS to raise the limit)",
            self.tool,
            self.after.as_secs()
        )
    }
}

impl std::error::Error for TimedOut {}

/// What a finished invocation produced.
#[derive(Debug)]
pub(crate) struct ToolOutput {
    pub(crate) status: ExitStatus,
    pub(crate) stdout: Vec<u8>,
    pub(crate) stderr: Vec<u8>,
    /// A failure feeding stdin; only meaningful when the tool otherwise succeeded.
    pub(crate) stdin_error: Option<std::io::Error>,
}

impl ToolOutput {
    /// The first non-empty stderr line, for naming why a tool refused.
    pub(crate) fn stderr_first_line(&self) -> String {
        String::from_utf8_lossy(&self.stderr)
            .lines()
            .map(str::trim)
            .find(|line| !line.is_empty())
            .unwrap_or("")
            .to_owned()
    }

    /// An error naming the tool, how it ended and what it said, for a run that
    /// must not be mistaken for a clean pass.
    pub(crate) fn failure(&self, tool: &str) -> anyhow::Error {
        let how = match self.status.code() {
            Some(code) => format!("exited with status {code}"),
            None => "was killed by a signal".to_owned(),
        };
        match self.stderr_first_line() {
            line if line.is_empty() => anyhow::anyhow!("'{tool}' {how}"),
            line => anyhow::anyhow!("'{tool}' {how}: {line}"),
        }
    }
}

/// The per-invocation deadline: `POLY_TOOL_TIMEOUT_SECS` when it holds a positive
/// integer, else [`DEFAULT_TIMEOUT`].
pub(crate) fn tool_timeout() -> Duration {
    std::env::var("POLY_TOOL_TIMEOUT_SECS")
        .ok()
        .and_then(|raw| raw.trim().parse::<u64>().ok())
        .filter(|secs| *secs > 0)
        .map_or(DEFAULT_TIMEOUT, Duration::from_secs)
}

/// Run `command` with `stdin` fed from a dedicated thread while stdout and
/// stderr are drained on others, killing it after `timeout`.
///
/// stdin is written concurrently with draining so a tool that emits while it is
/// still reading cannot wedge both sides on full pipes.
pub(crate) fn run(
    command: &mut Command,
    tool: &str,
    stdin: Option<Arc<[u8]>>,
    timeout: Duration,
) -> anyhow::Result<ToolOutput> {
    command
        .stdin(if stdin.is_some() { Stdio::piped() } else { Stdio::null() })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let mut child = command.spawn().with_context(|| format!("failed to spawn '{tool}'"))?;

    let writer = match stdin {
        Some(bytes) => {
            let mut pipe = child
                .stdin
                .take()
                .with_context(|| format!("'{tool}' stdin pipe was not created"))?;
            Some(std::thread::spawn(move || pipe.write_all(&bytes)))
        }
        None => None,
    };
    let out_pipe = child.stdout.take().context("stdout pipe was not created")?;
    let err_pipe = child.stderr.take().context("stderr pipe was not created")?;
    let stdout = std::thread::spawn(move || drain(out_pipe, STDOUT_CAP));
    let stderr = std::thread::spawn(move || drain(err_pipe, STDERR_CAP));

    let deadline = Instant::now() + timeout;
    let mut pause = Duration::from_micros(200);
    let status = loop {
        if let Some(status) = child.try_wait().with_context(|| format!("waiting for '{tool}'"))? {
            break status;
        }
        if Instant::now() >= deadline {
            kill_group(&mut child);
            let _ = child.wait();
            return Err(anyhow::Error::new(TimedOut {
                tool: tool.to_owned(),
                after: timeout,
            }));
        }
        std::thread::sleep(pause);
        pause = (pause * 2).min(Duration::from_millis(10));
    };

    let stdin_error = writer.and_then(|handle| handle.join().ok()).and_then(Result::err);
    let (stdout, overflowed) = stdout.join().unwrap_or_default();
    let (stderr, _) = stderr.join().unwrap_or_default();
    if overflowed {
        anyhow::bail!("'{tool}' wrote more than {} MiB to stdout", STDOUT_CAP >> 20);
    }
    Ok(ToolOutput {
        status,
        stdout,
        stderr,
        stdin_error,
    })
}

/// Read to EOF keeping at most `cap` bytes; the rest is read and dropped so the
/// child never blocks on a full pipe. The flag says whether anything was dropped.
fn drain(mut pipe: impl Read, cap: usize) -> (Vec<u8>, bool) {
    let mut kept = Vec::new();
    let mut overflowed = false;
    let mut chunk = [0u8; 32 * 1024];
    loop {
        match pipe.read(&mut chunk) {
            Ok(0) | Err(_) => break,
            Ok(read) => {
                let room = cap.saturating_sub(kept.len());
                kept.extend_from_slice(&chunk[..read.min(room)]);
                overflowed |= read > room;
            }
        }
    }
    (kept, overflowed)
}

fn kill_group(child: &mut std::process::Child) {
    #[cfg(unix)]
    if let Ok(pid) = i32::try_from(child.id()) {
        // SAFETY: `kill` takes no memory operands. The child leads its own process
        // group (`process_group(0)` at spawn), so `-pid` signals only its tree.
        unsafe { libc::kill(-pid, libc::SIGKILL) };
    }
    let _ = child.kill();
}

/// Shell-script stand-ins for tools, shared by engine tests.
#[cfg(all(test, unix))]
pub(crate) mod fake_tool {
    use std::os::unix::fs::PermissionsExt;
    use std::path::{Path, PathBuf};

    /// Write an executable `/bin/sh` script `name` with `body` into `dir`.
    pub(crate) fn script(dir: &Path, name: &str, body: &str) -> PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).expect("write script");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        path
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::fake_tool::script;
    use super::*;

    #[test]
    fn a_hung_tool_is_killed_at_the_deadline() {
        let dir = tempfile::tempdir().expect("dir");
        let tool = script(dir.path(), "hang", "sleep 30");
        let started = Instant::now();
        let err = run(&mut Command::new(&tool), "hang", None, Duration::from_millis(300)).expect_err("must time out");
        assert!(err.downcast_ref::<TimedOut>().is_some(), "{err:#}");
        assert!(started.elapsed() < Duration::from_secs(10));
    }

    #[test]
    fn stderr_and_status_are_captured() {
        let dir = tempfile::tempdir().expect("dir");
        let tool = script(dir.path(), "bad", "echo 'bad key' >&2; exit 2");
        let out = run(&mut Command::new(&tool), "bad", None, Duration::from_secs(10)).expect("runs");
        assert_eq!(out.status.code(), Some(2));
        assert_eq!(out.stderr_first_line(), "bad key");
        assert!(format!("{:#}", out.failure("bad")).contains("status 2: bad key"));
    }

    #[test]
    fn stdout_beyond_the_cap_is_dropped_and_flagged() {
        let (kept, overflowed) = drain(&b"abcdefgh"[..], 5);
        assert_eq!(kept, b"abcde");
        assert!(overflowed);
    }
}
