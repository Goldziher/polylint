//! Refusing a run whose path arguments do not exist.

use std::path::PathBuf;
use std::process::ExitCode;

pub(crate) use poly_host_services::paths::missing_paths_message;

/// Fail the run (exit 2) when a path argument does not exist, printing the shared message the
/// MCP server returns as an `isError` result.
pub(crate) fn reject_missing_paths(paths: &[PathBuf]) -> Result<(), ExitCode> {
    match missing_paths_message(paths) {
        None => Ok(()),
        Some(message) => {
            eprint!("{message}");
            Err(ExitCode::from(2))
        }
    }
}
