//! `poly-mcp` — an [MCP](https://modelcontextprotocol.io) server, over stdio,
//! that mirrors the `poly` CLI's lint, format, config, rules, cache and component capabilities.
//!
//! The server exposes four tools (see [`server`]). MCP tool annotations are static and clients
//! grant permissions per tool name, so the tools are split along the annotation boundary —
//! `check` (no writes), `fix` (writes), `info` (read-only state) and `manage` (cache and
//! component maintenance) — and everything inside one class is a parameter (ADR 0076). Each
//! runs the synchronous, rayon-driven `poly-core` pipeline on a blocking task and returns the
//! **same JSON** the CLI emits under `--format json`.
//!
//! The server links no engine. Every tool is a call to a **host service** (see
//! [`backend::Backend`] and `poly_frontend_api::wire::service`): the host (the `poly` binary)
//! runs the pipeline and the server turns its reply into MCP structured content. That is what
//! lets the same server ship as a downloadable `mcp` component, reaching the host through the
//! component ABI's service table, or run in-process behind the `bundled-mcp` feature of
//! `poly-cli`, reaching `poly_host_services::Services` directly.
//!
//! [`serve`] is a synchronous entrypoint: it builds a multi-threaded tokio
//! runtime internally and runs the stdio server to completion, so callers
//! (e.g. `poly mcp`) stay synchronous.

pub mod backend;
pub mod dto;
pub mod identity;
pub mod schema;
pub mod server;

use std::path::PathBuf;
use std::sync::Arc;

use poly_frontend_api::ServiceCaller;
use rmcp::ServiceExt;

pub use backend::Backend;
pub use server::PolyMcpServer;

/// Run the MCP server over stdio until the client disconnects.
///
/// `caller` is how the server reaches the host's services. `config` optionally pins a config
/// file used for every request that does not name its own (the `poly mcp --config` passthrough).
/// Builds its own tokio runtime so the surrounding CLI can remain synchronous.
///
/// # Errors
///
/// Returns an error if the tokio runtime cannot be built or the stdio
/// transport fails to serve.
pub fn serve(caller: Arc<dyn ServiceCaller>, config: Option<PathBuf>) -> anyhow::Result<()> {
    let runtime = tokio::runtime::Builder::new_multi_thread().enable_all().build()?;
    runtime.block_on(serve_async(caller, config))
}

/// Async body of [`serve`]: wire the server to the stdio transport and wait for
/// the session to end.
async fn serve_async(caller: Arc<dyn ServiceCaller>, config: Option<PathBuf>) -> anyhow::Result<()> {
    let server = PolyMcpServer::new(caller, config);
    let service = server.serve(rmcp::transport::stdio()).await?;
    service.waiting().await?;
    Ok(())
}
