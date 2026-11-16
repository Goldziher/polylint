//! The `lint` and `fmt` builtins in process: the per-file tier of `poly lint`
//! and `poly fmt` as the re-exec child runs it, rendered into a buffer.
//!
//! Only a call this process can stand in for exactly is taken; anything else is
//! declined and re-execs (ADR 0071):
//!
//! - The hook's directory must be this process's working directory. Engines
//!   still read the working directory themselves (a native tool's config
//!   anchor, a recipe tool's run directory), so a staged snapshot or a hook
//!   `cwd` keeps the child, whose working directory is that directory.
//! - The parent's `--offline` and `--trust` flags, and a colour forced through
//!   the environment, would reach an in-process run but not the child.
//! - Only the arguments the lowering produces, plus flags that only feed
//!   [`RunOptions`], are reproduced: the pretty report, no skip budget, no
//!   timings, and no `--set` but `discovery.force_exclude=true`, which is
//!   applied to this run's options rather than installed process-wide.

use std::fmt::Write as _;
use std::path::PathBuf;

use anyhow::Result;
use clap::Parser;
use poly_core::components::ComponentsReport;
use poly_core::{Config, RunOptions, Verbosity, report};
use poly_hooks::in_process::{Call, FMT, LINT, Outcome};

use crate::component_notes::component_lines;
use crate::config_sources::{RemoteExtendsResolver, repo_root};
use crate::missing_paths::missing_paths_message;
use crate::{CommonArgs, EXIT_NOT_VERIFIED, FmtArgs, LintArgs, OutputFormat, WorkspaceMode};

/// `poly lint`'s argument grammar.
#[derive(Parser)]
#[command(name = "poly lint")]
struct LintCommand {
    #[command(flatten)]
    args: LintArgs,
}

/// `poly fmt`'s argument grammar.
#[derive(Parser)]
#[command(name = "poly fmt")]
struct FmtCommand {
    #[command(flatten)]
    args: FmtArgs,
}

/// The one `--set` a lowered `lint` / `fmt` line carries.
const FORCE_EXCLUDE_SET: &str = "discovery.force_exclude=true";

/// Variables under which the child would colour its piped output.
const FORCED_COLOR_ENV: &[&str] = &["FORCE_COLOR", "CLICOLOR_FORCE"];

/// What the child writes: stdout, then stderr.
#[derive(Default)]
struct Streams {
    stdout: String,
    stderr: String,
}

impl Streams {
    /// The hook's output and exit code. The report was coloured against this
    /// process's stdout; the child's is a pipe, which it never colours.
    fn finish(self, code: u8) -> Outcome {
        let mut output = report::strip_ansi(&self.stdout).into_bytes();
        output.extend_from_slice(self.stderr.as_bytes());
        Outcome {
            code: i32::from(code),
            output,
        }
    }
}

/// Run a `lint` or `fmt` call as `poly <name> <args> <files>` would from
/// `call.dir`, or decline when this process cannot reproduce that child.
pub(super) fn run(call: &Call<'_>) -> Option<Result<Outcome>> {
    if !super::is_working_dir(call.dir) || !environment_matches_child() {
        return None;
    }
    match call.name {
        LINT => {
            let args = LintCommand::try_parse_from(super::argv(LINT, call)).ok()?.args;
            (args.workspace == WorkspaceMode::Never && reproducible(&args.common)).then(|| Ok(lint(&args.common)))
        }
        FMT => {
            let args = FmtCommand::try_parse_from(super::argv(FMT, call)).ok()?.args;
            (!args.check && reproducible(&args.common)).then(|| Ok(fmt(&args)))
        }
        _ => None,
    }
}

/// Whether nothing this process was started with would make it behave
/// differently from a child started by the runner.
fn environment_matches_child() -> bool {
    super::flags_match_child()
        && crate::global().config.is_none()
        && FORCED_COLOR_ENV.iter().all(|name| std::env::var_os(name).is_none())
}

/// Whether `common` asks for nothing beyond what [`lint`] / [`fmt`] reproduce.
fn reproducible(common: &CommonArgs) -> bool {
    common.format == OutputFormat::Pretty
        && !common.deny_skips
        && common.max_skips.is_none()
        && !crate::timings::requested(common)
        && common.set.iter().all(|set| set == FORCE_EXCLUDE_SET)
}

/// `poly lint --workspace never`'s pretty path (`crate::run_lint`).
fn lint(common: &CommonArgs) -> Outcome {
    let mut streams = Streams::default();
    let Some((paths, config, opts)) = prepare(common, &mut streams) else {
        return streams.finish(EXIT_NOT_VERIFIED);
    };
    let run = match poly_core::lint_run(&paths, &config, &opts, common.fix, false) {
        Ok(run) => run,
        Err(error) => {
            let _ = writeln!(streams.stderr, "error: {error:#}");
            return streams.finish(EXIT_NOT_VERIFIED);
        }
    };
    let (text, _) = report::render_lint_pretty_run(&run, Verbosity::from_flags(false, false, false));
    streams.stdout.push_str(&text);
    let blocking = components(&run.components, &mut streams);
    let code = if blocking || !run.errors.is_empty() {
        EXIT_NOT_VERIFIED
    } else {
        u8::from(crate::lint_has_errors(&run.results))
    };
    streams.finish(code)
}

/// `poly fmt`'s pretty path (`crate::run_fmt`).
fn fmt(args: &FmtArgs) -> Outcome {
    let common = &args.common;
    let mut streams = Streams::default();
    let Some((paths, config, opts)) = prepare(common, &mut streams) else {
        return streams.finish(EXIT_NOT_VERIFIED);
    };
    let write = common.fix;
    let run = match poly_core::format_run(&paths, &config, &opts, write, false) {
        Ok(run) => run,
        Err(error) => {
            let _ = writeln!(streams.stderr, "error: {error:#}");
            return streams.finish(EXIT_NOT_VERIFIED);
        }
    };
    // `run_fmt` reports the components before the report itself.
    let errors = run.errors.len() + usize::from(components(&run.components, &mut streams));
    let (text, changed) = report::render_format_pretty_run(&run, !write, Verbosity::from_flags(false, false, false));
    streams.stdout.push_str(&text);
    let code = if errors > 0 {
        EXIT_NOT_VERIFIED
    } else {
        u8::from(changed > 0 && !(write && !args.exit_nonzero_on_fix))
    };
    streams.finish(code)
}

/// The child's `prepare` (`crate::prepare_with_poly`) without its process-wide
/// side effects: the `--set` override lands on this run's config, and nothing
/// is printed but into `streams`. `None` once it has written why the run stops.
fn prepare(common: &CommonArgs, streams: &mut Streams) -> Option<(Vec<PathBuf>, Config, RunOptions)> {
    let paths = if common.paths.is_empty() {
        vec![PathBuf::from(".")]
    } else {
        common.paths.clone()
    };
    if let Some(message) = missing_paths_message(&paths) {
        streams.stderr.push_str(&message);
        return None;
    }
    let resolver = match repo_root().and_then(|root| RemoteExtendsResolver::new(&root)) {
        Ok(resolver) => std::sync::Arc::new(resolver),
        Err(error) => {
            let _ = writeln!(streams.stderr, "error: failed to prepare config resolver: {error:#}");
            return None;
        }
    };
    // The child's working directory is this one (`run` checked), and it loads from there.
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let poly_config = match poly_config::PolyConfig::load_with(&cwd, resolver.as_ref()) {
        Ok(poly_config) => poly_config,
        Err(error) => {
            let _ = writeln!(streams.stderr, "error: failed to load config: {error:#}");
            return None;
        }
    };
    let mut config = Config::from(poly_config);
    if !common.set.is_empty() {
        // Where the child's `--set discovery.force_exclude=true` lands.
        config.force_exclude = true;
    }
    let opts = RunOptions {
        no_cache: common.no_cache,
        jobs: common.jobs,
        exclude: common.exclude.clone(),
        force_exclude: crate::resolve_force_exclude(common, &config),
        fix_generated: common.fix_generated,
        generated: None,
        explicit_config: false,
        config_resolver: Some(resolver),
        externally_linted_languages: Vec::new(),
        only: common.only.clone(),
        skip: common.skip.clone(),
        on_missing: common.on_missing.map(Into::into),
        timings: false,
        ..RunOptions::default()
    };
    Some((paths, config, opts))
}

/// The child's `report_blocking_components`, into `streams`: a line per
/// component the run could not use, and whether one fails the run.
fn components(components: &ComponentsReport, streams: &mut Streams) -> bool {
    for line in component_lines(components) {
        let _ = writeln!(streams.stderr, "{line}");
    }
    components.blocking()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lint_args(argv: &[&str]) -> LintArgs {
        LintCommand::parse_from(std::iter::once("lint").chain(argv.iter().copied())).args
    }

    #[test]
    fn the_lowered_lint_line_is_reproducible() {
        let args = lint_args(&["-j", "4", "--workspace", "never", "--set", FORCE_EXCLUDE_SET, "a.py"]);
        assert_eq!(args.workspace, WorkspaceMode::Never);
        assert!(reproducible(&args.common));
    }

    #[test]
    fn flags_the_child_reports_differently_are_declined() {
        for extra in [
            &["--format", "json"][..],
            &["--deny-skips"],
            &["--max-skips", "1"],
            &["--timings"],
            &["--set", "discovery.generated=false"],
        ] {
            let argv: Vec<&str> = ["--workspace", "never"].iter().chain(extra).copied().collect();
            assert!(!reproducible(&lint_args(&argv).common), "{extra:?}");
        }
    }

    #[test]
    fn a_call_from_another_directory_is_declined() {
        let dir = tempfile::tempdir().expect("tempdir");
        let args = ["-j".to_string(), "1".to_string()];
        let call = Call {
            name: FMT,
            args: &args,
            dir: dir.path(),
            files: &[],
        };
        assert!(run(&call).is_none());
    }
}
