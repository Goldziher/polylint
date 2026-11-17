//! One analysis: plan the programs, build a few at a time (the largest first, each with checkers in proportion to its
//! share of the judged files), walk their judged files on their checkers, copy the facts out and free each program as
//! soon as its walk ends (skif ADR 0010).
//!
//! The programs can start before the host knows its query ([`start`]): they are planned from the files that may be
//! judged, and the first ones build while the host works the query out. Each waits, built, until
//! [`PendingTypes::finish`] plans the judged files, then walks them or, when no judged file needs it, is freed
//! unwalked. Which programs start early depends on timing, so nothing that shapes a fact does: a program gets the
//! checkers the early plan gives it whether it starts early or not, and the walks read the judged files alone.
//!
//! The analyses of a run over many roots can share their cores ([`TypeBudget`]), so that their programs together hold
//! no more cores than there are, and their programs ([`TypePrograms`](crate::TypePrograms)), so that a program the
//! judged files of several roots go to is built once for the run. Every program reads its files from the files the run
//! shares (`shared_files`), the run's or, without shared programs, the analysis's: each is parsed and bound once
//! instead of once per program, and a project file is freed with the last program that read it. The plannings and the
//! programs read their configs, those the project references of a program reach and those they extend, from the
//! configs the run shares (`shared_configs`) in the same way: each is read and parsed once instead of once per
//! planning and program.

use std::collections::{BTreeMap, BTreeSet};
use std::num::NonZeroUsize;
use std::panic::{self, AssertUnwindSafe};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::thread::{Scope, ScopedJoinHandle};
use std::time::{Duration, Instant};

use polylint_tsrs::{
    Admitted, BuildRequest, BuildSplit, Claim, Cores, Executor, Parallelism, ProgramFiles, ProgramHost, ProgramKey,
    ProgramOwner, Resident, Schedule, Taken, Ticket, charge, cores_of,
};
use tsrs_compiler::ProgramRegions;
use tsrs_core::arena::Region;

use crate::members::Candidates;
use crate::programs::{self, Lookups, PlannedProgram, ProgramPlan};
use crate::query::{self, FileFacts, QueryContext};
use crate::{TypeBuild, TypeError, TypeFacts, TypeInput, TypePackage, TypeQuery};

mod outcome;
mod queue;

use outcome::{BuiltProgram, Outcome, WalkedFile, assemble, project_files};
use queue::{Gate, Next, Queue};

/// The stack of the threads that build and check programs and of the types pool: checking recurses deeply, and
/// tsrs-lint gives its program threads the same.
const PROGRAM_STACK_SIZE: usize = 512 << 20;
/// The first chunk of the scratch region configs and program bookkeeping are allocated in.
const SCRATCH_CHUNK: usize = 1 << 20;
/// Runs the analysis of `input`.
pub(crate) fn analyze(input: &TypeInput<'_>) -> Result<TypeFacts, TypeError> {
    if input.cancel.is_some_and(|cancel| cancel.load(Ordering::Relaxed)) {
        return Err(TypeError::Cancelled);
    }
    let build = TypeBuild {
        fs: Arc::clone(&input.fs),
        root: input.root.to_path_buf(),
        packages: input.packages.to_vec(),
        tsconfig_override: input.tsconfig_override.map(std::path::Path::to_path_buf),
        threads: input.threads,
        budget: input.budget.cloned(),
        programs: input.programs.cloned(),
    };
    std::thread::scope(|scope| start(scope, build, input.cancel).finish(input.packages, input.query))
}

/// Starts building the programs of `build` on threads of `scope`.
pub(crate) fn start<'scope, 'env>(
    scope: &'scope Scope<'scope, 'env>,
    build: TypeBuild,
    cancel: Option<&'env AtomicBool>,
) -> PendingTypes<'scope> {
    let (fs, boms) = crate::vfs::tsrs_fs(Arc::clone(&build.fs));
    let cores = build.budget.as_ref().map_or_else(
        || Arc::new(Cores::new(cores_of(build.threads))),
        |budget| Arc::clone(&budget.cores),
    );
    let files = build
        .programs
        .as_ref()
        .map_or_else(Arc::default, |programs| programs.shared().files());
    let cwd = polylint_tsrs::tsrs_path(&build.root);
    let configs = build
        .programs
        .as_ref()
        .map_or_else(Arc::default, |programs| programs.shared().configs());
    let shared = Arc::new(Shared {
        host: ProgramHost::new(fs, boms, cwd.clone(), files, configs),
        cwd,
        pool: types_pool(build.threads),
        cores,
        build,
        planned: Slot::default(),
        judged: Slot::default(),
    });
    let spawned = {
        let shared = Arc::clone(&shared);
        std::thread::Builder::new()
            .name("js-types-plan".to_owned())
            .spawn_scoped(scope, move || {
                panic::catch_unwind(AssertUnwindSafe(|| Ok(coordinate(&shared, cancel)))).unwrap_or_else(|payload| {
                    // The host waits for the early plan; a panic before it was put must not leave it waiting.
                    shared
                        .planned
                        .put(Err(internal("planning the programs", payload.as_ref())));
                    Err(internal("building the programs", payload.as_ref()))
                })
            })
    };
    let (coordinator, spawn_error) = match spawned {
        Ok(handle) => (Some(handle), None),
        Err(error) => (None, Some(error.to_string())),
    };
    PendingTypes {
        shared,
        coordinator,
        spawn_error,
        cancel,
    }
}

/// Programs that are being built before the query is known ([`crate::start`]). [`PendingTypes::finish`] hands the
/// judged files and the query over and returns the facts; dropping it gives the analysis up and frees every program
/// built so far.
#[must_use = "dropping it gives the analysis up"]
pub struct PendingTypes<'scope> {
    shared: Arc<Shared>,
    coordinator: Option<ScopedJoinHandle<'scope, Result<Coordinated, TypeError>>>,
    spawn_error: Option<String>,
    cancel: Option<&'scope AtomicBool>,
}

impl std::fmt::Debug for PendingTypes<'_> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PendingTypes")
            .field("root", &self.shared.build.root)
            .finish_non_exhaustive()
    }
}

impl PendingTypes<'_> {
    /// Plans the judged files of `packages`, walks them on the programs (those built already and the rest) for
    /// `query`, frees every program and returns the facts with the coverage of every package. The file system, root,
    /// `tsconfig` override and threads are the build's.
    ///
    /// # Errors
    /// As [`crate::analyze`].
    pub fn finish(mut self, packages: &[TypePackage], query: &TypeQuery) -> Result<TypeFacts, TypeError> {
        let asked = Instant::now();
        let shared = Arc::clone(&self.shared);
        let build = &shared.build;
        let input = TypeInput {
            fs: Arc::clone(&build.fs),
            root: &build.root,
            packages,
            tsconfig_override: build.tsconfig_override.as_deref(),
            query,
            threads: build.threads,
            budget: build.budget.as_ref(),
            programs: build.programs.as_ref(),
            cancel: self.cancel,
        };
        let handed = self.hand_over(&input);
        // Wakes the program threads when nothing was handed over, so that they free their programs and end.
        self.shared.judged.put(None);
        let coordinated = self.join();
        let Handed {
            judged,
            planning,
            region,
        } = handed?;
        let coordinated = coordinated?;
        let wall = coordinated.wall;
        let outcomes = into_outcomes(&judged, coordinated)?;
        let mut facts = assemble(&input, &judged.plan, &judged.candidates, outcomes)?;
        facts.timings.planning = planning;
        facts.timings.wall = wall;
        facts.timings.threads = judged.threads;
        facts.timings.waited = asked.elapsed();
        facts.timings.shared_files = shared.host.file_counts().snapshot().into();
        let project = shared.host.file_counts().project_snapshot();
        facts.timings.project_files_parsed = project.parsed + project.unshared;
        let (configs, extended) = shared.host.configs().counts();
        facts.timings.shared_configs = configs.into();
        facts.timings.shared_extended_configs = extended.into();
        drop(region);
        Ok(facts)
    }

    /// Plans the judged files of `input` and hands the plan to the program threads.
    fn hand_over(&self, input: &TypeInput<'_>) -> Result<Handed, TypeError> {
        if let Some(error) = &self.spawn_error {
            return Err(TypeError::Internal {
                message: format!("cannot start a type-checker thread: {error}"),
            });
        }
        let planned = self.shared.planned.take()?;
        if self.cancel.is_some_and(|cancel| cancel.load(Ordering::Relaxed)) {
            return Err(TypeError::Cancelled);
        }
        // What this planning allocates is freed when the analysis ends.
        let region = Region::new_scratch(SCRATCH_CHUNK);
        let start = Instant::now();
        let (plan, _) = panic::catch_unwind(AssertUnwindSafe(|| {
            self.shared.on_pool(|| {
                let _scope = region.enter();
                programs::plan_programs(input, self.shared.host.configs(), &planned.lookups)
            })
        }))
        .unwrap_or_else(|payload| Err(internal("planning the programs", payload.as_ref())))?;
        let judged = Arc::new(Judged::new(
            self.shared.build.threads,
            &planned.early,
            plan,
            Candidates::new(input.query),
        ));
        self.shared.judged.put(Some(Arc::clone(&judged)));
        Ok(Handed {
            judged,
            planning: planned.planning.saturating_add(start.elapsed()),
            region,
        })
    }

    fn join(&mut self) -> Result<Coordinated, TypeError> {
        let Some(handle) = self.coordinator.take() else {
            return Ok(Coordinated::default());
        };
        handle
            .join()
            .unwrap_or_else(|payload| Err(internal("building the programs", payload.as_ref())))
    }
}

impl Drop for PendingTypes<'_> {
    fn drop(&mut self) {
        self.shared.judged.put(None);
        // The analysis asks no more: a program kept only for its root goes.
        if let Some(programs) = &self.shared.build.programs {
            programs.done(&self.shared.build.root);
        }
        // tsrs gives the pages of retired regions back in batches; the project files this analysis's programs freed
        // would otherwise keep the last batch resident until another run retires more.
        if polylint_tsrs::retiring() {
            tsrs_core::arena::flush_retired();
        }
    }
}

/// What the host, the planning thread and the program threads share.
struct Shared {
    build: TypeBuild,
    /// What the programs are built from: the analysis's file system, and the files and configs the plannings and
    /// programs read (the run's, with shared programs, else the analysis's own) with how this analysis got them.
    host: ProgramHost,
    /// The workspace root as tsrs names files, the current directory configs are parsed in.
    cwd: String,
    /// The early plan, put by the planning thread.
    planned: Slot<Result<Planned, TypeError>>,
    /// The plan of the judged files, put by the host; `None` once the host gave the analysis up.
    judged: Slot<Option<Arc<Judged>>>,
    /// The threads tsrs parallelises on. tsrs runs its parallel work on the rayon pool of the calling thread; on the
    /// global pool, a host that runs many analyses at once on that pool's threads (one per root) blocks them all
    /// waiting for plans whose parallel work then finds no thread, and a waiting tsrs call there takes other jobs of
    /// the host that block. `None` when the pool cannot be started: tsrs then
    /// uses the global pool. Shared with the programs built on it, which are freed on it ([`ProgramOwner`]).
    pool: Option<Arc<rayon::ThreadPool>>,
    /// The cores the programs check on: the analysis's own, or those of the run's [`TypeBudget`].
    cores: Arc<Cores>,
}

impl Shared {
    /// Runs `work` on the analysis's own threads, so that the parallel work tsrs does in it stays there.
    fn on_pool<R: Send>(&self, work: impl FnOnce() -> R + Send) -> R {
        match &self.pool {
            Some(pool) => pool.install(work),
            None => work(),
        }
    }
}

/// The pool of an analysis as the place its programs are freed.
struct PoolExecutor(Arc<rayon::ThreadPool>);

impl Executor for PoolExecutor {
    fn run(&self, work: &mut (dyn FnMut() + Send)) {
        self.0.install(work);
    }
}

/// The rayon pool of one analysis, a thread per core. Only tsrs's own work runs on it, never a wait for a plan or for
/// cores: a pool thread that waits inside tsrs takes other queued jobs, so a job that blocks would block it too.
fn types_pool(threads: Option<NonZeroUsize>) -> Option<Arc<rayon::ThreadPool>> {
    rayon::ThreadPoolBuilder::new()
        .num_threads(cores_of(threads))
        .stack_size(PROGRAM_STACK_SIZE)
        .thread_name(|index| format!("js-types-{index}"))
        .build()
        .ok()
        .map(Arc::new)
}

/// A value one thread puts once and others wait for.
struct Slot<T> {
    value: Mutex<Option<T>>,
    put: Condvar,
}

impl<T> Default for Slot<T> {
    fn default() -> Self {
        Self {
            value: Mutex::new(None),
            put: Condvar::new(),
        }
    }
}

impl<T> Slot<T> {
    /// Puts `value` unless the slot holds one, and wakes the threads that wait for it.
    fn put(&self, value: T) {
        let mut slot = lock(&self.value);
        if slot.is_none() {
            *slot = Some(value);
        }
        drop(slot);
        self.put.notify_all();
    }

    /// Waits for the value and takes it.
    fn take(&self) -> T {
        let mut slot = lock(&self.value);
        loop {
            if let Some(value) = slot.take() {
                return value;
            }
            slot = self.put.wait(slot).unwrap_or_else(PoisonError::into_inner);
        }
    }
}

impl<T: Clone> Slot<T> {
    /// The value, if it was put.
    fn get(&self) -> Option<T> {
        lock(&self.value).clone()
    }

    /// Waits for the value.
    fn wait(&self) -> T {
        let mut slot = lock(&self.value);
        loop {
            if let Some(value) = &*slot {
                return value.clone();
            }
            slot = self.put.wait(slot).unwrap_or_else(PoisonError::into_inner);
        }
    }
}

/// What the early planning hands the host.
struct Planned {
    /// Where every early file went, so the planning of the judged files searches only the files it lacks.
    lookups: Lookups,
    early: Early,
    planning: Duration,
}

/// The programs of the early plan in the order they start, with the checkers of each.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Early {
    configs: Vec<String>,
    checkers: Vec<usize>,
}

impl Early {
    fn new(plan: &ProgramPlan, schedule: &Schedule) -> Self {
        Self {
            configs: schedule
                .order
                .iter()
                .map(|&index| plan.programs[index].config.clone())
                .collect(),
            checkers: schedule.order.iter().map(|&index| schedule.checkers[index]).collect(),
        }
    }
}

/// The plan of the judged files with what its walks need.
struct Judged {
    plan: ProgramPlan,
    candidates: Candidates,
    schedule: Schedule,
    /// The configs of the programs in [`Schedule::order`].
    order: Vec<String>,
    /// The index of each program in [`ProgramPlan::programs`], by config.
    by_config: BTreeMap<String, usize>,
    /// The programs built at once.
    builders: usize,
    /// The threads the programs share.
    threads: usize,
}

impl Judged {
    fn new(threads: Option<NonZeroUsize>, early: &Early, plan: ProgramPlan, candidates: Candidates) -> Self {
        let parallelism = Parallelism::new(threads, plan.programs.len());
        let schedule = judged_schedule(&parallelism, early, &plan.programs);
        let order = schedule
            .order
            .iter()
            .map(|&index| plan.programs[index].config.clone())
            .collect();
        let by_config = plan
            .programs
            .iter()
            .enumerate()
            .map(|(index, program)| (program.config.clone(), index))
            .collect();
        Self {
            builders: parallelism.programs,
            threads: parallelism.threads,
            plan,
            candidates,
            schedule,
            order,
            by_config,
        }
    }
}

/// The schedule of the judged files' `programs`: those of the early plan first, in its order and with its checkers,
/// since they may be built with them already; then the others, the largest first, with the checkers of `parallelism`.
fn judged_schedule(parallelism: &Parallelism, early: &Early, programs: &[PlannedProgram]) -> Schedule {
    let sizes: Vec<usize> = programs.iter().map(|program| program.files.len()).collect();
    let own = Schedule::new(parallelism, &sizes);
    let by_config: BTreeMap<&str, usize> = programs
        .iter()
        .enumerate()
        .map(|(index, program)| (program.config.as_str(), index))
        .collect();
    let early_checkers: BTreeMap<&str, usize> = early
        .configs
        .iter()
        .map(String::as_str)
        .zip(early.checkers.iter().copied())
        .collect();
    let mut order: Vec<usize> = early
        .configs
        .iter()
        .filter_map(|config| by_config.get(config.as_str()).copied())
        .collect();
    let first: BTreeSet<usize> = order.iter().copied().collect();
    order.extend(own.order.iter().copied().filter(|index| !first.contains(index)));
    let checkers = programs
        .iter()
        .zip(own.checkers)
        .map(|(program, checkers)| early_checkers.get(program.config.as_str()).copied().unwrap_or(checkers))
        .collect();
    Schedule { order, checkers }
}

/// What the host takes from handing its plan over.
struct Handed {
    judged: Arc<Judged>,
    planning: Duration,
    /// The region of the judged files' planning, freed when the analysis ends.
    region: Region,
}

/// What the planning thread returns once every program thread has ended.
#[derive(Default)]
struct Coordinated {
    /// The outcome of each program of the judged files' plan, by its index there.
    outcomes: BTreeMap<usize, Outcome>,
    wall: Duration,
    spawn_error: Option<String>,
}

/// Plans the early files of `shared`, puts the plan for the host and builds and walks the programs until the host's
/// plan is done or given up.
fn coordinate(shared: &Shared, cancel: Option<&AtomicBool>) -> Coordinated {
    // What planning allocates is freed when the programs are; the configs it reads stay with the shared configs.
    let region = Region::new_scratch(SCRATCH_CHUNK);
    let start = Instant::now();
    let query = TypeQuery::default();
    let input = shared.build.input(&query);
    let planned = {
        panic::catch_unwind(AssertUnwindSafe(|| {
            shared.on_pool(|| {
                let _scope = region.enter();
                programs::plan_programs(&input, shared.host.configs(), &Lookups::new())
            })
        }))
        .unwrap_or_else(|payload| Err(internal("planning the programs", payload.as_ref())))
    };
    let (plan, lookups) = match planned {
        Ok(planned) => planned,
        Err(error) => {
            shared.planned.put(Err(error));
            return Coordinated::default();
        }
    };
    let parallelism = Parallelism::new(shared.build.threads, plan.programs.len());
    let sizes: Vec<usize> = plan.programs.iter().map(|program| program.files.len()).collect();
    let early = Early::new(&plan, &Schedule::new(&parallelism, &sizes));
    drop(plan);
    shared.planned.put(Ok(Planned {
        lookups,
        early: early.clone(),
        planning: start.elapsed(),
    }));
    let session = Session {
        shared,
        early: &early,
        cancel,
        single_threaded: parallelism.single_threaded,
        queue: Mutex::default(),
        outcomes: Mutex::default(),
        regions: Mutex::default(),
    };
    let run_start = Instant::now();
    let spawn_error = session.run(parallelism.programs);
    let wall = run_start.elapsed();
    let Session { outcomes, regions, .. } = session;
    drop(regions);
    drop(region);
    Coordinated {
        outcomes: outcomes.into_inner().unwrap_or_else(PoisonError::into_inner),
        wall,
        spawn_error: spawn_error.map(|error| error.to_string()),
    }
}

/// The outcome of every program of `judged`, once the threads are done.
fn into_outcomes(judged: &Judged, coordinated: Coordinated) -> Result<Vec<Outcome>, TypeError> {
    let Coordinated {
        mut outcomes,
        spawn_error,
        ..
    } = coordinated;
    let programs = judged.plan.programs.len();
    if let Some(error) = spawn_error
        && (0..programs).any(|index| !outcomes.contains_key(&index))
    {
        return Err(TypeError::Internal {
            message: format!("cannot start a type-checker thread: {error}"),
        });
    }
    // A panic outside a program's work leaves its outcome out.
    let outcomes: Vec<Outcome> = (0..programs)
        .map(|index| outcomes.remove(&index).unwrap_or(Outcome::Failed))
        .collect();
    if outcomes.iter().any(|outcome| matches!(outcome, Outcome::Cancelled)) {
        return Err(TypeError::Cancelled);
    }
    Ok(outcomes)
}

/// The cores of a run that several analyses share: each analysis takes its programs' checkers from them instead of
/// cores of its own, so that the programs of a run over many roots together hold no more cores than the budget has,
/// which also bounds how many programs are in memory at once. Give the same budget (or a clone, which shares it) to
/// every analysis of the run with [`TypeBuild::with_budget`](crate::TypeBuild::with_budget) or
/// [`TypeInput::with_budget`](crate::TypeInput::with_budget). The facts do not depend on it.
#[derive(Clone)]
pub struct TypeBudget {
    cores: Arc<Cores>,
}

impl TypeBudget {
    /// A budget of `threads` cores; `None` for every core of the machine.
    #[must_use]
    pub fn new(threads: Option<NonZeroUsize>) -> Self {
        Self {
            cores: Arc::new(Cores::shared(cores_of(threads))),
        }
    }

    /// The cores of the budget.
    #[must_use]
    pub fn threads(&self) -> usize {
        self.cores.threads()
    }

    /// The most cores the analyses held at once, for tests that check the budget holds.
    #[doc(hidden)]
    #[must_use]
    pub fn peak(&self) -> usize {
        self.cores.peak()
    }

    /// How many programs took cores from the budget, for tests that check every program does.
    #[doc(hidden)]
    #[must_use]
    pub fn takes(&self) -> usize {
        self.cores.takes()
    }
}

impl std::fmt::Debug for TypeBudget {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("TypeBudget")
            .field("threads", &self.cores.threads())
            .finish_non_exhaustive()
    }
}

/// A program of the judged files' plan, by its index there.
type Target = (Arc<Judged>, usize);

struct Session<'a> {
    shared: &'a Shared,
    early: &'a Early,
    cancel: Option<&'a AtomicBool>,
    single_threaded: bool,
    queue: Mutex<Queue>,
    /// The outcome of each program of the judged files' plan, by its index there.
    outcomes: Mutex<BTreeMap<usize, Outcome>>,
    /// The scratch regions of the program threads, freed when the analysis ends.
    regions: Mutex<Vec<Region>>,
}

impl Session<'_> {
    fn cancelled(&self) -> bool {
        self.cancel.is_some_and(|cancel| cancel.load(Ordering::Relaxed))
    }

    /// Builds and walks every program, `builders` at once until the judged files are planned and then as many as
    /// their plan builds at once; returns why a thread could not start, if one could not.
    fn run(&self, builders: usize) -> Option<std::io::Error> {
        let mut spawn_error = None;
        std::thread::scope(|scope| {
            let spawn = |index: usize| {
                std::thread::Builder::new()
                    .name(format!("js-types-{index}"))
                    .stack_size(PROGRAM_STACK_SIZE)
                    .spawn_scoped(scope, || self.work())
            };
            let mut handles: Vec<_> = (0..builders).map(&spawn).collect();
            if let Some(judged) = self.shared.judged.wait() {
                handles.extend((builders..judged.builders).map(&spawn));
            }
            for handle in handles {
                match handle {
                    // A panic outside a program's work leaves its outcome out, which `into_outcomes` reports.
                    Ok(handle) => drop(handle.join()),
                    Err(error) => spawn_error = Some(error),
                }
            }
        });
        spawn_error
    }

    fn work(&self) {
        let region = Region::new_scratch(SCRATCH_CHUNK);
        let entered = region.enter();
        loop {
            let judged = self.shared.judged.get();
            let gate = match &judged {
                None => Gate::Closed,
                Some(None) => Gate::Abandoned,
                Some(Some(judged)) => Gate::Open(&judged.order),
            };
            let mut admitted = None;
            let next = lock(&self.queue).next(&self.early.configs, gate, |position| {
                admitted = self.shared.cores.admit_early(self.early.checkers[position]);
                admitted.is_some()
            });
            match next {
                Next::Early(position) => {
                    let held = match admitted {
                        Some(Admitted::Now(taken)) => Some(taken),
                        Some(Admitted::Later) | None => None,
                    };
                    self.start(&self.early.configs[position], self.early.checkers[position], None, held);
                }
                Next::Judged(position) => {
                    let Some(Some(judged)) = judged else {
                        break;
                    };
                    let index = judged.schedule.order[position];
                    let checkers = judged.schedule.checkers[index];
                    let config = judged.plan.programs[index].config.clone();
                    self.start(&config, checkers, Some((judged, index)), None);
                }
                Next::Wait => {
                    self.shared.judged.wait();
                }
                Next::Done => break,
            }
        }
        drop(entered);
        lock(&self.regions).push(region);
    }

    /// Runs the program of `config` on `checkers` checkers with the cores it `held` already or once they are free,
    /// and keeps its outcome for its program of the judged files' plan: `target`, or the one with its config once
    /// that plan is known.
    fn start(&self, config: &str, checkers: usize, target: Option<Target>, held: Option<Taken<'_>>) {
        let early = target.is_none();
        let outcome = self.build_and_walk(config, checkers, target.clone(), held, early);
        let Some(outcome) = outcome else {
            return;
        };
        if let Some((_, index)) = target.or_else(|| self.target(config)) {
            lock(&self.outcomes).insert(index, outcome);
        }
    }

    fn build_and_walk(
        &self,
        config: &str,
        checkers: usize,
        target: Option<Target>,
        held: Option<Taken<'_>>,
        early: bool,
    ) -> Option<Outcome> {
        if self.cancelled() {
            return Some(Outcome::Cancelled);
        }
        let waiting = Instant::now();
        // Before taking cores: a build of the program by another analysis of the run holds its cores already and
        // waits for no host, so waiting for it waits for nothing this analysis holds.
        let programs = self.shared.build.programs.as_ref();
        let claim = programs.map(|programs| programs.shared().claim(&ProgramKey::new(config, self.single_threaded)));
        let built_checkers = match &claim {
            Some(Claim::Ready(resident)) => resident.checkers(),
            _ => checkers,
        };
        // Without cores held, the program is of the judged files' plan, or an early one of an analysis's own cores
        // ([`Cores::admit_early`]): waiting for them cannot wait for a host.
        let _taken = held.unwrap_or_else(|| {
            let count = target.as_ref().map_or(built_checkers, |(judged, index)| {
                charge(built_checkers, judged.plan.programs[*index].files.len())
            });
            self.shared.cores.take(count)
        });
        let cores_waited = waiting.elapsed();
        if self.cancelled() {
            return Some(Outcome::Cancelled);
        }
        let mut outcome = panic::catch_unwind(AssertUnwindSafe(|| match claim {
            Some(Claim::Ready(resident)) => self.walk_shared(&resident, config, target, (early, false)),
            Some(Claim::Build(ticket)) => self.run_program(config, checkers, target, early, Some(ticket)),
            None => self.run_program(config, checkers, target, early, None),
        }))
        .unwrap_or(Some(Outcome::Failed));
        if let Some(Outcome::Built(built)) = &mut outcome {
            built.cores_waited = cores_waited;
        }
        outcome
    }

    /// The program of the judged files' plan with `config`, once the plan is known; `None` when no judged file needs
    /// it or the host gave the analysis up.
    fn target(&self, config: &str) -> Option<Target> {
        let judged = self.shared.judged.wait()?;
        let index = *judged.by_config.get(config)?;
        Some((judged, index))
    }

    /// Builds the program of `path` (in a run that shares its programs, under `ticket`, the claim to build it for the
    /// run), walks its judged files and frees it unless another root of the run may still need it; `None` when no
    /// judged file needs it.
    fn run_program(
        &self,
        path: &str,
        checkers: usize,
        target: Option<Target>,
        early: bool,
        ticket: Option<Ticket<'_>>,
    ) -> Option<Outcome> {
        let start = Instant::now();
        // The project files the program reads, which everything below points into: declared first, this hold drops
        // last if a panic unwinds before the resident takes over its own.
        let program_files = Arc::new(ProgramFiles::default());
        // What the config and the host allocate lives as long as the program, which points into it.
        let region = Region::new_scratch(SCRATCH_CHUNK);
        let built = {
            let _scope = region.enter();
            self.build(path, checkers, ticket.is_some(), Arc::clone(&program_files))
        };
        let Some((owner, loader_regions, files, split)) = built else {
            return Some(Outcome::ConfigError);
        };
        let project_files = project_files(&owner);
        let resident = Arc::new(Resident::new(
            (owner, loader_regions, region),
            (
                Arc::clone(self.shared.host.boms()),
                Arc::clone(self.shared.host.files()),
                self.shared.host.configs().configs(),
            ),
            ProgramKey::new(path, self.single_threaded),
            checkers,
            (start.elapsed(), split, project_files),
        ));
        if let Some(ticket) = ticket {
            ticket.publish(&resident, &files, &self.shared.cwd);
        }
        drop(files);
        self.walk_shared(&resident, path, target, (early, true))
    }

    /// Walks `resident`, the program of `path`, as [`Session::walk`] does, and lets it go: it is freed once no other
    /// root of the run may need it.
    fn walk_shared(
        &self,
        resident: &Arc<Resident>,
        path: &str,
        target: Option<Target>,
        started: (bool, bool),
    ) -> Option<Outcome> {
        let outcome = self.walk(resident, path, target, started);
        if let Some(programs) = &self.shared.build.programs {
            programs.shared().walked(resident, &self.shared.cwd);
        }
        outcome
    }

    /// Reads the config at `path` and builds and binds its program on `checkers` checkers, holding the project files
    /// it reads in `program_files`, with the program's loader regions and, when `listed`, the files the config
    /// includes; `None` when the config cannot be read.
    fn build(
        &self,
        path: &str,
        checkers: usize,
        listed: bool,
        program_files: Arc<ProgramFiles>,
    ) -> Option<(ProgramOwner, Arc<ProgramRegions>, Vec<String>, BuildSplit)> {
        // The files come parsed and bound from the files the run shares, which the run (the lib and `node_modules`
        // declaration files) or the program's owner and resident (its project files) keep until after it is freed.
        // The checkers' memory goes back with the program, the diagnostics they make while answering included:
        // without a region of their own those go to the arena of the thread that made them, which is never freed,
        // so every program of a run would add its diagnostics. No diagnostic is read.
        let request = BuildRequest {
            config_path: path,
            checkers,
            single_threaded: self.single_threaded,
            list_files: listed,
            diagnostics_in_checker_regions: true,
        };
        let pool = self
            .shared
            .pool
            .clone()
            .map(|pool| Arc::new(PoolExecutor(pool)) as Arc<dyn Executor>);
        let built = self
            .shared
            .host
            .build(&request, program_files, pool.as_ref(), |_| {})
            .ok()?;
        Some((built.owner, built.loader_regions, built.files, built.split))
    }

    /// Walks the judged files of `resident`, the program of `path`, copying out what the walks find; `started` says
    /// whether the program started from the early plan and whether this analysis built it. An early program waits
    /// here, built, for the judged files' plan, before it enters the program: another analysis of the run may walk it
    /// meanwhile.
    fn walk(&self, resident: &Resident, path: &str, target: Option<Target>, started: (bool, bool)) -> Option<Outcome> {
        let (early, built_here) = started;
        let (judged, index) = match target {
            Some(target) => target,
            None => self.target(path)?,
        };
        if self.cancelled() {
            return Some(Outcome::Cancelled);
        }
        let _walking = resident.walking();
        let tsrs = resident.program();
        let program = &judged.plan.programs[index];
        let mut sources = Vec::new();
        let mut indices = Vec::new();
        let mut missing = Vec::new();
        for &file in &program.files {
            match tsrs.get_source_file(&judged.plan.files[file].name) {
                Some(source) => {
                    sources.push(source);
                    indices.push(file);
                }
                None => missing.push(file),
            }
        }
        // The program's own record: it read the files through the file system of the analysis that built it.
        let context = QueryContext::new(&judged.candidates, resident.boms(), tsrs);
        let slots: Vec<Mutex<Option<(FileFacts, u32)>>> = sources.iter().map(|_| Mutex::new(None)).collect();
        let query_start = Instant::now();
        let pooled = self.shared.on_pool(|| {
            tsrs.for_each_checker_group(&sources, |checker, index, source| {
                if self.cancelled() {
                    return;
                }
                let mut facts = query::query_file(checker, source, &context);
                // A file reads a member many times; one read of each is all the credit needs.
                facts.reads.sort_unstable();
                facts.reads.dedup();
                facts.whole.sort_unstable();
                facts.whole.dedup();
                let unresolved_modules = query::unresolved_modules(checker, tsrs, source);
                *lock(&slots[index]) = Some((facts, unresolved_modules));
            })
        });
        if !pooled {
            return Some(Outcome::Failed);
        }
        let query = query_start.elapsed();
        if self.cancelled() {
            return Some(Outcome::Cancelled);
        }
        let mut walked = Vec::with_capacity(indices.len());
        for (slot, file) in slots.into_iter().zip(indices) {
            let Some((facts, unresolved_modules)) = slot.into_inner().unwrap_or_else(PoisonError::into_inner) else {
                return Some(Outcome::Failed);
            };
            walked.push(WalkedFile {
                file,
                facts,
                unresolved_modules,
            });
        }
        Some(Outcome::Built(BuiltProgram {
            walked,
            missing,
            files: tsrs.get_source_files().len(),
            checkers: resident.checkers(),
            build: if built_here { resident.build() } else { Duration::ZERO },
            split: if built_here {
                resident.split()
            } else {
                BuildSplit::default()
            },
            project_files: if built_here {
                resident.project_files().to_vec()
            } else {
                Vec::new()
            },
            query,
            cores_waited: Duration::ZERO,
            early,
            shared: !built_here,
        }))
    }
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// A panic inside tsrs outside any program's work, as an error.
fn internal(stage: &str, payload: &(dyn std::any::Any + Send)) -> TypeError {
    let detail = payload
        .downcast_ref::<&str>()
        .map(|message| (*message).to_owned())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "a panic without a message".to_owned());
    TypeError::Internal {
        message: format!("{stage}: {detail}"),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests;
