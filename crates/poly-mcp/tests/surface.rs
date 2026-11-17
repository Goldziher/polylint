//! The tool surface a model pays for: a snapshot of `tools/list` and byte budgets for it.
//!
//! Every listed tool's name, description, input schema and annotations are sent to the model
//! on every session, and the instructions with them. The snapshot makes a change to any of them
//! a reviewed diff; the budgets fail a change that makes them grow past what was measured when
//! the surface was consolidated (ADR 0076).

mod common;

use rmcp::ServiceExt;
use rmcp::model::{ClientCapabilities, ClientConfig};
use serde_json::{Value, json};

/// Model-visible bytes of `tools/list` (names, descriptions, input schemas, annotations).
const MODEL_VISIBLE_BUDGET: usize = 7_700;
/// Bytes of the whole `tools/list` result.
const FULL_LIST_BUDGET: usize = 7_700;
/// Bytes of the server instructions.
const INSTRUCTIONS_BUDGET: usize = 900;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn tools_list_fits_its_budget() {
    let (server_io, client_io) = tokio::io::duplex(1 << 16);
    let (server_read, server_write) = tokio::io::split(server_io);
    let (client_read, client_write) = tokio::io::split(client_io);
    let server_task = tokio::spawn(async move {
        let service = common::server(None).serve((server_read, server_write)).await.unwrap();
        service.waiting().await.unwrap();
    });
    let client = ClientConfig::new(
        ClientCapabilities::builder().enable_tasks().build(),
        rmcp::model::Implementation::new("poly-mcp-test", "0.0.0"),
    )
    .serve((client_read, client_write))
    .await
    .unwrap();

    let instructions = client
        .peer_info()
        .and_then(|info| info.instructions.clone())
        .expect("the server sends instructions");
    let tools = client.list_all_tools().await.unwrap();
    client.cancel().await.unwrap();
    let _ = server_task.await;

    let full = serde_json::to_string(&tools).unwrap();
    let projection: Vec<Value> = tools
        .iter()
        .map(|tool| {
            json!({
                "name": tool.name,
                "description": tool.description,
                "inputSchema": tool.input_schema,
                "annotations": tool.annotations,
            })
        })
        .collect();
    let model_visible = serde_json::to_string(&projection).unwrap();
    println!(
        "tools/list: model-visible {} B, full {} B; instructions {} B",
        model_visible.len(),
        full.len(),
        instructions.len()
    );

    insta::assert_json_snapshot!("tools_list", projection);
    insta::assert_snapshot!("instructions", instructions);
    assert!(
        model_visible.len() <= MODEL_VISIBLE_BUDGET,
        "model-visible tools/list is {} B, over its {MODEL_VISIBLE_BUDGET} B budget",
        model_visible.len()
    );
    assert!(
        full.len() <= FULL_LIST_BUDGET,
        "tools/list is {} B, over its {FULL_LIST_BUDGET} B budget",
        full.len()
    );
    assert!(
        instructions.len() <= INSTRUCTIONS_BUDGET,
        "instructions are {} B, over their {INSTRUCTIONS_BUDGET} B budget",
        instructions.len()
    );
    for tool in &tools {
        let schema = serde_json::to_string(&tool.input_schema).unwrap();
        assert!(
            !schema.contains("oneOf") && !schema.contains("$ref"),
            "{}'s input schema inlines its enums: {schema}",
            tool.name
        );
        assert!(
            tool.output_schema.is_none(),
            "{}: output schemas are resources",
            tool.name
        );
    }
}
