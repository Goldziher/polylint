//! Where a run's time goes: per-phase wall time and per-engine call statistics
//! behind `--timings`.
//!
//! Collection is off until [`enable`] is called, and costs one relaxed atomic
//! load per engine call while off. When on, engine calls are accumulated in
//! shards selected by rayon worker index, so a worker only ever takes its own
//! uncontended lock; [`take`] merges the shards once, after the run.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock, PoisonError};
use std::time::{Duration, Instant};

use rustc_hash::FxHashMap;

const SHARDS: usize = 64;

static ENABLED: AtomicBool = AtomicBool::new(false);
static COLLECTOR: OnceLock<Collector> = OnceLock::new();

struct Collector {
    shards: Vec<Mutex<Shard>>,
    phases: Mutex<Vec<(&'static str, Duration)>>,
}

fn collector() -> &'static Collector {
    COLLECTOR.get_or_init(|| Collector {
        shards: (0..SHARDS).map(|_| Mutex::new(Shard::default())).collect(),
        phases: Mutex::new(Vec::new()),
    })
}

/// Start collecting. Idempotent; earlier recordings are kept.
pub fn enable() {
    ENABLED.store(true, Ordering::Relaxed);
}

/// Whether [`enable`] has been called and [`take`] has not yet drained it.
pub fn enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

/// Accumulated calls of one engine within one shard.
#[derive(Default)]
struct EngineAcc {
    calls: u64,
    wall: Duration,
    cpu: Duration,
    child_cpu: Duration,
    wall_micros: Vec<u64>,
    max: Duration,
    max_file: PathBuf,
}

/// One worker's accumulation, keyed by engine id.
#[derive(Default)]
struct Shard {
    engines: FxHashMap<&'static str, EngineAcc>,
}

impl Shard {
    fn record(
        &mut self,
        engine: &'static str,
        path: &Path,
        wall: Duration,
        cpu: Option<Duration>,
        child_cpu: Option<Duration>,
    ) {
        let acc = self.engines.entry(engine).or_default();
        acc.calls += 1;
        acc.wall += wall;
        acc.cpu += cpu.unwrap_or_default();
        acc.child_cpu += child_cpu.unwrap_or_default();
        acc.wall_micros.push(wall.as_micros() as u64);
        if wall > acc.max || acc.calls == 1 {
            acc.max = wall;
            acc.max_file = path.to_path_buf();
        }
    }
}

/// Record one engine call. A no-op unless [`enable`] was called.
pub(crate) fn record_engine(
    engine: &'static str,
    path: &Path,
    wall: Duration,
    cpu: Option<Duration>,
    child_cpu: Option<Duration>,
) {
    if !enabled() {
        return;
    }
    let shard = rayon::current_thread_index().unwrap_or(0) % SHARDS;
    collector().shards[shard]
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .record(engine, path, wall, cpu, child_cpu);
}

/// Time a named phase of the run until the guard drops, inside an info span of
/// the same name. The wall time is kept only while collection is [enabled](enable).
pub fn phase(name: &'static str) -> PhaseGuard {
    PhaseGuard {
        name,
        started: Instant::now(),
        _span: tracing::info_span!("phase", phase = name).entered(),
    }
}

/// Ends a [`phase`] when dropped.
#[must_use = "a phase is timed until its guard drops"]
pub struct PhaseGuard {
    name: &'static str,
    started: Instant,
    _span: tracing::span::EnteredSpan,
}

impl Drop for PhaseGuard {
    fn drop(&mut self) {
        if enabled() {
            collector()
                .phases
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push((self.name, self.started.elapsed()));
        }
    }
}

/// Statistics of every call one engine made in a run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EngineTimings {
    /// The engine's tool id.
    pub engine: &'static str,
    /// Number of engine calls that ran (cache hits do not call the engine).
    pub calls: u64,
    /// Summed wall time of the calls.
    pub wall: Duration,
    /// Summed CPU time of poly's own worker threads in those calls.
    pub cpu: Duration,
    /// Summed CPU time of the child processes those calls ran.
    pub child_cpu: Duration,
    /// 95th percentile of one call's wall time.
    pub p95: Duration,
    /// The slowest call's wall time.
    pub max: Duration,
    /// The file the slowest call ran on.
    pub max_file: PathBuf,
}

/// Everything `--timings` reports.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TimingsReport {
    /// Phases in first-seen order; a phase entered more than once is summed.
    pub phases: Vec<(&'static str, Duration)>,
    /// Engines, slowest total wall first.
    pub engines: Vec<EngineTimings>,
}

/// Stop collecting and return, then clear, what was recorded.
pub fn take() -> TimingsReport {
    ENABLED.store(false, Ordering::Relaxed);
    let Some(collector) = COLLECTOR.get() else {
        return TimingsReport::default();
    };
    let phases = std::mem::take(&mut *collector.phases.lock().unwrap_or_else(PoisonError::into_inner));
    let shards = collector
        .shards
        .iter()
        .map(|shard| std::mem::take(&mut *shard.lock().unwrap_or_else(PoisonError::into_inner)));
    TimingsReport {
        phases: sum_phases(phases),
        engines: merge_shards(shards),
    }
}

fn sum_phases(phases: Vec<(&'static str, Duration)>) -> Vec<(&'static str, Duration)> {
    let mut summed: Vec<(&'static str, Duration)> = Vec::new();
    for (name, wall) in phases {
        match summed.iter_mut().find(|(seen, _)| *seen == name) {
            Some((_, total)) => *total += wall,
            None => summed.push((name, wall)),
        }
    }
    summed
}

fn merge_shards(shards: impl Iterator<Item = Shard>) -> Vec<EngineTimings> {
    let mut merged: FxHashMap<&'static str, EngineAcc> = FxHashMap::default();
    for shard in shards {
        for (engine, acc) in shard.engines {
            let into = merged.entry(engine).or_default();
            into.calls += acc.calls;
            into.wall += acc.wall;
            into.cpu += acc.cpu;
            into.child_cpu += acc.child_cpu;
            into.wall_micros.extend(acc.wall_micros);
            if acc.max > into.max || into.max_file.as_os_str().is_empty() {
                into.max = acc.max;
                into.max_file = acc.max_file;
            }
        }
    }
    let mut engines: Vec<EngineTimings> = merged
        .into_iter()
        .map(|(engine, mut acc)| EngineTimings {
            engine,
            calls: acc.calls,
            wall: acc.wall,
            cpu: acc.cpu,
            child_cpu: acc.child_cpu,
            p95: Duration::from_micros(percentile_95(&mut acc.wall_micros)),
            max: acc.max,
            max_file: acc.max_file,
        })
        .collect();
    engines.sort_by(|a, b| b.wall.cmp(&a.wall).then(a.engine.cmp(b.engine)));
    engines
}

/// The nearest-rank 95th percentile of `samples` (reordered); `0` when empty.
fn percentile_95(samples: &mut [u64]) -> u64 {
    if samples.is_empty() {
        return 0;
    }
    samples.sort_unstable();
    let rank = (samples.len() * 95).div_ceil(100);
    samples[rank.max(1) - 1]
}

impl TimingsReport {
    /// The human-readable block `--timings` prints to stderr.
    #[must_use]
    pub fn render(&self) -> String {
        let mut out = String::from("timings: phases (wall)\n");
        for (name, wall) in &self.phases {
            let _ = writeln!(out, "  {name:<24} {:>10}", fmt_duration(*wall));
        }
        if self.engines.is_empty() {
            out.push_str("timings: engines: no engine ran (every file was answered from the cache)\n");
            return out;
        }
        out.push_str("timings: engines\n");
        let _ = writeln!(
            out,
            "  {:<14} {:>7} {:>10} {:>10} {:>10} {:>10} {:>10}  slowest file",
            "engine", "calls", "wall", "cpu", "child cpu", "p95", "max"
        );
        for e in &self.engines {
            let _ = writeln!(
                out,
                "  {:<14} {:>7} {:>10} {:>10} {:>10} {:>10} {:>10}  {}",
                e.engine,
                e.calls,
                fmt_duration(e.wall),
                fmt_duration(e.cpu),
                fmt_duration(e.child_cpu),
                fmt_duration(e.p95),
                fmt_duration(e.max),
                e.max_file.display()
            );
        }
        out
    }
}

fn fmt_duration(d: Duration) -> String {
    format!("{:.1}ms", d.as_secs_f64() * 1000.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    #[test]
    fn percentile_is_nearest_rank() {
        let mut samples: Vec<u64> = (1..=100).rev().collect();
        assert_eq!(percentile_95(&mut samples), 95);
        assert_eq!(percentile_95(&mut [7]), 7);
        assert_eq!(percentile_95(&mut []), 0);
    }

    #[test]
    fn shards_merge_into_one_row_per_engine() {
        let mut a = Shard::default();
        let mut b = Shard::default();
        a.record("rustfmt", Path::new("a.rs"), ms(10), Some(ms(1)), Some(ms(8)));
        a.record("rustfmt", Path::new("slow.rs"), ms(90), Some(ms(2)), Some(ms(80)));
        b.record("rustfmt", Path::new("b.rs"), ms(20), Some(ms(1)), None);
        b.record("ruff", Path::new("c.py"), ms(5), Some(ms(5)), None);
        let merged = merge_shards([a, b].into_iter());
        assert_eq!(merged.len(), 2);
        let rustfmt = &merged[0];
        assert_eq!(rustfmt.engine, "rustfmt");
        assert_eq!(rustfmt.calls, 3);
        assert_eq!(rustfmt.wall, ms(120));
        assert_eq!(rustfmt.cpu, ms(4));
        assert_eq!(rustfmt.child_cpu, ms(88));
        assert_eq!(rustfmt.max, ms(90));
        assert_eq!(rustfmt.max_file, PathBuf::from("slow.rs"));
        assert_eq!(rustfmt.p95, ms(90));
        assert_eq!(merged[1].engine, "ruff");
    }

    #[test]
    fn a_phase_entered_twice_is_summed_in_first_seen_order() {
        let summed = sum_phases(vec![("discovery", ms(5)), ("planning", ms(1)), ("discovery", ms(7))]);
        assert_eq!(summed, vec![("discovery", ms(12)), ("planning", ms(1))]);
    }

    #[test]
    fn render_names_phases_and_engines() {
        let report = TimingsReport {
            phases: vec![("discovery", ms(3))],
            engines: merge_shards(
                {
                    let mut shard = Shard::default();
                    shard.record("gofmt", Path::new("m.go"), ms(4), None, None);
                    [shard]
                }
                .into_iter(),
            ),
        };
        let text = report.render();
        assert!(text.contains("discovery"), "{text}");
        assert!(text.contains("gofmt") && text.contains("m.go"), "{text}");
    }
}
