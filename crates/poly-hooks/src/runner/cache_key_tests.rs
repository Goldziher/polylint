use std::path::Path;
use std::process::Command;

use tempfile::TempDir;

use super::*;

fn git(root: &Path, args: &[&str]) {
    let status = Command::new("git")
        .current_dir(root)
        .args([
            "-c",
            "user.email=t@t",
            "-c",
            "user.name=t",
            "-c",
            "commit.gpgsign=false",
        ])
        .args(args)
        .status()
        .expect("spawn git");
    assert!(status.success(), "git {args:?} failed");
}

fn repo(files: &[(&str, &str)]) -> TempDir {
    let dir = TempDir::new().expect("tempdir");
    git(dir.path(), &["init", "-q"]);
    for (name, body) in files {
        std::fs::write(dir.path().join(name), body).expect("write");
    }
    git(dir.path(), &["add", "."]);
    git(dir.path(), &["commit", "-q", "-m", "seed"]);
    dir
}

fn txt() -> FilePattern {
    FilePattern::glob(vec!["*.txt".into()]).expect("glob")
}

fn digest(memo: &DeclaredMemo, root: &Path) -> String {
    memo.declared(root, root, &txt())
        .expect("digest")
        .digest
        .as_str()
        .to_owned()
}

#[test]
fn the_digest_is_stable_across_memos() {
    let dir = repo(&[("a.txt", "a"), ("b.txt", "b"), ("c.md", "c")]);
    let first = digest(&DeclaredMemo::default(), dir.path());
    let second = digest(&DeclaredMemo::default(), dir.path());
    assert_eq!(first, second);
}

#[test]
fn editing_a_declared_file_changes_the_digest() {
    let dir = repo(&[("a.txt", "a"), ("b.txt", "b")]);
    let before = digest(&DeclaredMemo::default(), dir.path());
    std::fs::write(dir.path().join("b.txt"), "changed").unwrap();
    assert_ne!(before, digest(&DeclaredMemo::default(), dir.path()));
}

#[test]
fn editing_an_undeclared_file_leaves_the_digest_alone() {
    let dir = repo(&[("a.txt", "a"), ("c.md", "c")]);
    let before = digest(&DeclaredMemo::default(), dir.path());
    std::fs::write(dir.path().join("c.md"), "changed").unwrap();
    assert_eq!(before, digest(&DeclaredMemo::default(), dir.path()));
}

#[test]
fn a_memo_lists_and_hashes_once_per_pattern() {
    let dir = repo(&[("a.txt", "a"), ("b.md", "b")]);
    let memo = DeclaredMemo::default();
    let first = digest(&memo, dir.path());
    let second = digest(&memo, dir.path());
    assert_eq!(first, second);
    assert_eq!(memo.hash_passes(), 1, "second lookup must be served from the memo");
    assert_eq!(memo.list_calls(), 1);

    let md = FilePattern::glob(vec!["*.md".into()]).expect("glob");
    memo.declared(dir.path(), dir.path(), &md).expect("md digest");
    assert_eq!(memo.hash_passes(), 2, "a different pattern is hashed separately");
    assert_eq!(memo.list_calls(), 1, "but shares the one git listing");
}

#[test]
fn an_unreadable_declared_file_yields_no_digest() {
    let dir = repo(&[("a.txt", "a"), ("b.txt", "b")]);
    std::fs::remove_file(dir.path().join("b.txt")).unwrap();
    assert!(
        DeclaredMemo::default()
            .declared(dir.path(), dir.path(), &txt())
            .is_none()
    );
}

#[test]
fn declared_modified_reports_only_files_changed_since_the_key() {
    let dir = repo(&[("a.txt", "a"), ("b.txt", "b")]);
    let memo = DeclaredMemo::default();
    digest(&memo, dir.path());
    let clean = declared_modified(&memo, dir.path(), dir.path(), &txt()).expect("probe");
    assert!(clean.is_empty());

    std::fs::write(dir.path().join("b.txt"), "changed").unwrap();
    let dirty = declared_modified(&memo, dir.path(), dir.path(), &txt()).expect("probe");
    assert_eq!(dirty, vec![PathBuf::from("b.txt")]);
}

#[test]
fn a_dirty_but_unchanged_worktree_is_not_reported_as_modified() {
    let dir = repo(&[("a.txt", "a")]);
    std::fs::write(dir.path().join("a.txt"), "edited before the run").unwrap();
    let memo = DeclaredMemo::default();
    digest(&memo, dir.path());
    let probe = declared_modified(&memo, dir.path(), dir.path(), &txt()).expect("probe");
    assert!(probe.is_empty());
}
