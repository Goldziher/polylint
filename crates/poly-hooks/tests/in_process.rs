//! In-process hooks (ADR 0067): a registered handler runs the check in the
//! runner's own process, over the same tree and file list the re-exec child
//! would have seen, and anything it does not take runs the hook's command line.

#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::process::Command;

use poly_hooks::in_process::{self, Call, Outcome};
use poly_hooks::model::{HookCommand, HookStatus};
use poly_hooks::{Hook, HookRunRequest, Stage, StageSpec, run};
use tempfile::TempDir;

fn git(repo: &Path, args: &[&str]) {
    let output = Command::new("git")
        .args(args)
        .current_dir(repo)
        .output()
        .expect("git invocation");
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn init_repo() -> TempDir {
    let dir = TempDir::new().expect("tempdir");
    git(dir.path(), &["init", "-q"]);
    dir
}

/// Fails with the names of the files under `dir` that contain `BROKEN`; declines
/// every other check name.
fn handler(call: &Call<'_>) -> Option<anyhow::Result<Outcome>> {
    if call.name != "reject-broken" {
        return None;
    }
    let broken: Vec<String> = call
        .files
        .iter()
        .filter(|file| std::fs::read_to_string(call.dir.join(file)).is_ok_and(|text| text.contains("BROKEN")))
        .map(|file| file.display().to_string())
        .collect();
    Some(Ok(Outcome {
        code: i32::from(!broken.is_empty()),
        output: format!("{} {}\n", call.args.join(" "), broken.join(" ")).into_bytes(),
    }))
}

/// An in-process hook whose re-exec line is `line`, so a test can tell which form ran.
fn in_process_hook(name: &str, line: &str) -> Hook {
    let mut hook = Hook::run(name, line);
    hook.command = HookCommand::InProcess {
        name: name.to_string(),
        args: vec!["--flag".to_string()],
        line: line.to_string(),
    };
    hook
}

/// A repository whose index holds `BROKEN` in `data.txt` while the worktree copy
/// was fixed afterwards, plus the snapshot of that index.
fn broken_index() -> (TempDir, TempDir) {
    let repo = init_repo();
    let root = repo.path();
    std::fs::write(root.join("data.txt"), "BROKEN\n").unwrap();
    git(root, &["add", "data.txt"]);
    std::fs::write(root.join("data.txt"), "fine\n").unwrap();
    let snapshot = TempDir::new().expect("snapshot dir");
    let prefix = format!("--prefix={}/", snapshot.path().display());
    git(root, &["checkout-index", "-f", "-a", &prefix]);
    (repo, snapshot)
}

fn staged_run(repo: &TempDir, snapshot: &TempDir, hook: Hook) -> (HookStatus, String) {
    in_process::register(handler);
    let outcome = run(HookRunRequest {
        root: repo.path().to_path_buf(),
        work_root: Some(snapshot.path().to_path_buf()),
        files: vec![PathBuf::from("data.txt")],
        stages: vec![StageSpec {
            stage: Stage::PreCommit,
            hooks: vec![hook],
            ..StageSpec::default()
        }],
        ..HookRunRequest::default()
    })
    .expect("run");
    let hook = &outcome.stages[0].hooks[0];
    (hook.status.clone(), String::from_utf8_lossy(&hook.output).into_owned())
}

#[test]
fn an_in_process_hook_judges_the_staged_snapshot_and_reports_its_output() {
    let (repo, snapshot) = broken_index();
    // The line would pass; only the handler reading the snapshot can fail.
    let (status, output) = staged_run(&repo, &snapshot, in_process_hook("reject-broken", "exit 0"));
    assert_eq!(status, HookStatus::Failed { code: Some(1) });
    assert_eq!(output, "--flag data.txt\n");
}

#[test]
fn a_declined_check_runs_its_command_line() {
    let (repo, snapshot) = broken_index();
    let (status, _) = staged_run(&repo, &snapshot, in_process_hook("not-handled", "exit 3"));
    assert_eq!(status, HookStatus::Failed { code: Some(3) });
}

#[test]
fn a_hook_with_its_own_env_keeps_the_reexec() {
    let (repo, snapshot) = broken_index();
    let mut hook = in_process_hook("reject-broken", "exit 0");
    hook.env.insert("EXTRA".to_string(), "1".to_string());
    let (status, _) = staged_run(&repo, &snapshot, hook);
    assert_eq!(status, HookStatus::Passed);
}
