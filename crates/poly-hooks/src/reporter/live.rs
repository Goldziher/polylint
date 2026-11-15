//! Shared state between a transient single-line spinner and everything else that
//! writes to stderr while it is on screen.
//!
//! The spinner redraws its line with `\r` and no newline, so any other write
//! lands on the end of the last frame (`⠸ formatting… 24.2s2026-… INFO …`).
//! Whoever draws a frame marks it here, and every line written through
//! [`write_above_spinner`] erases that frame first, so the line starts at column
//! zero and the spinner redraws on its next tick.

use std::io::Write as _;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

/// Erase the current line and return the cursor to its start.
pub const CLEAR_LINE: &str = "\r\x1b[2K";

static FRAME_ON_SCREEN: AtomicBool = AtomicBool::new(false);
static STDERR_GATE: Mutex<()> = Mutex::new(());

/// What must be written before a log line: the erase sequence when a spinner
/// frame is on screen, nothing otherwise.
#[must_use]
pub const fn prefix_for(frame_on_screen: bool) -> &'static str {
    if frame_on_screen { CLEAR_LINE } else { "" }
}

/// Draw `frame` as the spinner line, or erase it when `frame` is `None`.
pub fn draw_spinner_frame(frame: Option<&str>) {
    let _gate = STDERR_GATE.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut stderr = std::io::stderr().lock();
    let _ = write!(stderr, "{CLEAR_LINE}{}", frame.unwrap_or_default());
    FRAME_ON_SCREEN.store(frame.is_some(), Ordering::Relaxed);
    let _ = stderr.flush();
}

/// Write `bytes` to stderr, first erasing a spinner frame if one is showing.
pub fn write_above_spinner(bytes: &[u8]) -> std::io::Result<()> {
    let _gate = STDERR_GATE.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut stderr = std::io::stderr().lock();
    let prefix = prefix_for(FRAME_ON_SCREEN.swap(false, Ordering::Relaxed));
    stderr.write_all(prefix.as_bytes())?;
    stderr.write_all(bytes)
}

/// Print `line` plus a newline on stderr without tearing a spinner frame.
pub fn eprintln_above_spinner(line: &str) {
    let mut bytes = Vec::with_capacity(line.len() + 1);
    bytes.extend_from_slice(line.as_bytes());
    bytes.push(b'\n');
    let _ = write_above_spinner(&bytes);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_visible_frame_is_erased_before_a_log_line() {
        assert_eq!(prefix_for(true), "\r\x1b[2K");
    }

    #[test]
    fn no_frame_means_no_prefix() {
        assert_eq!(prefix_for(false), "");
    }
}
