//! The plannings and programs of a run share the configs they read (skif ADR 0010): each config a planning searches or a
//! program's project references reach, and each config those extend, is parsed once for the run, and the facts are
//! those of analyses that parse every config for themselves.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::collections::{BTreeMap, BTreeSet};
use std::num::NonZeroUsize;

use common::{Options, Workspace, package, query, try_run};
use polylint_js_types::{DeclId, DeclKind, SharedConfigCounts, TypeFacts, TypePackage, TypeQuery};

/// The options every package's config extends.
const BASE: &str = r#"{
  "compilerOptions": {
    "target": "es2022",
    "module": "esnext",
    "moduleResolution": "bundler",
    "strict": true
  }
}
"#;

/// The referenced project: a composite library.
const LIB: &str = r#"{
  "extends": "../../tsconfig.base.json",
  "compilerOptions": { "composite": true, "rootDir": "src", "outDir": "dist" },
  "include": ["src"]
}
"#;

const WIDGET: &str = "\
export class Widget {
  paint(): void {}
  erase(): void {}
  spin(): void {}
}
";

/// A package that references the library, with the config it extends.
fn app_config(base: &str) -> String {
    format!(
        r#"{{
  "extends": "../../{base}",
  "compilerOptions": {{ "noEmit": true }},
  "include": ["src"],
  "references": [{{ "path": "../lib" }}]
}}
"#
    )
}

/// A file that calls `member` on a widget of the library.
fn calling(member: &str) -> String {
    format!("import {{ Widget }} from '../../lib/src/widget';\n\nnew Widget().{member}();\n")
}

/// Packages `a` and `b` (and `c`, extending a config of its own) that reference the library, each calling one
/// member of its widget.
fn workspace() -> Workspace {
    Workspace::from_owned(
        [
            ("tsconfig.base.json", BASE.to_owned()),
            (
                "tsconfig.other.json",
                BASE.replace(r#""strict": true"#, r#""strict": false"#),
            ),
            ("packages/lib/tsconfig.json", LIB.to_owned()),
            ("packages/lib/src/widget.ts", WIDGET.to_owned()),
            ("packages/a/tsconfig.json", app_config("tsconfig.base.json")),
            ("packages/a/src/main.ts", calling("paint")),
            ("packages/b/tsconfig.json", app_config("tsconfig.base.json")),
            ("packages/b/src/main.ts", calling("erase")),
            ("packages/c/tsconfig.json", app_config("tsconfig.other.json")),
            ("packages/c/src/main.ts", calling("spin")),
        ]
        .into_iter()
        .map(|(rel, text)| (rel.to_owned(), text))
        .collect(),
    )
}

fn app(name: &str) -> TypePackage {
    let dir = format!("packages/{name}");
    let main = format!("{dir}/src/main.ts");
    package(&dir, name, &[main.as_str()])
}

fn widget_query(workspace: &Workspace) -> TypeQuery {
    query(vec![workspace.decl(
        "packages/lib/src/widget.ts",
        DeclKind::Class,
        "Widget",
        &["paint", "erase", "spin"],
    )])
}

fn analyse(workspace: &Workspace, packages: &[TypePackage], threads: usize) -> TypeFacts {
    try_run(
        workspace,
        packages,
        &widget_query(workspace),
        Options {
            threads: NonZeroUsize::new(threads),
            ..Options::default()
        },
    )
    .expect("the analysis fails")
}

fn split(counts: SharedConfigCounts) -> (usize, usize, usize) {
    (counts.parsed, counts.reused, counts.unshared)
}

fn reads(members: &[&str]) -> BTreeMap<DeclId, BTreeSet<String>> {
    BTreeMap::from([(DeclId(0), members.iter().map(|&member| member.to_owned()).collect())])
}

#[test]
fn programs_that_reference_one_project_read_its_config_and_the_config_they_extend_once() {
    let workspace = workspace();
    // One thread: the planning reads the configs one at a time, and the programs are built one after the other.
    let facts = analyse(&workspace, &[app("a"), app("b")], 1);
    assert_eq!(
        split(facts.timings.shared_configs),
        (3, 4, 0),
        "the planning reads the configs of `a` and `b` and the library they reference; both programs reuse the \
         library's and take their own from the planning's"
    );
    assert_eq!(
        split(facts.timings.shared_extended_configs),
        (1, 2, 0),
        "the three configs the planning reads extend one config, parsed once; the programs parse no config"
    );
    assert_eq!(facts.reads, reads(&["erase", "paint"]));
}

#[test]
fn analyses_that_read_their_configs_together_find_the_facts_of_analyses_that_read_them_alone() {
    let workspace = workspace();
    let packages = [app("a"), app("b"), app("c")];
    let together = analyse(&workspace, &packages, 8);
    let (configs, extended) = (
        together.timings.shared_configs,
        together.timings.shared_extended_configs,
    );
    assert_eq!(
        configs.parsed, 4,
        "the configs of `a`, `b`, `c` and the library, each parsed once"
    );
    assert_eq!(
        extended.parsed, 2,
        "two configs are extended, each parsed once for the run"
    );
    let alone: Vec<TypeFacts> = packages
        .iter()
        .map(|package| analyse(&workspace, std::slice::from_ref(package), 8))
        .collect();
    let mut expected_reads = BTreeMap::new();
    for facts in &alone {
        for (decl, members) in &facts.reads {
            expected_reads
                .entry(*decl)
                .or_insert_with(BTreeSet::new)
                .extend(members.iter().cloned());
        }
    }
    let expected_covered: BTreeSet<_> = alone.iter().flat_map(|facts| facts.covered.iter().cloned()).collect();
    assert_eq!(together.reads, reads(&["erase", "paint", "spin"]));
    assert_eq!(together.reads, expected_reads, "the reads of separate analyses");
    assert_eq!(together.covered, expected_covered, "the coverage of separate analyses");
    assert_eq!(together.coverage.len(), 3);
    for (index, facts) in alone.iter().enumerate() {
        assert_eq!(
            facts.coverage[0], together.coverage[index],
            "the coverage of package {index} alone"
        );
    }
}

#[test]
fn configs_that_extend_different_configs_keep_the_options_of_their_own() {
    // `c` extends a config of its own; both programs read the library's one parse and take their own from the
    // planning's.
    let workspace = workspace();
    let facts = analyse(&workspace, &[app("a"), app("c")], 1);
    assert_eq!(split(facts.timings.shared_configs), (3, 4, 0));
    assert_eq!(
        split(facts.timings.shared_extended_configs),
        (2, 1, 0),
        "each extended config is parsed once, for the configs that extend it"
    );
    assert_eq!(facts.reads, reads(&["paint", "spin"]));
}
