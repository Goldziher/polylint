use std::path::Path;
use std::process::Command;

use tempfile::TempDir;

use super::link_stays_inside;
use crate::snapshot::StagedSnapshot;

fn git(repo: &Path, args: &[&str]) {
    let ok = Command::new("git")
        .args(args)
        .current_dir(repo)
        .status()
        .expect("run git")
        .success();
    assert!(ok, "git {args:?} failed");
}

fn init(repo: &Path) {
    git(repo, &["init", "-q"]);
    git(repo, &["config", "user.email", "t@t"]);
    git(repo, &["config", "user.name", "t"]);
    git(repo, &["config", "commit.gpgsign", "false"]);
}

/// `d/e -> ..` and `t -> d/e/d/e/../../../..` each look in-tree lexically, but
/// the kernel resolves the `..` after following `d/e`, so `t` leaves the
/// snapshot and `t2` reads through it.
#[cfg(unix)]
#[test]
fn a_symlink_chain_that_escapes_physically_is_neutralised() {
    let tmp = TempDir::new().expect("tmp repo");
    let cache = TempDir::new().expect("cache home");
    let repo = tmp.path();
    init(repo);

    std::fs::create_dir_all(repo.join("d")).unwrap();
    std::os::unix::fs::symlink("..", repo.join("d/e")).unwrap();
    std::os::unix::fs::symlink("d/e/d/e/../../../..", repo.join("t")).unwrap();
    std::os::unix::fs::symlink("t/etc/passwd", repo.join("t2")).unwrap();
    git(repo, &["add", "."]);

    let snap = StagedSnapshot::create_in(cache.path(), repo, &[]).expect("snapshot");

    for name in ["t", "t2"] {
        let path = snap.path().join(name);
        assert!(
            std::fs::symlink_metadata(&path).is_err() || !path.exists(),
            "{name} must not resolve to anything outside the snapshot"
        );
    }
    assert!(
        std::fs::symlink_metadata(snap.path().join("t")).is_err(),
        "the escaping link itself is removed"
    );
}

#[cfg(unix)]
#[test]
fn links_through_a_sibling_directory_are_judged_physically() {
    let tmp = TempDir::new().expect("tmp repo");
    let cache = TempDir::new().expect("cache home");
    let repo = tmp.path();
    init(repo);

    std::fs::create_dir_all(repo.join("real/sub")).unwrap();
    std::fs::write(repo.join("top.toml"), "top = true\n").unwrap();
    std::os::unix::fs::symlink("real/sub", repo.join("alias")).unwrap();
    std::os::unix::fs::symlink("alias/../../outside", repo.join("bad")).unwrap();
    std::os::unix::fs::symlink("alias/../../top.toml", repo.join("good")).unwrap();
    git(repo, &["add", "."]);

    let snap = StagedSnapshot::create_in(cache.path(), repo, &[]).expect("snapshot");

    assert!(
        std::fs::symlink_metadata(snap.path().join("alias")).is_ok(),
        "a plain in-tree directory link is kept"
    );
    assert!(std::fs::symlink_metadata(snap.path().join("bad")).is_err());
    // `good` resolves physically to `top.toml` inside the snapshot, but the lexical prefilter
    // (`resolves_inside_tree`) reads `alias/../..` as climbing past the root and removes it first:
    // a deliberate conservative false positive, not a dangling-link decision.
    assert!(
        std::fs::symlink_metadata(snap.path().join("good")).is_err(),
        "a link whose lexical reading climbs past the root is removed even if it resolves inside"
    );
}

#[cfg(unix)]
#[test]
fn link_stays_inside_judges_physical_resolution() {
    let dir = TempDir::new().expect("snapshot");
    let canonical = std::fs::canonicalize(dir.path()).unwrap();
    std::fs::create_dir_all(dir.path().join("d")).unwrap();
    std::fs::write(dir.path().join("f.txt"), "x").unwrap();
    std::os::unix::fs::symlink("..", dir.path().join("d/e")).unwrap();
    let check = |link: &str, target: &str| {
        std::os::unix::fs::symlink(target, dir.path().join(link)).unwrap();
        link_stays_inside(dir.path(), &canonical, Path::new(link), Path::new(target))
    };

    assert!(check("ok", "f.txt"));
    assert!(!check("abs", "/etc/passwd"));
    assert!(!check("t", "d/e/d/e/../../../.."));
}

#[cfg(unix)]
#[test]
fn link_stays_inside_judges_dangling_links_by_their_lexical_reading() {
    let dir = TempDir::new().expect("snapshot");
    let canonical = std::fs::canonicalize(dir.path()).unwrap();
    std::fs::create_dir_all(dir.path().join("d")).unwrap();
    std::fs::write(dir.path().join("f.txt"), "x").unwrap();
    std::os::unix::fs::symlink("d", dir.path().join("alias")).unwrap();
    let check = |link: &str, target: &str| {
        std::os::unix::fs::symlink(target, dir.path().join(link)).unwrap();
        link_stays_inside(dir.path(), &canonical, Path::new(link), Path::new(target))
    };

    assert!(check("dangling", "missing/file"), "a plain dangling link is kept");
    assert!(
        !check("dotdot", "missing/../f.txt"),
        "`..` after a named component cannot be trusted when the target does not resolve"
    );
    assert!(
        !check("alias/dangling", "missing"),
        "a dangling link under a symlinked directory is removed"
    );
}
