//! The question the host asks the checker and the facts it gets back. Every type is owned data; no tsrs type leaks.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::time::Duration;

/// The declarations the host wants typed facts for. The index of a declaration in [`TypeQuery::decls`] is its
/// [`DeclId`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TypeQuery {
    /// The declarations, in the order the host numbers them.
    pub decls: Vec<DeclQuery>,
}

/// One declaration whose member reads the host asks about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeclQuery {
    /// The absolute path of the file declaring it.
    pub file: PathBuf,
    /// The UTF-8 byte offset of the declaration's name in the file text, as oxc spans count; for an anonymous
    /// `export default class {}`, which has no name, the offset just past its closing brace.
    pub name_start: u32,
    /// What it declares.
    pub kind: DeclKind,
    /// The names of the members the host judges; reads of other members are not reported.
    pub members: Vec<String>,
}

/// The kind of a [`DeclQuery`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum DeclKind {
    /// A class: its methods, fields and accessors.
    Class,
    /// An enum: its members.
    Enum,
    /// A namespace: its exported members.
    Namespace,
}

/// The index of a declaration in [`TypeQuery::decls`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DeclId(pub u32);

/// What the checker found for a [`TypeQuery`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct TypeFacts {
    /// The engine that produced the facts, with its version (`tsrs 7.1.0-beta.tsrs2`).
    pub engine: &'static str,
    /// The members of each declaration that a typed read reaches, directly or through a base class or an interface
    /// the declaration implements, or that a subclass inheriting the member implements.
    pub reads: BTreeMap<DeclId, BTreeSet<String>>,
    /// The declarations used as a whole (`keyof C`, `Object.values(E)`, an alias passed on), whose members all count
    /// as used.
    pub whole: BTreeSet<DeclId>,
    /// Per file, the member names read through a receiver the checker could not type (`any`, an error type, an
    /// unresolved module): these names count as read on every declaration, as without types.
    pub unresolved_names: BTreeMap<PathBuf, BTreeSet<String>>,
    /// The absolute paths of the files the checker covered. A file outside every program keeps the untyped analysis.
    pub covered: BTreeSet<PathBuf>,
    /// The type coverage of every package that has judged files.
    pub coverage: Vec<PackageCoverage>,
    /// How long the stages took.
    pub timings: TypeTimings,
}

/// The type coverage of one package.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageCoverage {
    /// The package directory, absolute.
    pub dir: PathBuf,
    /// The package name, when it has one.
    pub name: Option<String>,
    /// How much of the package is typed.
    pub state: CoverageState,
    /// Why the package is not fully typed; `None` when it is.
    pub reason: Option<CoverageReason>,
    /// The number of judged files of the package that stay untyped.
    pub untyped_files: u32,
}

/// How much of a package the checker covered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum CoverageState {
    /// Every judged file is in a program and its imports resolve.
    Typed,
    /// Some files are untyped or some imports do not resolve.
    Partial,
    /// No file of the package is typed.
    Untyped,
}

/// Why a package is not fully typed.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum CoverageReason {
    /// Type awareness is turned off.
    Disabled,
    /// The host has the type-aware analysis off (skif without its `type-aware` feature).
    NotBuilt,
    /// No `tsconfig.json` or `jsconfig.json` applies to the package.
    NoTsconfig,
    /// The package's `tsconfig` cannot be parsed.
    TsconfigError,
    /// The package has no TypeScript or checked JavaScript files.
    NotTypescript,
    /// This many imports resolve to no module, mostly dependencies that are not installed.
    UnresolvedModules(u32),
    /// This many judged files are in no program.
    FilesOutsidePrograms(u32),
    /// The checker failed on the package's program.
    CheckerFailed,
    /// The host cancelled the analysis.
    Cancelled,
}

impl CoverageReason {
    /// The reason as skif's report spells it, a stable label: `no-tsconfig`, `unresolved-modules: 3`.
    #[must_use]
    pub fn label(&self) -> String {
        match self {
            Self::Disabled => "disabled".to_owned(),
            Self::NotBuilt => "not-built".to_owned(),
            Self::NoTsconfig => "no-tsconfig".to_owned(),
            Self::TsconfigError => "tsconfig-error".to_owned(),
            Self::NotTypescript => "not-typescript".to_owned(),
            Self::UnresolvedModules(count) => format!("unresolved-modules: {count}"),
            Self::FilesOutsidePrograms(count) => format!("files-outside-programs: {count}"),
            Self::CheckerFailed => "checker-failed".to_owned(),
            Self::Cancelled => "cancelled".to_owned(),
        }
    }
}

/// How long the type-aware stages took.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct TypeTimings {
    /// Finding the config of every judged file, before any program is built.
    pub planning: Duration,
    /// Reading the configs of the programs, building and binding them, summed over the programs: several build at
    /// once, so the sum can exceed the wall time.
    pub programs: Duration,
    /// Reading and parsing the programs' own configs, summed over the programs; a part of [`TypeTimings::programs`].
    pub config_parse: Duration,
    /// Creating the programs (reading, parsing and resolving their files), summed over the programs; a part of
    /// [`TypeTimings::programs`].
    pub program_creation: Duration,
    /// Binding the programs' source files, summed over the programs; a part of [`TypeTimings::programs`].
    pub bind: Duration,
    /// The project files (not the lib or `node_modules` declaration files the run keeps) the programs this analysis
    /// built parsed: the programs of a run share a project file parsed with the same options, so a file in several
    /// programs counts once for each set of parse options, and a file another analysis's program had parsed and still
    /// holds counts nothing.
    pub project_files_parsed: usize,
    /// The distinct project files of the programs this analysis built; a gap from
    /// [`TypeTimings::project_files_parsed`] up to it is parsing repeated across programs. Counted only with timings
    /// on.
    pub project_files_distinct: usize,
    /// Walking the files and asking the checker, summed over the programs as [`TypeTimings::programs`] is.
    pub queries: Duration,
    /// Waiting for cores before building, summed over the programs as [`TypeTimings::programs`] is: on a
    /// [`TypeBudget`](crate::TypeBudget) the analyses of a run share, the time a program waited for other analyses';
    /// with [`TypePrograms`](crate::TypePrograms), also the time it waited for another analysis to build it.
    pub cores_waited: Duration,
    /// The wall time from the start of the first program to the end of the last.
    pub wall: Duration,
    /// The wall time from the query being handed over to the facts: what the analysis adds to the host's own work
    /// when the programs start before the query is known ([`crate::start`]).
    pub waited: Duration,
    /// The programs that started building before the query was handed over and were then walked.
    pub early: usize,
    /// The threads the programs shared.
    pub threads: usize,
    /// Every built or shared program the analysis walked, in the order of their configs.
    pub per_program: Vec<ProgramTiming>,
    /// The lib and `node_modules` declaration files the programs this analysis built read, by how each got them.
    pub shared_files: SharedFileCounts,
    /// The configs this analysis's plannings and programs read (the configs it plans files into and those their
    /// project references reach), by how each got them.
    pub shared_configs: SharedConfigCounts,
    /// The configs those configs extend, by how each got them.
    pub shared_extended_configs: SharedConfigCounts,
}

impl TypeTimings {
    /// The `count` programs that took longest to build and walk, slowest first; programs that took as long come in
    /// the order of their configs.
    #[must_use]
    pub fn slowest(&self, count: usize) -> Vec<&ProgramTiming> {
        let mut programs: Vec<&ProgramTiming> = self.per_program.iter().collect();
        programs.sort_by(|a, b| b.total().cmp(&a.total()).then_with(|| a.config.cmp(&b.config)));
        programs.truncate(count);
        programs
    }
}

/// How the programs got the files they share with the other programs of the run (the bundled lib files and the
/// declaration files below `node_modules`), one count per file a program read: each is parsed and bound once for the
/// run, the first time a program reads it, and reused by every later program.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct SharedFileCounts {
    /// Files a program read, parsed and bound for the run.
    pub parsed: usize,
    /// Files a program took parsed and bound from the run.
    pub reused: usize,
    /// Files a program parsed for itself alone, because the thread that loaded them was in a checker's memory scope.
    pub unshared: usize,
}

impl From<polylint_tsrs::ParseCounts> for SharedFileCounts {
    fn from(counts: polylint_tsrs::ParseCounts) -> Self {
        Self {
            parsed: counts.parsed,
            reused: counts.reused,
            unshared: counts.unshared,
        }
    }
}

/// How the plannings and programs of an analysis got the configs they read (skif ADR 0010), one count per request: each
/// config is read and parsed once for the run, the first time a planning or a program asks for it, and reused by every
/// later request. A program still parses the config it is built from, with the configs that one extends from the run.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct SharedConfigCounts {
    /// Configs a request read and parsed for the run.
    pub parsed: usize,
    /// Configs a request took parsed from the run.
    pub reused: usize,
    /// Configs a request parsed for itself alone: another thread was parsing them, or the request came from a
    /// checker's memory scope.
    pub unshared: usize,
}

impl From<polylint_tsrs::ParseCounts> for SharedConfigCounts {
    fn from(counts: polylint_tsrs::ParseCounts) -> Self {
        Self {
            parsed: counts.parsed,
            reused: counts.reused,
            unshared: counts.unshared,
        }
    }
}

/// How long one program took, with its size and the checkers it ran on.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct ProgramTiming {
    /// The config the program was built from, absolute.
    pub config: PathBuf,
    /// The judged files planned into the program.
    pub judged_files: usize,
    /// Every file of the program, with the declarations of its dependencies and the default libraries.
    pub files: usize,
    /// The checkers the analysis gave it; tsrs makes no more than the program has files.
    pub checkers: usize,
    /// Reading its config, building and binding it; zero when another analysis of the run built it.
    pub build: Duration,
    /// Walking its judged files on its checkers.
    pub query: Duration,
    /// Whether another analysis of the run built it, for this one to walk ([`TypePrograms`](crate::TypePrograms)).
    pub shared: bool,
}

impl ProgramTiming {
    fn total(&self) -> Duration {
        self.build.saturating_add(self.query)
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::time::Duration;

    use super::{CoverageReason, ProgramTiming, TypeTimings};

    fn program(config: &str, build: u64, query: u64) -> ProgramTiming {
        ProgramTiming {
            config: PathBuf::from(config),
            judged_files: 1,
            files: 1,
            checkers: 1,
            build: Duration::from_millis(build),
            query: Duration::from_millis(query),
            shared: false,
        }
    }

    #[test]
    fn the_slowest_programs_come_first_by_build_and_query_time_with_ties_in_config_order() {
        let timings = TypeTimings {
            per_program: vec![
                program("/ws/a/tsconfig.json", 10, 10),
                program("/ws/b/tsconfig.json", 5, 40),
                program("/ws/c/tsconfig.json", 20, 0),
                program("/ws/d/tsconfig.json", 1, 1),
            ],
            ..TypeTimings::default()
        };
        let configs: Vec<&str> = timings
            .slowest(3)
            .iter()
            .map(|program| program.config.to_str().unwrap_or_default())
            .collect();
        assert_eq!(
            configs,
            ["/ws/b/tsconfig.json", "/ws/a/tsconfig.json", "/ws/c/tsconfig.json"]
        );
        assert_eq!(timings.slowest(10).len(), 4);
        assert_eq!(TypeTimings::default().slowest(5), Vec::<&ProgramTiming>::new());
    }

    #[test]
    fn reasons_are_labelled_as_the_report_spells_them() {
        let cases = [
            (CoverageReason::Disabled, "disabled"),
            (CoverageReason::NotBuilt, "not-built"),
            (CoverageReason::NoTsconfig, "no-tsconfig"),
            (CoverageReason::TsconfigError, "tsconfig-error"),
            (CoverageReason::NotTypescript, "not-typescript"),
            (CoverageReason::UnresolvedModules(3), "unresolved-modules: 3"),
            (CoverageReason::FilesOutsidePrograms(2), "files-outside-programs: 2"),
            (CoverageReason::CheckerFailed, "checker-failed"),
            (CoverageReason::Cancelled, "cancelled"),
        ];
        for (reason, label) in cases {
            assert_eq!(reason.label(), label);
        }
    }
}
