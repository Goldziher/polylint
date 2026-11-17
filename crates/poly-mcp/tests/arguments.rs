//! Arguments a tool cannot act on are refused, and `info` reaches nothing that writes.
//!
//! A dropped argument answers a different question than the one asked: a typo for `paths` lints
//! the whole directory, `dry_run` on a destructive action destroys. Every refusal is an
//! `isError` result with `error.kind: usage` naming the key, never a protocol error.

mod common;

use std::sync::{Arc, Mutex};

use poly_frontend_api::ServiceCaller;
use poly_frontend_api::wire::service;
use poly_host_services::Services;
use poly_mcp::PolyMcpServer;
use rmcp::ServiceExt;
use rmcp::model::{CallToolRequestParams, CallToolResult, ClientCapabilities, ClientConfig};
use serde_json::{Value, json};

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

async fn call(tool: &str, arguments: Value) -> CallToolResult {
    call_on(common::server(None), tool, arguments).await
}

/// The message of a usage error, which must be the one failure shape.
fn usage(result: &CallToolResult) -> String {
    assert_eq!(result.is_error, Some(true), "{result:?}");
    let structured = result.structured_content.as_ref().expect("structured content");
    assert_eq!(structured["error"]["kind"], "usage", "{structured}");
    assert_eq!(structured["summary"], Value::Null, "{structured}");
    structured["error"]["message"].as_str().expect("a message").to_owned()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_unknown_argument_is_refused_by_name_on_every_tool() {
    let cases = [
        ("check", json!({ "path": ["a.py"] }), "path"),
        ("fix", json!({ "path": ["a.py"] }), "path"),
        ("info", json!({ "topic": ["version"] }), "topic"),
        ("manage", json!({ "action": "cache_gc", "max_age": 3 }), "max_age"),
    ];
    for (tool, arguments, key) in cases {
        let message = usage(&call(tool, arguments).await);
        assert!(message.contains(&format!("`{key}`")), "{tool}: {message}");
    }
}

/// A wrong value is the same kind of mistake as a wrong key, so it gets the same answer rather
/// than a JSON-RPC error the model never reads.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_invalid_value_is_a_usage_result_not_a_protocol_error() {
    let message = usage(&call("check", json!({ "kind": "fmt" })).await);
    assert!(message.contains("fmt"), "{message}");
    usage(&call("manage", json!({})).await);
    usage(&call("info", json!({ "topics": "version" })).await);
}

/// `check` takes `RunRequest`, which has no `apply`. Whatever a caller injects to ask for a
/// write is refused, and nothing is written.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn check_refuses_every_spelling_of_a_write_and_writes_nothing() {
    const UNFORMATTED: &str = "x   =   [1,2]\n";
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("a.py");
    std::fs::write(&file, UNFORMATTED).unwrap();
    for key in ["apply", "fix", "write", "fix_generated"] {
        let result = call("check", json!({ "kind": "format", "paths": [file], key: true })).await;
        let message = usage(&result);
        assert!(message.contains(&format!("`{key}`")), "{message}");
        assert_eq!(
            std::fs::read_to_string(&file).unwrap(),
            UNFORMATTED,
            "{key} wrote the file"
        );
    }
    let dry = call("check", json!({ "kind": "format", "paths": [file] })).await;
    assert_eq!(dry.is_error, Some(false), "{dry:?}");
    assert_eq!(
        std::fs::read_to_string(&file).unwrap(),
        UNFORMATTED,
        "a check wrote the file"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_parameter_that_does_not_apply_is_refused() {
    let cases = [
        ("manage", json!({ "action": "cache_clean", "dry_run": true }), "dry_run"),
        ("manage", json!({ "action": "cache_gc", "dry_run": true }), "dry_run"),
        (
            "manage",
            json!({ "action": "components_uninstall", "names": ["web"], "dry_run": true }),
            "dry_run",
        ),
        (
            "manage",
            json!({ "action": "components_prune", "names": ["web"] }),
            "names",
        ),
        (
            "manage",
            json!({ "action": "cache_clean", "max_size": "1M" }),
            "max_size",
        ),
        ("info", json!({ "topics": ["version"], "dirs": ["rules"] }), "dirs"),
        ("info", json!({ "topics": ["cache"], "paths": ["a.py"] }), "paths"),
        (
            "check",
            json!({ "scope": "workspace", "exclude": ["e2e/**"] }),
            "exclude",
        ),
        ("check", json!({ "scope": "workspace", "paths": ["a.py"] }), "paths"),
        ("fix", json!({ "scope": "workspace", "only": ["ruff"] }), "only"),
    ];
    for (tool, arguments, key) in cases {
        let message = usage(&call(tool, arguments.clone()).await);
        assert!(message.contains(&format!("`{key}`")), "{tool} {arguments}: {message}");
    }
}

/// The host services with every call recorded.
struct Recording {
    services: Services,
    called: Mutex<Vec<String>>,
}

impl ServiceCaller for Recording {
    fn call(&self, service: &str, request: &[u8]) -> Result<Vec<u8>, String> {
        self.called.lock().unwrap().push(service.to_owned());
        self.services.call(service, request)
    }
}

/// `info` is annotated read-only and closed-world; the annotation is a claim about what it
/// calls. Every topic at once reaches only the host's read services: no run, no install, no
/// uninstall, no cache maintenance.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn info_calls_only_the_hosts_read_services() {
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("poly.toml");
    std::fs::write(&config, "[components]\ninstall = \"auto\"\n").unwrap();
    let host = Arc::new(Recording {
        services: Services::new(),
        called: Mutex::new(Vec::new()),
    });
    let server = PolyMcpServer::new(host.clone(), None);
    host.called.lock().unwrap().clear();
    let result = call_on(
        server,
        "info",
        json!({
            "topics": ["version", "config", "validate", "rules", "components", "cache"],
            "config": config,
            "paths": [config],
            "install": "auto",
            "test": true,
        }),
    )
    .await;
    let structured = result.structured_content.as_ref().expect("structured content");
    for topic in ["version", "config", "validate", "rules", "components", "cache"] {
        assert!(structured[topic].is_object(), "{topic}: {structured}");
    }
    const READS: [&str; 7] = [
        service::ENGINES,
        service::COMPONENTS,
        service::COMPONENTS_STATUS,
        service::CONFIG_SHOW,
        service::CONFIG_VALIDATE,
        service::RULES,
        service::CACHE_STATS,
    ];
    let called = host.called.lock().unwrap().clone();
    assert!(called.len() >= 6, "every topic reached the host: {called:?}");
    for name in &called {
        assert!(READS.contains(&name.as_str()), "info called `{name}`: {called:?}");
    }
}
