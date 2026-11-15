//! `--timings` / `POLY_TIMINGS`: a per-phase and per-engine block on stderr after the report.

use std::process::{Command, Output};

const POLY: &str = env!("CARGO_BIN_EXE_poly");

const PHASES: [&str; 5] = ["config load", "discovery", "planning", "per-file loop", "cache flush"];

fn run(args: &[&str], env_timings: bool) -> Output {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(dir.path().join("a.py"), "x = 1\n").expect("write fixture");
    let mut command = Command::new(POLY);
    command
        .args(["--color", "never"])
        .args(args)
        .current_dir(dir.path())
        .env_remove("POLY_TIMINGS");
    if env_timings {
        command.env("POLY_TIMINGS", "1");
    }
    command.output().expect("run poly")
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

#[test]
fn fmt_timings_names_every_phase() {
    let text = stderr(&run(&["fmt", "--no-cache", "--timings", "a.py"], false));
    assert!(text.contains("timings: phases"), "{text}");
    for phase in PHASES {
        assert!(text.contains(phase), "missing phase {phase:?} in:\n{text}");
    }
}

#[test]
fn lint_timings_names_the_project_and_loop_phases() {
    let text = stderr(&run(
        &["lint", "--no-cache", "--timings", "--workspace", "never", "a.py"],
        false,
    ));
    for phase in ["config load", "discovery", "per-file loop", "project phase"] {
        assert!(text.contains(phase), "missing phase {phase:?} in:\n{text}");
    }
}

#[test]
fn the_environment_variable_enables_timings() {
    let text = stderr(&run(&["fmt", "--no-cache", "a.py"], true));
    assert!(text.contains("timings: phases"), "{text}");
}

#[test]
fn no_timings_block_by_default() {
    let text = stderr(&run(&["fmt", "--no-cache", "a.py"], false));
    assert!(!text.contains("timings:"), "{text}");
}
