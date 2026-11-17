//! Every `poly lint` / `poly fmt` / global flag either reaches the MCP `check` and `fix` tools as
//! a parameter, or is excluded here with the reason. A new flag fails this test until someone
//! decides which, so the two surfaces cannot drift apart silently again.
#![cfg(all(unix, feature = "bundled-mcp"))]

mod mcp_child;

use std::collections::BTreeSet;

use clap::{Args, Command};
use mcp_child::McpChild;
use poly_cli::{FmtArgs, GlobalArgs, LintArgs};

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
    assert_eq!(check, fix, "check and fix take the same parameters");

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
                    check.contains(*param),
                    "`--{flag}` maps to `{param}`, which `check` lacks: {check:?}"
                );
                mapped.insert(param);
            }
            Excluded(reason) => assert!(!reason.is_empty(), "`--{flag}` is excluded without a reason"),
        }
    }
    for param in &check {
        assert!(
            mapped.contains(param.as_str()),
            "`{param}` maps to no CLI flag: add a COVERAGE row or an MCP_ONLY reason"
        );
    }
}
