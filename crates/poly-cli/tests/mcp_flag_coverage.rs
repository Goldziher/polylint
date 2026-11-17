//! Every `poly lint` / `poly fmt` / global flag either reaches the MCP `check` and `fix` tools as
//! a parameter (`fix` alone for one only a write reads), or is excluded here with the reason. The
//! flags of the state and maintenance commands (`cache`, `config`, `rules`, `components`) are
//! held against `info` and `manage` the same way. A new flag fails these tests until someone
//! decides which, so the two surfaces cannot drift apart silently again.
#![cfg(all(unix, feature = "bundled-mcp"))]

mod mcp_child;

use std::collections::BTreeSet;

use clap::{Args, Command};
use mcp_child::McpChild;
use poly_cli::{CacheArgs, ComponentsArgs, ConfigArgs, FmtArgs, GlobalArgs, LintArgs, RulesArgs};

/// How one CLI flag is covered over MCP.
enum Coverage {
    /// The `check`/`fix` parameter carrying it.
    Param(&'static str),
    /// Not exposed, and why.
    Excluded(&'static str),
}

use Coverage::{Excluded, Param};

/// Every lint, fmt and global long flag (and the positional `paths`).
const COVERAGE: &[(&str, Coverage)] = &[
    ("paths", Param("paths")),
    ("exclude", Param("exclude")),
    ("include-excluded", Param("include_excluded")),
    ("only", Param("only")),
    ("skip", Param("skip")),
    ("max-skips", Param("max_skips")),
    ("deny-skips", Param("max_skips")),
    ("on-missing", Param("on_missing")),
    ("no-cache", Param("no_cache")),
    ("jobs", Param("jobs")),
    ("fix-generated", Param("fix_generated")),
    ("config", Param("config")),
    ("debug", Param("debug")),
    ("format", Param("format")),
    (
        "fix",
        Excluded("the `fix` tool, not a parameter: annotations are per tool"),
    ),
    ("workspace", Param("scope")),
    (
        "exit-nonzero-on-fix",
        Excluded("no exit codes over MCP; `results[].changed` says what was rewritten"),
    ),
    ("check", Excluded("removed flag, kept hidden for its error message")),
    ("timings", Excluded("stderr presentation only")),
    ("color", Excluded("presentation only")),
    ("quiet", Excluded("presentation only")),
    ("verbose", Excluded("presentation only")),
    (
        "offline",
        Excluded("server-wide: `poly --offline mcp`; per call, `install: never`"),
    ),
    (
        "trust",
        Excluded("never: the server enforces trust so a model cannot widen it"),
    ),
    (
        "set",
        Excluded("installs a process-global layer and can set exec-capable keys; unsafe in a shared server"),
    ),
];

/// MCP-only parameters, which no CLI flag maps to.
const MCP_ONLY: &[(&str, &str)] = &[
    ("kind", "selects lint or fmt, which the CLI does by subcommand"),
    (
        "install",
        "per-call component download policy; the CLI follows the config and `--offline`",
    ),
];

fn long_flags(command: Command) -> BTreeSet<String> {
    command
        .get_arguments()
        .filter(|arg| !arg.is_hide_set() || arg.get_long() == Some("check"))
        .filter_map(|arg| match arg.get_long() {
            Some(long) => Some(long.to_owned()),
            None if arg.is_positional() => Some(arg.get_id().as_str().to_owned()),
            None => None,
        })
        .filter(|name| name != "help" && name != "version")
        .collect()
}

fn cli_flags() -> BTreeSet<String> {
    let mut flags = long_flags(LintArgs::augment_args(Command::new("lint")));
    flags.extend(long_flags(FmtArgs::augment_args(Command::new("fmt"))));
    flags.extend(long_flags(GlobalArgs::augment_args(Command::new("poly"))));
    flags
}

#[test]
fn every_cli_flag_is_a_parameter_or_excluded_with_a_reason() {
    let cache = tempfile::tempdir().unwrap();
    let mut server = McpChild::start(cache.path(), cache.path());
    let tools = server.tools();
    let schema = |name: &str| {
        let tool = tools["tools"]
            .as_array()
            .unwrap()
            .iter()
            .find(|tool| tool["name"] == name)
            .unwrap_or_else(|| panic!("{name} is listed: {tools}"));
        tool["inputSchema"]["properties"]
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect::<BTreeSet<String>>()
    };
    let (check, fix) = (schema("check"), schema("fix"));
    let fix_only: BTreeSet<String> = fix.difference(&check).cloned().collect();
    assert_eq!(
        fix_only,
        BTreeSet::from(["fix_generated".to_owned()]),
        "`fix` takes `check`'s parameters plus the ones only a write reads"
    );
    assert!(check.is_subset(&fix), "`check` takes nothing `fix` lacks: {check:?}");

    let cli = cli_flags();
    let rows: BTreeSet<String> = COVERAGE.iter().map(|(flag, _)| (*flag).to_owned()).collect();
    for flag in &cli {
        assert!(
            rows.contains(flag),
            "`--{flag}` is new: add it to the MCP `RunRequest` or list it in COVERAGE with a reason"
        );
    }
    for flag in &rows {
        assert!(cli.contains(flag), "COVERAGE row `{flag}` names no CLI flag: remove it");
    }

    let mut mapped: BTreeSet<&str> = MCP_ONLY.iter().map(|(name, _)| *name).collect();
    for (flag, coverage) in COVERAGE {
        match coverage {
            Param(param) => {
                assert!(
                    fix.contains(*param),
                    "`--{flag}` maps to `{param}`, which `fix` lacks: {fix:?}"
                );
                mapped.insert(param);
            }
            Excluded(reason) => assert!(!reason.is_empty(), "`--{flag}` is excluded without a reason"),
        }
    }
    for param in &fix {
        assert!(
            mapped.contains(param.as_str()),
            "`{param}` maps to no CLI flag: add a COVERAGE row or an MCP_ONLY reason"
        );
    }
}

/// How one flag of a state or maintenance command is covered over MCP.
enum Reach {
    /// The tool and its parameter carrying the flag.
    On(&'static str, &'static str),
    /// Not exposed, and why.
    Left(&'static str),
}

use Reach::{Left, On};

/// Every long flag and positional of `poly cache`, `config`, `rules` and `components`, keyed
/// `<command> [<subcommand>] <flag>`.
const STATE_COVERAGE: &[(&str, Reach)] = &[
    (
        "cache cache-dir",
        Left("the server works on the repository's own cache; a model does not choose the directory"),
    ),
    ("cache format", On("info", "format")),
    ("cache gc max-age", On("manage", "max_age_days")),
    ("cache gc max-size", On("manage", "max_size")),
    ("config show format", On("info", "format")),
    (
        "config schema output",
        Left("writes a file; the schemas are resources under poly://schema/"),
    ),
    ("config validate paths", On("info", "paths")),
    ("rules test dirs", On("info", "dirs")),
    ("rules list dirs", On("info", "dirs")),
    ("rules list format", On("info", "format")),
    ("components format", On("info", "format")),
    ("components status paths", On("info", "paths")),
    (
        "components status verify",
        Left("re-hashing every installed archive is a CLI maintenance check; `info` reports the recorded state"),
    ),
    ("components install names", On("manage", "names")),
    ("components uninstall names", On("manage", "names")),
    ("components sync paths", On("manage", "paths")),
    (
        "components prune all",
        Left("`components_prune` removes only what the lock no longer references"),
    ),
    (
        "components prune dry-run",
        Left("`components_prune` has no dry run; `manage` refuses `dry_run` on it rather than ignore it"),
    ),
    (
        "components prune keep-days",
        Left("`components_prune` keeps the CLI's default window"),
    ),
    (
        "components import archive",
        Left("installs an archive from a path on the host; not offered to a model"),
    ),
];

/// `info` and `manage` parameters no flag of those commands maps to.
const STATE_ONLY: &[(&str, &str, &str)] = &[
    (
        "info",
        "topics",
        "selects the document, which the CLI does by subcommand",
    ),
    ("info", "config", "the global `--config`"),
    (
        "info",
        "install",
        "the download policy `components` judges against; the CLI follows the config and `--offline`",
    ),
    ("info", "test", "`poly rules test` is a subcommand"),
    (
        "manage",
        "action",
        "selects the operation, which the CLI does by subcommand",
    ),
    ("manage", "config", "the global `--config`"),
    (
        "manage",
        "format",
        "the result's text block; the maintenance commands print one form",
    ),
    (
        "manage",
        "languages",
        "installs by language; the CLI names components and presets",
    ),
    (
        "manage",
        "dry_run",
        "what an install would download; the CLI answers that with `components status`",
    ),
];

/// `prefix`'s own flags, then those of each of its subcommands.
fn command_flags(prefix: &str, command: Command, out: &mut BTreeSet<String>) {
    let subcommands: Vec<Command> = command.get_subcommands().cloned().collect();
    out.extend(long_flags(command).into_iter().map(|flag| format!("{prefix} {flag}")));
    for subcommand in subcommands {
        let name = format!("{prefix} {}", subcommand.get_name());
        command_flags(&name, subcommand, out);
    }
}

#[test]
fn every_state_command_flag_reaches_info_or_manage_or_is_excluded_with_a_reason() {
    let mut cli = BTreeSet::new();
    command_flags("cache", CacheArgs::augment_args(Command::new("cache")), &mut cli);
    command_flags("config", ConfigArgs::augment_args(Command::new("config")), &mut cli);
    command_flags("rules", RulesArgs::augment_args(Command::new("rules")), &mut cli);
    command_flags(
        "components",
        ComponentsArgs::augment_args(Command::new("components")),
        &mut cli,
    );
    assert!(cli.len() >= 15, "the commands' flags were collected: {cli:?}");

    let rows: BTreeSet<String> = STATE_COVERAGE.iter().map(|(flag, _)| (*flag).to_owned()).collect();
    for flag in &cli {
        assert!(
            rows.contains(flag),
            "`{flag}` is new: map it to an `info`/`manage` parameter or list it in STATE_COVERAGE with a reason"
        );
    }
    for flag in &rows {
        assert!(
            cli.contains(flag),
            "STATE_COVERAGE row `{flag}` names no CLI flag: remove it"
        );
    }

    let cache = tempfile::tempdir().unwrap();
    let mut server = McpChild::start(cache.path(), cache.path());
    let tools = server.tools();
    let mut mapped: BTreeSet<(&str, &str)> = STATE_ONLY
        .iter()
        .map(|(tool, param, reason)| {
            assert!(!reason.is_empty(), "`{tool}.{param}` is MCP-only without a reason");
            (*tool, *param)
        })
        .collect();
    for (flag, reach) in STATE_COVERAGE {
        match reach {
            On(tool, param) => {
                mapped.insert((tool, param));
            }
            Left(reason) => assert!(!reason.is_empty(), "`{flag}` is excluded without a reason"),
        }
    }
    for tool in ["info", "manage"] {
        let listed = tools["tools"]
            .as_array()
            .unwrap()
            .iter()
            .find(|listed| listed["name"] == tool)
            .unwrap_or_else(|| panic!("{tool} is listed: {tools}"));
        let params: BTreeSet<&str> = listed["inputSchema"]["properties"]
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        let claimed: BTreeSet<&str> = mapped
            .iter()
            .filter(|(owner, _)| *owner == tool)
            .map(|(_, param)| *param)
            .collect();
        assert_eq!(
            claimed, params,
            "every `{tool}` parameter maps to a CLI flag or carries an MCP-only reason, and no row names a parameter it lacks"
        );
    }
}
