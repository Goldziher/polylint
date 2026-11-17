//! End-to-end proof that the stat-validated fast path is both fast (an
//! unchanged clean file is not even opened) and never stale (every input that
//! can change a verdict changes the plan key, the stat, or both).
//!
//! "Not even opened" is asserted from the runner's own accounting of how each
//! file was resolved (stat hit, content hit or miss), captured from its debug log.
//!
//! Files must age past the index's racy window before they are recorded, so each
//! scenario sleeps once after writing its fixtures. `POLY_CACHE_HOME` is
//! process-global, so every test holds `ENV_LOCK`.
#![cfg(feature = "all-engines")]
#![cfg(unix)]

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use poly_core::{Config, RunOptions};

static ENV_LOCK: Mutex<()> = Mutex::new(());

const SETTLE: Duration = Duration::from_millis(2_300);

struct Project {
    _cache_home: tempfile::TempDir,
    dir: tempfile::TempDir,
    _guard: std::sync::MutexGuard<'static, ()>,
}

impl Project {
    fn new(ruff_select: &str) -> Self {
        let guard = ENV_LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let cache_home = tempfile::tempdir().unwrap();
        // SAFETY: serialized by `ENV_LOCK`, which the returned value holds until
        // it is dropped together with the override's tempdir.
        unsafe {
            std::env::set_var("POLY_CACHE_HOME", cache_home.path());
        }
        let project = Self {
            _cache_home: cache_home,
            dir: tempfile::tempdir().unwrap(),
            _guard: guard,
        };
        project.set_ruff_select(ruff_select);
        project
    }

    fn set_ruff_select(&self, select: &str) {
        self.write("poly.toml", &format!("[lint.python.ruff]\nselect = [\"{select}\"]\n"));
    }

    fn path(&self, name: &str) -> PathBuf {
        self.dir.path().join(name)
    }

    fn write(&self, name: &str, content: &str) {
        let path = self.path(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
    }

    fn opts() -> RunOptions {
        RunOptions {
            jobs: Some(2),
            explicit_config: true,
            // The taplo and skif engines live in the `web` component; skipping both keeps `poly.toml` from
            // planning a component a core build has no lock for. The core config check still answers it.
            // `yaml` likewise: the located fixtures hold an ast-grep rule file.
            skip: vec!["taplo".to_owned(), "skif".to_owned(), "yaml".to_owned()],
            ..RunOptions::default()
        }
    }

    fn config(&self) -> Config {
        Config::load_file(&self.path("poly.toml")).unwrap()
    }

    fn lint(&self) -> LintOutcome {
        lint_at(self.dir.path(), self.dir.path(), &self.config(), &Self::opts())
    }
}

/// Lint `paths_root` under `config`, reporting findings relative to `base`.
fn lint_at(base: &Path, paths_root: &Path, config: &Config, opts: &RunOptions) -> LintOutcome {
    let (run, log) = capture(|| poly_core::lint_run(&[paths_root.to_path_buf()], config, opts, false, false).unwrap());
    let mut findings: Vec<String> = run
        .results
        .iter()
        .flat_map(|result| {
            let name = relative(
                &std::path::absolute(base).unwrap(),
                &std::path::absolute(&result.path).unwrap(),
            );
            result
                .diagnostics
                .iter()
                .map(move |d| format!("{name}:{}", d.code.clone().unwrap_or_default()))
        })
        .collect();
    findings.sort();
    LintOutcome {
        findings,
        errors: run.errors.iter().map(|e| e.message.clone()).collect(),
        resolution: Resolution::parse(&log),
    }
}

impl Project {
    fn fmt_changed(&self) -> FmtOutcome {
        fmt_at(self.dir.path(), self.dir.path(), &self.config())
    }
}

/// Check the formatting of `paths_root` under `config`, naming changed files relative to `base`.
fn fmt_at(base: &Path, paths_root: &Path, config: &Config) -> FmtOutcome {
    let (run, log) =
        capture(|| poly_core::format_run(&[paths_root.to_path_buf()], config, &Project::opts(), false, false).unwrap());
    let mut changed: Vec<String> = run
        .results
        .iter()
        .filter(|r| r.changed)
        .map(|r| {
            relative(
                &std::path::absolute(base).unwrap(),
                &std::path::absolute(&r.path).unwrap(),
            )
        })
        .collect();
    changed.sort();
    FmtOutcome {
        changed,
        errors: run.errors.iter().map(|e| e.message.clone()).collect(),
        resolution: Resolution::parse(&log),
    }
}

impl Drop for Project {
    fn drop(&mut self) {
        // SAFETY: still under `ENV_LOCK` (the guard field drops after this).
        unsafe {
            std::env::remove_var("POLY_CACHE_HOME");
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
struct LintOutcome {
    findings: Vec<String>,
    errors: Vec<String>,
    resolution: Resolution,
}

struct FmtOutcome {
    changed: Vec<String>,
    errors: Vec<String>,
    resolution: Resolution,
}

/// How many files the fast path answered, and how.
#[derive(Debug, PartialEq, Eq)]
struct Resolution {
    stat_hits: usize,
    content_hits: usize,
    misses: usize,
}

impl Resolution {
    fn parse(log: &str) -> Self {
        let line = log
            .lines()
            .find(|line| line.contains("stat index resolution"))
            .unwrap_or_else(|| panic!("the run logged no resolution summary:\n{log}"));
        let field = |name: &str| -> usize {
            let at = line.find(&format!("{name}=")).unwrap() + name.len() + 1;
            line[at..]
                .split(|c: char| !c.is_ascii_digit())
                .next()
                .unwrap()
                .parse()
                .unwrap()
        };
        Self {
            stat_hits: field("stat_hits"),
            content_hits: field("content_hits"),
            misses: field("misses"),
        }
    }
}

#[derive(Clone, Default)]
struct SharedBuf(Arc<Mutex<Vec<u8>>>);

impl Write for SharedBuf {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn capture<T>(run: impl FnOnce() -> T) -> (T, String) {
    let buf = SharedBuf::default();
    let writer = buf.clone();
    let subscriber = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::DEBUG)
        .with_ansi(false)
        .with_writer(move || writer.clone())
        .finish();
    let value = tracing::subscriber::with_default(subscriber, run);
    let log = String::from_utf8(buf.0.lock().unwrap().clone()).unwrap();
    (value, log)
}

fn relative(root: &Path, path: &Path) -> String {
    path.strip_prefix(root).unwrap_or(path).to_string_lossy().into_owned()
}

fn settle() {
    std::thread::sleep(SETTLE);
}

#[test]
fn a_warm_run_does_not_open_unchanged_clean_files() {
    let project = Project::new("INP001");
    project.write("pkg/__init__.py", "");
    project.write("pkg/ok.py", "x = 1\n");
    settle();
    let cold = project.lint();
    assert_eq!(cold.resolution.stat_hits, 0);
    project.lint();

    let warm = project.lint();

    assert!(warm.findings.is_empty());
    assert!(warm.errors.is_empty());
    assert_eq!(
        warm.resolution,
        Resolution {
            stat_hits: 3,
            content_hits: 0,
            misses: 0
        },
        "every file, poly.toml included, must be answered from its stat alone"
    );
}

#[test]
fn a_file_with_findings_is_replayed_without_running_an_engine() {
    let project = Project::new("INP001");
    project.write("loose/mod.py", "x = 1\n");
    settle();
    let cold = project.lint();
    project.lint();
    let warm = project.lint();

    assert_eq!(cold.findings, vec!["loose/mod.py:INP001".to_owned()]);
    assert_eq!(warm.findings, cold.findings);
    assert_eq!(warm.resolution.stat_hits, 2, "mod.py and poly.toml");
}

#[test]
fn adding_and_removing_a_sibling_init_file_changes_the_findings_of_an_untouched_file() {
    let project = Project::new("INP001");
    project.write("pkg/mod.py", "x = 1\n");
    settle();
    project.lint();
    let warm = project.lint();
    assert_eq!(warm.findings, vec!["pkg/mod.py:INP001".to_owned()]);
    assert_eq!(
        warm.resolution.stat_hits, 2,
        "the premise: mod.py is served from the index"
    );

    project.write("pkg/__init__.py", "");
    assert!(
        project.lint().findings.is_empty(),
        "mod.py is byte-identical and its stat is unchanged, but its package root moved"
    );
    settle();
    project.lint();
    assert!(project.lint().findings.is_empty());

    fs::remove_file(project.path("pkg/__init__.py")).unwrap();
    assert_eq!(project.lint().findings, vec!["pkg/mod.py:INP001".to_owned()]);
}

#[test]
fn editing_the_config_invalidates_a_clean_verdict() {
    let project = Project::new("F401");
    project.write("loose/mod.py", "x = 1\n");
    settle();
    project.lint();
    let warm = project.lint();
    assert!(warm.findings.is_empty());
    assert_eq!(
        warm.resolution.stat_hits, 2,
        "the premise: the verdict is served from the index"
    );

    project.set_ruff_select("INP001");

    assert_eq!(project.lint().findings, vec!["loose/mod.py:INP001".to_owned()]);
}

#[test]
fn a_same_size_edit_that_keeps_the_mtime_is_still_detected() {
    let project = Project::new("F401");
    project.write("pkg/__init__.py", "");
    project.write("pkg/mod.py", "import_os\n");
    settle();
    project.lint();
    let warm = project.lint();
    assert!(warm.findings.is_empty());
    assert_eq!(
        warm.resolution.stat_hits, 3,
        "the premise: both files are served from the index"
    );

    let file = project.path("pkg/mod.py");
    let mtime = fs::metadata(&file).unwrap().modified().unwrap();
    fs::write(&file, "import os\n").unwrap();
    fs::File::options()
        .write(true)
        .open(&file)
        .unwrap()
        .set_modified(mtime)
        .unwrap();

    assert_eq!(project.lint().findings, vec!["pkg/mod.py:F401".to_owned()]);
}

#[test]
fn a_file_edited_inside_the_racy_window_is_never_trusted() {
    let project = Project::new("F401");
    project.write("pkg/__init__.py", "");
    project.write("pkg/mod.py", "import_os\n");
    settle();
    project.lint();

    project.write("pkg/mod.py", "import os\n");
    project.lint();
    project.write("pkg/mod.py", "import_os\n");
    project.lint();
    project.write("pkg/mod.py", "import os\n");

    assert_eq!(
        project.lint().findings,
        vec!["pkg/mod.py:F401".to_owned()],
        "back-to-back rewrites of one size within the timestamp granularity must never serve a stale clean verdict"
    );
}

#[test]
fn touching_a_file_without_changing_it_keeps_the_verdict_without_re_linting() {
    let project = Project::new("INP001");
    project.write("pkg/__init__.py", "");
    project.write("pkg/ok.py", "x = 1\n");
    project.write("loose/mod.py", "x = 1\n");
    settle();
    project.lint();
    let before = project.lint();

    project.write("pkg/ok.py", "x = 1\n");
    project.write("loose/mod.py", "x = 1\n");
    settle();
    let touched = project.lint();

    assert_eq!(touched.findings, before.findings);
    assert_eq!(
        touched.resolution.content_hits, 2,
        "identical bytes must be vouched for by digest"
    );
    assert_eq!(
        project.lint().resolution.stat_hits,
        4,
        "and the refreshed stat serves the next run"
    );
}

#[test]
fn a_warm_format_check_does_not_open_unchanged_files_and_still_reports_unformatted_ones() {
    let project = Project::new("F401");
    project.write("a.py", "a = 1\n");
    project.write("b.py", "b   =   2\n");
    settle();
    project.fmt_changed();
    let warm = project.fmt_changed();

    assert_eq!(warm.changed, vec!["b.py".to_owned()]);
    assert!(warm.errors.is_empty());
    assert_eq!(
        warm.resolution.stat_hits, 2,
        "poly.toml and a.py are answered from their stat; b.py needs a formatter"
    );
}

#[test]
fn editing_a_formatted_file_into_an_unformatted_one_is_reported() {
    let project = Project::new("F401");
    project.write("a.py", "a = 1\n");
    settle();
    project.fmt_changed();
    assert!(project.fmt_changed().changed.is_empty());

    project.write("a.py", "a    = 1\n");

    assert_eq!(project.fmt_changed().changed, vec!["a.py".to_owned()]);
}

/// The `prefilled` count of the run's lint batch, from its debug log.
fn batched(log: &str) -> usize {
    let line = log
        .lines()
        .find(|line| line.contains("lint batch prefill complete"))
        .unwrap_or_else(|| panic!("the run logged no batch summary:\n{log}"));
    let at = line.find("prefilled=").unwrap() + "prefilled=".len();
    line[at..]
        .split(|c: char| !c.is_ascii_digit())
        .next()
        .unwrap()
        .parse()
        .unwrap()
}

#[test]
fn batched_lint_results_are_recorded_and_a_warm_file_is_never_batched_again() {
    let project = Project::new("INP001");
    let names = ["a.sh", "b.sh", "c.sh", "d.sh"];
    for name in names {
        project.write(name, "#!/bin/sh\necho $1\n");
    }
    settle();
    let run = |project: &Project| {
        capture(|| {
            poly_core::lint_run(
                &[project.dir.path().to_path_buf()],
                &project.config(),
                &Project::opts(),
                false,
                false,
            )
            .unwrap()
        })
    };
    let findings = |run: &poly_core::LintRun| {
        let mut found: Vec<String> = run
            .results
            .iter()
            .filter(|result| result.path.extension().is_some_and(|ext| ext == "sh"))
            .flat_map(|result| result.diagnostics.iter().map(|d| d.code.clone().unwrap_or_default()))
            .collect();
        found.sort();
        found
    };

    let (cold, cold_log) = run(&project);
    if batched(&cold_log) == 0 {
        eprintln!("shellcheck is unavailable; nothing batched, so nothing to compose");
        return;
    }
    let expected = findings(&cold);
    assert!(!expected.is_empty(), "the premise: shellcheck reports on the fixtures");
    let (_, second_log) = run(&project);
    assert_eq!(
        batched(&second_log),
        0,
        "every file is clean to the index by now or reused"
    );

    let (warm, warm_log) = run(&project);
    assert_eq!(
        findings(&warm),
        expected,
        "batched findings must be replayed from the index"
    );
    assert_eq!(batched(&warm_log), 0, "an indexed file must not be batched again");
    let resolution = Resolution::parse(&warm_log);
    assert_eq!(resolution.misses, 0, "{resolution:?}");

    project.write("a.sh", "#!/bin/sh\necho $2\n");
    let (changed, changed_log) = run(&project);
    assert_eq!(
        batched(&changed_log),
        0,
        "one changed file is below the batch size and the rest are indexed"
    );
    assert_eq!(findings(&changed).len(), expected.len());
}

/// A project with a user ast-grep rule (whose rule directory the config resolves to a path) and a
/// Go module (whose root the ast-grep Go salt reads), committed to a git repository.
fn located_project() -> Project {
    let project = Project::new("INP001");
    project.write(
        ".poly/rules/no-print.yml",
        "id: no-print\nlanguage: python\nseverity: warning\nmessage: no print\nrule:\n  pattern: print($$$A)\n",
    );
    project.write("pkg/__init__.py", "");
    project.write("pkg/a.py", "print(1)\n");
    project.write("go/go.mod", "module example.com/m\n\ngo 1.22\n");
    project.write("go/a.go", "package m\n\nfunc A() int { return 1 }\n");
    git(project.dir.path(), &["init", "-q"]);
    git(project.dir.path(), &["add", "."]);
    git(project.dir.path(), &["commit", "-q", "-m", "seed"]);
    project
}

fn git(dir: &Path, args: &[&str]) {
    let status = std::process::Command::new("git")
        .args([
            "-c",
            "user.email=t@t",
            "-c",
            "user.name=t",
            "-c",
            "commit.gpgsign=false",
        ])
        .args(args)
        .current_dir(dir)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .unwrap();
    assert!(status.success(), "git {args:?}");
}

/// Run `run` with the process directory set to `dir`, as a `poly` started there has it: the result
/// cache is located from it. Callers hold `ENV_LOCK` through their [`Project`].
fn in_dir<T>(dir: &Path, run: impl FnOnce(&Path) -> T) -> T {
    let previous = std::env::current_dir().unwrap();
    std::env::set_current_dir(dir).unwrap();
    let here = std::env::current_dir().unwrap();
    let value = run(&here);
    std::env::set_current_dir(previous).unwrap();
    value
}

fn lint_here(here: &Path) -> LintOutcome {
    let config = Config::load_file(&here.join("poly.toml")).unwrap();
    lint_at(here, here, &config, &Project::opts())
}

/// M2-M4: no cache key carries the checkout's location, so a second worktree of the repository
/// answers every file from the first one's records (by content), with nothing re-linted.
#[test]
fn a_second_worktree_reuses_every_record_of_the_first() {
    let project = located_project();
    settle();
    let first = in_dir(project.dir.path(), lint_here);
    assert!(
        first.findings.contains(&"pkg/a.py:no-print".to_owned()),
        "{:?}",
        first.findings
    );
    let worktrees = tempfile::tempdir().unwrap();
    let second = worktrees.path().join("second");
    git(project.dir.path(), &["worktree", "add", "-q", second.to_str().unwrap()]);

    let other = in_dir(&second, lint_here);

    assert_eq!(other.findings, first.findings);
    assert_eq!(other.resolution.misses, 0, "{:?}", other.resolution);
    assert!(other.resolution.content_hits > 0, "{:?}", other.resolution);
}

/// M4: a relative and an absolute invocation of the same tree key alike, so alternating them (a
/// manual run and a hook run) re-lints nothing.
#[test]
fn an_absolute_and_a_relative_invocation_share_records() {
    let project = located_project();
    settle();
    let (cold, relative_warm, absolute) = in_dir(project.dir.path(), |here| {
        let relative = || {
            let config = Config::load_file(Path::new("poly.toml")).unwrap();
            lint_at(Path::new("."), Path::new("."), &config, &Project::opts())
        };
        (relative(), relative(), lint_here(here))
    });

    assert_eq!(
        relative_warm.resolution.misses, 0,
        "the premise: the relative run is warm"
    );
    assert_eq!(absolute.findings, cold.findings);
    assert_eq!(absolute.resolution.misses, 0, "{:?}", absolute.resolution);
}

/// The same for `poly fmt` of a Rust file (rustfmt is in no lint plan): its per-file key reads the
/// `rustfmt.toml` and `rust-toolchain.toml` above the file, found alike from `./rs` and from the
/// absolute directory.
#[test]
fn an_absolute_and_a_relative_format_check_share_a_rust_files_record() {
    let project = Project::new("INP001");
    project.write("rs/a.rs", "pub fn a() -> u8 {\n    1\n}\n");
    project.write("rustfmt.toml", "max_width = 100\n");
    project.write("rust-toolchain.toml", "[toolchain]\nchannel = \"stable\"\n");
    git(project.dir.path(), &["init", "-q"]);
    settle();
    let (cold, relative_warm, absolute) = in_dir(project.dir.path(), |here| {
        let relative = || {
            let config = Config::load_file(Path::new("poly.toml")).unwrap();
            fmt_at(Path::new("."), Path::new("."), &config)
        };
        let config = Config::load_file(&here.join("poly.toml")).unwrap();
        (relative(), relative(), fmt_at(here, here, &config))
    });

    assert!(cold.errors.is_empty(), "{:?}", cold.errors);
    assert!(
        cold.changed.is_empty(),
        "the premise: formatted files, which are recorded"
    );
    assert!(cold.resolution.misses >= 4, "{:?}", cold.resolution);
    assert_eq!(relative_warm.resolution.misses, 0, "{:?}", relative_warm.resolution);
    assert!(absolute.changed.is_empty());
    assert_eq!(absolute.resolution.misses, 0, "{:?}", absolute.resolution);
}

/// The same for a Python file after a run started in another directory: a format plan resets no
/// run-scoped memo, so the directory a previous run of the process keyed package roots against must
/// not outlive it.
#[test]
fn an_absolute_format_check_after_a_run_elsewhere_shares_a_python_files_record() {
    let project = located_project();
    // A format run records no empty file, so the empty marker would miss whatever its key.
    project.write("pkg/__init__.py", "\"\"\"Package.\"\"\"\n");
    settle();
    let elsewhere = project.lint();
    assert!(elsewhere.errors.is_empty(), "{:?}", elsewhere.errors);
    let (cold, relative_warm, absolute) = in_dir(project.dir.path(), |here| {
        let relative = || {
            let config = Config::load_file(Path::new("poly.toml")).unwrap();
            fmt_at(Path::new("."), Path::new("."), &config)
        };
        let config = Config::load_file(&here.join("poly.toml")).unwrap();
        (relative(), relative(), fmt_at(here, here, &config))
    });

    assert!(cold.errors.is_empty(), "{:?}", cold.errors);
    assert!(
        cold.changed.is_empty(),
        "the premise: formatted files, which are recorded: {:?}",
        cold.changed
    );
    assert_eq!(relative_warm.resolution.misses, 0, "{:?}", relative_warm.resolution);
    assert_eq!(absolute.resolution.misses, 0, "{:?}", absolute.resolution);
}

/// M5: a run narrowed with `--only` keeps the other engines' records, so the next full run is
/// answered from the index instead of re-running every other engine.
#[test]
fn a_narrowed_run_does_not_evict_the_other_engines_records() {
    let project = located_project();
    settle();
    project.lint();
    let full = project.lint();
    assert_eq!(full.resolution.misses, 0, "the premise: a full warm run");

    let narrowed = || {
        lint_at(
            project.dir.path(),
            project.dir.path(),
            &project.config(),
            &RunOptions {
                only: vec!["ruff".to_owned()],
                skip: Vec::new(),
                ..Project::opts()
            },
        )
    };
    let first = narrowed();
    assert!(first.errors.is_empty(), "{:?}", first.errors);
    let again = project.lint();

    assert_eq!(again.findings, full.findings);
    assert_eq!(again.resolution.misses, 0, "{:?}", again.resolution);

    // The second identical narrowed run is answered by `stat` for every file, those the selection left
    // with no engine included, and the full run still is.
    let repeated = narrowed();
    assert_eq!(repeated.findings, first.findings);
    assert_eq!(
        repeated.resolution,
        Resolution {
            stat_hits: full.resolution.stat_hits,
            content_hits: 0,
            misses: 0
        },
        "a repeated narrowed run must be answered from the stat index alone"
    );
    assert_eq!(project.lint().resolution, full.resolution);
}
