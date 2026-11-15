use poly_engine_api::config::GlobalDefaults;
use tempfile::TempDir;

use super::*;

/// A log rooted at a fresh directory with `docs/adr` created and `lock` as the manifest.
fn log_with_lock(lock: &str) -> (TempDir, AdrLog) {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = fs::canonicalize(dir.path()).expect("canonicalize");
    fs::create_dir_all(root.join("docs/adr")).expect("mkdir");
    let log = AdrLog {
        root,
        config: EngineConfig {
            globals: GlobalDefaults::default(),
            indent_width: 2,
            options: toml::Table::new(),
        },
        enabled: true,
        dirs: vec![PathBuf::from("docs/adr")],
        declared: ProjectDirs {
            dir_key: "dir".to_owned(),
            dir_default: "docs/adr".to_owned(),
            paths_key: Some("paths".to_owned()),
            recursive_key: Some("recursive".to_owned()),
            recursive_default: false,
            whole_tree: false,
            file_names: Vec::new(),
            list_unserved: true,
        },
        lock: inside_root(lock).expect("a relative lock path"),
    };
    (dir, log)
}

fn log() -> (TempDir, AdrLog) {
    log_with_lock("adrlint.lock")
}

fn write(path: &str, content: &str, create: bool) -> WireAdrWrite {
    WireAdrWrite {
        path: path.to_owned(),
        content: content.to_owned(),
        create,
    }
}

fn manifest_plan(writes: Vec<WireAdrWrite>) -> AdrPlan {
    AdrPlan {
        wire: WireAdrPlan {
            writes,
            changes: Vec::new(),
            summary: String::new(),
        },
        allowed: Allowed::Manifest,
    }
}

fn new_plan(writes: Vec<WireAdrWrite>) -> AdrPlan {
    AdrPlan {
        wire: WireAdrPlan {
            writes,
            changes: Vec::new(),
            summary: String::new(),
        },
        allowed: Allowed::NewRecord(PathBuf::from("docs/adr")),
    }
}

fn refused(log: &AdrLog, plan: &AdrPlan) -> String {
    log.apply(plan).expect_err("the write must be refused").to_string()
}

#[test]
fn the_manifest_and_a_new_record_are_written() {
    let (_dir, log) = log();
    let written = log
        .apply(&manifest_plan(vec![write("adrlint.lock", "version = 1\n", false)]))
        .expect("manifest");
    assert_eq!(written, [log.root.join("adrlint.lock")]);
    assert_eq!(
        fs::read_to_string(log.root.join("adrlint.lock")).unwrap(),
        "version = 1\n"
    );

    log.apply(&new_plan(vec![write(
        "docs/adr/0001-use-rust.md",
        "# Use Rust\n",
        true,
    )]))
    .expect("new record");
    assert!(log.root.join("docs/adr/0001-use-rust.md").is_file());
}

#[test]
fn a_symlinked_leaf_is_refused() {
    let (_dir, log) = log();
    let outside = tempfile::tempdir().expect("tempdir");
    let target = outside.path().join("victim");
    fs::write(&target, "untouched").unwrap();
    std::os::unix::fs::symlink(&target, log.root.join("adrlint.lock")).unwrap();

    let message = refused(&log, &manifest_plan(vec![write("adrlint.lock", "x", false)]));
    assert!(message.contains("symlink"), "{message}");
    assert_eq!(fs::read_to_string(&target).unwrap(), "untouched");
}

#[test]
fn a_symlinked_parent_resolving_outside_the_root_is_refused() {
    let (_dir, log) = log_with_lock("link/adrlint.lock");
    let outside = tempfile::tempdir().expect("tempdir");
    std::os::unix::fs::symlink(outside.path(), log.root.join("link")).unwrap();

    let message = refused(&log, &manifest_plan(vec![write("link/adrlint.lock", "x", false)]));
    assert!(message.contains("outside the project root"), "{message}");
    assert!(!outside.path().join("adrlint.lock").exists());
}

#[test]
fn creating_an_existing_record_is_refused() {
    let (_dir, log) = log();
    let path = log.root.join("docs/adr/0001-use-rust.md");
    fs::write(&path, "original").unwrap();

    let message = refused(&log, &new_plan(vec![write("docs/adr/0001-use-rust.md", "new", true)]));
    assert!(message.contains("already exists"), "{message}");
    assert_eq!(fs::read_to_string(&path).unwrap(), "original");
}

#[test]
fn absolute_and_parent_paths_are_refused() {
    let (_dir, log) = log();
    let outside = tempfile::tempdir().expect("tempdir");
    let absolute = outside.path().join("adrlint.lock");
    for path in [absolute.to_str().unwrap(), "../adrlint.lock", "docs/../../adrlint.lock"] {
        let message = refused(&log, &manifest_plan(vec![write(path, "x", false)]));
        assert!(message.contains("outside the project root"), "{path}: {message}");
    }
    assert!(!absolute.exists());
}

#[test]
fn a_plan_may_write_one_file() {
    let (_dir, log) = log();
    let plan = manifest_plan(vec![
        write("adrlint.lock", "x", false),
        write("adrlint.lock", "y", false),
    ]);
    let message = refused(&log, &plan);
    assert!(message.contains("2 writes"), "{message}");
    assert!(!log.root.join("adrlint.lock").exists(), "nothing is written");
}

#[test]
fn lock_and_unlock_may_only_replace_the_manifest() {
    let (_dir, log) = log();
    fs::create_dir_all(log.root.join(".git/hooks")).unwrap();
    for path in [
        ".git/config",
        ".git/hooks/pre-commit",
        "docs/adr/0001-use-rust.md",
        "other.lock",
    ] {
        let message = refused(&log, &manifest_plan(vec![write(path, "x", false)]));
        assert!(message.contains("may only replace adrlint.lock"), "{path}: {message}");
        assert!(!log.root.join(path).exists(), "{path}");
    }
    let message = refused(&log, &manifest_plan(vec![write("adrlint.lock", "x", true)]));
    assert!(
        message.contains("may only replace"),
        "a create of the manifest: {message}"
    );
}

#[test]
fn new_may_only_create_a_record_in_the_chosen_directory() {
    let (_dir, log) = log();
    fs::create_dir_all(log.root.join("docs/adr/sub")).unwrap();
    fs::create_dir_all(log.root.join(".git/hooks")).unwrap();
    for path in [
        ".git/hooks/0001-hook.md",
        "adrlint.lock",
        "docs/0001-use-rust.md",
        "docs/adr/sub/0001-use-rust.md",
        "docs/adr/0001_use_rust.md",
        "docs/adr/0001-Use-Rust.md",
        "docs/adr/001-use-rust.md",
        "docs/adr/0001-.md",
        "docs/adr/0001-use-rust.txt",
        "docs/adr/hooks.md",
    ] {
        let message = refused(&log, &new_plan(vec![write(path, "x", true)]));
        assert!(
            message.contains("may only create a new NNNN-slug.md"),
            "{path}: {message}"
        );
        assert!(!log.root.join(path).exists(), "{path}");
    }
    let message = refused(&log, &new_plan(vec![write("docs/adr/0001-use-rust.md", "x", false)]));
    assert!(message.contains("may only create"), "a replace by new: {message}");
}

#[test]
fn replacing_the_manifest_keeps_its_permissions() {
    use std::os::unix::fs::PermissionsExt;

    let (_dir, log) = log();
    let path = log.root.join("adrlint.lock");
    fs::write(&path, "old").unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o640)).unwrap();

    log.apply(&manifest_plan(vec![write("adrlint.lock", "new", false)]))
        .expect("replace");
    assert_eq!(fs::read_to_string(&path).unwrap(), "new");
    assert_eq!(fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o640);
}

#[test]
fn record_names_follow_the_new_record_pattern() {
    assert!(is_record_name("0001-use-rust.md"));
    assert!(is_record_name("9999-a.md"));
    assert!(!is_record_name("0001-.md"));
    assert!(!is_record_name("0001-Use.md"));
    assert!(!is_record_name("00001-a.md"));
    assert!(!is_record_name("0001-a.markdown"));
}

#[test]
fn civil_dates_match_known_days() {
    assert_eq!(civil_from_days(0), (1970, 1, 1));
    assert_eq!(civil_from_days(11_016), (2000, 2, 29));
    assert_eq!(civil_from_days(20_772), (2026, 11, 15));
}

#[test]
fn a_path_must_stay_inside_the_root() {
    assert_eq!(inside_root("./docs/adr"), Some(PathBuf::from("docs/adr")));
    assert_eq!(inside_root("."), Some(PathBuf::new()));
    assert_eq!(inside_root("../elsewhere"), None);
    assert_eq!(inside_root("/etc"), None);
}

#[test]
fn wire_paths_use_forward_slashes() {
    assert_eq!(
        wire_path(&Path::new("docs").join("adr").join("0001-x.md")),
        "docs/adr/0001-x.md"
    );
    assert_eq!(wire_path(Path::new("./adrlint.lock")), "adrlint.lock");
}
