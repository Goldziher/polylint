//! The tool verdict: every condition the CLI reports with exit code 2 reaches an MCP caller as
//! an `isError` result naming its cause, never as a clean document or a protocol error.
//!
//! A protocol error is a transport failure to most clients, so the model never sees the message
//! and cannot correct its call; a clean document is worse, since it reads as a pass.

mod common;

use std::sync::Arc;

use poly_host_services::{Services, WorkspaceGuard};
use poly_mcp::PolyMcpServer;
use rmcp::ServiceExt;
use rmcp::model::{CallToolRequestParams, CallToolResult, ClientCapabilities, ClientConfig};
use serde_json::Value;

/// Call `tool` with `arguments` on a fresh in-process server.
async fn call(tool: &str, arguments: Value) -> CallToolResult {
    call_on(common::server(None), tool, arguments).await
}

/// Call `tool` with `arguments` on `server`.
async fn call_on(server: PolyMcpServer, tool: &str, arguments: Value) -> CallToolResult {
    let (server_io, client_io) = tokio::io::duplex(1 << 16);
    let (server_read, server_write) = tokio::io::split(server_io);
    let (client_read, client_write) = tokio::io::split(client_io);
    let server_task = tokio::spawn(async move {
        let service = server.serve((server_read, server_write)).await.unwrap();
        service.waiting().await.unwrap();
    });
    let client = ClientConfig::new(
        ClientCapabilities::builder().build(),
        rmcp::model::Implementation::new("poly-mcp-test", "0.0.0"),
    )
    .serve((client_read, client_write))
    .await
    .unwrap();
    let Value::Object(arguments) = arguments else {
        panic!("arguments are an object")
    };
    let result = client
        .call_tool(CallToolRequestParams::new(tool.to_string()).with_arguments(arguments))
        .await
        .unwrap_or_else(|error| panic!("{tool} answered with a protocol error: {error}"));
    client.cancel().await.unwrap();
    let _ = server_task.await;
    result
}

/// The `error` block of an `isError` result.
fn failure(result: &CallToolResult) -> &Value {
    assert_eq!(result.is_error, Some(true), "{result:?}");
    let structured = result.structured_content.as_ref().expect("structured content");
    &structured["error"]
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_missing_path_is_an_error_naming_it_not_a_clean_run() {
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("nope.py").display().to_string();
    for kind in ["lint", "format"] {
        let result = call("check", serde_json::json!({ "kind": kind, "paths": [missing] })).await;
        let error = failure(&result);
        assert_eq!(error["kind"], "paths", "{kind}: {error}");
        assert!(
            error["message"].as_str().unwrap().contains("nope.py"),
            "{kind}: the message names the path: {error}"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_broken_config_is_an_error_result_not_a_protocol_error() {
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("poly.toml");
    std::fs::write(&config, "[discovery\nexclude = 3\n").unwrap();
    std::fs::write(dir.path().join("a.py"), "x = 1\n").unwrap();
    let result = call(
        "check",
        serde_json::json!({
            "paths": [dir.path().join("a.py")],
            "config": config,
        }),
    )
    .await;
    let error = failure(&result);
    assert_eq!(error["kind"], "config", "{error}");
    let text = result.content[0].as_text().expect("text block").text.clone();
    assert!(
        text.contains("\"config\""),
        "the text block carries the failure too: {text}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_failing_rule_test_sets_is_error() {
    let dir = tempfile::tempdir().unwrap();
    let rules = dir.path().join("rules");
    std::fs::create_dir(&rules).unwrap();
    std::fs::write(
        rules.join("no-print.yml"),
        "id: no-print\nlanguage: python\nseverity: warning\nmessage: no print\nrule:\n  pattern: print($A)\n",
    )
    .unwrap();
    // The `valid` snippet matches the rule, so the test fails.
    std::fs::write(
        rules.join("no-print-test.yml"),
        "id: no-print\nvalid:\n  - print(1)\ninvalid:\n  - print(2)\n",
    )
    .unwrap();
    let config = dir.path().join("poly.toml");
    std::fs::write(&config, "").unwrap();
    let result = call(
        "info",
        serde_json::json!({
            "topics": ["rules"],
            "dirs": [rules],
            "test": true,
            "config": config,
        }),
    )
    .await;
    assert_eq!(result.is_error, Some(true), "{result:?}");
    let structured = result.structured_content.as_ref().expect("structured content");
    assert_eq!(structured["rules"]["ok"], Value::Bool(false), "{structured}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn arguments_that_do_not_go_together_are_a_usage_error_result() {
    let result = call("check", serde_json::json!({ "kind": "format", "scope": "workspace" })).await;
    assert_eq!(failure(&result)["kind"], "usage");
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.py"), "x = 1\n").unwrap();
    let result = call(
        "check",
        serde_json::json!({ "paths": [dir.path().join("a.py")], "only": ["ruff"], "skip": ["typos"] }),
    )
    .await;
    assert_eq!(failure(&result)["kind"], "usage");
}

/// A skip is coverage information until the caller sets a budget for it; then exceeding it is
/// the CLI's exit 2.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn skips_over_the_budget_set_is_error() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("App.csproj");
    std::fs::write(&file, "<Project></Project>\n").unwrap();
    let within = call("check", serde_json::json!({ "paths": [file] })).await;
    assert_eq!(within.is_error, Some(false), "a skip alone is not an error");
    let over = call("check", serde_json::json!({ "paths": [file], "max_skips": 0 })).await;
    assert_eq!(over.is_error, Some(true), "the budget makes it one");
}

/// The schemas the tools no longer declare are readable as resources.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn info_answers_one_key_per_topic_and_validates_configs() {
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("poly.toml");
    std::fs::write(&config, "[engines.ruff]\nno_such_key = 1\n").unwrap();
    let result = call(
        "info",
        serde_json::json!({ "topics": ["version", "validate", "cache"], "paths": [config] }),
    )
    .await;
    let structured = result.structured_content.as_ref().unwrap();
    for topic in ["version", "validate", "cache"] {
        assert!(structured[topic].is_object(), "{topic}: {structured}");
    }
    assert_eq!(structured["validate"]["valid"], Value::Bool(false), "{structured}");
    assert_eq!(result.is_error, Some(true), "an invalid config is the CLI's exit 2");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_empty_topic_list_is_a_usage_error() {
    let result = call("info", serde_json::json!({ "topics": [] })).await;
    let error = failure(&result);
    assert_eq!(error["kind"], "usage", "{error}");
    assert!(error["message"].as_str().unwrap().contains("topics"), "{error}");
}

/// A topic the host could not answer carries the kind `check` would report for the same cause,
/// not a generic `pipeline`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_info_topic_failure_is_classified_like_a_run() {
    let dir = tempfile::tempdir().unwrap();
    let broken = dir.path().join("poly.toml");
    std::fs::write(&broken, "[discovery\n").unwrap();
    let result = call(
        "info",
        serde_json::json!({ "topics": ["version", "config", "rules", "cache"], "config": broken }),
    )
    .await;
    assert_eq!(result.is_error, Some(true), "{result:?}");
    let structured = result.structured_content.as_ref().unwrap();
    assert!(structured["version"]["engines"].is_object(), "{structured}");
    for topic in ["config", "rules", "cache"] {
        assert_eq!(structured[topic]["error"]["kind"], "config", "{topic}: {structured}");
    }

    let root = tempfile::tempdir().unwrap();
    let guard = WorkspaceGuard::new(root.path(), None, false).unwrap();
    let guarded = PolyMcpServer::new(Arc::new(Services::new().with_workspace_guard(guard)), None);
    let result = call_on(
        guarded,
        "info",
        serde_json::json!({ "topics": ["config", "validate"], "config": broken, "paths": [broken] }),
    )
    .await;
    assert_eq!(result.is_error, Some(true), "{result:?}");
    let structured = result.structured_content.as_ref().unwrap();
    for topic in ["config", "validate"] {
        assert_eq!(structured[topic]["error"]["kind"], "workspace", "{topic}: {structured}");
        let message = structured[topic]["error"]["message"].as_str().unwrap();
        assert!(message.contains("outside the workspace"), "{topic}: {message}");
    }
}

/// The cache a `[cache] dir` names is the one `info` measures and `manage` cleans, whether the
/// config comes with the request or from the server's `--config`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_cache_topic_and_actions_follow_the_requested_config() {
    let dir = tempfile::tempdir().unwrap();
    let cache = tempfile::tempdir().unwrap();
    let config = dir.path().join("poly.toml");
    std::fs::write(
        &config,
        format!("[cache]\ndir = {:?}\n", cache.path().display().to_string()),
    )
    .unwrap();
    let seeded = poly_cache::ResultCache::open(cache.path().to_path_buf(), true).unwrap();
    let key = poly_cache::ResultCache::key(
        poly_cache::Namespace::Lint,
        "seed",
        "1",
        &Default::default(),
        &poly_cache::ResultCache::single_file_digest("x"),
    );
    seeded.put(poly_cache::Namespace::Lint, &key, b"[]").unwrap();
    let entries =
        |result: &CallToolResult| result.structured_content.as_ref().unwrap()["cache"]["total_entries"].clone();

    let stats = call("info", serde_json::json!({ "topics": ["cache"], "config": config })).await;
    assert_eq!(stats.is_error, Some(false), "{stats:?}");
    assert!(
        entries(&stats).as_u64().unwrap() > 0,
        "the seeded entry is in the configured cache: {stats:?}"
    );
    let pinned = call_on(
        common::server(Some(config.clone())),
        "info",
        serde_json::json!({ "topics": ["cache"] }),
    )
    .await;
    assert_eq!(
        entries(&pinned),
        entries(&stats),
        "the server's --config is the fallback"
    );

    for action in [
        serde_json::json!({ "action": "cache_gc", "max_size": "1G" }),
        serde_json::json!({ "action": "cache_clean" }),
    ] {
        let mut arguments = action.clone();
        arguments["config"] = serde_json::json!(config);
        let result = call("manage", arguments).await;
        assert_eq!(result.is_error, Some(false), "{action}: {result:?}");
    }
    let after = call("info", serde_json::json!({ "topics": ["cache"], "config": config })).await;
    assert_eq!(
        entries(&after),
        serde_json::json!(0),
        "cache_clean emptied the configured cache: {after:?}"
    );
}

/// `scope: all` with fixes plans the whole-project phase before it rewrites anything: a phase
/// that cannot be planned must not leave the per-file tier's fixes behind.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_scope_all_fix_writes_nothing_when_the_phase_cannot_be_planned() {
    const BAD: &str = "import os\n";
    let fixable = |hooks: &str| {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("bad.py"), BAD).unwrap();
        std::fs::write(dir.path().join("poly.toml"), hooks).unwrap();
        dir
    };
    // Control: the same fix with a plannable (disabled) phase rewrites the file, so an unchanged
    // file below is the refusal and not a fix that never applies.
    let control = fixable("[workspace]\nrun = \"never\"\n");
    let result = call(
        "fix",
        serde_json::json!({
            "scope": "all",
            "paths": [control.path().join("bad.py")],
            "config": control.path().join("poly.toml"),
        }),
    )
    .await;
    assert_ne!(
        std::fs::read_to_string(control.path().join("bad.py")).unwrap(),
        BAD,
        "the control run applies the fix: {result:?}"
    );

    let broken =
        fixable("[hooks.pre-commit.commands.check]\nrun = \"true\"\nworkspace = true\ntimeout = \"whenever\"\n");
    let result = call(
        "fix",
        serde_json::json!({
            "scope": "all",
            "paths": [broken.path().join("bad.py")],
            "config": broken.path().join("poly.toml"),
        }),
    )
    .await;
    let error = failure(&result);
    assert!(error["message"].as_str().unwrap().contains("timeout"), "{error}");
    assert_eq!(
        std::fs::read_to_string(broken.path().join("bad.py")).unwrap(),
        BAD,
        "nothing was rewritten"
    );
}
