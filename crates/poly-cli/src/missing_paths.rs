//! Refusing a run whose path arguments do not exist.

use std::fmt::Write as _;
use std::path::PathBuf;
use std::process::ExitCode;

use poly_core::report;

/// Fail the run when a path argument does not exist.
///
/// A missing path used to be discarded silently by the walker, so
/// `poly fmt --check typo.py` reported `All formatted. (0 file(s) scanned)` and
/// exited 0 — a green result that verified nothing. Worse, a mix of real and
/// missing paths checked only the real ones and still exited 0, so the file
/// count looked plausible. A hook or CI step feeding poly a stale path list was
/// indistinguishable from a passing gate.
pub(crate) fn reject_missing_paths(paths: &[PathBuf]) -> Result<(), ExitCode> {
    match missing_paths_message(paths) {
        None => Ok(()),
        Some(message) => {
            eprint!("{message}");
            Err(ExitCode::from(2))
        }
    }
}

/// What [`reject_missing_paths`] prints, or `None` when every path exists.
pub(crate) fn missing_paths_message(paths: &[PathBuf]) -> Option<String> {
    let missing: Vec<&PathBuf> = paths.iter().filter(|p| !p.exists()).collect();
    if missing.is_empty() {
        return None;
    }
    let mut message = String::new();
    for path in &missing {
        let _ = writeln!(message, "error: path does not exist: {}", path.display());
    }
    let _ = writeln!(
        message,
        "error: refusing to report success for {} that could not be read",
        report::paths(missing.len())
    );
    Some(message)
}
