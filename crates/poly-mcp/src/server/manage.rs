//! `manage`: result-cache and component maintenance.
//!
//! A component install that has to download is a Task when the client supports them; the status
//! message names the components. There is no byte-level progress: host services are one blocking
//! call, so the host has no way to report mid-call.

use poly_frontend_api::dto::ComponentActions;
use poly_frontend_api::wire::{CacheGcRequest, CacheRequest, ComponentsInstallRequest, ComponentsUninstallRequest};
use rmcp::model::{CallToolResponse, CallToolResult, JsonObject};
use rmcp::service::RequestContext;
use rmcp::{ErrorData, RoleServer};

use super::arguments::{MANAGE_ACTIONS, applicable};
use super::params::{Action, ManageParams};
use super::{PolyMcpServer, effective_config, run_blocking};
use crate::dto::{TextRepr, actions_result, dto_result, error_result, usage_result};

impl PolyMcpServer {
    /// Answer `manage`.
    pub(super) async fn run_manage(
        &self,
        args: ManageParams,
        raw: &JsonObject,
        context: &RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        let repr = args.format;
        // A destructive action must not run under a parameter it ignores: `dry_run` in
        // particular reads as "change nothing".
        let action = raw
            .get("action")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default();
        if let Err(message) = applicable(raw, "action", &[action], MANAGE_ACTIONS, &["format"]) {
            return Ok(CallToolResponse::Complete(usage_result(&message, repr)));
        }
        let backend = self.backend.clone();
        let config = effective_config(args.config, &self.config_override);
        let result = match args.action {
            Action::CacheClean => {
                let request = CacheRequest { config };
                match run_blocking(move || Ok(backend.cache_clean(&request))).await? {
                    Ok(report) => dto_result(&report, repr)?,
                    Err(error) => error_result(&error, repr),
                }
            }
            Action::CacheGc => {
                let request = CacheGcRequest {
                    config,
                    max_age_days: args.max_age_days,
                    max_size: args.max_size,
                };
                match run_blocking(move || Ok(backend.cache_gc(&request))).await? {
                    Ok(report) => dto_result(&report, repr)?,
                    Err(error) => error_result(&error, repr),
                }
            }
            Action::ComponentsUninstall | Action::ComponentsPrune => {
                let request = ComponentsUninstallRequest {
                    names: args.names,
                    stale: args.action == Action::ComponentsPrune,
                };
                match run_blocking(move || Ok(backend.components_uninstall(&request))).await? {
                    Ok(report) => actions_result(&report, repr)?,
                    Err(error) => error_result(&error, repr),
                }
            }
            Action::ComponentsInstall => {
                let install = ComponentsInstallRequest {
                    names: args.names,
                    languages: args.languages,
                    paths: args.paths,
                    config,
                    dry_run: args.dry_run,
                };
                return self.install_components(install, repr, context).await;
            }
        };
        Ok(CallToolResponse::Complete(result))
    }

    /// `components_install`: a Task when the client supports them and a dry run finds something
    /// to download, otherwise a completed result.
    async fn install_components(
        &self,
        mut install: ComponentsInstallRequest,
        repr: TextRepr,
        context: &RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        let backend = self.backend.clone();
        let complete = move |reply: anyhow::Result<ComponentActions>| -> Result<CallToolResult, ErrorData> {
            match reply {
                Ok(reply) => actions_result(&reply, repr),
                Err(error) => Ok(error_result(&error, repr)),
            }
        };
        if install.dry_run || !super::supports_tasks(context) {
            let reply = run_blocking(move || Ok(backend.components_install(&install))).await?;
            return complete(reply).map(CallToolResponse::Complete);
        }

        // The plan decides whether this is a Task. With nothing to download it is the answer:
        // an install would report the same rows.
        install.dry_run = true;
        let plan_request = install.clone();
        let plan_backend = backend.clone();
        let plan = match run_blocking(move || Ok(plan_backend.components_install(&plan_request))).await? {
            Ok(plan) => plan,
            Err(error) => return Ok(CallToolResponse::Complete(error_result(&error, repr))),
        };
        let downloads: Vec<&str> = plan
            .results
            .iter()
            .filter(|row| row.action == "would_install")
            .map(|row| row.name.as_str())
            .collect();
        if downloads.is_empty() {
            return Ok(CallToolResponse::Complete(actions_result(&plan, repr)?));
        }
        let status = format!("downloading components: {}", downloads.join(", "));
        install.dry_run = false;
        Ok(self.spawn_task(status, move || complete(backend.components_install(&install))))
    }
}
