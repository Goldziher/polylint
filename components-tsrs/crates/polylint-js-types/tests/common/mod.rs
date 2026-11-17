//! The harness shared by the end-to-end tests of `polylint_js_types::analyze`.
//!
//! Every case is a small in-memory workspace below [`ROOT`] with a real `tsconfig.json`. Declarations are queried by
//! the UTF-8 byte offset of their name, found in the fixture text the way oxc's spans count it, so no offset is
//! written by hand. Expected facts name files relative to [`ROOT`] and declarations by their index in the query.

#![allow(dead_code, clippy::unwrap_used, clippy::expect_used)]

use std::collections::{BTreeMap, BTreeSet};
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use polylint_js_types::{
    CoverageReason, CoverageState, DeclId, DeclKind, DeclQuery, PackageCoverage, TypeError, TypeFacts, TypeInput,
    TypePackage, TypeQuery, analyze,
};
use polylint_js_types::{FileSystem, MemoryFs};

/// The workspace root every fixture lives below.
pub const ROOT: &str = "/ws";

/// A strict `tsconfig.json` over `src` with bundler resolution, so relative imports need no extension.
pub const TSCONFIG: &str = r#"{
  "compilerOptions": {
    "target": "es2022",
    "module": "esnext",
    "moduleResolution": "bundler",
    "strict": true,
    "noEmit": true
  },
  "include": ["src"]
}
"#;

/// The absolute path of `rel` below [`ROOT`]; `""` is the root itself.
pub fn abs(rel: &str) -> PathBuf {
    if rel.is_empty() {
        PathBuf::from(ROOT)
    } else {
        Path::new(ROOT).join(rel)
    }
}

/// The files of a fixture workspace, relative to [`ROOT`].
#[derive(Debug, Clone, Default)]
pub struct Workspace {
    files: Vec<(String, String)>,
}

impl Workspace {
    /// A workspace of `(relative path, text)` pairs.
    pub fn new(files: &[(&str, &str)]) -> Self {
        Self {
            files: files
                .iter()
                .map(|(rel, text)| ((*rel).to_owned(), (*text).to_owned()))
                .collect(),
        }
    }

    /// A workspace of generated files.
    pub fn from_owned(files: Vec<(String, String)>) -> Self {
        Self { files }
    }

    /// The text of the file at `rel`.
    pub fn text(&self, rel: &str) -> &str {
        self.files
            .iter()
            .find(|(path, _)| path == rel)
            .map_or_else(|| panic!("the fixture has no file {rel}"), |(_, text)| text.as_str())
    }

    /// The workspace as a file system.
    pub fn fs(&self) -> Arc<dyn FileSystem> {
        let memory = self
            .files
            .iter()
            .fold(MemoryFs::new(), |memory, (rel, text)| memory.with_file(abs(rel), text));
        Arc::new(memory)
    }

    /// The query for the declaration `name` of `kind` in the file at `rel`, judging `members`.
    pub fn decl(&self, rel: &str, kind: DeclKind, name: &str, members: &[&str]) -> DeclQuery {
        DeclQuery {
            file: abs(rel),
            name_start: name_start(self.text(rel), kind, name),
            kind,
            members: members.iter().map(|member| (*member).to_owned()).collect(),
        }
    }
}

/// The UTF-8 byte offset of the name of the one declaration `<keyword> <name>` in `text`, where the keyword is
/// `class`, `enum` or `namespace` after `kind`.
///
/// # Panics
/// When `text` declares `name` of that kind other than exactly once.
pub fn name_start(text: &str, kind: DeclKind, name: &str) -> u32 {
    let keyword = match kind {
        DeclKind::Class => "class",
        DeclKind::Enum => "enum",
        DeclKind::Namespace => "namespace",
        _ => panic!("no fixture keyword for {kind:?}"),
    };
    let needle = format!("{keyword} {name}");
    let starts: Vec<usize> = text
        .match_indices(&needle)
        .map(|(start, _)| start)
        .filter(|start| ends_identifier(text, start + needle.len()))
        .filter(|start| starts_identifier(text, *start))
        .collect();
    assert_eq!(
        starts.len(),
        1,
        "expected exactly one `{needle}` in the fixture, found {}",
        starts.len()
    );
    u32::try_from(starts[0] + keyword.len() + 1).unwrap()
}

fn is_identifier_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '$'
}

fn ends_identifier(text: &str, end: usize) -> bool {
    text[end..].chars().next().is_none_or(|c| !is_identifier_char(c))
}

fn starts_identifier(text: &str, start: usize) -> bool {
    text[..start].chars().next_back().is_none_or(|c| !is_identifier_char(c))
}

/// A package at `dir` (relative to [`ROOT`]) named `name`, judging `files` (relative to [`ROOT`]).
pub fn package(dir: &str, name: &str, files: &[&str]) -> TypePackage {
    TypePackage {
        dir: abs(dir),
        name: Some(name.to_owned()),
        files: files.iter().map(|rel| abs(rel)).collect(),
    }
}

/// The query over `decls`, numbered in order.
pub fn query(decls: Vec<DeclQuery>) -> TypeQuery {
    TypeQuery { decls }
}

/// How a test changes the default input of [`run`].
#[derive(Debug, Default, Clone, Copy)]
pub struct Options<'o> {
    /// The `tsconfig` that replaces discovery for the root package, relative to [`ROOT`].
    pub tsconfig_override: Option<&'o str>,
    /// The checker threads.
    pub threads: Option<NonZeroUsize>,
    /// The cancellation flag.
    pub cancel: Option<&'o AtomicBool>,
}

/// Analyses `workspace` with the default input.
pub fn run(workspace: &Workspace, packages: &[TypePackage], query: &TypeQuery) -> TypeFacts {
    try_run(workspace, packages, query, Options::default()).expect("the analysis fails")
}

/// Analyses `workspace` with the input `options` describe.
pub fn try_run(
    workspace: &Workspace,
    packages: &[TypePackage],
    query: &TypeQuery,
    options: Options<'_>,
) -> Result<TypeFacts, TypeError> {
    let root = abs("");
    let tsconfig = options.tsconfig_override.map(abs);
    let mut input = TypeInput::new(workspace.fs(), &root, packages, query);
    if let Some(tsconfig) = &tsconfig {
        input = input.with_tsconfig_override(tsconfig);
    }
    if let Some(threads) = options.threads {
        input = input.with_threads(threads);
    }
    if let Some(cancel) = options.cancel {
        input = input.with_cancel(cancel);
    }
    analyze(&input)
}

/// The facts a test expects; every field defaults to empty, so a test states each fact it expects and no other.
#[derive(Debug, Default)]
pub struct Expected {
    /// `(declaration index, members)` pairs of [`TypeFacts::reads`]; a declaration without typed reads has no entry.
    pub reads: Vec<(u32, Vec<&'static str>)>,
    /// The declaration indices of [`TypeFacts::whole`].
    pub whole: Vec<u32>,
    /// `(relative file, names)` pairs of [`TypeFacts::unresolved_names`]; a file without unsettled reads has no entry.
    pub unresolved_names: Vec<(&'static str, Vec<&'static str>)>,
    /// The covered files, relative to [`ROOT`].
    pub covered: Vec<&'static str>,
    /// The coverage of every package, in the order of the packages.
    pub coverage: Vec<PackageCoverage>,
}

/// Asserts that `facts` hold exactly the `expected` reads, whole uses, unsettled names, covered files and coverage.
pub fn assert_facts(facts: &TypeFacts, expected: &Expected) {
    assert_case("", facts, expected);
}

/// [`assert_facts`] for one case of a parameterized test, named `case` in every failure.
pub fn assert_case(case: &str, facts: &TypeFacts, expected: &Expected) {
    let reads: BTreeMap<DeclId, BTreeSet<String>> = expected
        .reads
        .iter()
        .map(|(id, members)| (DeclId(*id), names(members)))
        .collect();
    let whole: BTreeSet<DeclId> = expected.whole.iter().map(|id| DeclId(*id)).collect();
    let unresolved_names: BTreeMap<PathBuf, BTreeSet<String>> = expected
        .unresolved_names
        .iter()
        .map(|(rel, members)| (abs(rel), names(members)))
        .collect();
    let covered: BTreeSet<PathBuf> = expected.covered.iter().map(|rel| abs(rel)).collect();
    assert_eq!(facts.reads, reads, "{case}: reads");
    assert_eq!(facts.whole, whole, "{case}: whole uses");
    assert_eq!(facts.unresolved_names, unresolved_names, "{case}: unresolved names");
    assert_eq!(facts.covered, covered, "{case}: covered files");
    assert_eq!(facts.coverage, expected.coverage, "{case}: coverage");
}

fn names(members: &[&str]) -> BTreeSet<String> {
    members.iter().map(|member| (*member).to_owned()).collect()
}

/// The coverage of a fully typed package.
pub fn typed(dir: &str, name: &str) -> PackageCoverage {
    coverage(dir, name, CoverageState::Typed, None, 0)
}

/// The coverage of the package at `dir` named `name`.
pub fn coverage(
    dir: &str,
    name: &str,
    state: CoverageState,
    reason: Option<CoverageReason>,
    untyped_files: u32,
) -> PackageCoverage {
    PackageCoverage {
        dir: abs(dir),
        name: Some(name.to_owned()),
        state,
        reason,
        untyped_files,
    }
}

/// A generated workspace large enough to spread over several checker threads, with the reads it should yield.
pub struct Generated {
    /// The files.
    pub workspace: Workspace,
    /// The packages, each judging every file of its own.
    pub packages: Vec<TypePackage>,
    /// The classes `C0`..`Cn` of every package, judging `render` and `paint`.
    pub query: TypeQuery,
    /// The typed reads: `render` on every class, through a union of it and its successor, and `paint` on the even
    /// ones.
    pub reads: BTreeMap<DeclId, BTreeSet<String>>,
}

/// `classes` class files, each with a file that reads it, plus one `any` read of `paint` in `src/use0.ts`.
pub fn generated(classes: u32) -> Generated {
    let mut files = vec![("tsconfig.json".to_owned(), TSCONFIG.to_owned())];
    for i in 0..classes {
        let next = (i + 1) % classes;
        files.push((
            format!("src/c{i}.ts"),
            format!("export class C{i} {{\n  render(): void {{}}\n  paint(): void {{}}\n}}\n"),
        ));
        let paint = if i % 2 == 0 {
            format!("new C{i}().paint();\n")
        } else {
            String::new()
        };
        let loose = if i == 0 {
            "export function loose(x: any): void {\n  x.paint();\n}\n"
        } else {
            ""
        };
        files.push((
            format!("src/use{i}.ts"),
            format!(
                "import {{ C{i} }} from './c{i}';\nimport {{ C{next} }} from './c{next}';\n\nexport function draw(item: C{i} | C{next}): void {{\n  item.render();\n}}\n{paint}{loose}"
            ),
        ));
    }
    let workspace = Workspace::from_owned(files);
    let query = query(
        (0..classes)
            .map(|i| {
                workspace.decl(
                    &format!("src/c{i}.ts"),
                    DeclKind::Class,
                    &format!("C{i}"),
                    &["render", "paint"],
                )
            })
            .collect(),
    );
    let judged: Vec<String> = (0..classes)
        .flat_map(|i| [format!("src/c{i}.ts"), format!("src/use{i}.ts")])
        .collect();
    let judged: Vec<&str> = judged.iter().map(String::as_str).collect();
    let reads = (0..classes)
        .map(|i| {
            let members: &[&str] = if i % 2 == 0 { &["render", "paint"] } else { &["render"] };
            (DeclId(i), names(members))
        })
        .collect();
    Generated {
        packages: vec![package("", "app", &judged)],
        workspace,
        query,
        reads,
    }
}

/// One package per entry of `classes` below `packages/p<k>`, each with its own `tsconfig.json` and so its own program,
/// holding that many classes in the shape of [`generated`]: programs of different sizes for the scheduler.
pub fn generated_programs(classes: &[u32]) -> Generated {
    let mut files = Vec::new();
    let mut decls = Vec::new();
    let mut packages = Vec::new();
    let mut reads = BTreeMap::new();
    for (index, &count) in classes.iter().enumerate() {
        let dir = format!("packages/p{index}");
        let one = generated(count);
        let offset = u32::try_from(decls.len()).unwrap();
        reads.extend(
            one.reads
                .into_iter()
                .map(|(id, members)| (DeclId(id.0 + offset), members)),
        );
        decls.extend(one.query.decls.into_iter().map(|decl| {
            let rel = decl.file.strip_prefix(ROOT).unwrap().to_owned();
            DeclQuery {
                file: abs(&dir).join(rel),
                ..decl
            }
        }));
        let mut judged = Vec::new();
        for (rel, text) in one.workspace.files {
            let rel = format!("{dir}/{rel}");
            if std::path::Path::new(&rel)
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("ts"))
            {
                judged.push(rel.clone());
            }
            files.push((rel, text));
        }
        let judged: Vec<&str> = judged.iter().map(String::as_str).collect();
        packages.push(package(&dir, &format!("p{index}"), &judged));
    }
    Generated {
        workspace: Workspace::from_owned(files),
        packages,
        query: query(decls),
        reads,
    }
}

/// One analysis of a run over several roots: its root, the packages it judges and its query, with the reads it should
/// yield.
pub struct RootAnalysis {
    /// The root, absolute.
    pub root: PathBuf,
    /// The root's one package, judging every file of the root.
    pub packages: Vec<TypePackage>,
    /// The classes of the root, in the shape of [`generated`].
    pub query: TypeQuery,
    /// The typed reads of the root's classes, numbered as its query numbers them.
    pub reads: BTreeMap<DeclId, BTreeSet<String>>,
}

/// A run over `roots`, `(dir, classes)` pairs relative to [`ROOT`] (`""` for [`ROOT`] itself), each holding that many
/// classes in the shape of [`generated`] below `<dir>/src` and no config of its own: one `tsconfig.json` at [`ROOT`]
/// includes the `src` directory of every root, as a repository's root config includes the files of roots nested in
/// it, so every root's judged files go to that one program.
pub fn roots_under_one_tsconfig(roots: &[(&str, u32)]) -> (Workspace, Vec<RootAnalysis>) {
    let include: Vec<String> = roots
        .iter()
        .map(|(dir, _)| format!("\"{}\"", below(dir, "src")))
        .collect();
    let tsconfig = TSCONFIG.replace(r#"["src"]"#, &format!("[{}]", include.join(", ")));
    let mut files = vec![("tsconfig.json".to_owned(), tsconfig)];
    let mut analyses = Vec::new();
    for (index, &(dir, classes)) in roots.iter().enumerate() {
        let one = generated(classes);
        let decls = one
            .query
            .decls
            .into_iter()
            .map(|decl| {
                let rel = decl.file.strip_prefix(ROOT).unwrap().to_owned();
                DeclQuery {
                    file: abs(dir).join(rel),
                    ..decl
                }
            })
            .collect();
        let mut judged = Vec::new();
        for (rel, text) in one.workspace.files {
            if rel == "tsconfig.json" {
                continue;
            }
            let rel = below(dir, &rel);
            judged.push(rel.clone());
            files.push((rel, text));
        }
        let judged: Vec<&str> = judged.iter().map(String::as_str).collect();
        analyses.push(RootAnalysis {
            root: abs(dir),
            packages: vec![package(dir, &format!("r{index}"), &judged)],
            query: query(decls),
            reads: one.reads,
        });
    }
    (Workspace::from_owned(files), analyses)
}

/// `rel` below the directory `dir` relative to [`ROOT`].
fn below(dir: &str, rel: &str) -> String {
    if dir.is_empty() {
        rel.to_owned()
    } else {
        format!("{dir}/{rel}")
    }
}
