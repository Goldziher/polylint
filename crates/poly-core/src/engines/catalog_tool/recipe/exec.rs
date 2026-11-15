//! Running one tool invocation and capturing what it printed.

use std::collections::BTreeMap;
use std::path::Path;
use std::process::Command;
use std::sync::Arc;

use anyhow::Result;

use super::launch::Launcher;

/// What an invocation printed and how it ended.
#[derive(Debug)]
pub(super) struct Captured {
    /// The exit code; `None` when the process was killed by a signal.
    pub(super) code: Option<i32>,
    pub(super) stdout: String,
    pub(super) stderr: String,
}

impl Captured {
    /// Whether the process exited with 0 or one of `allowed`.
    pub(super) fn exited_ok(&self, allowed: &[i32]) -> bool {
        self.code.is_some_and(|code| code == 0 || allowed.contains(&code))
    }

    /// The tail of stderr (else stdout), for naming why an invocation failed.
    pub(super) fn explanation(&self) -> String {
        let source = if self.stderr.trim().is_empty() {
            &self.stdout
        } else {
            &self.stderr
        };
        let trimmed = source.trim();
        let cut = trimmed.len().saturating_sub(MAX_EXPLANATION);
        let mut start = cut;
        while !trimmed.is_char_boundary(start) {
            start += 1;
        }
        let tail = &trimmed[start..];
        match self.code {
            Some(code) => format!("exit {code}: {tail}"),
            None => format!("killed by a signal: {tail}"),
        }
    }
}

const MAX_EXPLANATION: usize = 2000;

/// Run `launcher` with `argv` in `cwd`, feeding `input` on stdin.
pub(super) fn run_with_stdin(
    launcher: &Launcher,
    argv: &[String],
    cwd: &Path,
    env: &BTreeMap<String, String>,
    input: &str,
) -> Result<Captured> {
    let mut command = Command::new(&launcher.program);
    command
        .args(&launcher.prefix)
        .args(argv)
        .current_dir(cwd)
        .envs(env)
        .env("NO_COLOR", "1");
    captured(command, launcher, Some(Arc::from(input.as_bytes())))
}

fn captured(mut command: Command, launcher: &Launcher, stdin: Option<Arc<[u8]>>) -> Result<Captured> {
    let name = launcher.program.display().to_string();
    let output = crate::tool_run::run(&mut command, &name, stdin, crate::tool_run::Budget::Unit)?;
    Ok(Captured {
        code: output.status.code(),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    })
}

/// Run `launcher` with `argv` in `cwd`, never touching stdin.
///
/// Tools are asked not to colour their reports (`NO_COLOR`): the parsers read
/// plain text. The variables in `unset` are removed from the inherited
/// environment.
pub(super) fn run(
    launcher: &Launcher,
    argv: &[String],
    cwd: &Path,
    env: &BTreeMap<String, String>,
    unset: &[&str],
) -> Result<Captured> {
    let mut command = Command::new(&launcher.program);
    for name in unset {
        command.env_remove(name);
    }
    command
        .args(&launcher.prefix)
        .args(argv)
        .current_dir(cwd)
        .envs(env)
        .env("NO_COLOR", "1");
    captured(command, launcher, None)
}
