//! Tool presence probing: spawn the binary once, then query its
//! version string. Cached for the process lifetime via the per-tool
//! `OnceLock`s in [`super::spec`].

use std::process::Command;

use super::spec::ToolSpec;

/// Determine if the tool described by `spec` is present on `PATH` and return
/// its version string.
///
/// Returns `Some(version_string)` on success, `None` when the binary cannot
/// be spawned (not on `PATH` or not executable).
///
/// The probe is inexpensive: it spawns the binary once with null I/O (just
/// to confirm it exists), then runs the version command. Callers cache the
/// result in the per-tool `OnceLock` so this function runs at most once per
/// process.
pub(crate) fn probe_tool(spec: &ToolSpec) -> Option<String> {
    match crate::tool_run::run(
        &mut Command::new(spec.probe_binary()),
        spec.probe_binary(),
        None,
        crate::tool_run::PROBE_TIMEOUT,
    ) {
        Err(error) if error.downcast_ref::<crate::tool_run::TimedOut>().is_none() => return None,
        _ => {}
    }

    let raw = crate::tool_run::run(
        Command::new(spec.version_binary).args(spec.version_args),
        spec.version_binary,
        None,
        crate::tool_run::PROBE_TIMEOUT,
    )
    .ok()
    .map(|o| {
        let stdout = String::from_utf8_lossy(&o.stdout).trim().to_string();
        if stdout.is_empty() {
            String::from_utf8_lossy(&o.stderr).trim().to_string()
        } else {
            stdout
        }
    })
    .filter(|s| !s.is_empty())
    .unwrap_or_else(|| format!("{}:found", spec.probe_binary()));

    Some(raw)
}
