//! A project check's verdict must not depend on how the run spelled its paths (ADR 0041): a bare
//! relative path, a `./` path, an absolute path, one file of the tree and any argument order all
//! judge the same tree from the same project root.
//!
//! Relative spellings only mean something against a working directory, so this binary holds a single
//! test and enters the project directory for its whole lifetime.
#![cfg(feature = "all-engines")]

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use poly_core::engine::{ProjectAnalysis, ProjectDirs, ProjectInputs, ProjectRequest, ProjectSideFile, Severity, Span};
use poly_core::runner::with_extra_project_engines;
use poly_core::{Capabilities, Config, Diagnostic, Engine, Language, LintRun, RunOptions, lint_run};

const ENGINE: &str = "log-check";
/// On every record sharing its four-digit number with another: a verdict that needs the whole log.
const DUPLICATE: &str = "LOG_DUPLICATE";
/// On every record the lock names that is in the log.
const LOCKED: &str = "LOG_LOCKED";
/// On every record the lock names that is not in the log.
const MISSING: &str = "LOG_MISSING";

/// Judges the Markdown under `log/` against `log.lock`, which names one record per line relative to
/// the project root: the shape of adrlint's numbering and lock rules.
struct LogCheck;

impl Engine for LogCheck {
    fn name(&self) -> &'static str {
        ENGINE
    }

    fn languages(&self) -> &'static [Language] {
        &[Language::Markdown]
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            project: true,
            ..Capabilities::default()
        }
    }

    fn version(&self) -> &str {
        "1"
    }

    fn project_analysis(&self) -> Option<&dyn ProjectAnalysis> {
        Some(self)
    }
}

impl ProjectAnalysis for LogCheck {
    fn project_inputs(&self) -> ProjectInputs {
        ProjectInputs {
            gate_key: None,
            files: vec![ProjectSideFile {
                option_key: "lock".to_owned(),
                default_path: "log.lock".to_owned(),
            }],
            dirs: Some(ProjectDirs {
                dir_key: "dir".to_owned(),
                dir_default: "log".to_owned(),
                paths_key: None,
                recursive_key: None,
                recursive_default: false,
                whole_tree: false,
                file_names: Vec::new(),
                list_unserved: false,
                narrows_to_targets: false,
            }),
        }
    }

    fn project_check(&self, req: &ProjectRequest<'_>) -> anyhow::Result<Vec<(PathBuf, Vec<Diagnostic>)>> {
        let relative = |path: &Path| path.strip_prefix(req.root).unwrap_or(path).to_path_buf();
        let records: Vec<_> = req.files.iter().filter(|f| f.language == Language::Markdown).collect();
        let locked: Vec<PathBuf> = req
            .files
            .iter()
            .filter(|f| matches!(f.language, Language::Other(_)))
            .flat_map(|f| f.content.lines().map(PathBuf::from))
            .collect();
        let number = |path: &Path| path.file_name().map(|name| name.to_string_lossy()[..4].to_owned());
        let mut reported: BTreeMap<PathBuf, Vec<Diagnostic>> = BTreeMap::new();
        for record in &records {
            let twins = records
                .iter()
                .filter(|other| number(&other.path) == number(&record.path))
                .count();
            if twins > 1 {
                reported
                    .entry(record.path.clone())
                    .or_default()
                    .push(finding(DUPLICATE));
            }
            if locked.contains(&relative(&record.path)) {
                reported.entry(record.path.clone()).or_default().push(finding(LOCKED));
            }
        }
        for gone in locked
            .iter()
            .filter(|path| !records.iter().any(|r| &relative(&r.path) == *path))
        {
            reported.entry(req.root.join(gone)).or_default().push(finding(MISSING));
        }
        Ok(reported.into_iter().collect())
    }
}

/// Reports on every Python file it is handed, under the path it was handed: an engine with no
/// directories of its own, served the run's files as the run spelled them (the shape of ty).
struct ServedCheck;

const SERVED_ENGINE: &str = "served-check";
const SERVED: &str = "SERVED";

impl Engine for ServedCheck {
    fn name(&self) -> &'static str {
        SERVED_ENGINE
    }

    fn languages(&self) -> &'static [Language] {
        &[Language::Python]
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            project: true,
            ..Capabilities::default()
        }
    }

    fn version(&self) -> &str {
        "1"
    }

    fn project_analysis(&self) -> Option<&dyn ProjectAnalysis> {
        Some(self)
    }
}

impl ProjectAnalysis for ServedCheck {
    fn project_inputs(&self) -> ProjectInputs {
        ProjectInputs::default()
    }

    fn project_check(&self, req: &ProjectRequest<'_>) -> anyhow::Result<Vec<(PathBuf, Vec<Diagnostic>)>> {
        Ok(req
            .files
            .iter()
            .filter(|file| file.language == Language::Python)
            .map(|file| {
                let mut found = finding(SERVED);
                found.engine = SERVED_ENGINE.to_owned();
                (file.path.clone(), vec![found])
            })
            .collect())
    }
}

/// The result paths of a run of `paths` that carry the served check's finding, sorted; a path
/// listed twice means the finding did not merge into the file's own result.
fn served(paths: &[&Path], cached: bool) -> Vec<PathBuf> {
    let mut served: Vec<PathBuf> = run(paths, cached)
        .results
        .iter()
        .filter(|result| result.diagnostics.iter().any(|d| d.engine == SERVED_ENGINE))
        .map(|result| result.path.clone())
        .collect();
    served.sort();
    served
}

fn finding(code: &str) -> Diagnostic {
    Diagnostic {
        engine: ENGINE.to_owned(),
        code: Some(code.to_owned()),
        severity: Severity::Error,
        title: code.to_owned(),
        description: None,
        span: Some(Span {
            start_line: 1,
            start_col: 1,
            end_line: 1,
            end_col: 1,
        }),
        url: None,
        fix: Vec::new(),
        metadata: BTreeMap::new(),
    }
}

fn write(root: &Path, name: &str, contents: &str) {
    let path = root.join(name);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
}

/// One run of `paths`; with `cached`, over the result cache, which a run of any spelling may have
/// filled (the entry is keyed by what the project holds, not by how the run named it).
fn run(paths: &[&Path], cached: bool) -> LintRun {
    let paths: Vec<PathBuf> = paths.iter().map(|path| path.to_path_buf()).collect();
    let options = RunOptions {
        jobs: Some(1),
        no_cache: !cached,
        ..RunOptions::default()
    };
    lint_run(&paths, &Config::default(), &options, false, false).unwrap()
}

/// `(result path, code)` of every finding of the log check, sorted.
fn found(run: &LintRun) -> Vec<(PathBuf, String)> {
    let mut found: Vec<_> = run
        .results
        .iter()
        .flat_map(|result| {
            result
                .diagnostics
                .iter()
                .filter(|d| d.engine == ENGINE)
                .map(|d| (result.path.clone(), d.code.clone().unwrap_or_default()))
        })
        .collect();
    found.sort();
    found
}

/// Records of the log, each with the codes expected on it.
type Records<'a> = &'a [(&'a str, &'a [&'a str])];

/// The paths a run names, the spelling it gives the log directory, and what it must report.
type Spelling<'a> = (&'a [&'a Path], &'a Path, Records<'a>);

/// The findings expected under `prefix`, the spelling the run gave the log directory.
fn expected(prefix: &Path, records: Records<'_>) -> Vec<(PathBuf, String)> {
    let mut expected: Vec<_> = records
        .iter()
        .flat_map(|(name, codes)| codes.iter().map(move |code| (prefix.join(name), (*code).to_owned())))
        .collect();
    expected.sort();
    expected
}

#[test]
fn the_verdict_is_the_same_however_the_run_spells_its_paths() {
    let project = tempfile::tempdir().unwrap();
    let cache_home = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(project.path()).unwrap();
    write(&root, "poly.toml", "[workspace]\nroot = true\n");
    write(&root, "README.md", "# readme\n");
    write(&root, "log/0000-a.md", "# a\n");
    write(&root, "log/0000-b.md", "# b\n");
    write(&root, "log.lock", "log/0000-a.md\n");
    write(&root, "log/m.py", "x = 1\n");
    std::env::set_current_dir(&root).unwrap();
    // SAFETY: the only test of this binary, so no other thread reads the environment.
    unsafe {
        std::env::set_var("POLY_CACHE_HOME", cache_home.path());
    }

    let whole: Records<'_> = &[("0000-a.md", &[DUPLICATE, LOCKED]), ("0000-b.md", &[DUPLICATE])];
    let one: Records<'_> = &[("0000-a.md", &[DUPLICATE, LOCKED])];
    let absolute = root.join("log");
    let absolute_readme = root.join("README.md");
    let (bare, dotted, readme) = (Path::new("log"), Path::new("./log"), Path::new("README.md"));
    let file = Path::new("log/0000-a.md");

    let dotted_file = Path::new("./log/0000-a.md");
    let absolute_file = absolute.join("0000-a.md");
    let runs: &[Spelling<'_>] = &[
        (&[bare], bare, whole),
        (&[dotted], dotted, whole),
        (&[&absolute], &absolute, whole),
        (&[Path::new(".")], dotted, whole),
        (&[readme, bare], bare, whole),
        (&[bare, readme], bare, whole),
        (&[readme, dotted], dotted, whole),
        (&[&absolute_readme, bare], bare, whole),
        (&[readme, &absolute], &absolute, whole),
        (&[file], bare, one),
        (&[readme, file], bare, one),
        (&[file, readme], bare, one),
        (&[dotted_file], dotted, one),
        (&[&absolute_file], &absolute, one),
    ];
    // From inside the log, where a relative path's ancestor chain never reaches the `poly.toml`.
    let (here, record, up) = (Path::new("."), Path::new("0000-a.md"), Path::new("../log"));
    let inside: &[Spelling<'_>] = &[
        (&[here], here, whole),
        (&[up], up, whole),
        (&[&absolute], &absolute, whole),
        (&[record], Path::new(""), one),
        (&[Path::new("./0000-a.md")], here, one),
        (&[Path::new("../README.md"), record], Path::new(""), one),
    ];
    let engine: Arc<dyn Engine> = Arc::new(LogCheck);
    let served_engine: Arc<dyn Engine> = Arc::new(ServedCheck);
    // Every spelling is run before any is judged, so a failure names all the ones that disagree.
    let disagreeing = |runs: &[Spelling<'_>]| -> Vec<String> {
        runs.iter()
            .filter_map(|(paths, prefix, records)| {
                let expected = expected(prefix, records);
                // Uncached, then cached twice: the second cached run is always a replay.
                let wrong: Vec<String> = [("uncached", false), ("cached", true), ("replayed", true)]
                    .into_iter()
                    .filter_map(|(how, cached)| {
                        let found = found(&run(paths, cached));
                        (found != expected).then(|| format!("poly lint {paths:?} ({how}) reported {found:?}"))
                    })
                    .collect();
                (!wrong.is_empty()).then(|| wrong.join("\n"))
            })
            .collect()
    };
    let (wrong, replays) = with_extra_project_engines(vec![engine, served_engine], || {
        let mut wrong = disagreeing(runs);
        std::env::set_current_dir(&absolute).unwrap();
        wrong.extend(disagreeing(inside).into_iter().map(|line| format!("in log/: {line}")));
        // A served file named relatively from below the project root: a replayed finding must come
        // back under the path a cold run reports it on, or it does not merge into the file's result.
        let module: &[&Path] = &[Path::new("./m.py")];
        let replays = [served(module, false), served(module, true), served(module, true)];
        (wrong, replays)
    });
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
    let [uncached, cached, replayed] = replays;
    assert_eq!(uncached.len(), 1, "{uncached:?}");
    assert_eq!(cached, uncached);
    assert_eq!(replayed, uncached);
}
