//! A transient progress indicator for the long-running phases of a run.
//!
//! `poly lint` over Kubernetes takes twelve seconds and, until it finished,
//! printed nothing at all — which reads as a hang. This is the smallest thing
//! that fixes that: one line on **stderr** saying what poly is doing and for how
//! long, erased completely before the report is printed.
//!
//! The rules it obeys, in order of how badly breaking them would hurt:
//!
//! - **stderr, never stdout.** `--format json` / `--format toon` write a single
//!   document to stdout; a control character there would corrupt it. Nothing
//!   here ever touches stdout.
//! - **Only when stderr is a terminal.** A pipe, a file, or a CI log gets
//!   nothing — a build log full of spinner frames is a regression, not a
//!   feature.
//! - **No cursor hiding.** Hiding the cursor means restoring it, and a process
//!   killed with Ctrl-C never gets to. Leaving the cursor alone makes that class
//!   of bug unreachable rather than merely unlikely.
//! - **No hot-path cost.** The worker threads never see this: a separate thread
//!   ticks on a timer and the pipeline is not told it exists.
//! - **Erases itself.** The line is cleared on stop and on drop, so a panic or
//!   an early return cannot leave half a frame on screen.
//!
//! Log lines and the spinner share one line of stderr, so `tracing` output goes
//! through [`LogWriter`], which erases the current frame before writing; the
//! next tick redraws it below the log line.

use std::io::IsTerminal;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use poly_hooks::live;
use tracing_subscriber::fmt::MakeWriter;

/// Frames of the spinner, one per tick.
const FRAMES: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

/// How often a frame is drawn. Fast enough to read as motion, slow enough that
/// the render thread is invisible in a profile.
const TICK: Duration = Duration::from_millis(80);

/// How long the run must last before the indicator appears at all.
///
/// A run that finishes in 200ms should print its report and nothing else;
/// flashing a spinner for two frames is noise. Only the runs that would
/// otherwise look like a hang get one.
const APPEAR_AFTER: Duration = Duration::from_millis(400);

/// A running progress indicator. Stops and erases itself when dropped.
pub struct Progress {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Progress {
    /// Start an indicator labelled `label` (e.g. `"linting"`), or a no-op handle
    /// when the environment is not one that should see it.
    ///
    /// `enabled` is the caller's own veto — poly passes `false` when colour is
    /// off, since a user who asked for plain output did not ask for animation.
    pub fn start(label: &'static str, enabled: bool) -> Self {
        if !enabled || !std::io::stderr().is_terminal() {
            return Self {
                stop: Arc::new(AtomicBool::new(true)),
                thread: None,
            };
        }
        let stop = Arc::new(AtomicBool::new(false));
        let signal = Arc::clone(&stop);
        let thread = std::thread::spawn(move || {
            let started = Instant::now();
            let mut frame = 0usize;
            let mut drawn = false;
            while !signal.load(Ordering::Relaxed) {
                std::thread::sleep(TICK);
                if signal.load(Ordering::Relaxed) {
                    break;
                }
                let elapsed = started.elapsed();
                if elapsed < APPEAR_AFTER {
                    continue;
                }
                live::draw_spinner_frame(Some(&format!(
                    "{} {label}… {:.1}s",
                    FRAMES[frame % FRAMES.len()],
                    elapsed.as_secs_f64()
                )));
                drawn = true;
                frame += 1;
            }
            if drawn {
                live::draw_spinner_frame(None);
            }
        });
        Self {
            stop,
            thread: Some(thread),
        }
    }

    /// Stop the indicator and erase its line, blocking until the render thread
    /// has done so — so nothing can land between the last frame and the report.
    pub fn finish(mut self) {
        self.stop_now();
    }

    fn stop_now(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Drop for Progress {
    fn drop(&mut self) {
        self.stop_now();
    }
}

/// The `tracing` writer: stderr, routed so a log line never lands on a spinner
/// frame. Without a frame on screen it writes exactly what it is given.
#[derive(Clone, Copy, Default)]
pub struct LogWriter;

impl std::io::Write for LogWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        live::write_above_spinner(buf)?;
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        std::io::stderr().flush()
    }
}

impl<'a> MakeWriter<'a> for LogWriter {
    type Writer = Self;

    fn make_writer(&'a self) -> Self::Writer {
        *self
    }
}
