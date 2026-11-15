//! The tsrs project check driven through the engine, with the files loaded the way the host hands them over.
//!
//! The project root never exists on disk: a finding proves the engine built its program from the request alone.

use std::path::{Path, PathBuf};

use poly_engine_api::config::{EngineConfig, GlobalDefaults};
use poly_engine_api::engine::{Diagnostic, Engine, ProjectEnvironment, ProjectFile, ProjectRequest, Severity};
use poly_engine_api::language::Language;
use poly_engines_tsrs::TsrsEngine;

type Findings = Vec<(PathBuf, Vec<Diagnostic>)>;

fn root() -> PathBuf {
    std::env::temp_dir().join("poly-tsrs-hermetic-project")
}

fn tsconfig(extra: &str) -> String {
    format!(
        r#"{{ "compilerOptions": {{ "strict": true, "target": "es2022", "module": "esnext", "moduleResolution": "bundler"{extra} }}, "include": ["src"] }}"#
    )
}

/// The files the host would hand over, under [`root`]; the configs and manifests as `Language::Other(name)`.
fn project(files: &[(&str, &str)]) -> Vec<ProjectFile> {
    files
        .iter()
        .map(|(name, content)| {
            let path = root().join(name);
            let file_name = path
                .file_name()
                .and_then(|n| n.to_str())
                .expect("a file name")
                .to_owned();
            let language = match file_name.as_str() {
                "tsconfig.json" | "package.json" => Language::Other(file_name.clone()),
                _ => match path.extension().and_then(|e| e.to_str()).expect("an extension") {
                    "ts" => Language::TypeScript,
                    "tsx" => Language::Tsx,
                    "js" => Language::JavaScript,
                    "json" => Language::Json,
                    other => panic!("no language for .{other}"),
                },
            };
            ProjectFile {
                path,
                language,
                content: (*content).into(),
            }
        })
        .collect()
}

fn options(extra: &str) -> EngineConfig {
    EngineConfig {
        globals: GlobalDefaults::default(),
        indent_width: 2,
        options: toml::from_str(&format!("enabled = true\n{extra}")).expect("options"),
    }
}

fn run(files: &[ProjectFile], config: &EngineConfig, threads: Option<usize>) -> anyhow::Result<Findings> {
    TsrsEngine.project_check(&ProjectRequest {
        root: &root(),
        files,
        config,
        environment: &ProjectEnvironment::default(),
        threads,
    })
}

fn check(files: &[(&str, &str)], extra: &str) -> Findings {
    run(&project(files), &options(extra), Some(1)).expect("tsrs runs")
}

fn slashed(path: &Path) -> String {
    path.strip_prefix(root())
        .expect("inside the root")
        .components()
        .map(|part| part.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

/// `(path relative to the root, code, start line)` of every finding, sorted.
fn triples(found: &Findings) -> Vec<(String, String, u32)> {
    let mut out: Vec<_> = found
        .iter()
        .flat_map(|(path, diagnostics)| {
            diagnostics.iter().map(move |d| {
                (
                    slashed(path),
                    d.code.clone().expect("a code"),
                    d.span.expect("a span").start_line,
                )
            })
        })
        .collect();
    out.sort();
    out
}

fn triple(path: &str, code: &str, line: u32) -> (String, String, u32) {
    (path.to_owned(), code.to_owned(), line)
}

const KNOWN_BAD: &str = "export const a: number = \"x\";
export interface P { name: string }
export function f(p: P): string { return p.missing; }
export function two(a: number, b: number): number { return a + b; }
export const r = two(1);
import { gone } from \"./nowhere\";
export const g = gone;
";

#[test]
fn a_known_bad_project_has_exactly_these_findings() {
    assert!(!root().exists(), "the project must exist only in the request");
    let found = check(&[("tsconfig.json", &tsconfig("")), ("src/a.ts", KNOWN_BAD)], "");
    assert_eq!(
        triples(&found),
        [
            triple("src/a.ts", "TS2307", 6),
            triple("src/a.ts", "TS2322", 1),
            triple("src/a.ts", "TS2339", 3),
            triple("src/a.ts", "TS2554", 5),
        ]
    );
    let all: Vec<&Diagnostic> = found.iter().flat_map(|(_, d)| d).collect();
    assert!(all.iter().all(|d| d.engine == "tsrs" && d.severity == Severity::Error));
    let mismatch = all
        .iter()
        .find(|d| d.code.as_deref() == Some("TS2322"))
        .expect("TS2322");
    assert!(mismatch.title.contains("not assignable"), "{}", mismatch.title);
}

#[test]
fn a_missing_import_of_a_relative_file_is_an_error_across_files() {
    let found = check(
        &[
            ("tsconfig.json", &tsconfig("")),
            ("src/util.ts", "export const util = 1;\n"),
            (
                "src/main.ts",
                "import { util } from \"./util\";\nimport { nope } from \"./nope\";\nexport const v = [util, nope];\n",
            ),
        ],
        "",
    );
    assert_eq!(triples(&found), [triple("src/main.ts", "TS2307", 2)]);
}

#[test]
fn a_known_clean_project_with_a_paths_alias_has_no_findings() {
    let config = tsconfig(r#", "paths": { "@lib/*": ["./src/lib/*"] }"#);
    let files = [
        ("tsconfig.json", config.as_str()),
        (
            "src/lib/util.ts",
            "export function add(a: number, b: number): number { return a + b; }\n",
        ),
        (
            "src/main.ts",
            "import { add } from \"@lib/util\";\nexport const total: number = add(1, 2);\n",
        ),
        ("src/index.ts", "export * from \"./main\";\n"),
    ];
    assert_eq!(triples(&check(&files, "")), []);

    let broken = [
        files[0],
        files[1],
        (
            "src/main.ts",
            "import { add } from \"@nolib/util\";\nexport const total: number = add(1, 2);\n",
        ),
    ];
    assert_eq!(
        triples(&check(&broken, "")),
        [triple("src/main.ts", "TS2307", 1)],
        "the control: an alias that maps nowhere is an error, so the clean run is not an empty check"
    );
}

#[test]
fn strict_follows_the_tsconfig_and_the_option_overrides_it() {
    let implicit_any = "export function id(x) { return x; }\n";
    let strict = tsconfig("");
    let lax = strict.replace("\"strict\": true", "\"strict\": false");
    let bad = [triple("src/s.ts", "TS7006", 1)];
    assert_eq!(
        triples(&check(&[("tsconfig.json", &strict), ("src/s.ts", implicit_any)], "")),
        bad
    );
    assert_eq!(
        triples(&check(&[("tsconfig.json", &lax), ("src/s.ts", implicit_any)], "")),
        []
    );
    assert_eq!(
        triples(&check(
            &[("tsconfig.json", &strict), ("src/s.ts", implicit_any)],
            "strict = false"
        )),
        []
    );
    assert_eq!(
        triples(&check(
            &[("tsconfig.json", &lax), ("src/s.ts", implicit_any)],
            "strict = true"
        )),
        bad
    );
}

#[test]
fn a_declared_package_that_is_not_installed_is_dropped_unless_asked_for() {
    let files = [
        ("tsconfig.json", tsconfig("")),
        (
            "package.json",
            r#"{ "dependencies": { "left-pad": "1.0.0" } }"#.to_owned(),
        ),
        (
            "src/b.ts",
            "import pad from \"left-pad\";\nimport other from \"undeclared-pkg\";\nexport const v = [pad, other];\n"
                .to_owned(),
        ),
    ];
    let files: Vec<(&str, &str)> = files.iter().map(|(n, c)| (*n, c.as_str())).collect();
    assert_eq!(
        triples(&check(&files, "")),
        [triple("src/b.ts", "TS2307", 2)],
        "only the package the project does not declare"
    );
    assert_eq!(
        triples(&check(&files, "report_unresolved_modules = true")),
        [triple("src/b.ts", "TS2307", 1), triple("src/b.ts", "TS2307", 2)]
    );
}

#[test]
fn a_projects_own_declaration_files_are_checked_and_reported() {
    let files = [
        ("tsconfig.json", tsconfig("")),
        (
            "src/global.d.ts",
            "declare const BUILD: string;\ndeclare const BAD: Missing;\n".to_owned(),
        ),
        ("src/c.ts", "export const n: number = BUILD;\n".to_owned()),
        (
            "node_modules/dep/index.d.ts",
            "declare const THEIRS: Missing;\nexport {};\n".to_owned(),
        ),
    ];
    let files: Vec<(&str, &str)> = files.iter().map(|(n, c)| (*n, c.as_str())).collect();
    assert_eq!(
        triples(&check(&files, "")),
        [triple("src/c.ts", "TS2322", 1), triple("src/global.d.ts", "TS2304", 2)],
        "BUILD comes from the declaration file, whose own error is reported; node_modules is not"
    );
    let skipped = tsconfig(r#", "skipLibCheck": true"#);
    let mut lib_skipped = files.clone();
    lib_skipped[0] = ("tsconfig.json", skipped.as_str());
    assert_eq!(
        triples(&check(&lib_skipped, "")),
        [triple("src/c.ts", "TS2322", 1)],
        "skipLibCheck skips declaration files, as in tsc"
    );
}

fn manifest(dependencies: &str) -> String {
    format!(r#"{{ "dependencies": {{ {dependencies} }} }}"#)
}

fn extending(spec: &str) -> String {
    format!(
        r#"{{ "extends": "{spec}", "compilerOptions": {{ "target": "es2022", "module": "esnext", "moduleResolution": "bundler" }}, "include": ["src"] }}"#
    )
}

#[test]
fn an_extended_package_the_project_declares_is_skipped_with_a_warning() {
    let config = extending("@tsconfig/node20/tsconfig.json");
    let package = manifest(r#""@tsconfig/node20": "1.0.0""#);
    let found = check(
        &[
            ("tsconfig.json", &config),
            ("package.json", &package),
            ("src/a.ts", "export const a: number = \"x\";\n"),
        ],
        "",
    );
    let config_findings: Vec<&Diagnostic> = found
        .iter()
        .filter(|(path, _)| slashed(path) == "tsconfig.json")
        .flat_map(|(_, d)| d)
        .collect();
    assert_eq!(config_findings.len(), 1, "{found:?}");
    assert_eq!(config_findings[0].severity, Severity::Warning);
    assert!(
        config_findings[0].title.contains("@tsconfig/node20/tsconfig.json")
            && config_findings[0].title.contains("not available without node_modules"),
        "{}",
        config_findings[0].title
    );
    let sources: Vec<_> = triples(&found).into_iter().filter(|t| t.0 == "src/a.ts").collect();
    assert_eq!(
        sources,
        [triple("src/a.ts", "TS2322", 1)],
        "the project is still checked"
    );
}

#[test]
fn an_extended_package_nothing_declares_stays_an_error() {
    let config = extending("@tsconfig/node20/tsconfig.json");
    let other = manifest(r#""left-pad": "1.0.0""#);
    let found = check(
        &[
            ("tsconfig.json", &config),
            ("package.json", &other),
            ("src/a.ts", "export const a: number = \"x\";\n"),
        ],
        "",
    );
    assert_eq!(
        triples(&found),
        [triple("src/a.ts", "TS2322", 1), triple("tsconfig.json", "TS6053", 1)],
        "the unresolved extends is reported and the project is still checked"
    );
    let diagnostic = found
        .iter()
        .flat_map(|(_, diagnostics)| diagnostics)
        .find(|d| d.code.as_deref() == Some("TS6053"))
        .expect("the extends error");
    assert_eq!(diagnostic.severity, Severity::Error);
}

#[test]
fn declared_packages_are_scoped_to_the_package_that_imports_them() {
    let config = r#"{ "compilerOptions": { "target": "es2022", "module": "esnext", "moduleResolution": "bundler" }, "include": ["packages"] }"#;
    let a = manifest(r#""only-a": "1.0.0""#);
    let b = manifest(r#""only-b": "1.0.0", "@w/a": "workspace:*""#);
    let found = check(
        &[
            ("tsconfig.json", config),
            ("packages/a/package.json", &a),
            ("packages/b/package.json", &b),
            (
                "packages/a/src/a.ts",
                "import x from \"only-b\";\nimport y from \"only-a\";\nexport const v = [x, y];\n",
            ),
            (
                "packages/b/src/b.ts",
                "import x from \"only-b\";\nimport y from \"only-a\";\nimport z from \"@w/a\";\nexport const v = [x, y, z];\n",
            ),
        ],
        "",
    );
    assert_eq!(
        triples(&found),
        [
            triple("packages/a/src/a.ts", "TS2307", 1),
            triple("packages/b/src/b.ts", "TS2307", 2),
        ],
        "each package sees its own dependencies; a workspace dependency is declared"
    );
}

#[test]
fn react_jsx_findings_are_dropped_only_when_react_is_declared() {
    let config = tsconfig(r#", "jsx": "react-jsx""#);
    let app = "export const App = () => <div className=\"a\" />;\n";
    let without = check(&[("tsconfig.json", &config), ("src/app.tsx", app)], "");
    assert!(
        !without.is_empty(),
        "the control: without react in a manifest the JSX findings stay"
    );
    let package = manifest(r#""react": "19.0.0""#);
    let with = check(
        &[
            ("tsconfig.json", &config),
            ("package.json", &package),
            ("src/app.tsx", app),
        ],
        "",
    );
    assert_eq!(triples(&with), []);
    let no_jsx = check(
        &[
            ("tsconfig.json", &tsconfig("")),
            ("package.json", &package),
            ("src/app.tsx", app),
        ],
        "",
    );
    assert!(!no_jsx.is_empty(), "without the jsx option the findings are real");
}

#[test]
fn javascript_is_reported_only_when_the_program_holds_it() {
    let js = "/** @type {number} */\nconst x = \"s\";\nexport { x };\n";
    let off = check(&[("tsconfig.json", &tsconfig("")), ("src/j.js", js)], "");
    assert!(
        off.iter().all(|(path, _)| slashed(path) != "src/j.js"),
        "the JavaScript file is outside the program, so it has no findings: {off:?}"
    );
    let on = check(
        &[
            ("tsconfig.json", &tsconfig(r#", "allowJs": true, "checkJs": true"#)),
            ("src/j.js", js),
        ],
        "",
    );
    assert_eq!(triples(&on), [triple("src/j.js", "TS2322", 2)]);
}

#[test]
fn a_broken_tsconfig_is_reported_on_the_tsconfig() {
    let found = check(
        &[
            ("tsconfig.json", "{ \"compilerOptions\": { \"strict\": } }"),
            ("src/a.ts", "export {};\n"),
        ],
        "",
    );
    assert!(!found.is_empty(), "a config error is a finding");
    assert!(
        found.iter().all(|(path, _)| slashed(path) == "tsconfig.json"),
        "{found:?}"
    );
}

#[test]
fn findings_do_not_depend_on_the_thread_count() {
    let files = project(&[("tsconfig.json", &tsconfig("")), ("src/a.ts", KNOWN_BAD)]);
    let render = |threads| format!("{:?}", run(&files, &options(""), Some(threads)).expect("tsrs runs"));
    let one = render(1);
    assert_eq!(render(2), one);
    assert_eq!(render(4), one);
    let with_option = format!("{:?}", run(&files, &options("threads = 3"), None).expect("tsrs runs"));
    assert_eq!(with_option, one);
}

#[test]
fn the_engine_does_nothing_until_it_is_enabled() {
    let files = project(&[("tsconfig.json", &tsconfig("")), ("src/a.ts", KNOWN_BAD)]);
    for text in ["", "enabled = false"] {
        let config = EngineConfig {
            globals: GlobalDefaults::default(),
            indent_width: 2,
            options: toml::from_str(text).expect("options"),
        };
        assert!(run(&files, &config, None).expect("runs").is_empty(), "`{text}`");
    }
}

#[test]
fn a_project_without_a_tsconfig_has_nothing_to_check() {
    assert!(check(&[("src/a.ts", KNOWN_BAD)], "").is_empty());
}

#[test]
fn a_named_project_must_be_among_the_files_and_picks_that_config() {
    let files = project(&[("tsconfig.json", &tsconfig("")), ("src/a.ts", KNOWN_BAD)]);
    let error = run(&files, &options("project = \"missing.json\""), None).expect_err("not among the files");
    assert!(format!("{error:#}").contains("missing.json"), "{error:#}");

    let named = check(
        &[
            ("configs/ts.json", &tsconfig("").replace("\"src\"", "\"../src\"")),
            ("src/a.ts", KNOWN_BAD),
        ],
        "project = \"configs/ts.json\"",
    );
    assert_eq!(named.len(), 1);
}

#[test]
fn a_wrong_option_type_is_an_error() {
    let files = project(&[("tsconfig.json", &tsconfig(""))]);
    let error = run(&files, &options("strict = \"yes\""), None).expect_err("a string is not a boolean");
    assert!(format!("{error:#}").contains("strict"), "{error:#}");
}

#[test]
fn the_engine_describes_a_gated_whole_tree_project_check() {
    let engine = TsrsEngine;
    assert_eq!(engine.name(), "tsrs");
    let capabilities = engine.capabilities();
    assert!(capabilities.project && !capabilities.lint && !capabilities.format && !capabilities.fix);
    let inputs = engine.project_inputs();
    assert_eq!(inputs.gate_key.as_deref(), Some("enabled"));
    let dirs = inputs.dirs.expect("directories");
    assert!(dirs.whole_tree && dirs.recursive_default);
    assert_eq!(dirs.dir_key, "dir");
    assert_eq!(dirs.file_names, ["tsconfig.json", "jsconfig.json", "package.json"]);
}
