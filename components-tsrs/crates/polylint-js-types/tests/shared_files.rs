//! The programs of a run share the files they read: each is parsed and bound once for the run,
//! whichever program reads it first, and the facts are those of programs that parse every file for themselves. The
//! bundled lib files and the declaration files below `node_modules` stay for the run, so every later program reads
//! that parse, also after the program that parsed it is freed. A project's own files, declaration files included,
//! are shared by the programs that read them at once and freed with the last of them.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::collections::{BTreeMap, BTreeSet};
use std::num::NonZeroUsize;

use common::{Options, TSCONFIG, Workspace, abs, package, query, try_run};
use polylint_js_types::{
    DeclId, DeclKind, SharedFileCounts, TypeBudget, TypeBuild, TypeFacts, TypePackage, TypePrograms, TypeQuery, start,
};

const THREADS: usize = 8;

const SHAPES_MANIFEST: &str = r#"{ "name": "shapes", "version": "1.0.0", "types": "index.d.ts" }"#;

/// The declaration file of the `shapes` dependency: an interface the project's classes implement.
const SHAPES: &str = "\
export interface Shape {
  area(): number;
}
";

const WIDGET: &str = "\
import type { Shape } from 'shapes';

export class Widget implements Shape {
  area(): number {
    return 1;
  }
  paint(): void {}
  erase(): void {}
}
";

/// Reads `area` through the interface of the dependency and `paint` on the class; `erase` is never read.
const MAIN: &str = "\
import type { Shape } from 'shapes';
import { Widget } from './widget';

export function total(shape: Shape): number {
  return shape.area();
}

new Widget().paint();
";

/// A class that reads nothing of the dependency.
const LONE: &str = "export class Lone {\n  paint(): void {}\n}\n";

/// A declaration file of the project, below no `node_modules`, that the programs of two packages include.
const GLOBALS: &str = "declare const SCALE: number;\n";

const WIDGET_SCALED: &str = "\
import type { Shape } from 'shapes';

export class Widget implements Shape {
  area(): number {
    return SCALE;
  }
  paint(): void {}
  erase(): void {}
}
";

fn threads(count: usize) -> NonZeroUsize {
    NonZeroUsize::new(count).unwrap()
}

/// `TSCONFIG` with `extra` compiler options and `include` instead of `["src"]`.
fn tsconfig(extra: &str, include: &str) -> String {
    TSCONFIG
        .replace(r#""strict": true,"#, &format!(r#""strict": true,{extra}"#))
        .replace(r#"["src"]"#, include)
}

/// One package of a fixture: its directory and its files relative to it.
struct Pkg {
    dir: &'static str,
    files: Vec<(&'static str, String)>,
}

/// A package at `dir` with the widget and the file that reads it, under `config`.
fn widgets(dir: &'static str, config: String) -> Pkg {
    Pkg {
        dir,
        files: vec![
            ("tsconfig.json", config),
            ("src/widget.ts", WIDGET.to_owned()),
            ("src/main.ts", MAIN.to_owned()),
        ],
    }
}

/// The workspace of `pkgs` with the `shapes` dependency installed at the root, and `extra` files.
fn workspace(pkgs: &[Pkg], extra: &[(&str, &str)]) -> Workspace {
    let mut files = vec![
        (
            "node_modules/shapes/package.json".to_owned(),
            SHAPES_MANIFEST.to_owned(),
        ),
        ("node_modules/shapes/index.d.ts".to_owned(), SHAPES.to_owned()),
    ];
    for pkg in pkgs {
        files.extend(
            pkg.files
                .iter()
                .map(|(rel, text)| (format!("{}/{rel}", pkg.dir), text.clone())),
        );
    }
    files.extend(extra.iter().map(|(rel, text)| ((*rel).to_owned(), (*text).to_owned())));
    Workspace::from_owned(files)
}

/// The package of `pkg`, judging its source files.
fn package_of(pkg: &Pkg) -> TypePackage {
    let files: Vec<String> = pkg
        .files
        .iter()
        .filter(|(rel, _)| rel.starts_with("src/"))
        .map(|(rel, _)| format!("{}/{rel}", pkg.dir))
        .collect();
    let files: Vec<&str> = files.iter().map(String::as_str).collect();
    package(pkg.dir, pkg.dir, &files)
}

/// The widget of every package of `pkgs`, numbered in order, judging `area`, `paint` and `erase`.
fn widgets_query(workspace: &Workspace, pkgs: &[&Pkg]) -> TypeQuery {
    query(
        pkgs.iter()
            .map(|pkg| {
                workspace.decl(
                    &format!("{}/src/widget.ts", pkg.dir),
                    DeclKind::Class,
                    "Widget",
                    &["area", "paint", "erase"],
                )
            })
            .collect(),
    )
}

/// The reads of `widgets` widgets, each read on `area` (through the dependency's interface) and on `paint`.
fn widget_reads(widgets: u32) -> BTreeMap<DeclId, BTreeSet<String>> {
    (0..widgets)
        .map(|id| (DeclId(id), ["area", "paint"].map(str::to_owned).into_iter().collect()))
        .collect()
}

fn analyse(workspace: &Workspace, pkgs: &[&Pkg], query: &TypeQuery, threads: Option<NonZeroUsize>) -> TypeFacts {
    let packages: Vec<TypePackage> = pkgs.iter().map(|pkg| package_of(pkg)).collect();
    try_run(
        workspace,
        &packages,
        query,
        Options {
            threads,
            ..Options::default()
        },
    )
    .expect("the analysis fails")
}

fn counts(facts: &TypeFacts) -> (usize, usize, usize) {
    split(facts.timings.shared_files)
}

fn split(counts: SharedFileCounts) -> (usize, usize, usize) {
    (counts.parsed, counts.reused, counts.unshared)
}

/// The shared files one program of `pkg`'s shape reads, from an analysis of it alone, which parses each of them.
fn files_of_one_program(workspace: &Workspace, pkg: &Pkg) -> usize {
    let facts = analyse(workspace, &[pkg], &widgets_query(workspace, &[pkg]), None);
    let (parsed, reused, unshared) = counts(&facts);
    assert_eq!((reused, unshared), (0, 0), "one program reads each shared file once");
    assert!(
        parsed > 1,
        "the program reads the lib files and the dependency's declaration file"
    );
    parsed
}

#[test]
fn programs_of_one_analysis_parse_each_shared_file_once_and_keep_their_facts() {
    let pkgs = [
        widgets("packages/a", TSCONFIG.to_owned()),
        widgets("packages/b", TSCONFIG.to_owned()),
    ];
    let workspace = workspace(&pkgs, &[]);
    let files = files_of_one_program(&workspace, &pkgs[0]);
    let both = [&pkgs[0], &pkgs[1]];
    let query = widgets_query(&workspace, &both);
    let facts = analyse(&workspace, &both, &query, Some(threads(THREADS)));
    assert_eq!(
        counts(&facts),
        (files, files, 0),
        "the second program reads the first one's parse of every shared file"
    );
    assert_eq!(facts.reads, widget_reads(2));
    // Each package alone, with programs that share no file with another program.
    let alone: Vec<TypeFacts> = both
        .iter()
        .map(|&pkg| analyse(&workspace, &[pkg], &query, Some(threads(THREADS))))
        .collect();
    let mut expected_reads = alone[0].reads.clone();
    expected_reads.extend(alone[1].reads.clone());
    let expected_covered: BTreeSet<_> = alone[0].covered.union(&alone[1].covered).cloned().collect();
    assert_eq!(facts.reads, expected_reads, "the reads of separate builds");
    assert_eq!(facts.covered, expected_covered, "the coverage of separate builds");
    assert_eq!(facts.whole, BTreeSet::new());
}

#[test]
fn a_program_built_after_another_was_freed_reads_the_files_that_one_parsed() {
    let pkgs = [
        widgets("packages/a", TSCONFIG.to_owned()),
        widgets("packages/b", TSCONFIG.to_owned()),
    ];
    let workspace = workspace(&pkgs, &[]);
    let files = files_of_one_program(&workspace, &pkgs[0]);
    let both = [&pkgs[0], &pkgs[1]];
    // One thread builds one program at a time, and frees each once its walk ends, before the next is built.
    let facts = analyse(&workspace, &both, &widgets_query(&workspace, &both), Some(threads(1)));
    assert_eq!(counts(&facts), (files, files, 0));
    assert_eq!(
        facts.reads,
        widget_reads(2),
        "the second program answers on the freed one's parse"
    );
}

#[test]
fn options_that_change_only_how_project_files_parse_still_share_the_declaration_files() {
    // JSX and module detection change how tsrs parses a project file, never a declaration file.
    let pkgs = [
        widgets("packages/a", TSCONFIG.to_owned()),
        widgets(
            "packages/b",
            tsconfig(
                r#"
    "jsx": "react-jsx",
    "moduleDetection": "force","#,
                r#"["src"]"#,
            ),
        ),
    ];
    let workspace = workspace(&pkgs, &[]);
    let files = files_of_one_program(&workspace, &pkgs[0]);
    let both = [&pkgs[0], &pkgs[1]];
    let facts = analyse(
        &workspace,
        &both,
        &widgets_query(&workspace, &both),
        Some(threads(THREADS)),
    );
    assert_eq!(counts(&facts), (files, files, 0));
    assert_eq!(facts.reads, widget_reads(2));
}

#[test]
fn a_projects_own_declaration_file_is_not_kept_for_the_run() {
    let lone = Pkg {
        dir: "packages/lone",
        files: vec![("tsconfig.json", TSCONFIG.to_owned()), ("src/lone.ts", LONE.to_owned())],
    };
    let scaled = |dir: &'static str| Pkg {
        dir,
        files: vec![
            ("tsconfig.json", tsconfig("", r#"["src", "../../shared"]"#)),
            ("src/widget.ts", WIDGET_SCALED.to_owned()),
            ("src/main.ts", MAIN.to_owned()),
        ],
    };
    let pkgs = [lone, scaled("packages/a"), scaled("packages/b")];
    let workspace = workspace(&pkgs, &[("shared/globals.d.ts", GLOBALS)]);
    // A program that reads only the lib files.
    let lone_query = query(vec![workspace.decl(
        "packages/lone/src/lone.ts",
        DeclKind::Class,
        "Lone",
        &["paint"],
    )]);
    let libs = counts(&analyse(&workspace, &[&pkgs[0]], &lone_query, None)).0;
    let both = [&pkgs[1], &pkgs[2]];
    let facts = analyse(
        &workspace,
        &both,
        &widgets_query(&workspace, &both),
        Some(threads(THREADS)),
    );
    assert_eq!(
        counts(&facts),
        (libs + 1, libs + 1, 0),
        "the lib files and the dependency's declaration file are kept for the run, `shared/globals.d.ts` is not"
    );
    assert_eq!(facts.reads, widget_reads(2));
}

/// A library whose `Widget` the root's program reads as a source, through a project reference: the root package
/// `app` at the workspace root, the library below it.
fn referenced() -> [Pkg; 2] {
    let app = Pkg {
        dir: "",
        files: vec![
            (
                "tsconfig.json",
                TSCONFIG.replace(
                    r#""include": ["src"]"#,
                    r#""include": ["src"], "references": [{ "path": "packages/lib" }]"#,
                ),
            ),
            (
                "src/main.ts",
                MAIN.replace("'./widget'", "'../packages/lib/src/widget'"),
            ),
        ],
    };
    let lib = Pkg {
        dir: "packages/lib",
        files: vec![
            (
                "tsconfig.json",
                tsconfig(
                    r#" "composite": true, "rootDir": "src", "outDir": "dist","#,
                    r#"["src"]"#,
                ),
            ),
            ("src/widget.ts", WIDGET.to_owned()),
        ],
    };
    [app, lib]
}

/// The workspace of `pkgs` with the `shapes` dependency, with the files of the package at the workspace root.
fn rooted_workspace(pkgs: &[Pkg]) -> Workspace {
    let mut files = vec![
        (
            "node_modules/shapes/package.json".to_owned(),
            SHAPES_MANIFEST.to_owned(),
        ),
        ("node_modules/shapes/index.d.ts".to_owned(), SHAPES.to_owned()),
    ];
    for pkg in pkgs {
        files.extend(pkg.files.iter().map(|(rel, text)| {
            let path = if pkg.dir.is_empty() {
                (*rel).to_owned()
            } else {
                format!("{}/{rel}", pkg.dir)
            };
            (path, text.clone())
        }));
    }
    Workspace::from_owned(files)
}

/// The package of `pkg`, judging its source files, named after its directory.
fn rooted_package(pkg: &Pkg) -> TypePackage {
    let files: Vec<String> = pkg
        .files
        .iter()
        .filter(|(rel, _)| rel.starts_with("src/"))
        .map(|(rel, _)| {
            if pkg.dir.is_empty() {
                (*rel).to_owned()
            } else {
                format!("{}/{rel}", pkg.dir)
            }
        })
        .collect();
    let files: Vec<&str> = files.iter().map(String::as_str).collect();
    package(pkg.dir, if pkg.dir.is_empty() { "app" } else { pkg.dir }, &files)
}

fn lib_widget_query(workspace: &Workspace) -> TypeQuery {
    query(vec![workspace.decl(
        "packages/lib/src/widget.ts",
        DeclKind::Class,
        "Widget",
        &["area", "paint", "erase"],
    )])
}

/// The facts of the analysis of `pkg` in a run that shares `programs`.
fn rooted(workspace: &Workspace, pkg: &Pkg, query: &TypeQuery, programs: &TypePrograms) -> TypeFacts {
    let packages = vec![rooted_package(pkg)];
    let build = TypeBuild::new(workspace.fs(), abs(pkg.dir), packages.clone())
        .with_threads(threads(THREADS))
        .with_programs(programs.clone());
    std::thread::scope(|scope| start(scope, build, None).finish(&packages, query)).expect("the analysis fails")
}

#[test]
fn a_source_a_kept_program_read_is_parsed_once_for_the_run_and_freed_with_the_last_program() {
    let [app, lib] = referenced();
    let workspace = rooted_workspace(&[app, lib]);
    let [app, lib] = referenced();
    let query = lib_widget_query(&workspace);
    let roots = || [abs(app.dir), abs(lib.dir)];
    let programs = TypePrograms::new(roots());
    // The library's program is kept after its analysis: the root, which has not finished, holds the library's files.
    let lib_facts = rooted(&workspace, &lib, &query, &programs);
    assert_eq!(programs.kept(), 1, "the library's program waits for the root");
    assert_eq!(
        lib_facts.timings.project_files_parsed, 1,
        "`packages/lib/src/widget.ts`"
    );
    assert_eq!(programs.project_files_held(), 1);
    let app_facts = rooted(&workspace, &app, &query, &programs);
    assert_eq!(
        app_facts.timings.project_files_parsed, 1,
        "`src/main.ts`; the library's source comes from the kept program's parse"
    );
    assert_eq!(programs.kept(), 0, "the root finished, so the library's program went");
    assert_eq!(
        programs.project_files_held(),
        0,
        "and every project file with the programs"
    );
    assert!(
        programs.shared_files().parsed > 0,
        "the declaration files stay for the run"
    );
    // The same analyses, each in a run of its own: the library's source is parsed for each program.
    let alone: Vec<TypeFacts> = [&lib, &app]
        .into_iter()
        .map(|pkg| rooted(&workspace, pkg, &query, &TypePrograms::new(roots())))
        .collect();
    assert_eq!(alone[1].timings.project_files_parsed, 2);
    for (shared, alone) in [&lib_facts, &app_facts].into_iter().zip(&alone) {
        assert_eq!(
            shared.reads, alone.reads,
            "the reads of programs that parse every file for themselves"
        );
        assert_eq!(shared.covered, alone.covered);
        assert_eq!(shared.whole, alone.whole);
    }
    assert_eq!(
        app_facts.reads,
        BTreeMap::from([(DeclId(0), BTreeSet::from(["paint".to_owned()]))])
    );
}

/// The root package `app` at the workspace root, whose own config includes the library's source with `extra` compiler
/// options, and the library below it.
fn including(extra: &str) -> [Pkg; 2] {
    let app = Pkg {
        dir: "",
        files: vec![
            ("tsconfig.json", tsconfig(extra, r#"["src", "packages/lib/src"]"#)),
            (
                "src/main.ts",
                MAIN.replace("'./widget'", "'../packages/lib/src/widget'"),
            ),
        ],
    };
    let lib = Pkg {
        dir: "packages/lib",
        files: vec![
            ("tsconfig.json", TSCONFIG.to_owned()),
            ("src/widget.ts", WIDGET.to_owned()),
        ],
    };
    [app, lib]
}

#[test]
fn a_project_file_two_programs_parse_with_different_options_is_parsed_for_each() {
    // JSX and module detection decide the external-module indicator a source is parsed with.
    for (extra, parsed) in [
        ("", 1),
        (r#" "jsx": "react-jsx","#, 2),
        (r#" "moduleDetection": "force","#, 2),
    ] {
        let workspace = rooted_workspace(&including(extra));
        let [app, lib] = including(extra);
        let query = lib_widget_query(&workspace);
        let programs = TypePrograms::new([abs(app.dir), abs(lib.dir)]);
        rooted(&workspace, &lib, &query, &programs);
        assert_eq!(
            programs.kept(),
            1,
            "{extra}: the library's program, holding its source, waits for the root"
        );
        let app_facts = rooted(&workspace, &app, &query, &programs);
        assert_eq!(
            app_facts.timings.project_files_parsed, parsed,
            "{extra}: `src/main.ts`, and the library's source unless the root's program parses it differently"
        );
        assert_eq!(
            app_facts.reads,
            BTreeMap::from([(DeclId(0), BTreeSet::from(["paint".to_owned()]))]),
            "{extra}"
        );
    }
}

/// One analysis of a run over several roots: a root with its own package and config.
struct Root {
    pkg: Pkg,
    query: TypeQuery,
}

fn roots(dirs: &[&'static str]) -> (Workspace, Vec<Root>) {
    let pkgs: Vec<Pkg> = dirs.iter().map(|dir| widgets(dir, TSCONFIG.to_owned())).collect();
    let workspace = workspace(&pkgs, &[]);
    let roots = pkgs
        .into_iter()
        .map(|pkg| {
            let query = widgets_query(&workspace, &[&pkg]);
            Root { pkg, query }
        })
        .collect();
    (workspace, roots)
}

/// The facts of `root` in a run that shares `programs` and `budget`.
fn sharing(workspace: &Workspace, root: &Root, programs: &TypePrograms, budget: &TypeBudget) -> TypeFacts {
    let packages = vec![package_of(&root.pkg)];
    let build = TypeBuild::new(workspace.fs(), abs(root.pkg.dir), packages.clone())
        .with_threads(threads(THREADS))
        .with_budget(budget.clone())
        .with_programs(programs.clone());
    std::thread::scope(|scope| start(scope, build, None).finish(&packages, &root.query)).expect("the analysis fails")
}

#[test]
fn a_later_analysis_of_the_run_reads_the_files_an_ended_one_parsed() {
    let (workspace, roots) = roots(&["packages/a", "packages/b"]);
    let files = files_of_one_program(&workspace, &roots[0].pkg);
    let programs = TypePrograms::new(roots.iter().map(|root| abs(root.pkg.dir)));
    let budget = TypeBudget::new(Some(threads(THREADS)));
    let first = sharing(&workspace, &roots[0], &programs, &budget);
    assert_eq!(counts(&first), (files, 0, 0));
    assert_eq!(programs.kept(), 0, "the first analysis freed its program");
    let second = sharing(&workspace, &roots[1], &programs, &budget);
    assert_eq!(
        counts(&second),
        (0, files, 0),
        "the second analysis reads every shared file from the run"
    );
    assert_eq!(split(programs.shared_files()), (files, files, 0), "the run's counts");
    assert_eq!(
        programs.project_files_held(),
        0,
        "the project files went with the programs, the declaration files stay"
    );
    for facts in [&first, &second] {
        assert_eq!(facts.reads, widget_reads(1));
    }
}

#[test]
fn analyses_run_at_once_parse_each_shared_file_once_for_the_run() {
    use rayon::prelude::*;
    const ROOTS: [&str; 6] = [
        "packages/r0",
        "packages/r1",
        "packages/r2",
        "packages/r3",
        "packages/r4",
        "packages/r5",
    ];
    let (workspace, roots) = roots(&ROOTS);
    let files = files_of_one_program(&workspace, &roots[0].pkg);
    let programs = TypePrograms::new(roots.iter().map(|root| abs(root.pkg.dir)));
    let budget = TypeBudget::new(Some(threads(THREADS)));
    let (done, finished) = std::sync::mpsc::channel();
    let facts = std::thread::scope(|outer| {
        outer.spawn(|| {
            let facts: Vec<TypeFacts> = roots
                .par_iter()
                .with_max_len(1)
                .map(|root| sharing(&workspace, root, &programs, &budget))
                .collect();
            done.send(facts).unwrap();
        });
        // An analysis waiting for another one's parse that never ends would hang the run.
        finished
            .recv_timeout(std::time::Duration::from_secs(300))
            .expect("the analyses wait for each other's parses")
    });
    assert_eq!(
        split(programs.shared_files()),
        (files, files * (ROOTS.len() - 1), 0),
        "each shared file is parsed once for the run and read by every other program"
    );
    let parsed: usize = facts.iter().map(|facts| facts.timings.shared_files.parsed).sum();
    assert_eq!(parsed, files, "the analyses' counts add up to the run's");
    for facts in &facts {
        assert_eq!(facts.reads, widget_reads(1));
    }
}
