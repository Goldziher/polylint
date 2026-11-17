//! The MCP server: a [`PolyMcpServer`] exposing poly over stdio as four tools.
//!
//! ## Tools
//!
//! Annotations are per tool, and clients grant permissions per tool name, so the tools are
//! split along the annotation boundary and everything inside one class is a parameter (ADR
//! 0076):
//!
//! - `check` — a lint or format run that writes nothing, the whole-project phase in check mode,
//!   or both. Not read-only: it runs the tools the repository's `poly.toml` configures, so a
//!   client auto-approving read-only tools would otherwise execute a cloned repository's
//!   programs. Open-world: a run may download a missing engine component.
//! - `fix` — the same runs with fixes applied. Destructive.
//! - `info` — version, effective config, config validation, rules, components and cache state.
//!   Read-only and closed-world: it never downloads and never runs repository tools.
//! - `manage` — result-cache and component maintenance. Destructive and open-world.
//!
//! ## Output
//!
//! Every tool returns rmcp **structured content** plus one text block carrying the same value
//! as JSON or compact **TOON** (`format`). `check` and `fix` return the CLI's `--format json`
//! document; a run that could not start, or a requested phase that did not run, returns
//! `{error: {kind, message}, summary: null}` with `isError`. So do arguments a tool cannot act
//! on (`kind: usage`): an unknown key, a wrong value, a parameter that does not apply to the
//! chosen `scope`, `action` or `topics`. Only an unknown tool name is a JSON-RPC error. The
//! schemas are resources (see [`resources`]), not tool `outputSchema`s.
//!
//! ## Identity
//!
//! Every result carries the serving binary's identity (see [`crate::identity`]) under a `poly`
//! key in `structured_content` and in `_meta`. The server fingerprints its own executable at
//! startup and re-checks it per request: if the binary is replaced or deleted underneath a
//! long-lived server, every call but `info {topics: ["version"]}` fails rather than answering
//! with superseded behaviour.
//!
//! ## Tasks
//!
//! `check`/`fix` with `scope: workspace` or `all`, a `files` run that must download a
//! component, and a `components_install` that must download are SEP-2663 **async Tasks** when
//! the client declared the tasks capability (via [`rmcp::task_manager`]); otherwise the call
//! blocks and returns the result.

use std::path::PathBuf;
use std::sync::Arc;

use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, CancelTaskParams, GetTaskParams, GetTaskResult,
    Implementation, JsonObject, ListResourcesResult, ListToolsResult, PaginatedRequestParams,
    ReadResourceRequestParams, ReadResourceResponse, ReadResourceResult, ResultType, ServerCapabilities, ServerConfig,
    Tool, UpdateTaskParams,
};
use rmcp::service::RequestContext;
use rmcp::task_manager::{TaskExit, TaskManager, TaskOptions};
use rmcp::{ErrorData, RoleServer, ServerHandler};

use poly_frontend_api::ServiceCaller;

mod arguments;
mod info;
mod manage;
mod params;
pub mod resources;
mod run;

use crate::backend::Backend;
use crate::dto::usage_result;
use crate::identity::{self, ExecutableWatch};
use params::{CHECK, FIX, INFO, InfoParams, ManageParams, RunParams, Topic};

/// What every tool's result means, said once instead of in every description.
const INSTRUCTIONS: &str = "poly lints and formats every language with one config. Tools: `check` (no \
    writes), `fix` (writes), `info` (read-only state), `manage` (cache and components). `check`/`fix` run the \
    tools the repository's poly.toml configures: use them only on repositories you trust. Results are the \
    CLI's `--format json` document plus a `poly` block naming the binary that answered. `isError` means the \
    run did not verify what was asked: a file failed, a requested phase did not run, or nothing ran at all, \
    which is `{error: {kind, message}, summary: null}` (invalid or inapplicable arguments are `kind: usage`). \
    Findings and drift are not errors. Gate on `summary.checked`, not on an empty `results`: a clean file has \
    no record. Whole-project scopes and component downloads return a Task when the client supports them; poll \
    tasks/get. Output schemas: resources under poly://schema/.";

/// MCP server exposing poly's checks, fixes, state and maintenance as four tools.
#[derive(Clone)]
pub struct PolyMcpServer {
    /// Store and executor for the async Tasks.
    task_manager: TaskManager,
    /// The host's services, the only way this server reaches an engine.
    backend: Backend,
    /// Config path passed on the command line (`poly mcp --config`); used as the
    /// fallback when a request does not name its own config.
    config_override: Option<PathBuf>,
    /// Fingerprint of the executable this server started from, re-checked on
    /// every request so an upgrade underneath a running server fails loudly
    /// instead of answering with superseded behaviour.
    executable: ExecutableWatch,
}

/// Teach the process the host's engine versions and schemas. A failure leaves the defaults (an
/// empty engine digest, no host schema resources) rather than refusing to serve: the tools
/// themselves report the host's error when it matters.
fn install_host_metadata(backend: &Backend) {
    match backend.engines() {
        Ok(engines) => identity::install_host(&engines),
        Err(error) => eprintln!("poly mcp: cannot read the host's engine versions: {error:#}"),
    }
    match backend.schemas() {
        Ok(schemas) => resources::install_host_schemas(schemas),
        Err(error) => eprintln!("poly mcp: cannot read the host's output schemas: {error:#}"),
    }
}

/// Resolve the effective config path for a request: an explicit per-request
/// path wins, otherwise the server-wide override (if any).
fn effective_config(request: Option<String>, server: &Option<PathBuf>) -> Option<String> {
    request.or_else(|| server.as_ref().map(|p| p.display().to_string()))
}

/// Whether the client declared the tasks capability.
fn supports_tasks(context: &RequestContext<RoleServer>) -> bool {
    context.client_capabilities().is_some_and(|caps| caps.supports_tasks())
}

/// Run a synchronous operation on a blocking task. Only a panic is an MCP error here: what the
/// operation reports is the caller's to turn into a result.
async fn run_blocking<F, T>(operation: F) -> Result<T, ErrorData>
where
    F: FnOnce() -> Result<T, ErrorData> + Send + 'static,
    T: Send + 'static,
{
    match tokio::task::spawn_blocking(operation).await {
        Ok(result) => result,
        Err(join_error) => Err(ErrorData::internal_error(
            format!("engine task panicked: {join_error}"),
            None,
        )),
    }
}

/// A usage error as the tool's answer: an `isError` result the model reads and corrects, where
/// a JSON-RPC error would reach most clients as a transport failure.
fn usage(message: &str, arguments: &JsonObject) -> CallToolResponse {
    CallToolResponse::Complete(usage_result(message, arguments::text_repr(arguments)))
}

impl PolyMcpServer {
    /// Build a server over the host's services, optionally pinning a config file used for every
    /// request that does not name its own.
    pub fn new(caller: Arc<dyn ServiceCaller>, config_override: Option<PathBuf>) -> Self {
        Self::with_executable_watch(caller, config_override, ExecutableWatch::capture())
    }

    /// Build a server watching a specific executable rather than the running
    /// one.
    ///
    /// [`new`](Self::new) is the normal entry point. This takes the watch as an
    /// explicit dependency for an embedder that knows which file backs the
    /// server — a supervisor watching the binary it launched, or a harness
    /// exercising the replaced-underneath-a-running-server path.
    pub fn with_executable_watch(
        caller: Arc<dyn ServiceCaller>,
        config_override: Option<PathBuf>,
        executable: ExecutableWatch,
    ) -> Self {
        let backend = Backend::new(caller);
        install_host_metadata(&backend);
        Self {
            backend,
            task_manager: TaskManager::new(),
            config_override,
            executable,
        }
    }

    /// The four tool definitions, in listing order.
    pub fn tools(&self) -> Vec<Tool> {
        params::definitions().to_vec()
    }

    /// Names of every registered tool.
    pub fn tool_names(&self) -> Vec<String> {
        self.tools().into_iter().map(|tool| tool.name.to_string()).collect()
    }

    /// `(read_only_hint, destructive_hint)` for a named tool, if registered.
    pub fn tool_hints(&self, name: &str) -> Option<(Option<bool>, Option<bool>)> {
        self.get_tool(name).map(|tool| {
            let annotations = tool.annotations.as_ref();
            (
                annotations.and_then(|a| a.read_only_hint),
                annotations.and_then(|a| a.destructive_hint),
            )
        })
    }

    /// Run `work` as an async Task with an unlimited TTL — the whole-project phase can
    /// legitimately run for minutes, past the default TTL that would otherwise mark it failed.
    fn spawn_task<F>(&self, status: String, work: F) -> CallToolResponse
    where
        F: FnOnce() -> Result<CallToolResult, ErrorData> + Send + 'static,
    {
        let task = self.task_manager.spawn(
            TaskOptions::new().with_ttl_ms(None).with_status_message(status),
            move |_ctx| {
                Box::pin(async move {
                    tokio::task::spawn_blocking(work)
                        .await
                        .map_err(|error| {
                            TaskExit::Error(ErrorData::internal_error(format!("task panicked: {error}"), None))
                        })?
                        .map_err(TaskExit::Error)
                })
            },
        );
        CallToolResponse::Task(rmcp::model::CreateTaskResult::new(task))
    }

    /// Refuse the request when the executable this server runs from is no longer
    /// the file on disk.
    ///
    /// An MCP server is long-lived **by design** and has no expiry: on macOS the
    /// kernel keeps a deleted inode alive for as long as a process holds it, so
    /// a server started before an upgrade keeps serving the pre-upgrade build
    /// indefinitely — answering, plausibly, with behaviour that was fixed hours
    /// ago. Erroring is the only outcome a caller cannot mistake for a result.
    fn reject_if_stale(&self) -> Result<(), ErrorData> {
        let Some(staleness) = self.executable.check() else {
            return Ok(());
        };
        let message = self.executable.stale_message(staleness);
        // Also surface it on stderr, once, for whoever is reading the server's
        // log rather than the tool result.
        static ANNOUNCED: std::sync::Once = std::sync::Once::new();
        ANNOUNCED.call_once(|| eprintln!("poly mcp: {message}"));
        Err(ErrorData::internal_error(message, None))
    }
}

impl ServerHandler for PolyMcpServer {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(
            ServerCapabilities::builder()
                .enable_tools()
                .enable_tasks()
                .enable_resources()
                .build(),
        )
        .with_server_info(Implementation::new("poly-mcp", env!("CARGO_PKG_VERSION")))
        .with_instructions(INSTRUCTIONS)
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        // Only a tool that does not exist is a protocol error. Everything wrong with the
        // arguments of one that does is a `usage` result (see [`arguments`]).
        let Some(tool) = params::definitions().iter().find(|tool| tool.name == request.name) else {
            return Err(ErrorData::invalid_params(
                format!("unknown tool `{}`", request.name),
                None,
            ));
        };
        let raw = request.arguments.unwrap_or_default();
        match tool.name.as_ref() {
            INFO => {
                let args: InfoParams = match arguments::parse(tool, &raw) {
                    Ok(args) => args,
                    Err(message) => return Ok(usage(&message, &raw)),
                };
                // `version` is exempt: it is the topic that *explains* a stale server, and
                // refusing it would leave a caller with an error and no diagnosis.
                if args.topics.iter().any(|topic| *topic != Topic::Version) {
                    self.reject_if_stale()?;
                }
                self.run_info(args, &raw).await.map(CallToolResponse::Complete)
            }
            CHECK | FIX => {
                self.reject_if_stale()?;
                match arguments::parse::<RunParams>(tool, &raw) {
                    Ok(args) => self.run_tool(args, &raw, &context, tool.name == FIX).await,
                    Err(message) => Ok(usage(&message, &raw)),
                }
            }
            _ => {
                self.reject_if_stale()?;
                match arguments::parse::<ManageParams>(tool, &raw) {
                    Ok(args) => self.run_manage(args, &raw, &context).await,
                    Err(message) => Ok(usage(&message, &raw)),
                }
            }
        }
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        Ok(ListToolsResult {
            result_type: Some(ResultType::COMPLETE),
            tools: self.tools(),
            meta: None,
            next_cursor: None,
            ttl_ms: None,
            cache_scope: None,
        })
    }

    fn get_tool(&self, name: &str) -> Option<Tool> {
        self.tools().into_iter().find(|tool| tool.name == name)
    }

    async fn list_resources(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListResourcesResult, ErrorData> {
        Ok(ListResourcesResult {
            result_type: Some(ResultType::COMPLETE),
            resources: resources::list(),
            meta: None,
            next_cursor: None,
            ttl_ms: None,
            cache_scope: None,
        })
    }

    async fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResponse, ErrorData> {
        match resources::read(&request.uri) {
            Some(contents) => Ok(ReadResourceResult::new(vec![contents]).into()),
            None => Err(ErrorData::resource_not_found(
                format!("no resource `{}`", request.uri),
                None,
            )),
        }
    }

    async fn get_task(
        &self,
        request: GetTaskParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<GetTaskResult, ErrorData> {
        self.task_manager.get_task(&request.task_id).map(GetTaskResult::new)
    }

    async fn update_task(
        &self,
        request: UpdateTaskParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<(), ErrorData> {
        self.task_manager.update_task(&request.task_id, request.input_responses)
    }

    async fn cancel_task(
        &self,
        request: CancelTaskParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<(), ErrorData> {
        self.task_manager.cancel_task(&request.task_id)
    }
}
