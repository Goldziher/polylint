//! Program discovery and coverage: each judged file belongs to its nearest `tsconfig.json`, solution
//! configs and project references are followed, and a package without usable types is reported, not failed.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::sync::atomic::AtomicBool;

use common::{
    Expected, Options, TSCONFIG, Workspace, abs, assert_case, assert_facts, coverage, package, query, run, try_run,
    typed,
};
use polylint_js_types::{CoverageReason, CoverageState, DeclKind, ENGINE, TypeError, TypeQuery};

const WIDGET: &str = "\
export class Widget {
  render(): void {}
  paint(): void {}
}
";

const MAIN: &str = "import { Widget } from './widget';\n\nnew Widget().paint();\n";

/// A one-package workspace with `src/widget.ts` and `src/main.ts` beside `extra` files.
fn app(extra: &[(&'static str, &'static str)]) -> Workspace {
    let files: Vec<(&str, &str)> = [("src/widget.ts", WIDGET), ("src/main.ts", MAIN)]
        .into_iter()
        .chain(extra.iter().copied())
        .collect();
    Workspace::new(&files)
}

fn widget_query(workspace: &Workspace, file: &str) -> TypeQuery {
    query(vec![workspace.decl(
        file,
        DeclKind::Class,
        "Widget",
        &["render", "paint"],
    )])
}

#[test]
fn facts_name_the_tsrs_engine() {
    let workspace = app(&[("tsconfig.json", TSCONFIG)]);
    let facts = run(
        &workspace,
        &[package("", "app", &["src/widget.ts", "src/main.ts"])],
        &widget_query(&workspace, "src/widget.ts"),
    );
    assert_eq!(facts.engine, ENGINE);
}

#[test]
fn project_references_resolve_to_the_source_of_the_referenced_project() {
    let workspace = Workspace::new(&[
        (
            "packages/lib/tsconfig.json",
            r#"{ "compilerOptions": { "composite": true, "strict": true, "module": "esnext", "moduleResolution": "bundler", "rootDir": "src", "outDir": "dist" }, "include": ["src"] }"#,
        ),
        ("packages/lib/src/widget.ts", WIDGET),
        (
            "packages/app/tsconfig.json",
            r#"{ "compilerOptions": { "strict": true, "module": "esnext", "moduleResolution": "bundler", "noEmit": true }, "include": ["src"], "references": [{ "path": "../lib" }] }"#,
        ),
        (
            "packages/app/src/main.ts",
            "import { Widget } from '../../lib/src/widget';\n\nnew Widget().paint();\n",
        ),
    ]);
    let packages = [
        package("packages/app", "app", &["packages/app/src/main.ts"]),
        package("packages/lib", "lib", &["packages/lib/src/widget.ts"]),
    ];
    let facts = run(
        &workspace,
        &packages,
        &widget_query(&workspace, "packages/lib/src/widget.ts"),
    );
    assert_facts(
        &facts,
        &Expected {
            reads: vec![(0, vec!["paint"])],
            covered: vec!["packages/app/src/main.ts", "packages/lib/src/widget.ts"],
            coverage: vec![typed("packages/app", "app"), typed("packages/lib", "lib")],
            ..Expected::default()
        },
    );
}

#[test]
fn a_solution_config_is_followed_to_the_referenced_config_that_includes_the_file() {
    let referenced = r#"{ "compilerOptions": { "composite": true, "strict": true, "module": "esnext", "moduleResolution": "bundler", "noEmit": true }, "include": ["src"] }"#;
    let tooling = r#"{ "compilerOptions": { "composite": true, "strict": true, "module": "esnext", "moduleResolution": "bundler", "noEmit": true }, "include": ["vite.config.ts"] }"#;
    let workspace = app(&[
        (
            "tsconfig.json",
            r#"{ "files": [], "references": [{ "path": "./tsconfig.node.json" }, { "path": "./tsconfig.app.json" }] }"#,
        ),
        ("tsconfig.app.json", referenced),
        ("tsconfig.node.json", tooling),
        ("vite.config.ts", "export default {};\n"),
    ]);
    let facts = run(
        &workspace,
        &[package("", "app", &["src/widget.ts", "src/main.ts", "vite.config.ts"])],
        &widget_query(&workspace, "src/widget.ts"),
    );
    assert_facts(
        &facts,
        &Expected {
            reads: vec![(0, vec!["paint"])],
            covered: vec!["src/widget.ts", "src/main.ts", "vite.config.ts"],
            coverage: vec![typed("", "app")],
            ..Expected::default()
        },
    );
}

#[test]
fn a_package_without_a_tsconfig_is_untyped() {
    let workspace = app(&[("package.json", r#"{ "name": "app" }"#)]);
    let facts = run(
        &workspace,
        &[package("", "app", &["src/widget.ts", "src/main.ts"])],
        &widget_query(&workspace, "src/widget.ts"),
    );
    assert_facts(
        &facts,
        &Expected {
            coverage: vec![coverage(
                "",
                "app",
                CoverageState::Untyped,
                Some(CoverageReason::NoTsconfig),
                2,
            )],
            ..Expected::default()
        },
    );
}

#[test]
fn uninstalled_dependencies_credit_reads_by_name_and_report_partial() {
    let workspace = Workspace::new(&[
        (
            "package.json",
            r#"{ "name": "app", "dependencies": { "missing-pkg": "^1.0.0" } }"#,
        ),
        ("tsconfig.json", TSCONFIG),
        ("src/widget.ts", WIDGET),
        (
            "src/main.ts",
            "import { Thing } from 'missing-pkg';\nimport { Widget } from './widget';\n\nnew Thing().render();\nnew Widget().paint();\n",
        ),
    ]);
    let facts = run(
        &workspace,
        &[package("", "app", &["src/widget.ts", "src/main.ts"])],
        &widget_query(&workspace, "src/widget.ts"),
    );
    assert_facts(
        &facts,
        &Expected {
            reads: vec![(0, vec!["paint"])],
            unresolved_names: vec![("src/main.ts", vec!["render"])],
            covered: vec!["src/widget.ts", "src/main.ts"],
            coverage: vec![coverage(
                "",
                "app",
                CoverageState::Partial,
                Some(CoverageReason::UnresolvedModules(1)),
                0,
            )],
            ..Expected::default()
        },
    );
}

const BROKEN_TSCONFIGS: [(&str, &str); 2] = [
    ("truncated JSON", r#"{ "compilerOptions": { "strict": true, "#),
    (
        "a value that is not JSON",
        r#"{ "compilerOptions": { "strict": yes } }"#,
    ),
];

#[test]
fn a_package_whose_tsconfig_does_not_parse_is_untyped() {
    for (case, tsconfig) in BROKEN_TSCONFIGS {
        let workspace = app(&[("tsconfig.json", tsconfig)]);
        let facts = run(
            &workspace,
            &[package("", "app", &["src/widget.ts", "src/main.ts"])],
            &widget_query(&workspace, "src/widget.ts"),
        );
        assert_case(
            case,
            &facts,
            &Expected {
                coverage: vec![coverage(
                    "",
                    "app",
                    CoverageState::Untyped,
                    Some(CoverageReason::TsconfigError),
                    2,
                )],
                ..Expected::default()
            },
        );
    }
}

#[test]
fn an_explicit_tsconfig_that_cannot_be_read_fails_the_analysis() {
    let unreadable = [("missing", None)]
        .into_iter()
        .chain(BROKEN_TSCONFIGS.map(|(case, text)| (case, Some(text))));
    for (case, text) in unreadable {
        let mut files = vec![("tsconfig.json", TSCONFIG)];
        if let Some(text) = text {
            files.push(("tsconfig.custom.json", text));
        }
        let workspace = app(&files);
        let result = try_run(
            &workspace,
            &[package("", "app", &["src/widget.ts", "src/main.ts"])],
            &widget_query(&workspace, "src/widget.ts"),
            Options {
                tsconfig_override: Some("tsconfig.custom.json"),
                ..Options::default()
            },
        );
        match result {
            Err(TypeError::Config { path, .. }) => assert_eq!(path, abs("tsconfig.custom.json"), "{case}"),
            other => panic!("{case}: expected a config error, got {other:?}"),
        }
    }
}

#[test]
fn an_explicit_tsconfig_replaces_discovery_for_the_root_package() {
    let workspace = app(&[
        (
            "tsconfig.json",
            r#"{ "compilerOptions": { "strict": true, "noEmit": true }, "include": ["nothing"] }"#,
        ),
        ("tsconfig.custom.json", TSCONFIG),
    ]);
    let facts = try_run(
        &workspace,
        &[package("", "app", &["src/widget.ts", "src/main.ts"])],
        &widget_query(&workspace, "src/widget.ts"),
        Options {
            tsconfig_override: Some("tsconfig.custom.json"),
            ..Options::default()
        },
    )
    .expect("the analysis fails");
    assert_facts(
        &facts,
        &Expected {
            reads: vec![(0, vec!["paint"])],
            covered: vec!["src/widget.ts", "src/main.ts"],
            coverage: vec![typed("", "app")],
            ..Expected::default()
        },
    );
}

#[test]
fn a_file_outside_every_program_keeps_the_untyped_analysis_and_makes_the_package_partial() {
    let workspace = app(&[
        ("tsconfig.json", TSCONFIG),
        (
            "scripts/tool.ts",
            "import { Widget } from '../src/widget';\n\nnew Widget().render();\n",
        ),
    ]);
    let facts = run(
        &workspace,
        &[package("", "app", &["src/widget.ts", "src/main.ts", "scripts/tool.ts"])],
        &widget_query(&workspace, "src/widget.ts"),
    );
    assert_facts(
        &facts,
        &Expected {
            reads: vec![(0, vec!["paint"])],
            covered: vec!["src/widget.ts", "src/main.ts"],
            coverage: vec![coverage(
                "",
                "app",
                CoverageState::Partial,
                Some(CoverageReason::FilesOutsidePrograms(1)),
                1,
            )],
            ..Expected::default()
        },
    );
}

#[test]
fn cancelled_before_start_returns_cancelled() {
    let workspace = app(&[("tsconfig.json", TSCONFIG)]);
    let cancel = AtomicBool::new(true);
    let result = try_run(
        &workspace,
        &[package("", "app", &["src/widget.ts", "src/main.ts"])],
        &widget_query(&workspace, "src/widget.ts"),
        Options {
            cancel: Some(&cancel),
            ..Options::default()
        },
    );
    assert!(
        matches!(result, Err(TypeError::Cancelled)),
        "expected a cancellation, got {result:?}"
    );
}
