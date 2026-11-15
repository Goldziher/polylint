//! `poly migrate` must never delete a source whose settings it did not honor: conflicts with an
//! existing `poly.toml`, a v2 target, unparsable sources and a missing git safety net.

use std::fs;
use std::path::Path;
use std::process::{Command, ExitCode};

use poly_cli::MigrateArgs;
use poly_cli::migrate::deletion::Action;
use poly_cli::migrate::importers::Absorb;
use poly_cli::migrate::{build_plan, run_migrate};
use tempfile::tempdir;

fn args(dir: &Path, write: bool, allow_dirty: bool) -> MigrateArgs {
    MigrateArgs {
        path: Some(dir.to_path_buf()),
        write,
        report: !write,
        recurse: false,
        verify: false,
        allow_dirty,
        strip_superseded: false,
        to_v2: false,
    }
}

fn ruff_result(plan: &poly_cli::migrate::MigrationPlan) -> &poly_cli::migrate::importers::ImportResult {
    plan.results.iter().find(|r| r.tool == "ruff").expect("ruff result")
}

#[test]
fn a_conflicting_scalar_keeps_the_source_and_names_the_key() {
    let dir = tempdir().unwrap();
    fs::write(dir.path().join("poly.toml"), "[lint.python.ruff]\nline_length = 100\n").unwrap();
    fs::write(dir.path().join("ruff.toml"), "line-length = 88\n").unwrap();

    let plan = build_plan(dir.path()).unwrap();
    let ruff = ruff_result(&plan);
    assert!(matches!(ruff.absorb, Absorb::Partial(_)), "got {:?}", ruff.absorb);
    assert!(
        plan.conflicts
            .iter()
            .any(|c| c.contains("lint.python.ruff.line_length")),
        "{:?}",
        plan.conflicts
    );
    assert!(ruff.notes.iter().any(|n| n.contains("ruff.toml") && n.contains("kept")));
    assert!(
        plan.actions
            .iter()
            .any(|a| matches!(a, Action::Keep { path, .. } if path.ends_with("ruff.toml"))),
        "{:?}",
        plan.actions
    );
    assert!(!plan.actions.iter().any(|a| matches!(a, Action::DeleteFile(_))));

    let _ = run_migrate(args(dir.path(), true, true));
    assert!(
        dir.path().join("ruff.toml").exists(),
        "ruff.toml must survive the conflict"
    );
    let written = fs::read_to_string(dir.path().join("poly.toml")).unwrap();
    assert!(written.contains("line_length = 100"), "{written}");
}

#[test]
fn a_different_existing_array_is_a_conflict_not_a_union() {
    let dir = tempdir().unwrap();
    fs::write(dir.path().join("poly.toml"), "[lint.python.ruff]\nselect = [\"E\"]\n").unwrap();
    fs::write(dir.path().join("ruff.toml"), "select = [\"E\", \"F\", \"I\"]\n").unwrap();

    let plan = build_plan(dir.path()).unwrap();
    assert!(
        plan.conflicts.iter().any(|c| c.contains("select")),
        "{:?}",
        plan.conflicts
    );
    let rendered = plan.doc.to_string();
    assert!(
        !rendered.contains("\"F\""),
        "the existing select must not be widened: {rendered}"
    );
    assert!(matches!(ruff_result(&plan).absorb, Absorb::Partial(_)));
}

#[test]
fn an_identical_existing_value_is_not_a_conflict() {
    let dir = tempdir().unwrap();
    fs::write(dir.path().join("ruff.toml"), "line-length = 88\n").unwrap();
    let _ = run_migrate(args(dir.path(), true, true));
    fs::write(dir.path().join("ruff.toml"), "line-length = 88\n").unwrap();

    let plan = build_plan(dir.path()).unwrap();
    assert!(plan.conflicts.is_empty(), "{:?}", plan.conflicts);
    assert_eq!(ruff_result(&plan).absorb, Absorb::Full);
}

#[test]
fn a_v2_target_receives_engine_tables_not_v1_tables() {
    let dir = tempdir().unwrap();
    fs::write(dir.path().join("poly.toml"), "[engines.typos]\nenabled = true\n").unwrap();
    fs::write(dir.path().join("ruff.toml"), "line-length = 88\n").unwrap();

    let plan = build_plan(dir.path()).unwrap();
    let rendered = plan.doc.to_string();
    assert!(rendered.contains("[engines.ruff"), "{rendered}");
    assert!(
        !rendered.contains("[lint"),
        "no v1 table may land in a v2 file: {rendered}"
    );
    assert!(!rendered.contains("[fmt"), "{rendered}");
    let table: toml::Table = rendered.parse().unwrap();
    assert!(poly_config::v2::detect_v1(&table).is_empty(), "{rendered}");
}

#[test]
fn a_v2_target_conflict_keeps_the_source() {
    let dir = tempdir().unwrap();
    fs::write(dir.path().join("poly.toml"), "[engines.ruff]\nline_length = 100\n").unwrap();
    fs::write(dir.path().join("ruff.toml"), "line-length = 88\n").unwrap();

    let plan = build_plan(dir.path()).unwrap();
    assert!(
        plan.conflicts.iter().any(|c| c.contains("engines.ruff.line_length")),
        "{:?}",
        plan.conflicts
    );
    assert!(matches!(ruff_result(&plan).absorb, Absorb::Partial(_)));
}

#[test]
fn a_malformed_source_is_reported_and_kept() {
    let dir = tempdir().unwrap();
    fs::write(dir.path().join("ruff.toml"), "line-length = [\n").unwrap();
    fs::write(dir.path().join(".markdownlint.json"), "{ not json").unwrap();

    let plan = build_plan(dir.path()).unwrap();
    assert_eq!(plan.parse_errors.len(), 2, "{:?}", plan.parse_errors);
    assert!(
        plan.parse_errors
            .iter()
            .any(|e| e.contains("could not parse") && e.contains("ruff.toml"))
    );
    assert!(
        plan.parse_errors
            .iter()
            .any(|e| e.contains(".markdownlint.json") && e.ends_with("; kept"))
    );

    let code = run_migrate(args(dir.path(), true, true));
    assert_eq!(format!("{code:?}"), format!("{:?}", ExitCode::SUCCESS));
    assert!(dir.path().join("ruff.toml").exists());
    assert!(dir.path().join(".markdownlint.json").exists());
}

#[test]
fn write_outside_a_git_work_tree_is_refused_without_allow_dirty() {
    let dir = tempdir().unwrap();
    fs::write(dir.path().join("ruff.toml"), "line-length = 88\n").unwrap();
    if Command::new("git").arg("--version").output().is_err() {
        return;
    }

    let code = run_migrate(args(dir.path(), true, false));
    assert_eq!(format!("{code:?}"), format!("{:?}", ExitCode::from(2)));
    assert!(dir.path().join("ruff.toml").exists(), "nothing may be deleted");
    assert!(!dir.path().join("poly.toml").exists());
}
