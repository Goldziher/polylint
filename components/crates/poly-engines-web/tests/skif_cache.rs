//! skif's own cache in poly's result cache (ADR 0080): a second identical run serves skif's module parses from the
//! store, a run without a cache uses no store, and a change to a result-affecting engine option or the host's salt
//! misses while a `threads` change hits.
#![allow(
    unsafe_code,
    reason = "POLY_CACHE_HOME is set once, before any run in this binary reads it"
)]

use std::path::{Path, PathBuf};

use poly_component_protocol::{
    PROTOCOL_VERSION, Payload, PolyComponent, Request, WireFileSlice, WireLogLevel, decode_response, encode_request,
};
use poly_engine_api::component_wire::engine_config_to_wire;
use poly_engine_api::config::{EngineConfig, GlobalDefaults};
use poly_engine_api::engine::{EngineCache, ProjectEnvironment, ProjectFile, ProjectRequest};
use poly_engine_api::language::Language;
use poly_engines_web::WebComponent;
use poly_engines_web::skif::{Analysis, analyze};
use skif::cache::CacheCounters;

mod common;

/// A pnpm workspace with three modules for skif to parse: two in one package, one in the other.
const WORKSPACE: &[(&str, &str)] = &[
    (
        "package.json",
        r#"{"name":"r","version":"1.0.0","private":true,"packageManager":"pnpm@9.0.0"}"#,
    ),
    ("pnpm-workspace.yaml", "packages:\n  - 'packages/*'\n"),
    (
        "packages/a/package.json",
        r#"{"name":"a","version":"1.0.0","main":"src/index.ts"}"#,
    ),
    (
        "packages/a/src/index.ts",
        "import { util } from './util';\nexport const a = util + 1;\n",
    ),
    ("packages/a/src/util.ts", "export const util = 1;\n"),
    (
        "packages/b/package.json",
        r#"{"name":"b","version":"1.0.0","main":"index.js"}"#,
    ),
    ("packages/b/index.js", "module.exports = 2;\n"),
];

fn language_of(path: &str) -> Language {
    match Path::new(path).extension().and_then(|ext| ext.to_str()) {
        Some("json") => Language::Json,
        Some("yaml") => Language::Yaml,
        Some("ts") => Language::TypeScript,
        Some("js") => Language::JavaScript,
        other => panic!("no language for {other:?}"),
    }
}

fn files(root: &Path) -> Vec<ProjectFile> {
    WORKSPACE
        .iter()
        .map(|(path, content)| ProjectFile {
            path: root.join(path),
            language: language_of(path),
            content: (*content).into(),
        })
        .collect()
}

fn cfg(options: &str) -> EngineConfig {
    EngineConfig {
        globals: GlobalDefaults::default(),
        indent_width: 2,
        options: toml::from_str(&format!("enabled = true\nthreads = 1\n{options}")).expect("options"),
    }
}

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("virtual-workspace")
}

fn run(config: &EngineConfig, cache: Option<&EngineCache>) -> Analysis {
    let root = root();
    let files = files(&root);
    let environment = ProjectEnvironment::default();
    analyze(&ProjectRequest {
        root: &root,
        files: &files,
        config,
        environment: &environment,
        threads: None,
        targets: None,
        cache,
    })
    .expect("skif runs")
}

fn modules(analysis: &Analysis) -> CacheCounters {
    analysis.cache.get("modules").copied().unwrap_or_default()
}

/// `(path, rule)` of every finding, to compare runs.
fn findings(analysis: &Analysis) -> Vec<(PathBuf, Option<String>)> {
    analysis
        .findings
        .iter()
        .flat_map(|(path, found)| found.iter().map(move |d| (path.clone(), d.code.clone())))
        .collect()
}

/// The entries in the engine namespace of the result cache rooted at `dir`.
fn engine_entries(dir: &Path) -> usize {
    std::fs::read_dir(dir.join("results/engine"))
        .map(|entries| {
            entries
                .flatten()
                .filter(|entry| !entry.file_name().to_string_lossy().starts_with('.'))
                .count()
        })
        .unwrap_or(0)
}

fn cache_in(dir: &Path, salt: &str) -> EngineCache {
    EngineCache {
        dir: dir.to_path_buf(),
        salt: salt.to_owned(),
    }
}

#[test]
fn a_second_identical_run_serves_every_module_from_the_store() {
    let dir = tempfile::tempdir().unwrap();
    let cache = cache_in(dir.path(), "host");
    let config = cfg("");

    let cold = run(&config, Some(&cache));
    let cold_modules = modules(&cold);
    assert_eq!(cold_modules.hits, 0, "{cold_modules:?}");
    assert!(cold_modules.misses >= 3, "every module is parsed: {cold_modules:?}");
    assert!(cold_modules.bytes_written > 0, "{cold_modules:?}");
    assert_eq!(
        engine_entries(dir.path()),
        usize::try_from(cold_modules.misses).unwrap()
    );

    let warm = run(&config, Some(&cache));
    let warm_modules = modules(&warm);
    assert_eq!(
        (warm_modules.hits, warm_modules.misses, warm_modules.bytes_written),
        (cold_modules.misses, 0, 0),
        "the warm run parses nothing"
    );
    assert_eq!(findings(&warm), findings(&cold), "the store never changes a finding");
    assert_eq!(findings(&warm), findings(&run(&config, None)));
}

#[test]
fn a_run_without_a_cache_uses_no_store() {
    let dir = tempfile::tempdir().unwrap();
    let analysis = run(&cfg(""), None);
    assert!(analysis.cache.is_empty(), "{:?}", analysis.cache);
    assert_eq!(engine_entries(dir.path()), 0);
}

#[test]
fn a_change_to_a_result_option_or_the_hosts_salt_misses_and_threads_hits() {
    let dir = tempfile::tempdir().unwrap();
    let cache = cache_in(dir.path(), "host");
    let first = modules(&run(&cfg(""), Some(&cache)));
    assert!(first.misses > 0);

    let reconfigured = modules(&run(&cfg("ignore_packages = [\"unrelated\"]"), Some(&cache)));
    assert_eq!(
        (reconfigured.hits, reconfigured.misses),
        (0, first.misses),
        "options are in the salt"
    );

    let other_host = modules(&run(&cfg(""), Some(&cache_in(dir.path(), "another build"))));
    assert_eq!(
        (other_host.hits, other_host.misses),
        (0, first.misses),
        "the host's salt is in every key"
    );

    let mut more_threads = cfg("");
    more_threads
        .options
        .insert("threads".to_owned(), toml::Value::Integer(4));
    let rethreaded = modules(&run(&more_threads, Some(&cache)));
    assert_eq!(
        (rethreaded.hits, rethreaded.misses),
        (first.misses, 0),
        "threads changes no result, so it is not in the salt"
    );

    let again = modules(&run(&cfg(""), Some(&cache)));
    assert_eq!(
        (again.hits, again.misses),
        (first.misses, 0),
        "the first entries are still there"
    );
}

fn call(component: &WebComponent, request: &Request, body: &[u8]) -> Payload {
    let header = encode_request(request).expect("encode");
    let bytes = component.call(&header, body).expect("call");
    decode_response(&bytes)
        .expect("decode")
        .result
        .expect("the call succeeds")
}

/// The core build's path: the host's cache reaches skif inside the component as two environment pairs.
#[test]
fn the_component_hands_skif_the_cache_the_host_sends() {
    let component = WebComponent::default();
    let init = Request::Init {
        protocol: PROTOCOL_VERSION,
        host_version: "test".into(),
        threads: 1,
        stack_size: 16 << 20,
        max_log_level: WireLogLevel::Warn,
    };
    assert!(matches!(call(&component, &init, &[]), Payload::Init { .. }));
    let register = Request::RegisterConfig {
        id: 1,
        config: engine_config_to_wire(&cfg("")),
    };
    call(&component, &register, &[]);

    let mut body = Vec::new();
    let slices: Vec<WireFileSlice> = WORKSPACE
        .iter()
        .map(|(path, content)| {
            let slice = WireFileSlice {
                path: (*path).to_owned(),
                language: language_of(path).id().to_owned(),
                offset: body.len() as u64,
                len: content.len() as u64,
            };
            body.extend_from_slice(content.as_bytes());
            slice
        })
        .collect();
    let dir = tempfile::tempdir().unwrap();
    let check = |environment: Vec<(String, String)>| Request::ProjectCheck {
        engine: "skif".into(),
        root: root().display().to_string(),
        files: slices.clone(),
        config: 1,
        environment,
        threads: Some(1),
        targets: None,
    };

    call(&component, &check(Vec::new()), &body);
    assert_eq!(
        engine_entries(dir.path()),
        0,
        "a request without the pairs caches nothing"
    );

    let pairs = cache_in(dir.path(), "host").to_wire_pairs().to_vec();
    call(&component, &check(pairs), &body);
    assert!(engine_entries(dir.path()) >= 3, "skif stored its module parses");
}

/// Drives `poly_core::lint_run` over the workspace on disk: `--no-cache` hands skif no store, a caching run does.
#[test]
fn the_pipeline_hands_skif_the_runs_cache_unless_it_is_off() {
    let cache_home = tempfile::tempdir().unwrap();
    // SAFETY: the only test of this binary that reads the environment, and nothing in it runs yet.
    unsafe {
        std::env::set_var("POLY_CACHE_HOME", cache_home.path());
    }
    common::register();
    let project = tempfile::tempdir().unwrap();
    for (path, content) in WORKSPACE {
        let path = project.path().join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, content).unwrap();
    }
    std::fs::write(
        project.path().join("poly.toml"),
        "[workspace]\nroot = true\n\n[engines.skif]\nenabled = true\n",
    )
    .unwrap();
    let lint = |no_cache: bool| {
        let options = poly_core::RunOptions {
            jobs: Some(1),
            no_cache,
            ..poly_core::RunOptions::default()
        };
        let run = poly_core::lint_run(
            &[project.path().to_path_buf()],
            &poly_core::Config::default(),
            &options,
            false,
            false,
        )
        .expect("lint run");
        assert!(run.errors.is_empty(), "{:?}", run.errors);
    };
    let stored = || {
        walk(cache_home.path())
            .into_iter()
            .filter(|path| path.parent().is_some_and(|parent| parent.ends_with("results/engine")))
            .count()
    };

    lint(true);
    assert_eq!(stored(), 0, "--no-cache gives skif no store");
    lint(false);
    assert!(
        stored() >= 3,
        "a caching run stores skif's module parses in the result cache"
    );
}

fn walk(dir: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut pending = vec![dir.to_path_buf()];
    while let Some(dir) = pending.pop() {
        for entry in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
            let path = entry.path();
            if path.is_dir() {
                pending.push(path);
            } else if !entry.file_name().to_string_lossy().starts_with('.') {
                found.push(path);
            }
        }
    }
    found
}
