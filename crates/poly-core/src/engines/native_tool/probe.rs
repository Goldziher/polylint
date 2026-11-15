//! Tool presence probing: spawn the binary once, then query its
//! version string. Cached for the process lifetime via the per-tool
//! `OnceLock`s in [`super::spec`].

use std::process::Command;

use super::spec::ToolSpec;

/// Determine if the tool described by `spec` is present on `PATH` and return
/// its version string.
///
/// Returns `Some(version_string)` on success, `None` when the binary cannot
/// be spawned (not on `PATH` or not executable) or does not answer in time; callers cache
/// that negative result.
///
/// The probe is inexpensive: it spawns the binary once with null I/O (just
/// to confirm it exists), then runs the version command. Callers cache the
/// result in the per-tool `OnceLock` so this function runs at most once per
/// process.
pub(crate) fn probe_tool(spec: &ToolSpec) -> Option<String> {
    // A probe that hangs (including by timing out) means the tool is not usable:
    // reporting it present would make every file wait out its full deadline.
    crate::tool_run::run(
        &mut Command::new(spec.probe_binary()),
        spec.probe_binary(),
        None,
        crate::tool_run::Budget::Probe,
    )
    .ok()?;

    let raw = crate::tool_run::run(
        Command::new(spec.version_binary).args(spec.version_args),
        spec.version_binary,
        None,
        crate::tool_run::Budget::Probe,
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
