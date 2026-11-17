//! What became of each program, and how the outcomes join into the facts.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::time::Duration;

use polylint_tsrs::{BuildSplit, ProgramOwner};

use crate::coverage::{self, FileState};
use crate::members::{self, Candidates, Found};
use crate::programs::{Placement, PlannedProgram, ProgramPlan};
use crate::query::FileFacts;
use crate::{ENGINE, ProgramTiming, TypeError, TypeFacts, TypeInput, TypeTimings};

/// What became of one program.
pub(super) enum Outcome {
    Built(BuiltProgram),
    ConfigError,
    Failed,
    Cancelled,
}

/// The owned facts of a built program.
pub(super) struct BuiltProgram {
    pub(super) walked: Vec<WalkedFile>,
    /// Planned files the program turned out not to hold.
    pub(super) missing: Vec<usize>,
    /// Every file of the program.
    pub(super) files: usize,
    pub(super) checkers: usize,
    pub(super) build: Duration,
    /// Where `build` went; zero when another analysis of the run built the program.
    pub(super) split: BuildSplit,
    /// The names of the files below the project (not the lib or `node_modules` declaration files the run shares) the
    /// program parsed, collected only with timings on and only by the analysis that built it.
    pub(super) project_files: Vec<String>,
    pub(super) query: Duration,
    /// Waiting for its cores.
    pub(super) cores_waited: Duration,
    /// Whether the program started from the early plan.
    pub(super) early: bool,
    /// Whether another analysis of the run built the program ([`TypePrograms`](crate::TypePrograms)).
    pub(super) shared: bool,
}

/// The names of the program's files that no other program of the run shares; empty with timings off, where nothing
/// reads them.
pub(super) fn project_files(owner: &ProgramOwner) -> Vec<String> {
    if !crate::timing::enabled() {
        return Vec::new();
    }
    owner
        .program()
        .get_source_files()
        .iter()
        .map(|file| file.file_name().to_owned())
        .filter(|name| !polylint_tsrs::kept_for_run(name))
        .collect()
}

pub(super) struct WalkedFile {
    /// The index into [`ProgramPlan::files`].
    pub(super) file: usize,
    pub(super) facts: FileFacts,
    pub(super) unresolved_modules: u32,
}

/// Joins the outcomes of every program into the facts.
pub(super) fn assemble(
    input: &TypeInput<'_>,
    plan: &ProgramPlan,
    candidates: &Candidates,
    outcomes: Vec<Outcome>,
) -> Result<TypeFacts, TypeError> {
    let mut states: Vec<FileState> = plan
        .files
        .iter()
        .map(|file| match file.placement {
            // Set from the program's outcome below.
            Placement::Program(_) | Placement::Outside => FileState::Outside,
            Placement::NoConfig => FileState::NoConfig,
            Placement::ConfigError => FileState::ConfigError,
        })
        .collect();
    let mut found = Found::default();
    let mut unresolved_names: BTreeMap<PathBuf, BTreeSet<String>> = BTreeMap::new();
    let mut timings = TypeTimings::default();
    let mut distinct_project_files: BTreeSet<String> = BTreeSet::new();
    for (program, outcome) in plan.programs.iter().zip(outcomes) {
        match outcome {
            Outcome::Built(built) => {
                timings.programs += built.build;
                timings.config_parse += built.split.config_parse;
                timings.program_creation += built.split.program_creation;
                timings.bind += built.split.bind;
                distinct_project_files.extend(built.project_files);
                timings.queries += built.query;
                timings.cores_waited += built.cores_waited;
                timings.early += usize::from(built.early);
                timings.per_program.push(ProgramTiming {
                    config: PathBuf::from(&program.config),
                    judged_files: program.files.len(),
                    files: built.files,
                    checkers: built.checkers,
                    build: built.build,
                    query: built.query,
                    shared: built.shared,
                });
                for index in built.missing {
                    states[index] = FileState::Outside;
                }
                for walked in built.walked {
                    states[walked.file] = FileState::Covered {
                        unresolved_modules: walked.unresolved_modules,
                    };
                    let FileFacts {
                        reads,
                        whole,
                        unresolved,
                        implementations,
                    } = walked.facts;
                    found.reads.extend(reads);
                    found.whole.extend(whole);
                    found.implementations.extend(implementations);
                    if !unresolved.is_empty() {
                        unresolved_names
                            .entry(plan.files[walked.file].path.clone())
                            .or_default()
                            .extend(unresolved);
                    }
                }
            }
            Outcome::ConfigError => mark(&mut states, program, FileState::ConfigError),
            Outcome::Failed => mark(&mut states, program, FileState::CheckerFailed),
            Outcome::Cancelled => return Err(TypeError::Cancelled),
        }
    }
    timings.project_files_distinct = distinct_project_files.len();
    let credits = members::credit(candidates, &found);
    let covered = plan
        .files
        .iter()
        .zip(&states)
        .filter(|(_, state)| matches!(state, FileState::Covered { .. }))
        .map(|(file, _)| file.path.clone())
        .collect();
    let coverage = input
        .packages
        .iter()
        .enumerate()
        .filter(|(_, package)| !package.files.is_empty())
        .map(|(index, package)| {
            let package_states: Vec<FileState> = plan
                .files
                .iter()
                .zip(&states)
                .filter(|(file, _)| file.package == index)
                .map(|(_, &state)| state)
                .collect();
            coverage::package_coverage(package, &package_states)
        })
        .collect();
    Ok(TypeFacts {
        engine: ENGINE,
        reads: credits.reads,
        whole: credits.whole,
        unresolved_names,
        covered,
        coverage,
        timings,
    })
}

fn mark(states: &mut [FileState], program: &PlannedProgram, state: FileState) {
    for &index in &program.files {
        states[index] = state;
    }
}
