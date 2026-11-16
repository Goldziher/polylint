//! Run a subprocess engine under a deadline with bounded output capture.
//!
//! `Command::output` waits forever and buffers without limit, so one hung
//! `gofmt` or `golangci-lint` would pin a rayon worker for good. [`run`] bounds
//! both, and on expiry kills the child's whole process group so a wrapper script
//! cannot leave the real tool running.

use std::cell::Cell;
use std::io::{Read, Write};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::Context;

use crate::children;

/// Captured stdout is a formatted file or a report; beyond this the run is refused.
pub(crate) const STDOUT_CAP: usize = 256 * 1024 * 1024;
/// stderr is only ever quoted in an error message.
pub(crate) const STDERR_CAP: usize = 64 * 1024;
/// How long output pipes may stay open after the tool itself has exited. A
/// daemon the tool started (gradle, bazel, dart servers) inherits them and
/// would otherwise hang the worker until it dies.
const DRAIN_GRACE: Duration = Duration::from_secs(2);

/// CPU time and spawn count of the child processes a thread has waited for.
///
/// Monotonic per thread, so a caller measures a region as the difference of two
/// snapshots. `cpu` is `None` where the platform reports no per-child usage.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct ChildUsage {
    pub(crate) cpu: Option<Duration>,
    pub(crate) spawns: u64,
}

thread_local! {
    static CHILD_USAGE: Cell<ChildUsage> = const { Cell::new(ChildUsage { cpu: None, spawns: 0 }) };
}

/// The child usage accumulated so far by tool runs made on the calling thread.
///
/// [`run_for`] blocks its caller until the child is reaped, so a child's CPU
/// lands on exactly the worker that waited for it: two rayon workers running
/// tools at once never see each other's children, unlike a process-wide
/// `RUSAGE_CHILDREN` delta.
pub(crate) fn thread_child_usage() -> ChildUsage {
    CHILD_USAGE.with(Cell::get)
}

fn note_child(cpu: Option<Duration>) {
    CHILD_USAGE.with(|cell| {
        let now = cell.get();
        cell.set(ChildUsage {
            cpu: match (now.cpu, cpu) {
                (Some(total), Some(add)) => Some(total + add),
                (None, Some(add)) => Some(add),
                (total, None) => total,
            },
            spawns: now.spawns + 1,
        });
    });
}

/// Wait for `child` and report its own user + system CPU time.
///
/// `wait4` on the child's pid returns that one child's `rusage` (including the
/// descendants it reaped), where `getrusage(RUSAGE_CHILDREN)` would mix in every
/// child any thread of the process had reaped.
#[cfg(unix)]
fn reap(child: &mut Child) -> std::io::Result<(ExitStatus, Option<Duration>)> {
    use std::os::unix::process::ExitStatusExt;
    let pid = child.id() as libc::pid_t;
    let mut raw: libc::c_int = 0;
    // SAFETY: `rusage` is plain data for which all-zero bytes are a valid value.
    let mut usage: libc::rusage = unsafe { std::mem::zeroed() };
    loop {
        // SAFETY: `raw` and `usage` are valid, writable and outlive the call.
        let reaped = unsafe { libc::wait4(pid, &mut raw, 0, &mut usage) };
        if reaped == pid {
            break;
        }
        if reaped == -1 {
            let error = std::io::Error::last_os_error();
            if error.kind() != std::io::ErrorKind::Interrupted {
                return Err(error);
            }
        }
    }
    let micros = |tv: libc::timeval| Duration::new(tv.tv_sec as u64, (tv.tv_usec as u32).saturating_mul(1000));
    Ok((
        ExitStatus::from_raw(raw),
        Some(micros(usage.ru_utime) + micros(usage.ru_stime)),
    ))
}

#[cfg(not(unix))]
fn reap(child: &mut Child) -> std::io::Result<(ExitStatus, Option<Duration>)> {
    child.wait().map(|status| (status, None))
}

/// What kind of invocation is being run, which sets its default deadline.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Budget {
    /// One file through a stdin formatter or linter.
    PerFile,
    /// A `--version` or presence probe.
    Probe,
    /// A recipe unit or a batch of files: a whole module, chart or thousands of members.
    Unit,
}

impl Budget {
    fn default_timeout(self) -> Duration {
        match self {
            Self::PerFile => Duration::from_secs(60),
            Self::Probe => Duration::from_secs(10),
            Self::Unit => Duration::from_secs(30 * 60),
        }
    }

    /// The deadline for this kind: `POLY_TOOL_TIMEOUT_SECS` when set to an
    /// integer (`0` means unlimited), else the kind's default. `None` is unlimited.
    pub(crate) fn timeout(self) -> Option<Duration> {
        self.resolve(std::env::var("POLY_TOOL_TIMEOUT_SECS").ok().as_deref())
    }

    fn resolve(self, raw: Option<&str>) -> Option<Duration> {
        match raw.and_then(|raw| raw.trim().parse::<u64>().ok()) {
            Some(0) => None,
            Some(secs) => Some(Duration::from_secs(secs)),
            None => Some(self.default_timeout()),
        }
    }
}

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
            "'{}' timed out after {}s and was killed (set POLY_TOOL_TIMEOUT_SECS to raise the limit, 0 for none)",
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
    /// A pipe was still held open by a leftover process after the grace period,
    /// so the captured output may be cut short.
    pub(crate) drain_incomplete: bool,
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

/// Bytes a drain thread has captured so far, shared so a caller that stops
/// waiting can still take what arrived.
#[derive(Default)]
struct Sink {
    kept: Mutex<Vec<u8>>,
    overflowed: AtomicBool,
}

impl Sink {
    fn take(&self) -> (Vec<u8>, bool) {
        let kept = std::mem::take(&mut *self.kept.lock().unwrap_or_else(std::sync::PoisonError::into_inner));
        (kept, self.overflowed.load(Ordering::Relaxed))
    }
}

/// Run `command` under `budget`'s deadline; see [`run_for`].
pub(crate) fn run(
    command: &mut Command,
    tool: &str,
    stdin: Option<Arc<[u8]>>,
    budget: Budget,
) -> anyhow::Result<ToolOutput> {
    run_for(command, tool, stdin, budget.timeout())
}

/// Start `command` where a `poly` child started in the run root would have
/// started it: in the root when the caller set no directory, and a relative
/// directory under the root (ADR 0073). Outside a rooted run it is untouched, so
/// the tool inherits the process working directory as before.
fn anchor_at_run_root(command: &mut Command) {
    let anchored = poly_engine_api::run_root::with(|root| {
        let root = root?;
        match command.get_current_dir() {
            None => Some(root.to_path_buf()),
            Some(dir) if dir.is_relative() => Some(root.join(dir)),
            Some(_) => None,
        }
    });
    if let Some(dir) = anchored {
        command.current_dir(dir);
    }
}

/// Run `command` with `stdin` fed from a dedicated thread while stdout and
/// stderr are drained on others, killing it after `timeout` (`None`: never).
///
/// stdin is written concurrently with draining so a tool that emits while it is
/// still reading cannot wedge both sides on full pipes. The child is reaped on its
/// own thread so the caller blocks on a channel, not a polling sleep.
pub(crate) fn run_for(
    command: &mut Command,
    tool: &str,
    stdin: Option<Arc<[u8]>>,
    timeout: Option<Duration>,
) -> anyhow::Result<ToolOutput> {
    anchor_at_run_root(command);
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
    let pid = child.id();
    children::register(pid);
    let writer = match stdin {
        Some(bytes) => match child.stdin.take() {
            Some(mut pipe) => {
                let (done, finished) = channel();
                std::thread::spawn(move || {
                    let _ = done.send(pipe.write_all(&bytes));
                });
                Some(finished)
            }
            None => {
                children::kill_group(pid);
                children::unregister(pid);
                anyhow::bail!("'{tool}' stdin pipe was not created");
            }
        },
        None => None,
    };
    let (Some(out_pipe), Some(err_pipe)) = (child.stdout.take(), child.stderr.take()) else {
        children::kill_group(pid);
        children::unregister(pid);
        anyhow::bail!("'{tool}' output pipes were not created");
    };
    let (stdout, stdout_done) = spawn_drain(out_pipe, STDOUT_CAP);
    let (stderr, stderr_done) = spawn_drain(err_pipe, STDERR_CAP);
    let (exited, reaped) = channel();
    std::thread::spawn(move || {
        let _ = exited.send(reap(&mut child));
    });
    let waited = match timeout.filter(|limit| Instant::now().checked_add(*limit).is_some()) {
        Some(limit) => reaped.recv_timeout(limit),
        None => reaped.recv().map_err(|_| RecvTimeoutError::Disconnected),
    };
    let status = match waited {
        Ok(waited) => {
            children::unregister(pid);
            let (status, cpu) = waited.with_context(|| format!("waiting for '{tool}'"))?;
            note_child(cpu);
            status
        }
        Err(RecvTimeoutError::Timeout) => {
            children::kill_group(pid);
            if let Ok(Ok((_, cpu))) = reaped.recv() {
                note_child(cpu);
            }
            children::unregister(pid);
            return Err(anyhow::Error::new(TimedOut {
                tool: tool.to_owned(),
                after: timeout.unwrap_or_default(),
            }));
        }
        Err(RecvTimeoutError::Disconnected) => {
            children::unregister(pid);
            anyhow::bail!("lost track of '{tool}' while waiting for it");
        }
    };

    Pipes {
        writer,
        stdout,
        stdout_done,
        stderr,
        stderr_done,
    }
    .collect(tool, status)
}

/// The threads attached to a finished tool's pipes.
struct Pipes {
    writer: Option<Receiver<std::io::Result<()>>>,
    stdout: Arc<Sink>,
    stdout_done: Receiver<()>,
    stderr: Arc<Sink>,
    stderr_done: Receiver<()>,
}

impl Pipes {
    /// Gather what the pipes hold, waiting at most [`DRAIN_GRACE`] for stragglers.
    fn collect(self, tool: &str, status: ExitStatus) -> anyhow::Result<ToolOutput> {
        let grace = Instant::now() + DRAIN_GRACE;
        let mut incomplete = false;
        let stdin_error = self
            .writer
            .and_then(|finished| match finished.recv_timeout(left(grace)) {
                Ok(result) => result.err(),
                Err(_) => {
                    incomplete = true;
                    None
                }
            });
        incomplete |= self.stdout_done.recv_timeout(left(grace)).is_err();
        incomplete |= self.stderr_done.recv_timeout(left(grace)).is_err();
        let (stdout, overflowed) = self.stdout.take();
        let (stderr, _) = self.stderr.take();
        if overflowed {
            anyhow::bail!("'{tool}' wrote more than {} MiB to stdout", STDOUT_CAP >> 20);
        }
        Ok(ToolOutput {
            status,
            stdout,
            stderr,
            stdin_error,
            drain_incomplete: incomplete,
        })
    }
}

fn left(deadline: Instant) -> Duration {
    deadline.saturating_duration_since(Instant::now())
}

/// Drain `pipe` on its own thread into a shared [`Sink`]; the receiver fires at EOF.
fn spawn_drain(pipe: impl Read + Send + 'static, cap: usize) -> (Arc<Sink>, Receiver<()>) {
    let sink = Arc::new(Sink::default());
    let (done, finished) = channel();
    let shared = Arc::clone(&sink);
    std::thread::spawn(move || {
        drain(pipe, cap, &shared);
        let _ = done.send(());
    });
    (sink, finished)
}

/// Read to EOF keeping at most `cap` bytes; the rest is read and dropped so the
/// child never blocks on a full pipe, and the sink records whether anything was dropped.
fn drain(mut pipe: impl Read, cap: usize, sink: &Sink) {
    let mut chunk = [0u8; 32 * 1024];
    loop {
        match pipe.read(&mut chunk) {
            Ok(0) | Err(_) => break,
            Ok(read) => {
                let mut kept = sink.kept.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                let room = cap.saturating_sub(kept.len());
                kept.extend_from_slice(&chunk[..read.min(room)]);
                if read > room {
                    sink.overflowed.store(true, Ordering::Relaxed);
                }
            }
        }
    }
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
    #[cfg(unix)]
    fn child_usage_counts_the_spawn_on_the_waiting_thread() {
        let dir = tempfile::tempdir().expect("tempdir");
        let tool = script(dir.path(), "tool", "exit 0");
        let before = thread_child_usage();
        run_for(&mut Command::new(&tool), "tool", None, LONG).expect("run");
        let after = thread_child_usage();
        assert_eq!(after.spawns, before.spawns + 1);
        assert!(after.cpu.is_some(), "wait4 reports child CPU on unix");
    }

    const SHORT: Option<Duration> = Some(Duration::from_millis(300));
    const LONG: Option<Duration> = Some(Duration::from_secs(10));

    #[test]
    fn a_hung_tool_is_killed_at_the_deadline() {
        let dir = tempfile::tempdir().expect("dir");
        let tool = script(dir.path(), "hang", "sleep 30");
        let started = Instant::now();
        let err = run_for(&mut Command::new(&tool), "hang", None, SHORT).expect_err("must time out");
        assert!(err.downcast_ref::<TimedOut>().is_some(), "{err:#}");
        assert!(started.elapsed() < Duration::from_secs(10));
    }

    #[test]
    fn stderr_and_status_are_captured() {
        let dir = tempfile::tempdir().expect("dir");
        let tool = script(dir.path(), "bad", "echo 'bad key' >&2; exit 2");
        let out = run_for(&mut Command::new(&tool), "bad", None, LONG).expect("runs");
        assert_eq!(out.status.code(), Some(2));
        assert_eq!(out.stderr_first_line(), "bad key");
        assert!(format!("{:#}", out.failure("bad")).contains("status 2: bad key"));
        assert!(!out.drain_incomplete);
    }

    #[test]
    fn stdout_beyond_the_cap_is_dropped_and_flagged() {
        let sink = Sink::default();
        drain(&b"abcdefgh"[..], 5, &sink);
        let (kept, overflowed) = sink.take();
        assert_eq!(kept, b"abcde");
        assert!(overflowed);
    }

    #[test]
    fn a_leftover_process_holding_the_pipes_does_not_hang_the_run() {
        let dir = tempfile::tempdir().expect("dir");
        let tool = script(dir.path(), "daemon", "echo hello; sleep 30 &\nexit 0");
        let started = Instant::now();
        let out = run_for(&mut Command::new(&tool), "daemon", None, LONG).expect("runs");
        assert!(started.elapsed() < Duration::from_secs(8), "{:?}", started.elapsed());
        assert!(out.status.success());
        assert_eq!(out.stdout, b"hello\n");
        assert!(out.drain_incomplete);
    }

    #[test]
    fn no_deadline_waits_for_the_tool_and_stdin_round_trips() {
        let dir = tempfile::tempdir().expect("dir");
        let tool = script(dir.path(), "cat", "cat");
        let input: Arc<[u8]> = Arc::from(&b"abc"[..]);
        let out = run_for(&mut Command::new(&tool), "cat", Some(input), None).expect("runs");
        assert_eq!(out.stdout, b"abc");
    }

    #[test]
    fn budgets_default_by_kind_and_the_env_value_overrides_all_of_them() {
        assert_eq!(Budget::PerFile.resolve(None), Some(Duration::from_secs(60)));
        assert_eq!(Budget::Probe.resolve(None), Some(Duration::from_secs(10)));
        assert_eq!(Budget::Unit.resolve(None), Some(Duration::from_secs(1800)));
        for budget in [Budget::PerFile, Budget::Probe, Budget::Unit] {
            assert_eq!(budget.resolve(Some(" 7 ")), Some(Duration::from_secs(7)));
            assert_eq!(budget.resolve(Some("0")), None);
            assert_eq!(budget.resolve(Some("soon")), Some(budget.default_timeout()));
            assert_eq!(budget.resolve(Some("-3")), Some(budget.default_timeout()));
        }
    }

    #[test]
    fn a_huge_timeout_is_clamped_to_no_deadline_instead_of_panicking() {
        let huge = Budget::Unit.resolve(Some("18446744073709551615"));
        assert_eq!(huge, Some(Duration::from_secs(u64::MAX)));
        let dir = tempfile::tempdir().expect("dir");
        let tool = script(dir.path(), "quick", "echo ok");
        let out = run_for(&mut Command::new(&tool), "quick", None, huge).expect("runs");
        assert_eq!(out.stdout, b"ok\n");
    }
}
