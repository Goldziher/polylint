//! Tests of the session's queue, schedule, slot and error mapping.

use super::*;

fn parallelism(threads: usize, programs: usize) -> Parallelism {
    Parallelism::new(NonZeroUsize::new(threads), programs)
}

const fn admitted(_position: usize) -> bool {
    true
}

fn configs(names: &[&str]) -> Vec<String> {
    names.iter().map(|name| (*name).to_owned()).collect()
}

fn planned(configs_and_sizes: &[(&str, usize)]) -> Vec<PlannedProgram> {
    let mut next = 0;
    configs_and_sizes
        .iter()
        .map(|&(config, size)| {
            let files = (next..next + size).collect();
            next += size;
            PlannedProgram {
                config: config.to_owned(),
                files,
            }
        })
        .collect()
}

#[test]
fn early_programs_start_until_the_judged_files_are_planned_then_the_rest_of_their_plan() {
    let early = configs(&["a", "b"]);
    let mut queue = Queue::default();
    assert_eq!(queue.next(&early, Gate::Closed, admitted), Next::Early(0));
    assert_eq!(queue.next(&early, Gate::Closed, admitted), Next::Early(1));
    assert_eq!(
        queue.next(&early, Gate::Closed, admitted),
        Next::Wait,
        "every early program has started"
    );
    let judged = configs(&["b", "c", "a", "d"]);
    assert_eq!(
        queue.next(&early, Gate::Open(&judged), admitted),
        Next::Judged(1),
        "`b` started early"
    );
    assert_eq!(
        queue.next(&early, Gate::Open(&judged), admitted),
        Next::Judged(3),
        "`a` started early"
    );
    assert_eq!(queue.next(&early, Gate::Open(&judged), admitted), Next::Done);
}

#[test]
fn an_early_program_not_started_when_the_judged_files_are_planned_starts_from_their_plan() {
    let early = configs(&["a", "b"]);
    let mut queue = Queue::default();
    assert_eq!(queue.next(&early, Gate::Closed, admitted), Next::Early(0));
    let judged = configs(&["a", "b"]);
    assert_eq!(queue.next(&early, Gate::Open(&judged), admitted), Next::Judged(1));
    assert_eq!(queue.next(&early, Gate::Open(&judged), admitted), Next::Done);
}

#[test]
fn nothing_starts_once_the_analysis_is_given_up() {
    let early = configs(&["a"]);
    let mut queue = Queue::default();
    assert_eq!(queue.next(&early, Gate::Abandoned, admitted), Next::Done);
    assert_eq!(
        queue.next(&early, Gate::Closed, admitted),
        Next::Early(0),
        "the early program had not started"
    );
}

#[test]
fn an_early_program_refused_cores_starts_from_the_judged_files_plan() {
    let early = configs(&["a", "b"]);
    let mut queue = Queue::default();
    assert_eq!(
        queue.next(&early, Gate::Closed, |_| false),
        Next::Wait,
        "a program refused cores waits for the judged files' plan"
    );
    assert_eq!(
        queue.next(&early, Gate::Closed, admitted),
        Next::Early(0),
        "and is still queued"
    );
    let judged = configs(&["a", "b"]);
    assert_eq!(queue.next(&early, Gate::Open(&judged), |_| false), Next::Judged(1));
    assert_eq!(queue.next(&early, Gate::Open(&judged), admitted), Next::Done);
}

#[test]
fn the_judged_files_keep_the_schedule_of_an_early_plan_of_the_same_programs() {
    let programs = planned(&[("a", 100), ("b", 10_000), ("c", 100), ("d", 100), ("e", 100)]);
    let sizes: Vec<usize> = programs.iter().map(|program| program.files.len()).collect();
    let parallelism = parallelism(16, programs.len());
    let own = Schedule::new(&parallelism, &sizes);
    let plan = ProgramPlan {
        programs,
        files: Vec::new(),
    };
    let early = Early::new(&plan, &own);
    assert_eq!(judged_schedule(&parallelism, &early, &plan.programs), own);
}

#[test]
fn a_judged_program_keeps_its_early_checkers_and_order_and_the_others_follow_by_size() {
    let early = Early {
        configs: configs(&["gone", "b", "a"]),
        checkers: vec![8, 6, 5],
    };
    // Fewer judged files than early ones in `a` and `b`, which may already be built with their early checkers.
    let programs = planned(&[("a", 1), ("b", 2), ("c", 30), ("d", 400)]);
    let schedule = judged_schedule(&parallelism(16, programs.len()), &early, &programs);
    assert_eq!(
        schedule,
        Schedule {
            order: vec![1, 0, 3, 2],
            checkers: vec![5, 6, 4, 8],
        }
    );
}

#[test]
fn a_slot_hands_its_first_value_to_every_waiting_thread() {
    let slot: Slot<Option<u32>> = Slot::default();
    assert_eq!(slot.get(), None);
    std::thread::scope(|scope| {
        let waiter = scope.spawn(|| slot.wait());
        slot.put(Some(7));
        slot.put(None);
        assert_eq!(waiter.join().unwrap(), Some(7));
    });
    assert_eq!(slot.get(), Some(Some(7)), "a later value does not replace the first");
    assert_eq!(slot.take(), Some(7));
    assert_eq!(slot.get(), None);
}

#[test]
fn panic_payloads_become_internal_errors() {
    let message = internal("planning the programs", &"boom");
    assert!(matches!(message, TypeError::Internal { message } if message == "planning the programs: boom"));
    let owned = internal("x", &String::from("bad"));
    assert!(matches!(owned, TypeError::Internal { message } if message == "x: bad"));
}
