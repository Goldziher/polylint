//! CLI ↔ MCP parity, end to end: the same request through `poly … --format json` and through a
//! spawned `poly mcp` must produce the same document and the same verdict.
//!
//! The in-process MCP tests render one host-side run two ways, so they cannot see a divergence
//! on the input side — a parameter one surface drops, a check only one surface makes. These run
//! both shipped surfaces from their own front doors.
#![cfg(all(unix, feature = "bundled-mcp"))]

mod mcp_child;

use std::path::Path;
use std::process::Command;

use mcp_child::{McpChild, POLY};
use serde_json::{Value, json};
use tempfile::TempDir;

/// Whether a run verified what it was asked to: the one distinction both surfaces must agree on.
#[derive(Debug, PartialEq, Eq)]
enum Class {
    /// CLI exit 0 or 1; MCP `isError: false`.
    Verified,
    /// CLI exit 2; MCP `isError: true`.
    NotVerified,
}

/// A fixture directory and a cache directory shared by both surfaces.
struct Fixture {
    dir: TempDir,
    cache: TempDir,
}

impl Fixture {
    fn new(files: &[(&str, &[u8])]) -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        for (name, contents) in files {
            let path = dir.path().join(name);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, contents).unwrap();
        }
        Self {
            dir,
            cache: tempfile::tempdir().expect("cache"),
        }
    }

    fn path(&self) -> &Path {
        self.dir.path()
    }

    /// Run the CLI, returning its stdout parsed as JSON (or null) and its class.
    fn cli(&self, args: &[&str]) -> (Value, Class, i32) {
        cli_in(self.path(), self.cache.path(), args)
    }

    fn server(&self) -> McpChild {
        McpChild::start(self.path(), self.cache.path())
    }
}

fn cli_in(dir: &Path, cache: &Path, args: &[&str]) -> (Value, Class, i32) {
    let output = Command::new(POLY)
        .args(args)
        .current_dir(dir)
        .env("POLY_OFFLINE", "1")
        .env("POLY_CACHE_HOME", cache)
        .output()
        .expect("run poly");
    let code = output.status.code().expect("exit code");
    let document = serde_json::from_slice(&output.stdout).unwrap_or(Value::Null);
    let class = if code == 2 { Class::NotVerified } else { Class::Verified };
    (document, class, code)
}

/// The tool result's document without the identity block, and its class.
fn mcp(response: &Value) -> (Value, Class) {
    let result = &response["result"];
    assert!(result.is_object(), "a tool result, not a protocol error: {response}");
    let mut document = result["structuredContent"].clone();
    if let Value::Object(map) = &mut document {
        map.remove("poly");
    }
    let class = if result["isError"] == json!(true) {
        Class::NotVerified
    } else {
        Class::Verified
    };
    (document, class)
}

/// Assert both surfaces return one document and one verdict for a run.
fn assert_same_run(fixture: &Fixture, server: &mut McpChild, cli: &[&str], tool: &str, arguments: Value) -> Value {
    let (cli_document, cli_class, code) = fixture.cli(cli);
    let (mcp_document, mcp_class) = mcp(&server.call(tool, arguments.clone()));
    assert_eq!(
        cli_class, mcp_class,
        "{cli:?} exited {code}; {tool} {arguments}: {mcp_document}"
    );
    assert_eq!(cli_document, mcp_document, "{cli:?} vs {tool} {arguments}");
    mcp_document
}

const CLEAN: &[u8] = b"print(\"hi\")\n";
const UNUSED_IMPORT: &[u8] = b"import os\n";
const INVALID_UTF8: &[u8] = b"x = 1\n\xff\xfe not utf-8\n";
const CSPROJ: &[u8] = b"<Project Sdk=\"Microsoft.NET.Sdk\">\n</Project>\n";
const DRIFT: &[u8] = b"{\"a\":1,\n  \"b\":   2}\n";

#[test]
fn lint_and_format_runs_return_the_cli_document_and_verdict() {
    let fixture = Fixture::new(&[
        ("ok.py", CLEAN),
        ("bad.py", UNUSED_IMPORT),
        ("broken.py", INVALID_UTF8),
        ("App.csproj", CSPROJ),
        ("data.json", DRIFT),
        ("skipme/d.py", CLEAN),
        ("poly.toml", b"[discovery]\nexclude = [\"skipme/**\"]\n"),
    ]);
    let mut server = fixture.server();
    fn lint<'a>(paths: &[&'a str]) -> Vec<&'a str> {
        let mut args = vec!["lint", "--format", "json", "--workspace", "never"];
        args.extend(paths);
        args
    }

    // A clean file, an error-severity finding, an unrouted named file, invalid text.
    for paths in [&["ok.py"][..], &["bad.py"], &["App.csproj"], &["broken.py"]] {
        assert_same_run(
            &fixture,
            &mut server,
            &lint(paths),
            "check",
            json!({ "kind": "lint", "paths": paths }),
        );
    }
    // A named path the config excludes: the discovery block says why nothing was checked.
    let excluded = assert_same_run(
        &fixture,
        &mut server,
        &lint(&["skipme/d.py"]),
        "check",
        json!({ "paths": ["skipme/d.py"] }),
    );
    assert_eq!(excluded["discovery"]["excluded_explicit"], 1, "{excluded}");
    assert_same_run(
        &fixture,
        &mut server,
        &[
            "lint",
            "--format",
            "json",
            "--workspace",
            "never",
            "--include-excluded",
            "skipme/d.py",
        ],
        "check",
        json!({ "paths": ["skipme/d.py"], "include_excluded": true }),
    );
    // An exclude glob, engine selection and a skip budget.
    assert_same_run(
        &fixture,
        &mut server,
        &[
            "lint",
            "--format",
            "json",
            "--workspace",
            "never",
            "--exclude",
            "bad.py",
            ".",
        ],
        "check",
        json!({ "paths": ["."], "exclude": ["bad.py"] }),
    );
    assert_same_run(
        &fixture,
        &mut server,
        &[
            "lint",
            "--format",
            "json",
            "--workspace",
            "never",
            "--only",
            "ruff",
            "bad.py",
        ],
        "check",
        json!({ "paths": ["bad.py"], "only": ["ruff"] }),
    );
    assert_same_run(
        &fixture,
        &mut server,
        &[
            "lint",
            "--format",
            "json",
            "--workspace",
            "never",
            "--deny-skips",
            "App.csproj",
        ],
        "check",
        json!({ "paths": ["App.csproj"], "max_skips": 0 }),
    );
    // Formatting drift (exit 1, a finding) and a file the formatter failed on (exit 2).
    for paths in [&["data.json"][..], &["broken.py"]] {
        let mut args = vec!["fmt", "--format", "json"];
        args.extend(paths);
        assert_same_run(
            &fixture,
            &mut server,
            &args,
            "check",
            json!({ "kind": "format", "paths": paths }),
        );
    }
}

/// Whether any result of `document` is for `path`.
fn reports(document: &Value, path: &str) -> bool {
    document["results"]
        .as_array()
        .is_some_and(|results| results.iter().any(|result| result["path"] == path))
}

/// The `cache_hit` flag of every per-engine debug record in `document`.
fn cache_hits(document: &Value) -> Vec<bool> {
    document["results"]
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|result| result["debug"]["engines"].as_array().into_iter().flatten())
        .filter_map(|engine| engine["cache_hit"].as_bool())
        .collect()
}

/// The options that select the config and shape the run, each with an effect that shows it was
/// routed: a parameter that merely exists would leave the two documents different.
#[test]
fn config_selection_and_run_options_reach_both_surfaces() {
    const LINT: [&str; 5] = ["lint", "--format", "json", "--workspace", "never"];
    let fixture = Fixture::new(&[
        ("ok.py", CLEAN),
        ("bad.py", UNUSED_IMPORT),
        ("sub/b.py", UNUSED_IMPORT),
        ("poly.toml", b""),
        ("sub/poly.toml", b"[defaults]\nline_length = 100\n"),
        ("alt.toml", b"[discovery]\nexclude = [\"bad.py\"]\n"),
    ]);
    let mut server = fixture.server();
    let lint = |extra: &[&'static str]| [&LINT[..], extra].concat();

    // A nested `poly.toml` governs its own directory on both surfaces.
    let nested = assert_same_run(
        &fixture,
        &mut server,
        &lint(&["bad.py", "sub/b.py"]),
        "check",
        json!({ "paths": ["bad.py", "sub/b.py"] }),
    );
    assert_eq!(nested["configs"].as_array().map(Vec::len), Some(2), "{nested}");
    assert_eq!(nested["configs"][1]["root"], "sub", "{nested}");

    // An explicit config replaces the discovered one.
    let explicit = assert_same_run(
        &fixture,
        &mut server,
        &[
            "--config",
            "alt.toml",
            "lint",
            "--format",
            "json",
            "--workspace",
            "never",
            "bad.py",
            "ok.py",
        ],
        "check",
        json!({ "paths": ["bad.py", "ok.py"], "config": "alt.toml" }),
    );
    assert!(
        reports(&nested, "bad.py") && !reports(&explicit, "bad.py"),
        "{explicit}"
    );
    assert_eq!(explicit["discovery"]["excluded_explicit"], 1, "{explicit}");

    // Dropping an engine, a worker count and the missing-component policy.
    let skipped = assert_same_run(
        &fixture,
        &mut server,
        &lint(&["--skip", "ruff", "-j", "1", "--on-missing", "skip", "bad.py"]),
        "check",
        json!({ "paths": ["bad.py"], "skip": ["ruff"], "jobs": 1, "on_missing": "skip" }),
    );
    assert!(!reports(&skipped, "bad.py"), "ruff was skipped: {skipped}");
}

/// Debug records carry timings, which differ between any two runs, so compare what they say
/// about the cache: a plain run fills it first, and `no_cache` must bypass it on both surfaces.
#[test]
fn debug_records_and_the_cache_bypass_reach_both_surfaces() {
    const LINT: [&str; 5] = ["lint", "--format", "json", "--workspace", "never"];
    let fixture = Fixture::new(&[("bad.py", UNUSED_IMPORT)]);
    let mut server = fixture.server();
    let lint = |extra: &[&'static str]| [&LINT[..], extra].concat();
    fixture.cli(&lint(&["bad.py"]));
    for (flags, arguments, hit) in [
        (
            &["--debug", "bad.py"][..],
            json!({ "paths": ["bad.py"], "debug": true }),
            true,
        ),
        (
            &["--debug", "--no-cache", "bad.py"],
            json!({ "paths": ["bad.py"], "debug": true, "no_cache": true }),
            false,
        ),
    ] {
        let (cli_document, _, _) = fixture.cli(&lint(flags));
        let (mcp_document, _) = mcp(&server.call("check", arguments.clone()));
        let (cli_hits, mcp_hits) = (cache_hits(&cli_document), cache_hits(&mcp_document));
        assert!(
            !cli_hits.is_empty(),
            "the CLI attached no debug records: {cli_document}"
        );
        assert_eq!(cli_hits, mcp_hits, "{arguments}");
        assert!(
            mcp_hits.iter().all(|cached| *cached == hit),
            "{arguments}: {mcp_hits:?}"
        );
    }
}

#[test]
fn a_run_that_cannot_start_is_not_verified_on_both_surfaces() {
    let fixture = Fixture::new(&[("ok.py", CLEAN)]);
    let mut server = fixture.server();
    let (_, class, _) = fixture.cli(&["lint", "--format", "json", "--workspace", "never", "nope.py"]);
    let (document, mcp_class) = mcp(&server.call("check", json!({ "paths": ["nope.py"] })));
    assert_eq!((class, mcp_class), (Class::NotVerified, Class::NotVerified));
    assert_eq!(document["error"]["kind"], "paths", "{document}");

    let (_, class, _) = fixture.cli(&[
        "lint",
        "--format",
        "json",
        "--workspace",
        "never",
        "--only",
        "nope",
        "ok.py",
    ]);
    let (document, mcp_class) = mcp(&server.call("check", json!({ "paths": ["ok.py"], "only": ["nope"] })));
    assert_eq!(
        (class, mcp_class),
        (Class::NotVerified, Class::NotVerified),
        "{document}"
    );

    let broken = Fixture::new(&[("ok.py", CLEAN), ("poly.toml", b"[discovery\n")]);
    let mut server = broken.server();
    let (_, class, _) = broken.cli(&["lint", "--format", "json", "--workspace", "never", "ok.py"]);
    let (document, mcp_class) = mcp(&server.call("check", json!({ "paths": ["ok.py"] })));
    assert_eq!((class, mcp_class), (Class::NotVerified, Class::NotVerified));
    assert_eq!(document["error"]["kind"], "config", "{document}");
}

#[test]
fn fixes_write_the_same_tree_and_report_the_same_document() {
    let files: &[(&str, &[u8])] = &[("data.json", DRIFT), ("bad.py", UNUSED_IMPORT)];
    for (cli, kind) in [
        (&["fmt", "--fix"][..], "format"),
        (&["lint", "--fix", "--workspace", "never"], "lint"),
    ] {
        let by_cli = Fixture::new(files);
        let by_mcp = Fixture::new(files);
        let mut args = cli.to_vec();
        args.extend(["--format", "json", "."]);
        let (cli_document, cli_class, _) = by_cli.cli(&args);
        let (mcp_document, mcp_class) = mcp(&by_mcp.server().call("fix", json!({ "kind": kind, "paths": ["."] })));
        assert_eq!(cli_class, mcp_class, "{kind}");
        assert_eq!(cli_document, mcp_document, "{kind}");
        for (name, _) in files {
            assert_eq!(
                std::fs::read(by_cli.path().join(name)).unwrap(),
                std::fs::read(by_mcp.path().join(name)).unwrap(),
                "{kind}: {name} is rewritten identically"
            );
        }
    }
}

#[test]
fn info_topics_are_the_cli_documents() {
    let fixture = Fixture::new(&[
        ("ok.py", CLEAN),
        (
            "poly.toml",
            b"[engines.ruff]\nselect = [\"E\"]\n\n[engines.typos]\nenabled = true\n",
        ),
    ]);
    let mut server = fixture.server();
    let (config, _, _) = fixture.cli(&["config", "show", "--format", "json"]);
    let (cache, _, _) = fixture.cli(&["cache", "stats", "--format", "json"]);
    let (rules, _, _) = fixture.cli(&["rules", "list", "--format", "json"]);
    let (info, class) = mcp(&server.call("info", json!({ "topics": ["config", "cache", "rules"] })));
    assert_eq!(class, Class::Verified, "{info}");
    // Both sides of each comparison below would be null if neither surface answered.
    for (topic, cli) in [("config", &config), ("cache", &cache), ("rules", &rules)] {
        assert!(cli.is_object(), "the CLI printed no {topic} document");
        assert!(info[topic].is_object(), "info has no {topic} document: {info}");
    }
    assert_eq!(
        info["config"], config,
        "info config is `poly config show --format json`"
    );
    assert_eq!(
        info["config"]["config"]["engines"]["typos"]["enabled"],
        json!(true),
        "{info}"
    );
    assert_eq!(info["cache"], cache, "info cache is `poly cache stats --format json`");
    assert_eq!(info["rules"], rules, "info rules is `poly rules list --format json`");
}
