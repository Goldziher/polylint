//! Link and anchor checking in the rumdl backend: what the per-file pass already reports
//! (`MD057`, same-file `MD051`), and the opt-in cross-file `MD051` project check, first driven
//! through the engine and then through the pipeline to prove the cached result follows the target file.
#![allow(unsafe_code, reason = "POLY_CACHE_HOME is set once, before any thread reads it")]

use std::fs;
use std::path::{Path, PathBuf};

use poly_engine_api::config::{EngineConfig, GlobalDefaults};
use poly_engine_api::engine::{Engine, ProjectAnalysis, ProjectEnvironment, ProjectFile, ProjectRequest, SourceFile};
use poly_engine_api::language::Language;
use poly_engines_web::rumdl::RumdlEngine;

mod common;

fn fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/rumdl/links")
}

fn cfg(extra: &[(&str, toml::Value)]) -> EngineConfig {
    let mut options = toml::Table::new();
    for (key, value) in extra {
        options.insert((*key).to_string(), value.clone());
    }
    EngineConfig {
        globals: GlobalDefaults::default(),
        indent_width: 4,
        options,
    }
}

fn enabled() -> EngineConfig {
    cfg(&[("cross_file_links", toml::Value::Boolean(true))])
}

fn markdown_files(root: &Path) -> Vec<ProjectFile> {
    let mut files: Vec<ProjectFile> = fs::read_dir(root)
        .expect("read dir")
        .map(|entry| entry.expect("entry").path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "md"))
        .map(|path| ProjectFile {
            content: fs::read_to_string(&path).expect("read").into(),
            path,
            language: Language::Markdown,
        })
        .collect();
    files.sort_by(|a, b| a.path.cmp(&b.path));
    files
}

/// `(file name, code, line)` of every project-check finding.
fn project(root: &Path, config: &EngineConfig) -> Vec<(String, String, u32)> {
    let files = markdown_files(root);
    let environment = ProjectEnvironment::default();
    let request = ProjectRequest {
        root,
        files: &files,
        config,
        environment: &environment,
        threads: None,
        targets: None,
    };
    let mut found: Vec<(String, String, u32)> = RumdlEngine
        .project_check(&request)
        .expect("project check")
        .into_iter()
        .flat_map(|(path, diagnostics)| {
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or_default().to_owned();
            diagnostics.into_iter().map(move |d| {
                (
                    name.clone(),
                    d.code.unwrap_or_default(),
                    d.span.map_or(0, |span| span.start_line),
                )
            })
        })
        .collect();
    found.sort();
    found
}

fn lint_index(config: &EngineConfig) -> Vec<(String, u32)> {
    let path = fixture().join("index.md");
    let src = SourceFile {
        content: fs::read_to_string(&path).expect("read").into(),
        path,
        language: Language::Markdown,
    };
    RumdlEngine
        .lint(&src, config)
        .expect("lint")
        .into_iter()
        .map(|d| (d.code.unwrap_or_default(), d.span.map_or(0, |span| span.start_line)))
        .collect()
}

#[test]
fn the_per_file_pass_reports_a_missing_file_and_a_missing_local_anchor_but_no_cross_file_anchor() {
    let found = lint_index(&cfg(&[]));
    assert!(
        found.contains(&("MD057".to_owned(), 5)),
        "broken relative link: {found:?}"
    );
    assert!(
        found.contains(&("MD051".to_owned(), 6)),
        "missing same-file anchor: {found:?}"
    );
    assert!(
        !found.iter().any(|(code, line)| code == "MD051" && *line == 4),
        "the cross-file anchor is not a per-file finding: {found:?}"
    );
}

#[test]
fn a_missing_cross_file_anchor_is_reported_by_the_project_check() {
    assert_eq!(
        project(&fixture(), &enabled()),
        [("index.md".to_owned(), "MD051".to_owned(), 4)]
    );
}

#[test]
fn the_project_check_is_off_unless_enabled() {
    assert_eq!(project(&fixture(), &cfg(&[])), []);
    let off = cfg(&[("cross_file_links", toml::Value::Boolean(false))]);
    assert_eq!(project(&fixture(), &off), []);
}

#[test]
fn ignoring_md051_silences_the_project_check() {
    let ignore = toml::Value::Array(vec!["MD051".into()]);
    let config = cfg(&[("cross_file_links", toml::Value::Boolean(true)), ("ignore", ignore)]);
    assert_eq!(project(&fixture(), &config), []);
}

fn run(root: &Path) -> Vec<(String, u32)> {
    common::register();
    let config = poly_core::Config::default();
    let options = poly_core::RunOptions {
        jobs: Some(1),
        ..poly_core::RunOptions::default()
    };
    let run = poly_core::lint_run(&[root.to_path_buf()], &config, &options, false, false).expect("lint run");
    assert!(run.errors.is_empty(), "{:?}", run.errors);
    let mut found: Vec<(String, u32)> = run
        .results
        .iter()
        .flat_map(|result| {
            let name = result
                .path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or_default()
                .to_owned();
            result
                .diagnostics
                .iter()
                .filter(|d| d.engine == "rumdl" && d.code.as_deref() == Some("MD051"))
                .map(move |d| (name.clone(), d.span.map_or(0, |span| span.start_line)))
        })
        .collect();
    found.sort();
    found
}

/// Editing the target file must re-run the check: the result is cached on every Markdown file the
/// check saw, so a stale entry would keep reporting the old verdict.
#[test]
fn editing_the_target_file_invalidates_the_cached_result() {
    static CACHE_HOME: std::sync::OnceLock<tempfile::TempDir> = std::sync::OnceLock::new();
    CACHE_HOME.get_or_init(|| {
        let dir = tempfile::tempdir().expect("cache dir");
        // SAFETY: set once, from the `OnceLock` initialiser, before any run in this binary reads it.
        unsafe {
            std::env::set_var("POLY_CACHE_HOME", dir.path());
        }
        dir
    });
    let project = tempfile::tempdir().expect("project");
    fs::write(
        project.path().join("poly.toml"),
        "[workspace]\nroot = true\n\n[engines.rumdl]\ncross_file_links = true\n",
    )
    .expect("write config");
    fs::write(
        project.path().join("index.md"),
        "# Index\n\n[Guide](guide.md#install)\n",
    )
    .expect("write");
    fs::write(project.path().join("guide.md"), "# Guide\n\n## Setup\n").expect("write");

    assert_eq!(
        run(project.path()),
        [("index.md".to_owned(), 3)],
        "the anchor is missing"
    );
    assert_eq!(
        run(project.path()),
        [("index.md".to_owned(), 3)],
        "and stays reported from the cache"
    );

    fs::write(project.path().join("guide.md"), "# Guide\n\n## Install\n").expect("edit target");
    assert_eq!(
        run(project.path()),
        [],
        "adding the heading to the target clears the finding"
    );

    fs::write(project.path().join("guide.md"), "# Guide\n\n## Setup\n").expect("edit target back");
    assert_eq!(
        run(project.path()),
        [("index.md".to_owned(), 3)],
        "removing it brings the finding back"
    );
}
