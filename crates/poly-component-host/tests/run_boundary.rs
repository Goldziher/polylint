//! The run boundary: a component loaded by an earlier run of the same process is told when a new
//! one starts, before the new run's first request, so it forgets what it memoised from project
//! files. `poly mcp` on a core build is that process.
//!
//! The run counter is process-wide, so these tests live in a binary of their own and take a lock.

mod common;

use std::sync::{Arc, Mutex, PoisonError};

use common::{FakeComponent, config, desc, describe_of, init, source};
use poly_component_host::{ComponentEngine, ComponentSlot};
use poly_component_protocol::hooks;
use poly_engine_api::engine::Engine;
use poly_engine_api::host_hooks::begin_run;

static RUNS: Mutex<()> = Mutex::new(());

fn engine_with(hooks: u32) -> (ComponentEngine, Arc<FakeComponent>) {
    let wire = desc(hooks, "none");
    let fake = FakeComponent::new(describe_of(wire.clone()));
    let slot = ComponentSlot::from_transport("demo-component", Some(describe_of(wire.clone())), init(), fake.clone());
    let engine = ComponentEngine::new("demo-component", &wire, "0123456789abcdef0123456789abcdef", slot).unwrap();
    (engine, fake)
}

#[test]
fn a_component_loaded_in_this_run_is_not_told_about_it() {
    let _serial = RUNS.lock().unwrap_or_else(PoisonError::into_inner);
    begin_run();
    let (engine, fake) = engine_with(0);
    engine.lint(&source("a"), &config(4)).unwrap();
    engine.format(&source("a"), &config(4)).unwrap();
    assert_eq!(fake.count("RunBoundary"), 0, "{:?}", fake.calls());
}

#[test]
fn a_new_run_is_announced_once_before_its_first_lint() {
    let _serial = RUNS.lock().unwrap_or_else(PoisonError::into_inner);
    let (engine, fake) = engine_with(0);
    engine.lint(&source("a"), &config(4)).unwrap();
    begin_run();
    engine.lint(&source("a"), &config(4)).unwrap();
    engine.lint(&source("b"), &config(4)).unwrap();
    assert_eq!(
        fake.calls(),
        ["Init", "RegisterConfig", "Lint", "RunBoundary", "Lint", "Lint"]
    );
}

#[test]
fn a_new_run_is_announced_before_a_format_and_before_a_configless_request() {
    let _serial = RUNS.lock().unwrap_or_else(PoisonError::into_inner);
    let (engine, fake) = engine_with(hooks::LINT_CACHE_SALT);
    engine.format(&source("a"), &config(4)).unwrap();
    begin_run();
    engine.format(&source("a"), &config(4)).unwrap();
    begin_run();
    let _ = engine.lint_cache_salt(&source("a"));
    let calls = fake.calls();
    assert_eq!(calls[2..5], ["Format", "RunBoundary", "Format"], "{calls:?}");
    assert_eq!(calls[5], "RunBoundary", "{calls:?}");
    assert_eq!(calls.len(), 7, "{calls:?}");
}

#[test]
fn concurrent_first_requests_of_a_run_announce_it_once_and_never_overtake_it() {
    let _serial = RUNS.lock().unwrap_or_else(PoisonError::into_inner);
    let (engine, fake) = engine_with(0);
    engine.lint(&source("a"), &config(4)).unwrap();
    begin_run();
    let barrier = std::sync::Barrier::new(8);
    std::thread::scope(|scope| {
        for _ in 0..8 {
            scope.spawn(|| {
                barrier.wait();
                engine.lint(&source("a"), &config(4)).unwrap();
            });
        }
    });
    let calls = fake.calls();
    assert_eq!(fake.count("RunBoundary"), 1, "{calls:?}");
    assert_eq!(
        calls[3], "RunBoundary",
        "no request of the new run precedes it: {calls:?}"
    );
}
