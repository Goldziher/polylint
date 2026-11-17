//! The `lint` and `fmt` builtins in process: the per-file tier of `poly lint`
//! and `poly fmt` as the re-exec child runs it, rendered into a buffer.
//!
//! Only a call this process can stand in for exactly is taken; anything else is
//! declined and re-execs (ADR 0071):
//!
//! - A hook directory other than this process's working directory (the staged
//!   snapshot, a hook `cwd`) becomes the run's root ([`RunOptions::root`], ADR
//!   0073): the run reads its config, files and every working-directory anchor
//!   there, as the child started in it does. Such a call is declined when it
//!   names no file or a file the directory does not hold, whose report the child
//!   spells from paths this process does not have, and when it would serve any
//!   engine from a downloadable component, which never sees the root.
//! - The parent's `--offline` and `--trust` flags, and a colour forced through
//!   the environment, would reach an in-process run but not the child.
//! - Only the arguments the lowering produces, plus flags that only feed
//!   [`RunOptions`], are reproduced: the pretty report, no skip budget, no
//!   timings, and no `--set` but `discovery.force_exclude=true`, which is
//!   applied to this run's options rather than installed process-wide.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use anyhow::Result;
use clap::Parser;
use poly_core::components::ComponentsReport;
use poly_core::{Config, RunOptions, Verbosity, report};
use poly_hooks::in_process::{Call, FMT, LINT, Outcome};

use crate::component_notes::component_lines;
use crate::config_sources::{RemoteExtendsResolver, repo_root, repo_root_from};
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
    if !environment_matches_child() {
        return None;
    }
    let root = run_root(call)?;
    match call.name {
        LINT => {
            let args = LintCommand::try_parse_from(super::argv(LINT, call)).ok()?.args;
            if args.workspace != WorkspaceMode::Never || !reproducible(&args.common) {
                return None;
            }
            lint(&args.common, root.as_deref()).map(Ok)
        }
        FMT => {
            let args = FmtCommand::try_parse_from(super::argv(FMT, call)).ok()?.args;
            if args.check || !reproducible(&args.common) {
                return None;
            }
            fmt(&args, root.as_deref()).map(Ok)
        }
        _ => None,
    }
}

/// Where the run is rooted: `Some(None)` in this process's working directory,
/// `Some(Some(dir))` in the hook's directory `dir` (canonical, as the child's
/// working directory is), `None` to decline. A rooted call must name files that
/// exist under the root: with no file the child walks `.`, and a missing file is
/// reported by the child in its own spelling.
fn run_root(call: &Call<'_>) -> Option<Option<PathBuf>> {
    if super::is_working_dir(call.dir) {
        return Some(None);
    }
    let root = call.dir.canonicalize().ok()?;
    let held = !call.files.is_empty() && call.files.iter().all(|file| root.join(file).exists());
    held.then_some(Some(root))
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

/// `poly lint --workspace never`'s pretty path (`crate::run_lint`), rooted at
/// `root` when the hook's directory is not this process's. `None` declines; see
/// [`prepared`].
fn lint(common: &CommonArgs, root: Option<&Path>) -> Option<Outcome> {
    let mut streams = Streams::default();
    let (paths, config, opts) = match prepared(common, root, &mut streams) {
        Prepared::Run(prepared) => *prepared,
        Prepared::Stop => return Some(streams.finish(EXIT_NOT_VERIFIED)),
        Prepared::Decline => return None,
    };
    let run = match poly_core::lint_run(&paths, &config, &opts, common.fix, false) {
        Ok(run) => run,
        Err(error) => {
            let _ = writeln!(streams.stderr, "error: {error:#}");
            return Some(streams.finish(EXIT_NOT_VERIFIED));
        }
    };
    let (text, _) = report::render_lint_pretty_run(&run, Verbosity::from_flags(false, false, false));
    streams.stdout.push_str(&text);
    components(&run.components, &mut streams);
    let code = poly_host_services::run::Verdict::of_lint(&run, None).exit_code();
    Some(streams.finish(code))
}

/// `poly fmt`'s pretty path (`crate::run_fmt`), rooted and declining as [`lint`] does.
fn fmt(args: &FmtArgs, root: Option<&Path>) -> Option<Outcome> {
    let common = &args.common;
    let mut streams = Streams::default();
    let (paths, config, opts) = match prepared(common, root, &mut streams) {
        Prepared::Run(prepared) => *prepared,
        Prepared::Stop => return Some(streams.finish(EXIT_NOT_VERIFIED)),
        Prepared::Decline => return None,
    };
    let write = common.fix;
    let run = match poly_core::format_run(&paths, &config, &opts, write, false) {
        Ok(run) => run,
        Err(error) => {
            let _ = writeln!(streams.stderr, "error: {error:#}");
            return Some(streams.finish(EXIT_NOT_VERIFIED));
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
    Some(streams.finish(code))
}

/// What [`prepared`] decided.
enum Prepared {
    /// Run with these paths, config and options.
    Run(Box<(Vec<PathBuf>, Config, RunOptions)>),
    /// Stop: the reason is in the streams, as the child prints it.
    Stop,
    /// Leave the call to the child.
    Decline,
}

/// [`prepare`], and for a rooted run the decision to leave it to the child: when
/// it cannot be prepared (the child reports that in its own terms), or when it
/// would serve any engine from a downloadable component. A component engine runs
/// in its own copy of the engine API, which never sees the root, so it would read
/// this process's working directory where the child reads the snapshot (ADR 0073).
fn prepared(common: &CommonArgs, root: Option<&Path>, streams: &mut Streams) -> Prepared {
    match (prepare(common, root, streams), root) {
        (None, None) => Prepared::Stop,
        (None, Some(_)) => Prepared::Decline,
        (Some(prepared), None) => Prepared::Run(Box::new(prepared)),
        (Some((paths, config, opts)), Some(_)) => {
            if needs_components(&paths, &config, &opts) {
                Prepared::Decline
            } else {
                Prepared::Run(Box::new((paths, config, opts)))
            }
        }
    }
}

/// Whether the run would load an engine from a component, or cannot tell.
fn needs_components(paths: &[PathBuf], config: &Config, opts: &RunOptions) -> bool {
    poly_core::needs_components(paths, config, opts).unwrap_or(true)
}

/// The child's `prepare` (`crate::prepare_with_poly`) without its process-wide
/// side effects: the `--set` override lands on this run's config, and nothing
/// is printed but into `streams`. `None` once it has written why the run stops.
///
/// With a `root`, everything the child reads from its working directory is read
/// from the root instead: the repository the resolver is rooted at, the config,
/// and (through [`RunOptions::root`]) the files and every anchor below the runner.
/// `run_root` already checked that the named files exist there.
fn prepare(
    common: &CommonArgs,
    root: Option<&Path>,
    streams: &mut Streams,
) -> Option<(Vec<PathBuf>, Config, RunOptions)> {
    let paths = if common.paths.is_empty() {
        vec![PathBuf::from(".")]
    } else {
        common.paths.clone()
    };
    if root.is_none()
        && let Some(message) = missing_paths_message(&paths)
    {
        streams.stderr.push_str(&message);
        return None;
    }
    let repository = match root {
        Some(root) => Ok(repo_root_from(root)),
        None => repo_root(),
    };
    let resolver = match repository.and_then(|repository| RemoteExtendsResolver::new(&repository)) {
        Ok(resolver) => std::sync::Arc::new(resolver),
        Err(error) => {
            let _ = writeln!(streams.stderr, "error: failed to prepare config resolver: {error:#}");
            return None;
        }
    };
    // The child loads from its working directory: the root, else this process's own.
    let dir = match root {
        Some(root) => root.to_path_buf(),
        None => std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
    };
    let poly_config = match poly_config::PolyConfig::load_with(&dir, resolver.as_ref()) {
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
    let resolver: std::sync::Arc<dyn poly_config::BaseConfigResolver> = resolver;
    let mut opts = poly_host_services::run::run_options(&crate::run_request(common), &config, Some(resolver));
    // The hook's child never passes `--config`: the config is the one found at the hook's root.
    opts.explicit_config = false;
    opts.root = root.map(Path::to_path_buf);
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
    fn a_call_from_another_directory_is_rooted_there_when_it_holds_the_files() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("a.py"), "x = 1\n").expect("write");
        let call = |files: &[&Path]| {
            run_root(&Call {
                name: FMT,
                args: &[],
                dir: dir.path(),
                files,
            })
        };
        let canonical = dir.path().canonicalize().expect("canonical");
        assert_eq!(call(&[Path::new("a.py")]), Some(Some(canonical)));
        assert_eq!(call(&[]), None, "a rooted call that names no file walks `.`");
        assert_eq!(
            call(&[Path::new("missing.py")]),
            None,
            "the child reports a missing file"
        );
    }

    /// A run rooted away from this process's directory is left to the child when it would load
    /// an engine from a component: `secrets` is never linked into this binary.
    #[test]
    fn a_rooted_run_that_needs_a_component_is_declined() {
        const SECRETS: &str = "[engines.secrets]\nenabled = true\n";
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path().canonicalize().expect("canonical");
        std::fs::write(root.join("a.py"), "x = 1\n").expect("write");
        let common = lint_args(&["--workspace", "never", "a.py"]).common;
        let decision = |config: &str| {
            std::fs::write(root.join("poly.toml"), config).expect("write config");
            let mut streams = Streams::default();
            match prepared(&common, Some(&root), &mut streams) {
                Prepared::Run(_) => "run",
                Prepared::Stop => "stop",
                Prepared::Decline => "decline",
            }
        };
        assert_eq!(decision(SECRETS), "decline");
        let (paths, config, opts) =
            prepare(&common, Some(&root), &mut Streams::default()).expect("the config loads, so the decline is real");
        assert!(poly_core::needs_components(&paths, &config, &opts).expect("components resolve"));
        #[cfg(feature = "all-engines")]
        assert_eq!(decision(""), "run", "every engine a Python file needs is linked");
    }
}
