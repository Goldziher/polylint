//! Wall, thread-CPU and child-process CPU time of one engine call.
//!
//! Wall time alone cannot tell an engine that is slow from an engine that sat
//! behind a lock or a saturated core while other workers ran. The thread's CPU
//! time can: it stops counting whenever the thread is not executing. A
//! subprocess engine's work happens in its child, so the child's CPU time (from
//! [`crate::tool_run`]) is added before a call is called idle.

use std::time::{Duration, Instant};

use crate::tool_run::{ChildUsage, thread_child_usage};

/// Per-file engine runtimes at or above this threshold are useful diagnostic
/// signals for pathological inputs or backend performance regressions.
const SLOW_ENGINE_DEBUG: Duration = Duration::from_millis(250);
const SLOW_ENGINE_WARN: Duration = Duration::from_secs(2);
const GENERIC_TIER_ENGINE: &str = "treesitter";

/// A running measurement of one engine call.
pub(super) struct EngineClock {
    wall: Instant,
    cpu: Option<Duration>,
    child: ChildUsage,
}

/// Elapsed wall time and, where the platform reports it, the CPU time the
/// calling thread and the child processes it waited for spent in the interval.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct EngineTime {
    pub(super) wall: Duration,
    pub(super) cpu: Option<Duration>,
    /// CPU time of the child processes the call ran; `None` where unmeasured.
    pub(super) child_cpu: Option<Duration>,
    /// How many child processes the call spawned and waited for.
    pub(super) child_spawns: u64,
}

/// Why a mostly-waiting call was not computing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum WaitKind {
    /// It ran a child process that was itself mostly idle or starved of a core.
    Child,
    /// It ran no child: it was parked on a lock or a saturated core.
    Blocked,
}

impl EngineTime {
    /// Whether the call was mostly waiting rather than computing: its own plus
    /// its children's CPU time is under half its wall time. `false` when CPU
    /// time is unavailable, so an unmeasurable call is never excused.
    pub(super) fn mostly_waiting(&self) -> bool {
        self.cpu
            .is_some_and(|cpu| (cpu + self.child_cpu.unwrap_or_default()) * 2 < self.wall)
    }

    /// What a mostly-waiting call was waiting on; `None` when it was not waiting.
    pub(super) fn wait_kind(&self) -> Option<WaitKind> {
        self.mostly_waiting().then_some(if self.child_spawns > 0 {
            WaitKind::Child
        } else {
            WaitKind::Blocked
        })
    }
}

impl EngineClock {
    pub(super) fn start() -> Self {
        Self {
            wall: Instant::now(),
            cpu: thread_cpu_time(),
            child: thread_child_usage(),
        }
    }

    pub(super) fn started(&self) -> Instant {
        self.wall
    }

    pub(super) fn stop(&self) -> EngineTime {
        let child = thread_child_usage();
        EngineTime {
            wall: self.wall.elapsed(),
            cpu: self
                .cpu
                .zip(thread_cpu_time())
                .map(|(start, end)| end.saturating_sub(start)),
            child_cpu: child
                .cpu
                .map(|end| end.saturating_sub(self.child.cpu.unwrap_or_default())),
            child_spawns: child.spawns - self.child.spawns,
        }
    }
}

#[cfg(unix)]
fn thread_cpu_time() -> Option<Duration> {
    let mut ts = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    // SAFETY: `ts` is a valid, writable timespec for the duration of the call.
    let status = unsafe { libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &mut ts) };
    (status == 0).then(|| Duration::new(ts.tv_sec as u64, ts.tv_nsec as u32))
}

#[cfg(not(unix))]
fn thread_cpu_time() -> Option<Duration> {
    None
}

/// Record an engine call for `--timings` and report it if it took long. Only a
/// call that spent its time computing is flagged as a slow backend: one that
/// mostly waited says nothing about the backend's own cost on this file, so it
/// is logged at `info` with every clock and what it waited on.
pub(super) fn note_slow_engine(path: &std::path::Path, bytes: usize, engine: &'static str, time: EngineTime) {
    crate::timings::record_engine(engine, path, time.wall, time.cpu, time.child_cpu);
    if time.wall < SLOW_ENGINE_DEBUG {
        return;
    }
    let bytes = bytes as u64;
    let elapsed_ms = time.wall.as_millis() as u64;
    let cpu_ms = time.cpu.map(|cpu| cpu.as_millis() as u64);
    let child_cpu_ms = time.child_cpu.map(|cpu| cpu.as_millis() as u64);
    let file = path.display();
    if time.wall >= SLOW_ENGINE_WARN {
        match time.wait_kind() {
            Some(WaitKind::Child) => tracing::info!(
                %file, bytes, engine, elapsed_ms, ?cpu_ms, ?child_cpu_ms,
                "backend run mostly waiting on a child process"
            ),
            Some(WaitKind::Blocked) => tracing::info!(
                %file, bytes, engine, elapsed_ms, ?cpu_ms, ?child_cpu_ms,
                "backend run mostly blocked (lock or saturated cores)"
            ),
            None if engine == GENERIC_TIER_ENGINE => tracing::warn!(
                %file, bytes, engine, elapsed_ms, ?cpu_ms, ?child_cpu_ms,
                "slow generic formatter run"
            ),
            None => tracing::warn!(
                %file, bytes, engine, elapsed_ms, ?cpu_ms, ?child_cpu_ms,
                "slow backend run; consider reporting it upstream"
            ),
        }
    } else {
        tracing::debug!(%file, bytes, engine, elapsed_ms, ?cpu_ms, ?child_cpu_ms, "notable backend runtime");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sleeping_is_waiting_not_working() {
        let clock = EngineClock::start();
        std::thread::sleep(Duration::from_millis(80));
        let time = clock.stop();
        assert!(time.wall >= Duration::from_millis(80));
        if time.cpu.is_some() {
            assert!(time.mostly_waiting(), "{time:?}");
        }
    }

    #[test]
    fn spinning_is_working() {
        // Spin until the thread itself has run for 40ms rather than for a wall-clock interval: on a
        // loaded CI runner the scheduler can preempt a wall-clock spin for most of its window. ~keep
        let target = Duration::from_millis(40);
        let clock = EngineClock::start();
        let Some(start) = clock.cpu else {
            return;
        };
        let deadline = Instant::now() + Duration::from_secs(10);
        while thread_cpu_time().is_some_and(|now| now.saturating_sub(start) < target) && Instant::now() < deadline {
            std::hint::spin_loop();
        }
        let time = clock.stop();
        assert!(time.cpu.is_some_and(|cpu| cpu >= target), "{time:?}");
    }

    #[test]
    fn unmeasurable_cpu_is_never_excused() {
        let time = EngineTime {
            wall: Duration::from_secs(5),
            cpu: None,
            child_cpu: None,
            child_spawns: 0,
        };
        assert!(!time.mostly_waiting());
    }

    fn timed(cpu_ms: u64, child_ms: Option<u64>, spawns: u64) -> EngineTime {
        EngineTime {
            wall: Duration::from_secs(10),
            cpu: Some(Duration::from_millis(cpu_ms)),
            child_cpu: child_ms.map(Duration::from_millis),
            child_spawns: spawns,
        }
    }

    #[test]
    fn a_busy_child_is_not_waiting() {
        let time = timed(1, Some(9000), 1);
        assert!(!time.mostly_waiting());
        assert_eq!(time.wait_kind(), None);
    }

    #[test]
    fn an_idle_child_is_waiting_on_the_child() {
        let time = timed(1, Some(10), 1);
        assert_eq!(time.wait_kind(), Some(WaitKind::Child));
    }

    #[test]
    fn no_child_and_no_cpu_is_blocked() {
        let time = timed(1, Some(0), 0);
        assert_eq!(time.wait_kind(), Some(WaitKind::Blocked));
    }

    #[test]
    fn own_and_child_cpu_add_up_at_the_halfway_mark() {
        // 3s + 2s of 10s is exactly half, which is not "under half".
        assert!(!timed(3000, Some(2000), 1).mostly_waiting());
        assert!(timed(3000, Some(1999), 1).mostly_waiting());
    }
}
