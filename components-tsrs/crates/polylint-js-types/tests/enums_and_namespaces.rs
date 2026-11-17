//! Typed enum and namespace reads (skif ADR 0010): reads through aliases, `this` and qualified names credit the member they
//! reach, and a declaration that escapes so that any member may be read is used as a whole.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use common::{Expected, TSCONFIG, Workspace, assert_case, assert_facts, package, query, run, typed};
use polylint_js_types::DeclKind;

const COLORS: &str = "\
export enum Color {
  Red,
  Green,
  Blue,
}
";

fn analyze_colors(main: &str) -> polylint_js_types::TypeFacts {
    let workspace = Workspace::new(&[
        ("tsconfig.json", TSCONFIG),
        ("src/colors.ts", COLORS),
        ("src/main.ts", main),
    ]);
    let query = query(vec![workspace.decl(
        "src/colors.ts",
        DeclKind::Enum,
        "Color",
        &["Red", "Green", "Blue"],
    )]);
    run(
        &workspace,
        &[package("", "app", &["src/colors.ts", "src/main.ts"])],
        &query,
    )
}

#[test]
fn an_enum_member_read_through_an_alias_or_a_literal_key_credits_that_member() {
    for (case, main, member) in [
        (
            "a const alias",
            "import { Color } from './colors';\n\nconst Palette = Color;\nexport const red = Palette.Red;\n",
            "Red",
        ),
        (
            "a literal element access",
            "import { Color } from './colors';\n\nexport const green = Color['Green'];\n",
            "Green",
        ),
        (
            "a destructuring of an alias",
            "import { Color } from './colors';\n\nconst Palette = Color;\nexport const { Blue } = Palette;\n",
            "Blue",
        ),
    ] {
        assert_case(
            case,
            &analyze_colors(main),
            &Expected {
                reads: vec![(0, vec![member])],
                covered: vec!["src/colors.ts", "src/main.ts"],
                coverage: vec![typed("", "app")],
                ..Expected::default()
            },
        );
    }
}

#[test]
fn an_enum_that_escapes_is_used_as_a_whole() {
    for (case, main) in [
        (
            "passed to a function",
            "import { Color } from './colors';\n\ndeclare function register(value: object): void;\nregister(Color);\n",
        ),
        (
            "Object.values",
            "import { Color } from './colors';\n\nexport const all = Object.values(Color);\n",
        ),
        (
            "an alias passed on",
            "import { Color } from './colors';\n\ndeclare function register(value: object): void;\nconst Palette = Color;\nregister(Palette);\n",
        ),
        (
            "keyof typeof",
            "import { Color } from './colors';\n\nexport type ColorName = keyof typeof Color;\n",
        ),
        (
            "a spread",
            "import { Color } from './colors';\n\nexport const copy = { ...Color };\n",
        ),
        (
            "a shorthand property",
            "import { Color } from './colors';\n\nexport const palette = { Color };\n",
        ),
    ] {
        assert_case(
            case,
            &analyze_colors(main),
            &Expected {
                whole: vec![0],
                covered: vec!["src/colors.ts", "src/main.ts"],
                coverage: vec![typed("", "app")],
                ..Expected::default()
            },
        );
    }
}

const SETTINGS: &str = "\
export namespace Settings {
  export const port = 8080;
  export const host = 'localhost';
}
";

#[test]
fn a_namespace_member_read_through_this_or_a_qualified_name_credits_that_member() {
    for (case, main) in [
        (
            "this typed as the namespace",
            "import { Settings } from './settings';\n\nexport function port(this: typeof Settings): number {\n  return this.port;\n}\n",
        ),
        (
            "a qualified name in a type query",
            "import { Settings } from './settings';\n\nexport type Port = typeof Settings.port;\n",
        ),
    ] {
        let workspace = Workspace::new(&[
            ("tsconfig.json", TSCONFIG),
            ("src/settings.ts", SETTINGS),
            ("src/main.ts", main),
        ]);
        let query = query(vec![workspace.decl(
            "src/settings.ts",
            DeclKind::Namespace,
            "Settings",
            &["port", "host"],
        )]);
        let facts = run(
            &workspace,
            &[package("", "app", &["src/settings.ts", "src/main.ts"])],
            &query,
        );
        assert_case(
            case,
            &facts,
            &Expected {
                reads: vec![(0, vec!["port"])],
                covered: vec!["src/settings.ts", "src/main.ts"],
                coverage: vec![typed("", "app")],
                ..Expected::default()
            },
        );
    }
}

/// Names under which `Color` reaches other modules: a re-export, an import passed on, the default export, a variable
/// initialized from it, one declared `typeof` it and a namespace import alias.
const BARREL: &str = "\
import { Color as Tint } from './colors';
export { Color as Hue } from './colors';
export { Tint as Shade };
export default Tint;
export const Palette = Tint;
export let Swatch: typeof Tint;
export namespace Kit {
  export import Tone = Tint;
}
";

/// The escapes of the barrel's names that are property accesses: their names are not `Color`, and only the program's
/// aliases and alias-capable variables lead from them to it.
#[test]
fn an_enum_that_escapes_through_a_member_of_another_name_is_used_as_a_whole() {
    let prelude = "import * as barrel from './barrel';\n\ndeclare function register(value: object): void;\n";
    for (case, body) in [
        ("a re-export", "register(barrel.Hue);\n"),
        ("an import exported again", "register(barrel.Shade);\n"),
        ("the default export", "register(barrel.default);\n"),
        ("a variable initialized from it", "register(barrel.Palette);\n"),
        ("a variable declared typeof it", "register(barrel.Swatch);\n"),
        ("an import alias in a namespace", "register(barrel.Kit.Tone);\n"),
        (
            "an alias of the namespace import",
            "const alias = barrel;\nregister(alias.Hue);\n",
        ),
        (
            "a parameter typed as the namespace",
            "export function pass(module: typeof barrel): void {\n  register(module.Palette);\n}\n",
        ),
        ("a return", "export function hue(): object {\n  return barrel.Hue;\n}\n"),
        ("an array element", "export const all = [barrel.Shade];\n"),
        ("a spread", "export const copy = { ...barrel.Swatch };\n"),
        ("an assignment", "export let held: object;\nheld = barrel.Palette;\n"),
        (
            "an object literal value",
            "export const held = { color: barrel.Hue };\n",
        ),
    ] {
        let workspace = Workspace::new(&[
            ("tsconfig.json", TSCONFIG),
            ("src/colors.ts", COLORS),
            ("src/barrel.ts", BARREL),
            ("src/main.ts", &format!("{prelude}{body}")),
        ]);
        let query = query(vec![workspace.decl(
            "src/colors.ts",
            DeclKind::Enum,
            "Color",
            &["Red", "Green", "Blue"],
        )]);
        let facts = run(
            &workspace,
            &[package("", "app", &["src/colors.ts", "src/main.ts"])],
            &query,
        );
        assert_case(
            case,
            &facts,
            &Expected {
                whole: vec![0],
                covered: vec!["src/colors.ts", "src/main.ts"],
                coverage: vec![typed("", "app")],
                ..Expected::default()
            },
        );
    }
}

/// A member destructured from a namespace import is read on the module; the variable it declares is no alias of the
/// enum, so passing it on is no whole use.
#[test]
fn a_member_destructured_from_a_namespace_import_is_read_and_not_used_as_a_whole() {
    let workspace = Workspace::new(&[
        ("tsconfig.json", TSCONFIG),
        ("src/colors.ts", COLORS),
        ("src/barrel.ts", BARREL),
        (
            "src/main.ts",
            "import * as barrel from './barrel';\n\ndeclare function register(value: object): void;\nconst { Hue } = barrel;\nregister(Hue);\nexport const red = Hue.Red;\n",
        ),
    ]);
    let query = query(vec![workspace.decl(
        "src/colors.ts",
        DeclKind::Enum,
        "Color",
        &["Red", "Green", "Blue"],
    )]);
    let facts = run(
        &workspace,
        &[package("", "app", &["src/colors.ts", "src/main.ts"])],
        &query,
    );
    assert_facts(
        &facts,
        &Expected {
            reads: vec![(0, vec!["Red"])],
            covered: vec!["src/colors.ts", "src/main.ts"],
            coverage: vec![typed("", "app")],
            ..Expected::default()
        },
    );
}

/// `exports.Hue = Color` in checked JavaScript is an alias the binder declares on the module, not a statement the host
/// reads: the escape of `cjs.Hue` reaches the enum through it.
#[test]
fn an_enum_exported_by_a_commonjs_assignment_escapes_through_the_export() {
    let workspace = Workspace::new(&[
        (
            "tsconfig.json",
            r#"{ "compilerOptions": { "allowJs": true, "checkJs": true, "noEmit": true, "strict": true, "module": "esnext", "moduleResolution": "bundler", "esModuleInterop": true }, "include": ["src"] }"#,
        ),
        ("src/colors.ts", COLORS),
        (
            "src/cjs.js",
            "const { Color } = require('./colors');\nexports.Hue = Color;\n",
        ),
        (
            "src/main.ts",
            "import * as cjs from './cjs';\n\ndeclare function register(value: object): void;\nregister(cjs.Hue);\n",
        ),
    ]);
    let query = query(vec![workspace.decl(
        "src/colors.ts",
        DeclKind::Enum,
        "Color",
        &["Red", "Green", "Blue"],
    )]);
    let facts = run(
        &workspace,
        &[package("", "app", &["src/colors.ts", "src/main.ts"])],
        &query,
    );
    assert_facts(
        &facts,
        &Expected {
            whole: vec![0],
            covered: vec!["src/colors.ts", "src/main.ts"],
            coverage: vec![typed("", "app")],
            ..Expected::default()
        },
    );
}
