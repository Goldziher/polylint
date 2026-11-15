//! Running one tool invocation and capturing what it printed.

use std::collections::BTreeMap;
use std::path::Path;
use std::process::Command;
use std::sync::Arc;

use anyhow::{Context, Result};
use poly_catalog::OUTPUT_PLACEHOLDER;

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
///
/// An argument containing `$OUTPUT` names a file the tool writes its report to:
/// the placeholder becomes a path in a registered scratch directory, and the
/// file's content, read once the tool has exited, replaces `stdout`. A tool
/// that wrote nothing there leaves `stdout` empty, which the report parser
/// refuses rather than reading as a clean run.
pub(super) fn run(
    launcher: &Launcher,
    argv: &[String],
    cwd: &Path,
    env: &BTreeMap<String, String>,
    unset: &[&str],
) -> Result<Captured> {
    if !argv.iter().any(|arg| arg.contains(OUTPUT_PLACEHOLDER)) {
        return run_plain(launcher, argv, cwd, env, unset);
    }
    let scratch = crate::tempreg::RegisteredTempDir::new("poly-report-").context("creating a report directory")?;
    let report = scratch.path().join("report");
    let report_arg = report.to_string_lossy();
    let argv = substitute_output(argv, &report_arg);
    let mut captured = run_plain(launcher, &argv, cwd, env, unset)?;
    captured.stdout = std::fs::read(&report)
        .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
        .unwrap_or_default();
    Ok(captured)
}

/// `argv` with `$OUTPUT` replaced by `path` wherever it appears in an argument.
fn substitute_output(argv: &[String], path: &str) -> Vec<String> {
    argv.iter().map(|arg| arg.replace(OUTPUT_PLACEHOLDER, path)).collect()
}

fn run_plain(
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_output_placeholder_is_replaced_inside_an_argument() {
        let argv = vec![
            "--report".to_owned(),
            "sarif:$OUTPUT".to_owned(),
            "--input".to_owned(),
            "a.kt".to_owned(),
        ];
        assert_eq!(
            substitute_output(&argv, "/tmp/r/report"),
            ["--report", "sarif:/tmp/r/report", "--input", "a.kt"]
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_report_the_tool_wrote_replaces_stdout_and_replaces_stdout() {
        let launcher = Launcher {
            program: "/bin/sh".into(),
            prefix: vec!["-c".to_owned()],
            identity: String::new(),
        };
        let argv = vec![
            "printf REPORT > \"$1\"; echo noise; echo err >&2".to_owned(),
            "sh".to_owned(),
            "$OUTPUT".to_owned(),
        ];
        let captured = run(&launcher, &argv, Path::new("/"), &BTreeMap::new(), &[]).expect("runs");
        assert_eq!(captured.stdout, "REPORT");
        assert_eq!(captured.stderr.trim(), "err");
    }

    #[cfg(unix)]
    #[test]
    fn a_tool_that_writes_no_report_leaves_stdout_empty() {
        let launcher = Launcher {
            program: "/bin/sh".into(),
            prefix: vec!["-c".to_owned()],
            identity: String::new(),
        };
        let argv = vec!["echo ignored".to_owned(), "sh".to_owned(), "$OUTPUT".to_owned()];
        let captured = run(&launcher, &argv, Path::new("/"), &BTreeMap::new(), &[]).expect("runs");
        assert_eq!(captured.stdout, "");
    }
}
