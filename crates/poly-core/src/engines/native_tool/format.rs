//! Per-file format dispatch: pipe source content through a native formatter
//! CLI and collect its stdout output.

use std::process::Command;

use anyhow::Context;

use crate::engine::{FormatOutput, SourceFile};

use super::spec::ToolSpec;

/// Pipe `src.content` through the format binary described by `spec`
/// (stdin → stdout).
///
/// `indent_width` is injected as `-i {indent_width}` when
/// `spec.format_indent_flag` is true (used by shfmt). `use_tabs` overrides it
/// with `-i 0`, which is how shfmt spells "indent with tabs": poly's shared
/// `indent_width` cannot carry that value, because `Config::engine_config`
/// filters `0` out as "unset" for every engine, so the tab convention needs a
/// key of its own rather than a magic width. ~keep
///
/// For rustfmt (`spec.rustfmt_config_flag`), the child runs in the source
/// file's directory so rustfmt discovers the governing `rustfmt.toml` itself;
/// with no project config, rustfmt applies its own defaults — poly's output
/// matches `cargo fmt` either way.
///
/// Returns:
/// - [`FormatOutput::Unchanged`] when the output equals the input byte-for-byte.
/// - [`FormatOutput::Formatted(s)`] when the tool produced different output.
/// - `Err` when the tool exits non-zero or is killed (a bad tool config and a
///   syntax error in the source look alike), naming the tool's first stderr
///   line, and on timeout. Reporting either as unchanged would pass a file no
///   formatter looked at.
///
/// # Deadlock prevention
///
/// stdin is always written from a dedicated OS thread while stdout is drained
/// by another (see [`crate::tool_run::run`]). Feeding stdin and draining stdout concurrently
/// prevents the classic pipe-buffer deadlock: a tool that emits to stdout while
/// still reading stdin can fill its stdout pipe and block on the write, which —
/// if this thread were itself blocked in a `write_all` on stdin instead of
/// draining — would wedge both processes forever. Writing stdin inline on this
/// thread (a former fast path for small inputs) is unsound for exactly that
/// reason and must not be reintroduced.
pub(crate) fn format_via_tool(
    spec: &ToolSpec,
    src: &SourceFile,
    indent_width: usize,
    use_tabs: bool,
) -> anyhow::Result<FormatOutput> {
    let format_binary = spec
        .format_binary
        .expect("format_via_tool called on a lint-only ToolSpec");

    let mut cmd = Command::new(format_binary);

    if spec.format_indent_flag {
        cmd.arg("-i");
        cmd.arg(if use_tabs {
            "0".to_string()
        } else {
            indent_width.to_string()
        });
    }
    if spec.edition_flag {
        cmd.arg("--edition");
        cmd.arg(super::edition::resolve_edition(&src.path));
    }
    if (spec.rustfmt_config_flag || spec.run_in_file_dir)
        && let Some(parent) = src.path.parent().filter(|p| p.is_dir())
    {
        cmd.current_dir(parent);
    }
    cmd.args(spec.format_args);

    let output = crate::tool_run::run(
        &mut cmd,
        format_binary,
        Some(std::sync::Arc::from(src.content.as_bytes())),
        crate::tool_run::Budget::PerFile,
    )?;

    if !output.status.success() {
        return Err(output.failure(format_binary));
    }
    if let Some(error) = output.stdin_error {
        return Err(error).with_context(|| format!("failed to write to '{format_binary}' stdin"));
    }
    if output.drain_incomplete {
        anyhow::bail!("'{format_binary}' left its output pipe open after exiting; its output may be cut short");
    }

    let formatted = normalize_newlines(
        String::from_utf8(output.stdout).with_context(|| format!("'{format_binary}' produced non-UTF-8 output"))?,
    );

    if formatted == src.content.as_ref() {
        Ok(FormatOutput::Unchanged)
    } else {
        Ok(FormatOutput::Formatted(formatted))
    }
}

/// Normalize a native tool's stdout to LF line endings.
///
/// Some first-party CLIs emit CRLF on Windows (e.g. `Rscript`/styler), which
/// would make output platform-dependent and diverge from poly's LF default.
/// Collapsing `\r\n` to `\n` keeps formatted output identical across hosts; it is
/// a no-op on Unix, where the tool already emits LF, so no allocation happens.
pub(crate) fn normalize_newlines(formatted: String) -> String {
    if formatted.contains('\r') {
        formatted.replace("\r\n", "\n")
    } else {
        formatted
    }
}

#[cfg(test)]
mod tests {
    use super::normalize_newlines;

    #[test]
    fn crlf_output_is_normalized_to_lf() {
        assert_eq!(
            normalize_newlines("x <- 1\r\ny <- 2\r\n".to_string()),
            "x <- 1\ny <- 2\n"
        );
    }

    #[test]
    fn lf_output_is_left_untouched() {
        assert_eq!(normalize_newlines("x <- 1\ny <- 2\n".to_string()), "x <- 1\ny <- 2\n");
    }

    // Only the unix fake-tool tests build a spec.
    #[cfg(unix)]
    fn spec_for(binary: &'static str) -> super::super::spec::ToolSpec {
        super::super::spec::ToolSpec {
            engine_name: "test-fake",
            format_binary: Some(binary),
            format_args: &[],
            format_indent_flag: false,
            lint_binary: None,
            lint_args: &[],
            version_binary: binary,
            version_args: &[],
            default_on: false,
            edition_flag: false,
            rustfmt_config_flag: false,
            run_in_file_dir: false,
            config_files: &[],
            batch_format_args: None,
        }
    }

    #[cfg(unix)]
    fn rust_source() -> crate::engine::SourceFile {
        crate::engine::SourceFile {
            path: "x.rs".into(),
            language: crate::language::Language::Rust,
            content: "fn main(){}\n".into(),
        }
    }

    /// A tool that exits non-zero must surface as an error carrying its stderr,
    /// not as an unchanged file.
    #[cfg(unix)]
    #[test]
    fn nonzero_exit_is_an_error_with_the_first_stderr_line() {
        let dir = tempfile::tempdir().expect("dir");
        let tool = crate::tool_run::fake_tool::script(
            dir.path(),
            "badfmt",
            "cat >/dev/null; echo 'unknown key bogus_key' >&2; echo more >&2; exit 2",
        );
        let binary: &'static str = Box::leak(tool.to_string_lossy().into_owned().into_boxed_str());
        let err = super::format_via_tool(&spec_for(binary), &rust_source(), 4, false).expect_err("exit 2 is an error");
        let text = format!("{err:#}");
        assert!(
            text.contains("status 2") && text.contains("unknown key bogus_key"),
            "{text}"
        );
        assert!(!text.contains("more"), "only the first stderr line is quoted: {text}");
    }

    #[cfg(unix)]
    #[test]
    fn a_signal_killed_tool_is_an_error() {
        let dir = tempfile::tempdir().expect("dir");
        let tool = crate::tool_run::fake_tool::script(dir.path(), "diefmt", "kill -9 $$");
        let binary: &'static str = Box::leak(tool.to_string_lossy().into_owned().into_boxed_str());
        let err = super::format_via_tool(&spec_for(binary), &rust_source(), 4, false).expect_err("signal is an error");
        assert!(format!("{err:#}").contains("killed by a signal"), "{err:#}");
    }

    /// Regression: stdin must be fed while stdout is drained, never before it.
    ///
    /// `tr a b` reads stdin and writes an equal-length transformed stream to
    /// stdout. A payload many times the OS pipe capacity (64 KiB on Linux) fills
    /// both pipes at once: unless this side drains stdout while a writer thread
    /// feeds stdin, the child blocks writing stdout, this side blocks writing
    /// stdin, and the two deadlock forever. The former "write small inputs inline
    /// before calling `wait_with_output`" fast path violated that invariant and
    /// could hang `poly hooks` intermittently; this locks in the fix.
    #[cfg(unix)]
    #[test]
    fn format_via_tool_drains_stdout_while_writing_stdin() {
        use std::sync::Arc;
        use std::sync::mpsc;
        use std::time::Duration;

        use super::super::spec::ToolSpec;
        use super::format_via_tool;
        use crate::engine::{FormatOutput, SourceFile};
        use crate::language::Language;

        static TR_SPEC: ToolSpec = ToolSpec {
            engine_name: "test-tr",
            format_binary: Some("tr"),
            format_args: &["a", "b"],
            format_indent_flag: false,
            lint_binary: None,
            lint_args: &[],
            version_binary: "tr",
            version_args: &[],
            default_on: false,
            edition_flag: false,
            rustfmt_config_flag: false,
            run_in_file_dir: false,
            config_files: &[],
            batch_format_args: None,
        };

        let size = 1 << 20; // 1 MiB — 16x the Linux pipe buffer, so both pipes fill.
        let content: Arc<str> = "a".repeat(size).into();
        let src = SourceFile {
            path: "big.txt".into(),
            language: Language::Rust,
            content,
        };

        // Run under a watchdog so a reintroduced deadlock fails the test instead
        // of hanging the whole suite.
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(format_via_tool(&TR_SPEC, &src, 4, false));
        });
        let result = rx
            .recv_timeout(Duration::from_secs(30))
            .expect("format_via_tool deadlocked: stdin was not drained concurrently with stdout")
            .expect("`tr a b` must succeed");

        match result {
            FormatOutput::Formatted(text) => {
                assert_eq!(text.len(), size, "the full transformed stream must be captured");
                assert!(
                    text.bytes().all(|byte| byte == b'b'),
                    "`tr a b` must transform every byte"
                );
            }
            FormatOutput::Unchanged => panic!("`tr a b` changes the content; expected Formatted"),
        }
    }
}
