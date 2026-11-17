//! Refusing a run whose path arguments do not exist, for every surface that starts one.

use std::fmt::Write as _;
use std::path::PathBuf;

use poly_core::report;

/// What a run over `paths` must refuse with, or `None` when every path exists.
///
/// A missing path used to be discarded silently by the walker, so
/// `poly fmt typo.py` reported `All formatted. (0 file(s) scanned)` and succeeded — a green
/// result that verified nothing. A mix of real and missing paths checked only the real ones, so
/// the file count looked plausible too. The CLI turns the message into exit code 2 and the MCP
/// server into an `isError` result: the two surfaces share this one check.
#[must_use]
pub fn missing_paths_message(paths: &[PathBuf]) -> Option<String> {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn existing_paths_pass_and_each_missing_one_is_named() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(missing_paths_message(&[dir.path().to_path_buf()]), None);
        let message = missing_paths_message(&[dir.path().to_path_buf(), dir.path().join("nope.py")]).unwrap();
        assert!(
            message.contains("path does not exist") && message.contains("nope.py"),
            "{message}"
        );
        assert!(message.contains("1 path that could not be read"), "{message}");
    }
}
