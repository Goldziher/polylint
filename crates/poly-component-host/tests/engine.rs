//! `ComponentEngine` against an in-process fake component.

mod common;

use std::path::Path;
use std::sync::Arc;

use common::{FakeComponent, config, desc, describe_of, init, source};
use poly_component_host::{ComponentEngine, ComponentSlot, SlotStatus, component_version};
use poly_component_protocol::hooks;
use poly_engine_api::declined::Declined;
use poly_engine_api::engine::{BatchSupport, Batching, Engine, FormatOutput, PlanPolicy, ProjectAnalysis};

fn engine_with(hooks: u32, host_hook: &str) -> (ComponentEngine, Arc<FakeComponent>) {
    let wire = desc(hooks, host_hook);
    let fake = FakeComponent::new(describe_of(wire.clone()));
    let slot = ComponentSlot::from_transport("demo-component", Some(describe_of(wire.clone())), init(), fake.clone());
    let engine = ComponentEngine::new("demo-component", &wire, "0123456789abcdef0123456789abcdef", slot).unwrap();
    (engine, fake)
}

#[test]
fn lint_and_format_round_trip() {
    let (engine, _) = engine_with(0, "none");
    let found = engine.lint(&source("select 1"), &config(4)).unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].code.as_deref(), Some("D001"));
    assert_eq!(found[0].title, "8 bytes");
    match engine.format(&source("select 1"), &config(4)).unwrap() {
        FormatOutput::Formatted(text) => assert_eq!(text, "SELECT 1"),
        FormatOutput::Unchanged => panic!("expected a rewrite"),
    }
    assert!(matches!(
        engine.format(&source("SELECT 1"), &config(4)).unwrap(),
        FormatOutput::Unchanged
    ));
}

#[test]
fn config_is_registered_once_per_fingerprint() {
    let (engine, fake) = engine_with(0, "none");
    for _ in 0..3 {
        engine.lint(&source("a"), &config(4)).unwrap();
    }
    assert_eq!(fake.count("RegisterConfig"), 1);
    engine.lint(&source("a"), &config(2)).unwrap();
    assert_eq!(fake.count("RegisterConfig"), 2);
    assert_eq!(fake.count("Init"), 1);
}

#[test]
fn concurrent_calls_register_a_config_once() {
    let (engine, fake) = engine_with(0, "none");
    let barrier = std::sync::Barrier::new(8);
    std::thread::scope(|scope| {
        for _ in 0..8 {
            scope.spawn(|| {
                barrier.wait();
                for _ in 0..20 {
                    engine.lint(&source("a"), &config(4)).unwrap();
                }
            });
        }
    });
    assert_eq!(fake.count("RegisterConfig"), 1);
    assert_eq!(fake.count("Lint"), 160);
}

#[test]
fn unknown_config_is_registered_again_and_retried() {
    let (engine, fake) = engine_with(0, "none");
    engine.lint(&source("a"), &config(4)).unwrap();
    fake.forget_configs();
    let found = engine.lint(&source("a"), &config(4)).unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!(fake.count("RegisterConfig"), 2);
    assert_eq!(fake.count("Lint"), 3);
}

#[test]
fn panic_reads_like_the_engine_guard() {
    let (engine, _) = engine_with(0, "none");
    let error = engine.lint(&source("panic"), &config(4)).unwrap_err();
    assert_eq!(
        error.to_string(),
        "the 'demo' backend panicked on this file: index out of bounds"
    );
}

#[test]
fn error_keeps_message_and_chain() {
    let (engine, _) = engine_with(0, "none");
    let error = engine.format(&source("error"), &config(4)).unwrap_err();
    assert_eq!(error.to_string(), "parse failed");
    let chain: Vec<String> = error.chain().map(ToString::to_string).collect();
    assert_eq!(chain, ["parse failed", "bad token", "at byte 3"]);
}

/// An undeclared cache-path hook is the host hook's answer, without loading the component.
#[test]
fn undeclared_cache_path_hooks_never_reach_the_component() {
    let (engine, fake) = engine_with(hooks::PROVIDES_LANGUAGE, "none");
    let cfg = config(4);
    let path = Path::new("q.sql");
    assert_eq!(engine.run_context(&cfg), "");
    assert_eq!(engine.file_context(path, &cfg), None);
    assert_eq!(engine.lint_input_salt(path, &cfg), None);
    assert_eq!(engine.format_input_salt(path, &cfg), None);
    let src = source("x");
    assert!(matches!(
        engine.format_config(&src, &cfg),
        std::borrow::Cow::Borrowed(_)
    ));
    assert_eq!(engine.name(), "demo");
    assert!(engine.capabilities().lint);
    assert!(engine.skip_reason(&src).is_none());
    assert!(fake.calls().is_empty(), "{:?}", fake.calls());
    assert_eq!(engine.slot().status(), SlotStatus::Unloaded);
}

/// A component engine that overrides a cache-path hook computes an input the host hook cannot
/// know (air digests `air.toml`), so the host asks the component, and its answer reaches the key.
#[test]
fn declared_cache_path_hooks_are_answered_by_the_component() {
    let all = hooks::RUN_CONTEXT | hooks::FILE_CONTEXT | hooks::LINT_INPUT_SALT | hooks::FORMAT_INPUT_SALT;
    let (engine, fake) = engine_with(all, "none");
    let cfg = config(4);
    let path = Path::new("q.sql");
    assert_eq!(engine.run_context(&cfg), "component run context");
    assert_eq!(engine.file_context(path, &cfg).as_deref(), Some("FileContext:q.sql"));
    assert_eq!(
        engine.lint_input_salt(path, &cfg).as_deref(),
        Some("LintInputSalt:q.sql")
    );
    assert_eq!(
        engine.format_input_salt(path, &cfg).as_deref(),
        Some("FormatInputSalt:q.sql")
    );
    for request in ["RunContext", "FileContext", "LintInputSalt", "FormatInputSalt"] {
        assert_eq!(fake.count(request), 1, "{request}: {:?}", fake.calls());
    }
}

/// A component that cannot answer a declared salt never yields the trait default, which would key
/// the result as if the input did not exist.
#[test]
fn a_failed_declared_salt_is_not_the_default() {
    let wire = desc(hooks::FORMAT_INPUT_SALT, "none");
    let engine = ComponentEngine::new(
        "demo-component",
        &wire,
        "ab",
        ComponentSlot::failed("demo-component", "x"),
    )
    .unwrap();
    let salt = engine.format_input_salt(Path::new("q.sql"), &config(4));
    assert!(salt.is_some_and(|salt| salt.contains("hook-failed")));
}

/// Every method `coverage.rs` says is carried by a request is sent as that request by the host,
/// for an engine that declares every hook.
#[test]
fn every_request_covered_method_reaches_the_component() {
    use poly_component_protocol::coverage::{Coverage, ENGINE_METHODS};
    let all = hooks::RUN_CONTEXT
        | hooks::FILE_CONTEXT
        | hooks::PROVIDES_LANGUAGE
        | hooks::SKIP_REASON
        | hooks::LINT_INPUT_SALT
        | hooks::FORMAT_INPUT_SALT
        | hooks::LINT_CACHE_SALT
        | hooks::BATCH_SUPPORT;
    let path = Path::new("q.sql");
    for (method, coverage) in ENGINE_METHODS {
        let Coverage::Request(request) = coverage else {
            continue;
        };
        let (engine, fake) = engine_with(all, "none");
        let cfg = config(4);
        let src = source("x");
        match *method {
            "run_context" => drop(engine.run_context(&cfg)),
            "file_context" => drop(engine.file_context(path, &cfg)),
            "lint_input_salt" => drop(engine.lint_input_salt(path, &cfg)),
            "format_input_salt" => drop(engine.format_input_salt(path, &cfg)),
            "lint_cache_salt" => drop(engine.lint_cache_salt(&src)),
            "provides_language_lint" => drop(engine.provides_language_lint(&src.language, &cfg)),
            "provides_language_format" => drop(engine.provides_language_format(&src.language, &cfg)),
            "skip_reason" | "format" => drop(engine.format(&src, &cfg)),
            "lint" => drop(engine.lint(&src, &cfg)),
            "batch_support" => drop(engine.batch_support(&cfg)),
            "format_batch" => drop(engine.format_batch(std::slice::from_ref(&src), &cfg)),
            "lint_batch" => drop(engine.lint_batch(std::slice::from_ref(&src), &cfg)),
            "project_check" | "project_report" => {
                let files = [poly_engine_api::project::ProjectFile {
                    path: path.to_path_buf(),
                    language: src.language.clone(),
                    content: src.content.clone(),
                }];
                let environment = poly_engine_api::project::ProjectEnvironment::default();
                let request = poly_engine_api::project::ProjectRequest {
                    root: Path::new(""),
                    files: &files,
                    config: &cfg,
                    environment: &environment,
                    threads: None,
                    targets: None,
                };
                if *method == "project_check" {
                    drop(engine.project_check(&request));
                } else {
                    drop(engine.project_report(&request));
                }
            }
            other => panic!("no exercise for `{other}`, which coverage.rs says is carried by {request}"),
        }
        assert!(
            fake.count(request) >= 1,
            "{method} did not send {request}: {:?}",
            fake.calls()
        );
    }
}

/// A component's decline reaches the host as a decline (ADR 0079). `project_check` flattens it into no findings, the
/// same as a clean pass, so a comparison that must tell the two apart compares `project_report`.
#[test]
fn a_declined_project_crosses_the_proxy_as_a_decline() {
    use poly_engine_api::project::{ProjectEnvironment, ProjectRequest};
    let (engine, _) = engine_with(0, "none");
    let cfg = config(4);
    let environment = ProjectEnvironment::default();
    let request = |root: &'static str| ProjectRequest {
        root: Path::new(root),
        files: &[],
        config: &cfg,
        environment: &environment,
        threads: None,
        targets: None,
    };
    let declined = engine.project_report(&request("declined")).unwrap();
    let clean = engine.project_report(&request("clean")).unwrap();
    assert_eq!(format!("{declined:?}"), r#"Declined("nothing to check")"#);
    assert_eq!(format!("{clean:?}"), "Checked([])");
    assert_eq!(
        format!("{:?}", engine.project_check(&request("declined")).unwrap()),
        format!("{:?}", engine.project_check(&request("clean")).unwrap()),
        "project_check cannot tell a decline from a clean pass"
    );
}

#[test]
fn per_file_hooks_are_called_only_when_the_bit_is_set() {
    let (engine, fake) = engine_with(hooks::PROVIDES_LANGUAGE, "none");
    let src = source("x");
    let cfg = config(4);
    assert!(engine.provides_language_lint(&src.language, &cfg));
    assert!(!engine.provides_language_lint(&poly_engine_api::language::Language::Go, &cfg));
    assert_eq!(fake.count("ProvidesLanguage"), 2);
}

/// v3: a component's own declines come back in the `Format` reply, so `skip_reason` never makes a
/// call, even for an engine that declares it overrides the method.
#[test]
fn skip_reason_never_reaches_the_component() {
    let (engine, fake) = engine_with(hooks::SKIP_REASON, "none");
    assert_eq!(engine.skip_reason(&source("{{ x }}")), None);
    assert!(fake.calls().is_empty(), "{:?}", fake.calls());
    assert_eq!(engine.slot().status(), SlotStatus::Unloaded);
}

#[test]
fn a_declining_format_reply_is_a_declined_error_from_one_call() {
    let (engine, fake) = engine_with(hooks::SKIP_REASON, "none");
    let error = engine.format(&source("{{ x }}"), &config(4)).unwrap_err();
    assert_eq!(Declined::reason(&error), Some("templated input"));
    assert_eq!(fake.calls(), ["Init", "RegisterConfig", "Format"]);
    // Any other failure is not a decline.
    let error = engine.format(&source("error"), &config(4)).unwrap_err();
    assert_eq!(Declined::reason(&error), None);
}

#[test]
fn a_declined_batch_member_is_a_declined_error_beside_its_siblings() {
    let (engine, fake) = engine_with(hooks::BATCH_SUPPORT, "none");
    let batch = [source("a"), source("{{ x }}"), source("B")];
    let results = engine.format_batch(&batch, &config(4)).unwrap();
    assert_eq!(results.len(), 3);
    assert!(matches!(&results[0], Ok(FormatOutput::Formatted(text)) if text == "A"));
    let declined = results[1].as_ref().unwrap_err();
    assert_eq!(Declined::reason(declined), Some("templated input"));
    assert!(matches!(results[2], Ok(FormatOutput::Unchanged)));
    assert_eq!(fake.count("FormatBatch"), 1);
}

#[test]
fn batch_support_is_asked_once_per_config() {
    let (engine, fake) = engine_with(hooks::BATCH_SUPPORT, "none");
    let expected = BatchSupport {
        format: true,
        lint: false,
    };
    for _ in 0..50 {
        assert_eq!(engine.batch_support(&config(4)), expected);
    }
    assert_eq!(fake.count("BatchSupport"), 1);
    // A clone of the engine shares the connection, and with it the answer.
    assert_eq!(engine.clone().batch_support(&config(4)), expected);
    assert_eq!(fake.count("BatchSupport"), 1);
    // Another config is another question.
    assert_eq!(engine.batch_support(&config(2)), expected);
    assert_eq!(fake.count("BatchSupport"), 2);
}

#[test]
fn concurrent_batch_support_asks_are_answered_from_one_memo() {
    let (engine, fake) = engine_with(hooks::BATCH_SUPPORT, "none");
    engine.batch_support(&config(4));
    std::thread::scope(|scope| {
        for _ in 0..8 {
            scope.spawn(|| {
                for _ in 0..20 {
                    assert!(engine.batch_support(&config(4)).format);
                }
            });
        }
    });
    assert_eq!(fake.count("BatchSupport"), 1);
}

#[test]
fn undeclared_per_file_hooks_answer_the_default_without_connecting() {
    let (engine, fake) = engine_with(0, "none");
    let src = source("x");
    let cfg = config(4);
    assert_eq!(engine.lint_cache_salt(&src), None);
    assert_eq!(engine.lint_cache_version(&src), engine.version());
    assert_eq!(engine.batch_support(&cfg), BatchSupport::default());
    assert!(engine.provides_language_lint(&src.language, &cfg));
    assert!(engine.provides_language_format(&src.language, &cfg));
    assert!(engine.provides_language_lint(&poly_engine_api::language::Language::Go, &cfg));
    assert!(engine.skip_reason(&src).is_none());
    assert!(fake.calls().is_empty(), "{:?}", fake.calls());
    assert_eq!(engine.slot().status(), SlotStatus::Unloaded);
}

#[test]
fn unknown_host_hook_is_rejected() {
    let wire = desc(0, "bogus");
    let slot = ComponentSlot::failed("demo-component", "x");
    assert!(ComponentEngine::new("demo-component", &wire, "ab", slot).is_err());
}

#[test]
fn version_embeds_the_first_sixteen_lock_digits() {
    let (engine, _) = engine_with(0, "none");
    assert_eq!(engine.version(), "1.2.3+c0123456789abcdef");
    assert_eq!(component_version("1.2.3", "abcd"), "1.2.3+cabcd");
    assert_ne!(component_version("1.2.3", "aa".repeat(32).as_str()), engine.version());
}

#[test]
fn static_metadata_comes_from_the_index() {
    let (engine, fake) = engine_with(0, "oxfmt-scope");
    assert_eq!(engine.languages().len(), 1);
    assert_eq!(engine.languages()[0].id(), "sql");
    assert_eq!(engine.batch_shard_floor(), 2);
    assert_eq!(engine.host_hook(), poly_engine_api::host_hooks::HostHook::OxfmtScope);
    assert!(fake.calls().is_empty());
}

#[test]
fn unavailable_engine_carries_its_reason() {
    let wire = desc(0, "none");
    let engine =
        ComponentEngine::unavailable("demo-component", &wire, "feedfacefeedface00", "offline and not cached").unwrap();
    let src = source("x");
    let cfg = config(4);
    assert_eq!(engine.unavailable_reason(), Some("offline and not cached"));
    assert_eq!(engine.skip_reason(&src).as_deref(), Some("offline and not cached"));
    assert!(!engine.provides_language_lint(&src.language, &cfg));
    assert_eq!(engine.version(), "1.2.3+cfeedfacefeedface+unavailable");
    let error = engine.lint(&src, &cfg).unwrap_err();
    assert!(error.to_string().contains("offline and not cached"), "{error:#}");
}

#[test]
fn describe_mismatch_fails_the_load_once() {
    let wire = desc(0, "none");
    let mut other = wire.clone();
    other.version = "9.9.9".into();
    let fake = FakeComponent::new(describe_of(other));
    let slot = ComponentSlot::from_transport("demo-component", Some(describe_of(wire.clone())), init(), fake.clone());
    let engine = ComponentEngine::new("demo-component", &wire, "ab", slot).unwrap();
    let first = engine.lint(&source("x"), &config(4)).unwrap_err().to_string();
    assert!(first.contains("differently from the index"), "{first}");
    let second = engine.lint(&source("x"), &config(4)).unwrap_err().to_string();
    assert_eq!(first, second);
    assert_eq!(fake.count("Init"), 1);
    assert!(matches!(engine.slot().status(), SlotStatus::Failed(_)));
}

#[test]
fn failing_stub_declines_nothing_and_fails_every_call() {
    let wire = desc(0, "none");
    let engine = ComponentEngine::unavailable_failing("demo-component", &wire, "feedfacefeedface00", "boom").unwrap();
    let src = source("x");
    assert_eq!(engine.skip_reason(&src), None);
    assert_eq!(engine.unavailable_reason(), Some("boom"));
    let error = engine.lint(&src, &config(4)).unwrap_err();
    assert!(error.to_string().contains("boom"), "{error:#}");
}

#[test]
fn declining_stub_skips_and_does_nothing_else() {
    let wire = desc(0, "none");
    let engine =
        ComponentEngine::unavailable_declining("demo-component", &wire, "feedfacefeedface00", "absent").unwrap();
    let src = source("x");
    assert_eq!(engine.skip_reason(&src).as_deref(), Some("absent"));
    assert!(engine.lint(&src, &config(4)).unwrap().is_empty());
    assert!(matches!(
        engine.format(&src, &config(4)).unwrap(),
        FormatOutput::Unchanged
    ));
}
