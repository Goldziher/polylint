//! How the programs of an analysis share the machine: how many are built at once, the checkers of each, and the
//! cores a program holds while it is built and checked.
//!
//! A program's *judged* files are those the host wants an answer about, as opposed to the files the program only
//! reads to check them.

use std::num::NonZeroUsize;
use std::sync::{Condvar, Mutex, PoisonError};

/// Cores per program built at once, unless the host sets its own ([`Limits`]): each program checks on a pool of its
/// own, so a quarter of the cores at a time fills the machine without holding every program in memory (tsrs-lint's
/// measurement).
pub const CORES_PER_PROGRAM: usize = 4;
/// The most checkers a program gets while other programs are built beside it, unless the host sets its own
/// ([`Limits`]). Each checker holds its own type
/// caches: on the largest repository of the corpus, 16 checkers on its main program raised peak memory past the
/// memory budget without a shorter run than 8.
pub const MOST_SHARED_CHECKERS: usize = 8;

/// The bounds a host sets on how its programs share the machine. The defaults are those measured on a corpus of
/// large TypeScript monorepos ([`CORES_PER_PROGRAM`], [`MOST_SHARED_CHECKERS`]); a host with other programs or
/// another memory budget sets its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// Cores per program built at once; at least one.
    pub cores_per_program: usize,
    /// The most checkers a program gets while other programs are built beside it.
    pub most_shared_checkers: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            cores_per_program: CORES_PER_PROGRAM,
            most_shared_checkers: MOST_SHARED_CHECKERS,
        }
    }
}

/// `threads`, or every core of the machine.
#[must_use]
pub fn cores_of(threads: Option<NonZeroUsize>) -> usize {
    threads
        .or_else(|| std::thread::available_parallelism().ok())
        .map_or(1, NonZeroUsize::get)
}

/// How many programs are built at once and how many checkers a program may get.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Parallelism {
    /// The threads the programs share.
    pub threads: usize,
    /// The programs built at once.
    pub programs: usize,
    /// The checkers of a program when the programs built at once share the threads evenly, and the fewest a program
    /// gets.
    pub even: usize,
    /// The most checkers a program gets: tsrs's own count for the machine, within the threads and
    /// [`Limits::most_shared_checkers`], unless the even share is larger.
    pub most: usize,
    /// Whether tsrs runs single-threaded: one thread in all.
    pub single_threaded: bool,
}

impl Parallelism {
    /// The parallelism of `programs` programs on `threads` threads; `None` for every core of the machine. The
    /// default [`Limits`] apply.
    #[must_use]
    pub fn new(threads: Option<NonZeroUsize>, programs: usize) -> Self {
        Self::with_limits(threads, programs, Limits::default())
    }

    /// [`Parallelism::new`] within the host's own `limits`.
    #[must_use]
    pub fn with_limits(threads: Option<NonZeroUsize>, programs: usize, limits: Limits) -> Self {
        let threads = cores_of(threads);
        let at_once = (threads / limits.cores_per_program.max(1)).clamp(1, programs.max(1));
        let even = (threads / at_once).max(1);
        let machine =
            tsrs_compiler::checkerpool::default_checkers_for_parallelism(i64::try_from(threads).unwrap_or(i64::MAX));
        let machine = usize::try_from(machine).unwrap_or(threads);
        Self {
            threads,
            programs: at_once,
            even,
            most: machine.min(threads).min(limits.most_shared_checkers).max(even),
            single_threaded: threads == 1,
        }
    }

    /// The checkers of a program that holds `files` of the `total` judged files in programs: its share of the
    /// threads, at least the even share and at most [`Parallelism::most`]. A program with most of the judged files
    /// would otherwise check on a quarter of the cores while the rest wait for it.
    #[must_use]
    pub fn checkers(&self, files: usize, total: usize) -> usize {
        let share = if total == 0 {
            0
        } else {
            self.threads.saturating_mul(files).div_ceil(total)
        };
        share.clamp(self.even, self.most)
    }
}

/// The order the programs start in and the checkers of each.
#[derive(Debug, PartialEq, Eq)]
pub struct Schedule {
    /// Indices into the programs of the plan, the programs with the most judged files first: the longest program then
    /// does not start last and hold up the end of the analysis. Programs of one size keep their plan order.
    pub order: Vec<usize>,
    /// The checkers of each program of the plan.
    pub checkers: Vec<usize>,
}

impl Schedule {
    /// The schedule of programs that hold `sizes` judged files each. A program gets no more checkers than it has
    /// files to walk: tsrs creates every checker of a program before the walk, each with the program's global types,
    /// and one that owns no walked file only holds memory.
    #[must_use]
    pub fn new(parallelism: &Parallelism, sizes: &[usize]) -> Self {
        let total = sizes.iter().sum();
        let mut order: Vec<usize> = (0..sizes.len()).collect();
        order.sort_by_key(|&index| std::cmp::Reverse(sizes[index]));
        Self {
            order,
            checkers: sizes
                .iter()
                .map(|&files| parallelism.checkers(files, total).min(files.max(1)))
                .collect(),
        }
    }
}

/// The cores not handed to a program's checkers yet. A program starts once its checkers fit, so the programs that
/// run at once never hold more checkers than there are cores, and a program that takes most of them runs with few
/// others beside it, which bounds the memory they hold together.
///
/// The cores of one analysis ([`Cores::new`]) are only its own programs'. Those a run's analyses share
/// ([`Cores::shared`]) must never be waited for by a program that waits for its host: a program built before its
/// analysis's judged files are planned waits for its host to plan them, and the host's thread can be running
/// another analysis meanwhile (a rayon thread that waits runs other jobs), whose host would then wait for cores held
/// by a program waiting for that thread. So a program waits for shared cores only once its judged files are planned,
/// when it no longer waits for its host; an early program takes cores only of a share kept for early programs, only
/// when they are free ([`Cores::admit_early`]), and nothing ever waits for those.
pub struct Cores {
    threads: usize,
    /// The cores of `threads` that only early programs take, without waiting; `None` for the cores of one analysis,
    /// whose early programs wait for cores like the others.
    early: Option<usize>,
    held: Mutex<Held>,
    returned: Condvar,
}

/// The cores held now, and the most held at once.
#[derive(Debug, Default)]
struct Held {
    /// By programs of a known plan or, for one analysis, by every program.
    planned: usize,
    /// By early programs of a shared budget.
    early: usize,
    peak: usize,
    /// The programs that took cores.
    takes: usize,
}

impl Held {
    fn took(&mut self) {
        self.peak = self.peak.max(self.planned + self.early);
        self.takes += 1;
    }
}

/// How an early program gets its cores.
pub enum Admitted<'a> {
    /// It waits for them before it builds, with the other programs of its analysis.
    Later,
    /// It holds them.
    Now(Taken<'a>),
}

impl Cores {
    /// The cores of one analysis.
    #[must_use]
    pub fn new(threads: usize) -> Self {
        Self::with_early(threads, None)
    }

    /// The cores the analyses of a run share, a quarter of them kept for early programs as for one program built at
    /// once ([`CORES_PER_PROGRAM`]); fewer than that keep none, and no program starts early.
    #[must_use]
    pub fn shared(threads: usize) -> Self {
        Self::shared_with(threads, Limits::default())
    }

    /// [`Cores::shared`] with the early share of the host's own `limits`: the cores of one program built at once
    /// ([`Limits::cores_per_program`]).
    #[must_use]
    pub fn shared_with(threads: usize, limits: Limits) -> Self {
        Self::with_early(threads, Some(threads / limits.cores_per_program.max(1)))
    }

    fn with_early(threads: usize, early: Option<usize>) -> Self {
        Self {
            threads,
            early,
            held: Mutex::default(),
            returned: Condvar::new(),
        }
    }

    /// The cores in all.
    #[must_use]
    pub fn threads(&self) -> usize {
        self.threads
    }

    /// The cores programs of a known plan take.
    #[must_use]
    pub fn planned(&self) -> usize {
        self.threads - self.early.unwrap_or(0)
    }

    /// Waits until `count` cores are free (all those of programs of a known plan, when `count` is more) and holds
    /// them until the guard drops. Of shared cores, only a program whose judged files are planned waits.
    #[must_use]
    pub fn take(&self, count: usize) -> Taken<'_> {
        let planned = self.planned();
        let count = count.min(planned);
        let mut held = lock(&self.held);
        while held.planned + count > planned {
            held = self.returned.wait(held).unwrap_or_else(PoisonError::into_inner);
        }
        held.planned += count;
        held.took();
        Taken {
            cores: self,
            count,
            early: false,
        }
    }

    /// How a program started before its judged files are planned gets its `count` cores (all of the early share,
    /// when `count` is more): later with [`Cores::take`] for the cores of one analysis; at once of the early share
    /// of shared cores when that many are free, and not at all when they are not.
    #[must_use]
    pub fn admit_early(&self, count: usize) -> Option<Admitted<'_>> {
        let Some(share) = self.early else {
            return Some(Admitted::Later);
        };
        let count = count.min(share);
        let mut held = lock(&self.held);
        if count == 0 || held.early + count > share {
            return None;
        }
        held.early += count;
        held.took();
        Some(Admitted::Now(Taken {
            cores: self,
            count,
            early: true,
        }))
    }

    /// The most cores held at once so far.
    #[must_use]
    pub fn peak(&self) -> usize {
        lock(&self.held).peak
    }

    /// How many programs took cores so far.
    #[must_use]
    pub fn takes(&self) -> usize {
        lock(&self.held).takes
    }

    /// The cores held now.
    #[must_use]
    pub fn in_use(&self) -> usize {
        let held = lock(&self.held);
        held.planned + held.early
    }
}

/// Cores taken from [`Cores`], given back on drop.
pub struct Taken<'a> {
    cores: &'a Cores,
    count: usize,
    early: bool,
}

impl Drop for Taken<'_> {
    fn drop(&mut self) {
        let mut held = lock(&self.cores.held);
        if self.early {
            held.early -= self.count;
        } else {
            held.planned -= self.count;
        }
        drop(held);
        self.cores.returned.notify_all();
    }
}

/// The cores a program on `checkers` checkers with `judged` judged files holds: a checker that owns none of its
/// judged files walks nothing, so one for each checker up to the judged files, and at least one.
#[must_use]
pub fn charge(checkers: usize, judged: usize) -> usize {
    checkers.min(judged).max(1)
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use std::sync::mpsc;
    use std::time::Duration;

    use super::*;

    fn parallelism(threads: usize, programs: usize) -> Parallelism {
        Parallelism::new(NonZeroUsize::new(threads), programs)
    }

    fn schedule(threads: usize, sizes: &[usize]) -> Schedule {
        Schedule::new(&parallelism(threads, sizes.len()), sizes)
    }

    #[test]
    fn a_quarter_of_the_threads_build_programs_and_share_the_rest_as_checkers() {
        let cases = [
            ((16, 10), (4, 4, 8)),
            ((8, 10), (2, 4, 8)),
            ((3, 10), (1, 3, 3)),
            ((64, 40), (16, 4, 8)),
        ];
        for ((threads, programs), (at_once, even, most)) in cases {
            assert_eq!(
                parallelism(threads, programs),
                Parallelism {
                    threads,
                    programs: at_once,
                    even,
                    most,
                    single_threaded: false
                },
                "{threads} threads, {programs} programs"
            );
        }
    }

    #[test]
    fn a_host_sets_the_cores_per_program_and_the_most_shared_checkers() {
        let limits = Limits {
            cores_per_program: 2,
            most_shared_checkers: 4,
        };
        let tuned = Parallelism::with_limits(NonZeroUsize::new(16), 10, limits);
        assert_eq!((tuned.programs, tuned.even, tuned.most), (8, 2, 4));
        assert_eq!(
            Parallelism::with_limits(NonZeroUsize::new(16), 10, Limits::default()),
            parallelism(16, 10)
        );
        let none = Limits {
            cores_per_program: 0,
            most_shared_checkers: 8,
        };
        assert_eq!(Parallelism::with_limits(NonZeroUsize::new(4), 10, none).programs, 4);
        assert_eq!(Cores::shared_with(16, limits).planned(), 8);
        assert_eq!(
            Cores::shared_with(16, Limits::default()).planned(),
            Cores::shared(16).planned()
        );
    }

    #[test]
    fn fewer_programs_than_slots_get_more_checkers_each() {
        assert_eq!(parallelism(16, 1).programs, 1);
        assert_eq!(parallelism(16, 1).even, 16);
        assert_eq!(parallelism(16, 0).programs, 1);
        assert_eq!(schedule(16, &[40]).checkers, [16]);
    }

    #[test]
    fn one_thread_runs_tsrs_single_threaded() {
        assert_eq!(
            parallelism(1, 5),
            Parallelism {
                threads: 1,
                programs: 1,
                even: 1,
                most: 1,
                single_threaded: true
            }
        );
        assert_eq!(schedule(1, &[100, 1]).checkers, [1, 1]);
    }

    #[test]
    fn programs_of_one_size_share_the_threads_evenly_in_plan_order() {
        assert_eq!(
            schedule(16, &[50; 10]),
            Schedule {
                order: (0..10).collect(),
                checkers: vec![4; 10],
            }
        );
    }

    #[test]
    fn a_program_with_most_of_the_judged_files_starts_first_with_most_of_the_threads() {
        assert_eq!(
            schedule(16, &[100, 10_000, 100, 100, 100]),
            Schedule {
                order: vec![1, 0, 2, 3, 4],
                checkers: vec![4, 8, 4, 4, 4],
            }
        );
    }

    #[test]
    fn checkers_follow_the_share_of_the_judged_files_between_the_even_share_and_the_most() {
        assert_eq!(schedule(16, &[500, 300, 100, 100]).checkers, [8, 5, 4, 4]);
        let mut sizes = vec![10; 17];
        sizes[0] = 10_000;
        let wide = schedule(64, &sizes);
        assert_eq!(
            wide.checkers[0], MOST_SHARED_CHECKERS,
            "a program beside others gets at most MOST_SHARED_CHECKERS"
        );
        assert!(wide.checkers[1..].iter().all(|&checkers| checkers == 4));
    }

    #[test]
    fn a_program_gets_no_more_checkers_than_judged_files() {
        assert_eq!(schedule(16, &[1, 2, 3, 400]).checkers, [1, 2, 3, 8]);
        assert_eq!(
            schedule(16, &[0]).checkers,
            [1],
            "a program without judged files still has a checker"
        );
        assert_eq!(
            schedule(16, &[3]).checkers,
            [3],
            "a lone program's even share is capped too"
        );
    }

    #[test]
    fn programs_of_one_size_keep_their_plan_order_behind_larger_ones() {
        assert_eq!(schedule(16, &[5, 9, 5, 9]).order, [1, 3, 0, 2]);
    }

    #[test]
    fn a_program_waits_until_its_checkers_fit_beside_the_running_ones() {
        let cores = Cores::new(4);
        let running = cores.take(3);
        let (started, waited) = mpsc::channel();
        std::thread::scope(|scope| {
            let cores = &cores;
            scope.spawn(move || {
                let _taken = cores.take(2);
                started.send(()).expect("the test waits for the program");
            });
            assert!(
                waited.recv_timeout(Duration::from_millis(50)).is_err(),
                "two threads do not fit beside three of four"
            );
            drop(running);
            assert!(
                waited.recv_timeout(Duration::from_secs(30)).is_ok(),
                "the program starts once the running one gives its threads back"
            );
        });
        assert_eq!(cores.in_use(), 0);
        assert_eq!(cores.peak(), 3, "the waiting program never ran beside the first");
    }

    #[test]
    fn a_program_that_asks_for_more_threads_than_there_are_takes_them_all() {
        let cores = Cores::new(4);
        let all = cores.take(9);
        assert_eq!(cores.in_use(), 4);
        drop(all);
        assert_eq!(cores.in_use(), 0);
    }

    #[test]
    fn an_analysis_of_its_own_lets_its_early_programs_wait_for_cores() {
        let cores = Cores::new(4);
        let _running = cores.take(4);
        assert!(
            matches!(cores.admit_early(9), Some(Admitted::Later)),
            "an early program of one analysis waits for cores like the others"
        );
        assert_eq!(cores.in_use(), 4);
    }

    #[test]
    fn a_shared_budget_gives_early_programs_only_free_cores_of_its_early_share() {
        // A quarter of 8 cores is the early share; the other 6 are for programs of a known plan.
        let cores = Cores::shared(8);
        let Some(Admitted::Now(first)) = cores.admit_early(1) else {
            panic!("a free core of the early share is taken at once");
        };
        assert!(
            cores.admit_early(5).is_none(),
            "a program asking for more than the early share asks for all of it, and one core of it is held"
        );
        let Some(Admitted::Now(second)) = cores.admit_early(1) else {
            panic!("the last free core of the early share is taken at once");
        };
        assert!(cores.admit_early(1).is_none(), "the early share is held");
        let planned = cores.take(9);
        assert_eq!(
            cores.in_use(),
            8,
            "a planned program takes the planned share, whatever the early programs hold"
        );
        assert_eq!(cores.peak(), 8);
        assert_eq!(cores.takes(), 3);
        drop((first, second, planned));
        assert_eq!(cores.in_use(), 0);
    }

    #[test]
    fn a_planned_program_never_waits_for_cores_an_early_program_holds() {
        let cores = Cores::shared(4);
        let Some(Admitted::Now(early)) = cores.admit_early(4) else {
            panic!("the early share of 4 cores is one core");
        };
        let (started, waited) = mpsc::channel();
        std::thread::scope(|scope| {
            let cores = &cores;
            scope.spawn(move || {
                let _taken = cores.take(3);
                started.send(()).expect("the test waits for the program");
            });
            assert!(
                waited.recv_timeout(Duration::from_secs(30)).is_ok(),
                "the planned share is free while the early program holds its core"
            );
        });
        drop(early);
        assert_eq!(cores.peak(), 4);
    }

    #[test]
    fn a_shared_budget_too_small_for_an_early_share_starts_no_program_early() {
        for threads in 1..CORES_PER_PROGRAM {
            let cores = Cores::shared(threads);
            assert!(cores.admit_early(1).is_none(), "{threads} cores");
            drop(cores.take(threads));
            assert_eq!(cores.peak(), threads, "{threads} cores are all for planned programs");
        }
    }

    #[test]
    fn a_program_holds_a_core_for_each_checker_that_has_a_judged_file() {
        let cases = [((8, 3), 3), ((4, 100), 4), ((4, 0), 1), ((1, 1), 1)];
        for ((checkers, judged), expected) in cases {
            assert_eq!(
                charge(checkers, judged),
                expected,
                "{checkers} checkers, {judged} judged files"
            );
        }
    }
}
