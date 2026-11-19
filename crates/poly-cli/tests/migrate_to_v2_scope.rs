//! `poly migrate --to-v2` rewrites poly's own configs and nothing else: foreign tool configs are
//! neither absorbed nor deleted, and a directory without a poly config gets none.

use std::fs;
use std::path::Path;

use poly_cli::MigrateArgs;
use poly_cli::migrate::run_migrate;
use tempfile::tempdir;

fn to_v2(dir: &Path, recurse: bool) -> MigrateArgs {
    MigrateArgs {
        path: Some(dir.to_path_buf()),
        write: true,
        report: false,
        recurse,
        verify: false,
        allow_dirty: true,
        strip_superseded: false,
        to_v2: true,
    }
}

const PYPROJECT: &str = "[project]\nname = \"x\"\n\n[tool.ruff.lint]\nextend-select = [\"B\"]\n";
const TYPOS: &str = "[default.extend-words]\nopf = \"opf\"\n";

#[test]
fn to_v2_leaves_foreign_tool_configs_alone() {
    let dir = tempdir().unwrap();
    fs::write(dir.path().join("poly.toml"), "[lint.python.ruff]\nline_length = 100\n").unwrap();
    fs::write(dir.path().join("pyproject.toml"), PYPROJECT).unwrap();
    fs::write(dir.path().join("_typos.toml"), TYPOS).unwrap();

    let _ = run_migrate(to_v2(dir.path(), false));

    let written = fs::read_to_string(dir.path().join("poly.toml")).unwrap();
    assert!(written.contains("[engines.ruff.lint]"), "rewritten to v2:\n{written}");
    assert!(
        !written.contains("\"B\"") && !written.contains("opf"),
        "nothing absorbed:\n{written}"
    );
    assert_eq!(
        fs::read_to_string(dir.path().join("pyproject.toml")).unwrap(),
        PYPROJECT
    );
    assert_eq!(fs::read_to_string(dir.path().join("_typos.toml")).unwrap(), TYPOS);
}

#[test]
fn to_v2_recurse_writes_no_config_where_there_was_none() {
    let dir = tempdir().unwrap();
    fs::write(dir.path().join("poly.toml"), "[lint.python.ruff]\nline_length = 100\n").unwrap();
    fs::create_dir(dir.path().join("python")).unwrap();
    fs::write(dir.path().join("python/pyproject.toml"), PYPROJECT).unwrap();
    fs::write(dir.path().join("python/uv.lock"), "version = 1\n").unwrap();
    fs::create_dir(dir.path().join("locked")).unwrap();
    fs::write(dir.path().join("locked/uv.lock"), "version = 1\n").unwrap();
    fs::create_dir(dir.path().join("local")).unwrap();
    fs::write(
        dir.path().join("local/poly.local.toml"),
        "[lint.python.ruff]\nline_length = 90\n",
    )
    .unwrap();

    let _ = run_migrate(to_v2(dir.path(), true));

    for sub in ["python", "locked", "local"] {
        assert!(
            !dir.path().join(sub).join("poly.toml").exists(),
            "{sub}/poly.toml must not be created"
        );
    }
    assert_eq!(
        fs::read_to_string(dir.path().join("python/pyproject.toml")).unwrap(),
        PYPROJECT
    );
    let local = fs::read_to_string(dir.path().join("local/poly.local.toml")).unwrap();
    assert!(
        local.contains("[engines.ruff.lint]"),
        "the overlay is rewritten:\n{local}"
    );
}

#[test]
fn to_v2_recurse_rewrites_a_nested_config_that_replaces_its_parent() {
    let dir = tempdir().unwrap();
    fs::write(dir.path().join("poly.toml"), "[lint.python.ruff]\nselect = [\"E\"]\n").unwrap();
    fs::create_dir(dir.path().join("sub")).unwrap();
    fs::write(
        dir.path().join("sub/poly.toml"),
        "[lint.python.ruff]\ninherit_mode = \"replace\"\nselect = [\"F\"]\n",
    )
    .unwrap();

    let _ = run_migrate(to_v2(dir.path(), true));

    let nested = fs::read_to_string(dir.path().join("sub/poly.toml")).unwrap();
    assert!(nested.contains("inherit_mode = \"replace\""), "{nested}");
    let report = poly_core::config_validate::validate_source(&nested);
    assert!(report.is_valid(), "{nested}\n{:?}", report.problems);
}
