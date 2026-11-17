//! `check` and `fix`: a lint or format run, the whole-project phase, or both.
//!
//! A `files` run returns a Task only when the client declared the tasks capability and a probe
//! (`components.status`, which never downloads) finds components the run would download;
//! otherwise it blocks and returns the result. The whole-project phase drives the multi-minute
//! `cargo clippy` / `cargo-deny` tools, so a `workspace` or `all` run is a Task whenever the
//! client supports them, with an unlimited TTL.

use poly_frontend_api::dto::FailureKind;
use poly_frontend_api::wire::{ComponentsStatusRequest, InstallMode, RunCall, RunReply, WorkspaceRequest};
use rmcp::model::{CallToolResponse, JsonObject};
use rmcp::service::RequestContext;
use rmcp::{ErrorData, RoleServer};

use super::arguments::{WORKSPACE_SCOPE, applicable};
use super::params::{RunKind, RunParams, Scope};
use super::{PolyMcpServer, effective_config, run_blocking, supports_tasks};
use crate::backend::Backend;
use crate::dto::{run_result, usage_result};

impl PolyMcpServer {
    /// Answer `check` (`apply` false) or `fix` (`apply` true).
    pub(super) async fn run_tool(
        &self,
        args: RunParams,
        raw: &JsonObject,
        context: &RequestContext<RoleServer>,
        apply: bool,
    ) -> Result<CallToolResponse, ErrorData> {
        let repr = args.format;
        let mut request = args.run;
        request.config = effective_config(request.config, &self.config_override);
        let tasks = supports_tasks(context);
        match (args.kind, args.scope) {
            (RunKind::Format, Scope::Workspace | Scope::All) => Ok(CallToolResponse::Complete(usage_result(
                "the whole-project phase is lint-only: use `kind: lint` or `scope: files`",
                repr,
            ))),
            (RunKind::Lint, Scope::Workspace) => {
                if let Err(message) = applicable(raw, "scope", &["workspace"], WORKSPACE_SCOPE, &["kind", "format"]) {
                    return Ok(CallToolResponse::Complete(usage_result(
                        &format!(
                            "{message}: the whole-project phase runs the repository's own tools over the whole \
                             repository; use `scope: all` to lint files as well"
                        ),
                        repr,
                    )));
                }
                let workspace = WorkspaceRequest {
                    config: request.config,
                    fix: apply,
                    jobs: request.jobs,
                    no_cache: request.no_cache,
                };
                let backend = self.backend.clone();
                let work = move || Ok(run_result(host_reply(backend.workspace_lint(&workspace)), repr));
                if !tasks {
                    return run_blocking(move || Ok(work())).await?.map(CallToolResponse::Complete);
                }
                Ok(self.spawn_task(whole_project_status(apply).to_owned(), work))
            }
            (kind, scope) => {
                let call = RunCall {
                    request,
                    apply,
                    workspace: scope == Scope::All,
                };
                let backend = self.backend.clone();
                let status = if scope == Scope::All && tasks {
                    Some(whole_project_status(apply).to_owned())
                } else if tasks {
                    self.pending_downloads(&call).await?
                } else {
                    None
                };
                let work = move || Ok(run_result(run(kind, &backend, &call), repr));
                match status {
                    Some(status) => Ok(self.spawn_task(status, work)),
                    None => run_blocking(move || Ok(work())).await?.map(CallToolResponse::Complete),
                }
            }
        }
    }

    /// The Task status naming the components a run would download, or `None` when it would
    /// download nothing. A failed probe only costs the Task: the run reports the real error.
    async fn pending_downloads(&self, call: &RunCall) -> Result<Option<String>, ErrorData> {
        if call.request.install == InstallMode::Never {
            return Ok(None);
        }
        let probe = ComponentsStatusRequest {
            paths: if call.request.paths.is_empty() {
                vec![".".to_owned()]
            } else {
                call.request.paths.clone()
            },
            config: call.request.config.clone(),
            install: call.request.install,
        };
        let backend = self.backend.clone();
        let pending = run_blocking(move || Ok(backend.components_status(&probe)))
            .await?
            .map(|status| status.would_download)
            .unwrap_or_else(|error| {
                eprintln!("poly mcp: component pre-check failed, running inline: {error:?}");
                Vec::new()
            });
        Ok((!pending.is_empty()).then(|| format!("downloading components: {}", pending.join(", "))))
    }
}

/// Run `call` on the host. A host that could not answer at all (the service is unreachable or
/// its reply does not decode) is reported like a failed run, as a pipeline failure.
fn run(kind: RunKind, backend: &Backend, call: &RunCall) -> RunReply {
    host_reply(match kind {
        RunKind::Lint => backend.lint(call),
        RunKind::Format => backend.format(call),
    })
}

/// The host's reply, or a pipeline failure when it could not answer at all.
fn host_reply(reply: anyhow::Result<RunReply>) -> RunReply {
    reply.unwrap_or_else(|error| crate::dto::failure_reply(FailureKind::Pipeline, &format!("{error:#}")))
}

fn whole_project_status(apply: bool) -> &'static str {
    if apply {
        "running the whole-project phase (fix mode)"
    } else {
        "running the whole-project phase"
    }
}
