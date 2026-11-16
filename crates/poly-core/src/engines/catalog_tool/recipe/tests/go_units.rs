//! How golangci-lint's module runs fail and overlap, driven by fake tool scripts.

use super::*;

/// golangci-lint after its `run.timeout`: an empty report, then exit 4.
const TIMES_OUT: &str = r#"#!/bin/sh
echo "$PWD $*" >> "$POLY_TEST_LOG"
echo '{"Issues":[]}'
echo 'level=error msg="Timeout exceeded: try increasing it by passing --timeout option"' >&2
exit 4
"#;

/// A timeout that had already reported one finding.
const TIMES_OUT_PARTWAY: &str = r#"#!/bin/sh
echo "$PWD $*" >> "$POLY_TEST_LOG"
printf '{"Issues":[{"FromLinter":"govet","Text":"partial","Pos":{"Filename":"a/x.go","Line":1,"Column":1}}]}'
echo 'level=error msg="Timeout exceeded"' >&2
exit 4
"#;

/// A tool killed by a signal before it reported anything.
const KILLED: &str = r#"#!/bin/sh
echo "$PWD $*" >> "$POLY_TEST_LOG"
kill -9 $$
"#;

/// A clean run that records whether another run of it was in progress.
const OVERLAP_PROBE: &str = r#"#!/bin/sh
echo "$PWD $*" >> "$POLY_TEST_LOG"
mkdir "$POLY_TEST_BUSY" 2>/dev/null || echo OVERLAP >> "$POLY_TEST_LOG"
sleep 0.5
rmdir "$POLY_TEST_BUSY" 2>/dev/null
echo '{"Issues":[]}'
"#;

fn module_recipe(unsplittable: &[i32], serial: bool) -> serde_json::Value {
    serde_json::json!({
        "output": "golangci_json",
        "group": {"marker": ["go.mod"]},
        "target": "dir",
        "ok_exits": [1],
        "unsplittable_exits": unsplittable,
        "serial": serial
    })
}

/// One module with a package in each of four directories.
fn four_packages(fx: &Fixture) -> Vec<SourceFile> {
    std::fs::write(fx.dir.path().join("go.mod"), "module x\n").expect("go.mod");
    ["a", "b", "c", "d"]
        .iter()
        .map(|dir| fx.file(&format!("{dir}/x.go"), &format!("package {dir}\n")))
        .collect()
}

#[test]
fn a_timeout_is_reported_once_for_the_module_not_retried_on_halves() {
    let fx = Fixture::new();
    let files = four_packages(&fx);
    let tool = fx.tool(
        "run $DIR",
        module_recipe(&[3, 4], false),
        Role::Lint,
        &fx.config(fx.script("golangci", TIMES_OUT)),
    );

    let results = tool.lint_files(&refs(&files));

    let log = std::fs::read_to_string(&fx.log).expect("log");
    assert_eq!(fx.invocations(), 1, "halves of a timed-out run only repeat it:\n{log}");
    for result in &results {
        let error = result.as_ref().expect_err("a timeout is not a clean file");
        assert!(format!("{error:#}").contains("Timeout exceeded"), "{error:#}");
    }
}

/// The runner drops a failed prefill's files and asks again one file at a time; each
/// would be a new single-package run of the module that just failed.
#[test]
fn a_module_that_failed_as_a_whole_is_not_run_again_for_one_of_its_files() {
    let fx = Fixture::new();
    let files = four_packages(&fx);
    let tool = fx.tool(
        "run $DIR",
        module_recipe(&[3, 4], false),
        Role::Lint,
        &fx.config(fx.script("golangci", TIMES_OUT)),
    );

    tool.lint_files(&refs(&files));
    let again: Vec<_> = files.iter().map(|file| tool.lint_files(&[file])).collect();

    let log = std::fs::read_to_string(&fx.log).expect("log");
    assert_eq!(fx.invocations(), 1, "{log}");
    assert!(again.iter().flatten().all(Result::is_err), "{again:?}");
}

/// A `dir`-grouped tool runs every directory from the same root, so one directory's
/// whole failure must not answer for the next.
#[test]
fn a_whole_failure_in_one_directory_group_does_not_fail_another() {
    const FAILS_IN_A: &str = r#"#!/bin/sh
echo "$PWD $*" >> "$POLY_TEST_LOG"
case "$*" in *./a*) exit 4 ;; esac
echo '{"Issues":[]}'
"#;
    let fx = Fixture::new();
    let a = fx.file("a/x.go", "package a\n");
    let b = fx.file("b/x.go", "package b\n");
    let recipe = serde_json::json!({
        "output": "golangci_json",
        "group": "dir",
        "target": "dir",
        "unsplittable_exits": [4]
    });
    let tool = fx.tool(
        "run $DIR",
        recipe,
        Role::Lint,
        &fx.config(fx.script("golangci", FAILS_IN_A)),
    );

    let first = tool.lint_files(&[&a]);
    let second = tool.lint_files(&[&b]);

    assert!(first[0].is_err(), "{first:?}");
    assert!(second[0].as_ref().is_ok_and(Vec::is_empty), "{second:?}");
}

#[test]
fn an_exit_the_recipe_does_not_declare_unsplittable_is_still_halved() {
    let fx = Fixture::new();
    let files = four_packages(&fx);
    let tool = fx.tool(
        "run $DIR",
        module_recipe(&[], false),
        Role::Lint,
        &fx.config(fx.script("golangci", TIMES_OUT)),
    );

    tool.lint_files(&refs(&files));

    assert!(
        fx.invocations() > 1,
        "an exit 4 of a tool that never declared it is pinned on single packages"
    );
}

#[test]
fn findings_printed_before_an_unsplittable_exit_are_not_a_verdict() {
    let fx = Fixture::new();
    let files = four_packages(&fx);
    let tool = fx.tool(
        "run $DIR",
        module_recipe(&[4], false),
        Role::Lint,
        &fx.config(fx.script("golangci", TIMES_OUT_PARTWAY)),
    );

    let results = tool.lint_files(&refs(&files));

    assert!(results.iter().all(Result::is_err), "{results:?}");
    assert_eq!(fx.invocations(), 1);
}

#[test]
fn a_tool_killed_by_a_signal_is_not_retried_on_halves() {
    let fx = Fixture::new();
    let files = four_packages(&fx);
    let tool = fx.tool(
        "run $DIR",
        module_recipe(&[], false),
        Role::Lint,
        &fx.config(fx.script("golangci", KILLED)),
    );

    let results = tool.lint_files(&refs(&files));

    assert_eq!(fx.invocations(), 1);
    assert!(results.iter().all(Result::is_err), "{results:?}");
}

/// `serial` must hold across separate `lint_files` calls: the prefill's shards, the
/// per-file loop and the fix pass each call in on their own.
#[test]
fn a_serial_tool_never_runs_two_invocations_at_once_across_calls() {
    let fx = Fixture::new();
    let mut files = Vec::new();
    for module in ["m1", "m2"] {
        std::fs::create_dir_all(fx.dir.path().join(module)).expect("module");
        std::fs::write(fx.dir.path().join(module).join("go.mod"), format!("module {module}\n")).expect("go.mod");
        files.push(fx.file(&format!("{module}/x.go"), "package x\n"));
    }
    let mut config = fx.config(fx.script("golangci", OVERLAP_PROBE));
    config.env.insert(
        "POLY_TEST_BUSY".to_owned(),
        fx.dir.path().join("busy").to_string_lossy().into_owned(),
    );
    let tool = fx.tool("run $DIR", module_recipe(&[], true), Role::Lint, &config);

    std::thread::scope(|scope| {
        for file in &files {
            let tool = &tool;
            scope.spawn(move || tool.lint_files(&[file]));
        }
    });

    let log = std::fs::read_to_string(&fx.log).expect("log");
    assert_eq!(fx.invocations(), 2, "{log}");
    assert!(!log.contains("OVERLAP"), "{log}");
}

/// The argv a module run passes, with the project's golangci config set to `config` (none for
/// `None`).
fn argv_with_project_config(config: Option<&str>) -> String {
    let fx = Fixture::new();
    std::fs::create_dir(fx.dir.path().join(".git")).expect(".git");
    if let Some(text) = config {
        std::fs::write(fx.dir.path().join(".golangci.yml"), text).expect("config");
    }
    let files = go_module(&fx);
    let mut tool_config = fx.config(fx.script("golangci", GO_REPORTER));
    tool_config.root = Some(fx.dir.path().to_string_lossy().into_owned());
    let recipe = serde_json::json!({
        "output": "golangci_json",
        "group": {"marker": ["go.mod"]},
        "target": "dir",
        "config_files": [".golangci.yml"],
        "config_defaults": [{"arg": "--timeout=0", "key": "run.timeout"}]
    });
    let tool = fx.tool("run $DIR", recipe, Role::Lint, &tool_config);
    tool.lint_files(&refs(&files));
    std::fs::read_to_string(&fx.log).expect("log")
}

#[test]
fn the_projects_run_timeout_wins_over_polys_default() {
    let unset = argv_with_project_config(None);
    assert!(unset.contains(" run --timeout=0 ."), "{unset}");
    let other_keys = argv_with_project_config(Some("run:\n  tests: false\n"));
    assert!(other_keys.contains(" run --timeout=0 ."), "{other_keys}");
    let set = argv_with_project_config(Some("run:\n  timeout: 5m\n"));
    assert!(set.contains(" run ."), "{set}");
    assert!(!set.contains("--timeout"), "{set}");
}
