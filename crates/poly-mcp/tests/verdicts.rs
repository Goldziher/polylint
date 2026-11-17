//! The tool verdict: every condition the CLI reports with exit code 2 reaches an MCP caller as
//! an `isError` result naming its cause, never as a clean document or a protocol error.
//!
//! A protocol error is a transport failure to most clients, so the model never sees the message
//! and cannot correct its call; a clean document is worse, since it reads as a pass.

mod common;

use rmcp::ServiceExt;
use rmcp::model::{CallToolRequestParams, CallToolResult, ClientCapabilities, ClientConfig};
use serde_json::Value;

/// Call `tool` with `arguments` on a fresh in-process server.
async fn call(tool: &str, arguments: Value) -> CallToolResult {
    let (server_io, client_io) = tokio::io::duplex(1 << 16);
    let (server_read, server_write) = tokio::io::split(server_io);
    let (client_read, client_write) = tokio::io::split(client_io);
    let server_task = tokio::spawn(async move {
        let service = common::server(None).serve((server_read, server_write)).await.unwrap();
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
