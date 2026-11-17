//! The switch for the timings that cost work to collect. Process-wide and off by default; the library never reads the
//! environment, so a host that reports timings turns it on with [`enable_timings`](crate::enable_timings).

use std::sync::atomic::{AtomicBool, Ordering};

static ENABLED: AtomicBool = AtomicBool::new(false);

/// Turns the costly timings on for the rest of the process.
pub(crate) fn enable() {
    ENABLED.store(true, Ordering::Relaxed);
}

/// Whether the host turned the costly timings on.
pub(crate) fn enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}
