//! Typed class-member reads: a read credits the member the checker resolves it to, through unions,
//! overrides and implemented interfaces, and a read the checker cannot settle is credited by name.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use common::{Expected, TSCONFIG, Workspace, assert_case, assert_facts, package, query, run, typed};
use polylint_js_types::{DeclKind, DeclQuery, TypePackage, TypeQuery};

const WIDGETS: &str = "\
export class Widget {
  render(): void {}
  paint(): void {}
}

export class Gadget {
  render(): void {}
}
";

fn widgets(main: &str) -> Workspace {
    Workspace::new(&[
        ("tsconfig.json", TSCONFIG),
        ("src/widgets.ts", WIDGETS),
        ("src/main.ts", main),
    ])
}

fn widgets_query(workspace: &Workspace) -> TypeQuery {
    query(vec![
        workspace.decl("src/widgets.ts", DeclKind::Class, "Widget", &["render", "paint"]),
        workspace.decl("src/widgets.ts", DeclKind::Class, "Gadget", &["render"]),
    ])
}

fn widgets_package() -> TypePackage {
    package("", "app", &["src/widgets.ts", "src/main.ts"])
}

#[test]
fn a_typed_receiver_credits_its_class_and_not_a_same_named_member_of_another_class() {
    let workspace = widgets(
        "import { Widget } from './widgets';\n\nexport function draw(widget: Widget): void {\n  widget.render();\n}\n",
    );
    let facts = run(&workspace, &[widgets_package()], &widgets_query(&workspace));
    assert_facts(
        &facts,
        &Expected {
            reads: vec![(0, vec!["render"])],
            covered: vec!["src/widgets.ts", "src/main.ts"],
            coverage: vec![typed("", "app")],
            ..Expected::default()
        },
    );
}

#[test]
fn two_classes_with_one_member_name_are_credited_by_their_own_receivers() {
    let workspace =
        widgets("import { Gadget, Widget } from './widgets';\n\nnew Widget().paint();\nnew Gadget().render();\n");
    let facts = run(&workspace, &[widgets_package()], &widgets_query(&workspace));
    assert_facts(
        &facts,
        &Expected {
            reads: vec![(0, vec!["paint"]), (1, vec!["render"])],
            covered: vec!["src/widgets.ts", "src/main.ts"],
            coverage: vec![typed("", "app")],
            ..Expected::default()
        },
    );
}

#[test]
fn a_union_receiver_credits_the_member_on_every_class_of_the_union() {
    let workspace = widgets(
        "import { Gadget, Widget } from './widgets';\n\nexport function draw(item: Widget | Gadget): void {\n  item.render();\n}\n",
    );
    let facts = run(&workspace, &[widgets_package()], &widgets_query(&workspace));
    assert_facts(
        &facts,
        &Expected {
            reads: vec![(0, vec!["render"]), (1, vec!["render"])],
            covered: vec!["src/widgets.ts", "src/main.ts"],
            coverage: vec![typed("", "app")],
            ..Expected::default()
        },
    );
}

#[test]
fn reads_of_members_outside_the_query_are_not_reported() {
    let workspace = widgets("import { Widget } from './widgets';\n\nnew Widget().paint();\n");
    let query = query(vec![workspace.decl(
        "src/widgets.ts",
        DeclKind::Class,
        "Widget",
        &["render"],
    )]);
    let facts = run(&workspace, &[widgets_package()], &query);
    assert_facts(
        &facts,
        &Expected {
            covered: vec!["src/widgets.ts", "src/main.ts"],
            coverage: vec![typed("", "app")],
            ..Expected::default()
        },
    );
}

const SHAPES: &str = "\
export class Base {
  render(): void {}
  paint(): void {}
}

export class Derived extends Base {
  override render(): void {}
}

export class Unrelated {
  render(): void {}
}
";

fn shapes(main: &str) -> (Workspace, TypeQuery) {
    let workspace = Workspace::new(&[
        ("tsconfig.json", TSCONFIG),
        ("src/shapes.ts", SHAPES),
        ("src/main.ts", main),
    ]);
    let query = query(vec![
        workspace.decl("src/shapes.ts", DeclKind::Class, "Base", &["render", "paint"]),
        workspace.decl("src/shapes.ts", DeclKind::Class, "Derived", &["render"]),
        workspace.decl("src/shapes.ts", DeclKind::Class, "Unrelated", &["render"]),
    ]);
    (workspace, query)
}

#[test]
fn a_read_through_the_base_class_credits_the_overriding_members() {
    let (workspace, query) =
        shapes("import { Base } from './shapes';\n\nexport function draw(shape: Base): void {\n  shape.render();\n}\n");
    let facts = run(
        &workspace,
        &[package("", "app", &["src/shapes.ts", "src/main.ts"])],
        &query,
    );
    assert_facts(
        &facts,
        &Expected {
            reads: vec![(0, vec!["render"]), (1, vec!["render"])],
            covered: vec!["src/shapes.ts", "src/main.ts"],
            coverage: vec![typed("", "app")],
            ..Expected::default()
        },
    );
}

#[test]
fn an_inherited_member_read_through_the_subclass_credits_the_base_class() {
    let (workspace, query) = shapes("import { Derived } from './shapes';\n\nnew Derived().paint();\n");
    let facts = run(
        &workspace,
        &[package("", "app", &["src/shapes.ts", "src/main.ts"])],
        &query,
    );
    assert_facts(
        &facts,
        &Expected {
            reads: vec![(0, vec!["paint"])],
            covered: vec!["src/shapes.ts", "src/main.ts"],
            coverage: vec![typed("", "app")],
            ..Expected::default()
        },
    );
}

const AREAS: &str = "\
export interface Shape {
  area(): number;
}

export class Square implements Shape {
  area(): number {
    return 1;
  }
}

export class Circle implements Shape {
  area(): number {
    return 2;
  }
}

export class Table {
  area(): number {
    return 3;
  }
}
";

#[test]
fn a_read_through_an_interface_credits_the_classes_that_implement_it() {
    let workspace = Workspace::new(&[
        ("tsconfig.json", TSCONFIG),
        ("src/areas.ts", AREAS),
        (
            "src/main.ts",
            "import type { Shape } from './areas';\n\nexport function total(shape: Shape): number {\n  return shape.area();\n}\n",
        ),
    ]);
    let query = query(vec![
        workspace.decl("src/areas.ts", DeclKind::Class, "Square", &["area"]),
        workspace.decl("src/areas.ts", DeclKind::Class, "Circle", &["area"]),
        workspace.decl("src/areas.ts", DeclKind::Class, "Table", &["area"]),
    ]);
    let facts = run(
        &workspace,
        &[package("", "app", &["src/areas.ts", "src/main.ts"])],
        &query,
    );
    assert_facts(
        &facts,
        &Expected {
            reads: vec![(0, vec!["area"]), (1, vec!["area"])],
            covered: vec!["src/areas.ts", "src/main.ts"],
            coverage: vec![typed("", "app")],
            ..Expected::default()
        },
    );
}

const CELL_READER: &str = "\
import type { ICell } from './cell';
import { CodeCell } from './cell';

export function f(c: ICell): number {
  return c.getRange();
}

export const make = (): CodeCell => new CodeCell();
";

/// A read through an interface credits the member the implementing class has, its own or one it inherits from a base
/// class, and only that member. The implementing class declares no judged member, so the host does not ask about it.
#[test]
fn a_read_through_an_interface_credits_the_member_the_implementing_class_inherits() {
    for (case, cell, asked, reads) in [
        (
            "a member of the base class",
            "export interface ICell {\n  getRange(): number;\n}\n\n\
             export abstract class BaseCell {\n  getRange(): number {\n    return 1;\n  }\n}\n\n\
             export class CodeCell extends BaseCell implements ICell {}\n",
            vec![("BaseCell", vec!["getRange"])],
            vec![(0, vec!["getRange"])],
        ),
        (
            "a member of the base class of the base class",
            "export interface ICell {\n  getRange(): number;\n}\n\n\
             export abstract class BaseCell {\n  getRange(): number {\n    return 1;\n  }\n}\n\n\
             export abstract class MidCell extends BaseCell {}\n\n\
             export class CodeCell extends MidCell implements ICell {}\n",
            vec![("BaseCell", vec!["getRange"])],
            vec![(0, vec!["getRange"])],
        ),
        (
            "a member the interface inherits from another interface",
            "export interface IBase {\n  getRange(): number;\n}\n\n\
             export interface ICell extends IBase {}\n\n\
             export abstract class BaseCell {\n  getRange(): number {\n    return 1;\n  }\n}\n\n\
             export class CodeCell extends BaseCell implements ICell {}\n",
            vec![("BaseCell", vec!["getRange"])],
            vec![(0, vec!["getRange"])],
        ),
        (
            "a base member the interface does not declare",
            "export interface ICell {\n  getRange(): number;\n}\n\n\
             export abstract class BaseCell {\n  getRange(): number {\n    return 1;\n  }\n  getSize(): number {\n    return 2;\n  }\n}\n\n\
             export class CodeCell extends BaseCell implements ICell {}\n",
            vec![("BaseCell", vec!["getRange", "getSize"])],
            vec![(0, vec!["getRange"])],
        ),
        (
            "an override nearer than the base member",
            "export interface ICell {\n  getRange(): number;\n}\n\n\
             export abstract class BaseCell {\n  getRange(): number {\n    return 1;\n  }\n}\n\n\
             export abstract class MidCell extends BaseCell {\n  override getRange(): number {\n    return 2;\n  }\n}\n\n\
             export class CodeCell extends MidCell implements ICell {}\n",
            vec![("BaseCell", vec!["getRange"]), ("MidCell", vec!["getRange"])],
            vec![(1, vec!["getRange"])],
        ),
    ] {
        let workspace = Workspace::new(&[
            ("tsconfig.json", TSCONFIG),
            ("src/cell.ts", cell),
            ("src/use.ts", CELL_READER),
        ]);
        let query = query(
            asked
                .iter()
                .map(|(class, members)| workspace.decl("src/cell.ts", DeclKind::Class, class, members))
                .collect(),
        );
        let facts = run(
            &workspace,
            &[package("", "app", &["src/cell.ts", "src/use.ts"])],
            &query,
        );
        assert_case(
            case,
            &facts,
            &Expected {
                reads,
                covered: vec!["src/cell.ts", "src/use.ts"],
                coverage: vec![typed("", "app")],
                ..Expected::default()
            },
        );
    }
}

#[test]
fn a_read_the_checker_cannot_settle_is_credited_by_name() {
    for (receiver, main) in [
        (
            "an any parameter",
            "export function draw(item: any): void {\n  item.render();\n  item.unjudged();\n}\n",
        ),
        (
            "an undeclared global",
            "undeclaredWidget.render();\nundeclaredWidget.unjudged();\n",
        ),
        (
            "a cast to an unknown type",
            "const item = {} as MissingType;\nitem.render();\nitem.unjudged();\n",
        ),
    ] {
        let workspace = widgets(main);
        let facts = run(&workspace, &[widgets_package()], &widgets_query(&workspace));
        assert_case(
            receiver,
            &facts,
            &Expected {
                unresolved_names: vec![("src/main.ts", vec!["render"])],
                covered: vec!["src/widgets.ts", "src/main.ts"],
                coverage: vec![typed("", "app")],
                ..Expected::default()
            },
        );
    }
}

#[test]
fn a_require_cast_to_typeof_import_credits_the_class_it_reads() {
    let workspace = widgets(
        "declare function require(id: string): unknown;\n\nconst widgets = require('./widgets') as typeof import('./widgets');\n\nnew widgets.Widget().render();\n",
    );
    let facts = run(&workspace, &[widgets_package()], &widgets_query(&workspace));
    assert_facts(
        &facts,
        &Expected {
            reads: vec![(0, vec!["render"])],
            covered: vec!["src/widgets.ts", "src/main.ts"],
            coverage: vec![typed("", "app")],
            ..Expected::default()
        },
    );
}

const PENS: &str = "\
export class Pen {
  draw() {}
}

export class Brush {
  draw() {}
}
";

const SKETCH: &str = "\
import { Pen } from './pens.js';

/** @param {Pen} pen */
export function sketch(pen) {
  pen.draw();
}
";

#[test]
fn checked_javascript_credits_reads_through_jsdoc_types() {
    for (config_file, config) in [
        (
            "tsconfig.json",
            r#"{ "compilerOptions": { "allowJs": true, "checkJs": true, "noEmit": true, "strict": true, "module": "esnext", "moduleResolution": "bundler" }, "include": ["src"] }"#,
        ),
        (
            "jsconfig.json",
            r#"{ "compilerOptions": { "checkJs": true, "strict": true, "module": "esnext", "moduleResolution": "bundler" }, "include": ["src"] }"#,
        ),
    ] {
        let workspace = Workspace::new(&[(config_file, config), ("src/pens.js", PENS), ("src/sketch.js", SKETCH)]);
        let query = query(vec![
            workspace.decl("src/pens.js", DeclKind::Class, "Pen", &["draw"]),
            workspace.decl("src/pens.js", DeclKind::Class, "Brush", &["draw"]),
        ]);
        let facts = run(
            &workspace,
            &[package("", "app", &["src/pens.js", "src/sketch.js"])],
            &query,
        );
        assert_case(
            config_file,
            &facts,
            &Expected {
                reads: vec![(0, vec!["draw"])],
                covered: vec!["src/pens.js", "src/sketch.js"],
                coverage: vec![typed("", "app")],
                ..Expected::default()
            },
        );
    }
}

const UNICODE_WIDGETS: &str = "\u{feff}// Ünïcödé — 日本語 🚀 before every declaration
export const greeting = \"héllo wörld 🌍\";
export const ñandú = 1;

export class Ŝecond {
  render(): void {}
}

export class Wïdget {
  render(): void {}
  paint(): void {}
}
";

#[test]
fn declarations_after_a_bom_and_non_ascii_text_join_on_utf8_offsets() {
    let workspace = Workspace::new(&[
        ("tsconfig.json", TSCONFIG),
        ("src/widgets.ts", UNICODE_WIDGETS),
        (
            "src/main.ts",
            "\u{feff}// Ärger über Ünicode 🎉\nimport { Wïdget } from './widgets';\n\nconst label = 'ünïcödé 🌍';\nnew Wïdget().render();\nexport { label };\n",
        ),
    ]);
    let query = query(vec![
        workspace.decl("src/widgets.ts", DeclKind::Class, "Ŝecond", &["render"]),
        workspace.decl("src/widgets.ts", DeclKind::Class, "Wïdget", &["render", "paint"]),
    ]);
    let facts = run(
        &workspace,
        &[package("", "app", &["src/widgets.ts", "src/main.ts"])],
        &query,
    );
    assert_facts(
        &facts,
        &Expected {
            reads: vec![(1, vec!["render"])],
            covered: vec!["src/widgets.ts", "src/main.ts"],
            coverage: vec![typed("", "app")],
            ..Expected::default()
        },
    );
}

#[test]
fn an_interface_implemented_through_a_namespace_import_credits_the_class() {
    let workspace = Workspace::new(&[
        ("tsconfig.json", TSCONFIG),
        ("src/shape.ts", "export interface Shape {\n  area(): number;\n}\n"),
        (
            "src/square.ts",
            "import type * as shapes from './shape';\n\nexport class Square implements shapes.Shape {\n  area(): number {\n    return 1;\n  }\n}\n",
        ),
        (
            "src/main.ts",
            "import type { Shape } from './shape';\n\nexport function total(shape: Shape): number {\n  return shape.area();\n}\n",
        ),
    ]);
    let query = query(vec![workspace.decl(
        "src/square.ts",
        DeclKind::Class,
        "Square",
        &["area"],
    )]);
    let facts = run(
        &workspace,
        &[package("", "app", &["src/main.ts", "src/shape.ts", "src/square.ts"])],
        &query,
    );
    assert_facts(
        &facts,
        &Expected {
            reads: vec![(0, vec!["area"])],
            covered: vec!["src/main.ts", "src/shape.ts", "src/square.ts"],
            coverage: vec![typed("", "app")],
            ..Expected::default()
        },
    );
}

/// Syntax where a name, key or heritage element is not an identifier, or where a read sits in a class member other
/// than a method: tsrs's accessors panic on a node of another kind, which would leave the package untyped
/// (`checker-failed`), so each case also asserts the package stays typed.
#[test]
fn reads_through_names_of_every_kind_credit_their_member_and_keep_the_package_typed() {
    const IMPORT: &str = "import { Widget } from './widgets';\n\n";
    for (case, body, reads) in [
        (
            "a computed key and a shorthand in a binding pattern",
            "const key = 'paint';\n\nexport function draw(widget: Widget): unknown[] {\n  const { [key]: painted, render } = widget;\n  return [painted, render];\n}\n",
            vec![(0, vec!["render"])],
        ),
        (
            "a string-literal and a computed key in a destructuring assignment",
            "declare const widget: Widget;\nconst key = 'render';\nexport let painted: unknown;\nexport let rendered: unknown;\n({ 'paint': painted, [key]: rendered } = widget);\n",
            vec![(0, vec!["paint"])],
        ),
        (
            "a string-literal and a template-literal element access",
            "declare const widget: Widget;\nwidget['paint']();\nwidget[`render`]();\n",
            vec![(0, vec!["paint", "render"])],
        ),
        (
            "an element access with a non-literal key",
            "declare const widget: Widget;\ndeclare const key: 'render' | 'paint';\nwidget[key]();\n",
            vec![],
        ),
        (
            "a string-literal `in` and an indexed access type",
            "declare const widget: Widget;\nexport const renders = 'render' in widget;\nexport type Paint = Widget['paint'];\n",
            vec![(0, vec!["render", "paint"])],
        ),
        (
            "a private name and a private `in`",
            "export class Holder {\n  #widget = new Widget();\n  draw(other: object): boolean {\n    this.#widget.render();\n    return #widget in other;\n  }\n}\n",
            vec![(0, vec!["render"])],
        ),
        (
            "an optional chain, a non-null assertion and `satisfies`",
            "export function draw(widget?: Widget): void {\n  widget?.render();\n  (widget! satisfies Widget).paint();\n}\n",
            vec![(0, vec!["render", "paint"])],
        ),
        (
            "`super` in an anonymous default-exported class",
            "export default class extends Widget {\n  override render(): void {\n    super.render();\n  }\n}\n",
            vec![(0, vec!["render"])],
        ),
        (
            "a class expression bound to a variable",
            "export const Fancy = class extends Widget {};\nnew Fancy().paint();\n",
            vec![(0, vec!["paint"])],
        ),
        (
            "a decorator argument",
            "declare function tag(value: unknown): (target: unknown, context: ClassDecoratorContext) => void;\ndeclare const widget: Widget;\n\n@tag(widget.paint)\nexport class Tagged {}\n",
            vec![(0, vec!["paint"])],
        ),
        (
            "a parameter property read in an accessor and an overload, beside an index signature",
            "export class Panel {\n  [name: string]: unknown;\n  constructor(private readonly widget: Widget) {}\n  get painted(): void {\n    return this.widget.paint();\n  }\n  draw(): void;\n  draw(times: number): void;\n  draw(_times?: number): void {\n    this.widget.render();\n  }\n}\n",
            vec![(0, vec!["paint", "render"])],
        ),
        (
            "a class merged with a namespace",
            "export class Merged extends Widget {}\nexport namespace Merged {\n  export const made = new Merged();\n}\nMerged.made.render();\n",
            vec![(0, vec!["render"])],
        ),
    ] {
        let workspace = widgets(&format!("{IMPORT}{body}"));
        let facts = run(&workspace, &[widgets_package()], &widgets_query(&workspace));
        assert_case(
            case,
            &facts,
            &Expected {
                reads,
                covered: vec!["src/widgets.ts", "src/main.ts"],
                coverage: vec![typed("", "app")],
                ..Expected::default()
            },
        );
    }
}

const BRUSH: &str = "\
export default class {
  render(): void {}
  paint(): void {}
}
";

#[test]
fn an_anonymous_default_class_is_keyed_past_its_closing_brace() {
    for (case, main, reads, whole) in [
        (
            "a default import",
            "import Brush from './brush';\n\nnew Brush().paint();\n",
            vec![(0, vec!["paint"])],
            vec![],
        ),
        (
            "a namespace import",
            "import * as brushes from './brush';\n\nnew brushes.default().paint();\n",
            vec![(0, vec!["paint"])],
            vec![],
        ),
        (
            "a default re-export",
            "import { Brush } from './reexport';\n\nnew Brush().paint();\n",
            vec![(0, vec!["paint"])],
            vec![],
        ),
        (
            "a whole use",
            "import Brush from './brush';\n\nexport function keep(_value: unknown): void {}\n\nkeep(Brush);\n",
            vec![],
            vec![0],
        ),
    ] {
        let workspace = Workspace::new(&[
            ("tsconfig.json", TSCONFIG),
            ("src/brush.ts", BRUSH),
            ("src/reexport.ts", "export { default as Brush } from './brush';\n"),
            ("src/main.ts", main),
        ]);
        let end = BRUSH.rfind('}').unwrap() + 1;
        let query = query(vec![DeclQuery {
            file: common::abs("src/brush.ts"),
            name_start: u32::try_from(end).unwrap(),
            kind: DeclKind::Class,
            members: vec!["render".to_owned(), "paint".to_owned()],
        }]);
        let facts = run(
            &workspace,
            &[package("", "app", &["src/brush.ts", "src/main.ts", "src/reexport.ts"])],
            &query,
        );
        assert_case(
            case,
            &facts,
            &Expected {
                reads,
                whole,
                covered: vec!["src/brush.ts", "src/main.ts", "src/reexport.ts"],
                coverage: vec![typed("", "app")],
                ..Expected::default()
            },
        );
    }
}

/// `Square` declares no heritage clause itself; the interface merged into it extends `Shape`, so a read through `Shape`
/// still credits `Square`'s member. `Table` has neither and stays uncredited.
#[test]
fn a_read_through_an_interface_extended_by_an_interface_merged_into_a_class_credits_the_class() {
    let workspace = Workspace::new(&[
        ("tsconfig.json", TSCONFIG),
        (
            "src/areas.ts",
            "export interface Shape {\n  area(): number;\n}\n\nexport interface Square extends Shape {}\n\nexport class Square {\n  area(): number {\n    return 4;\n  }\n}\n\nexport class Table {\n  area(): number {\n    return 3;\n  }\n}\n",
        ),
        (
            "src/main.ts",
            "import type { Shape } from './areas';\n\nexport function total(shape: Shape): number {\n  return shape.area();\n}\n",
        ),
    ]);
    let query = query(vec![
        workspace.decl("src/areas.ts", DeclKind::Class, "Square", &["area"]),
        workspace.decl("src/areas.ts", DeclKind::Class, "Table", &["area"]),
    ]);
    let facts = run(
        &workspace,
        &[package("", "app", &["src/areas.ts", "src/main.ts"])],
        &query,
    );
    assert_facts(
        &facts,
        &Expected {
            reads: vec![(0, vec!["area"])],
            covered: vec!["src/areas.ts", "src/main.ts"],
            coverage: vec![typed("", "app")],
            ..Expected::default()
        },
    );
}
