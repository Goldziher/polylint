//! `info`: poly's state, one result key per requested topic.

use poly_frontend_api::dto::FailureKind;
use poly_frontend_api::wire::{CacheRequest, ComponentsStatusRequest, ConfigRequest, RulesRequest, ValidateRequest};
use rmcp::ErrorData;
use rmcp::model::{CallToolResult, JsonObject};
use serde_json::{Map, Value};

use super::arguments::{INFO_TOPICS, applicable};
use super::params::{InfoParams, Topic};
use super::{PolyMcpServer, effective_config, run_blocking};
use crate::backend::Backend;
use crate::dto::{VersionReport, dto_result_with_status, failure_value, failure_value_of, usage_result};

impl PolyMcpServer {
    /// Answer `info`. A topic the host cannot answer carries `{error: {kind, message}}` in its
    /// key and marks the result `isError`, as does a failing rule test; the other topics are
    /// still answered. On a stale server every topic but `version` carries a `stale` error, so
    /// the topic that explains the refusal answers whatever else was asked.
    pub(super) async fn run_info(&self, args: InfoParams, raw: &JsonObject) -> Result<CallToolResult, ErrorData> {
        let mut topics: Vec<Topic> = Vec::new();
        for topic in &args.topics {
            if !topics.contains(topic) {
                topics.push(*topic);
            }
        }
        let names: Vec<&str> = topics.iter().map(|topic| topic.name()).collect();
        if names.is_empty() {
            // An empty answer would read as "nothing to report" rather than "nothing asked".
            let known: Vec<&str> = INFO_TOPICS.iter().map(|(name, _)| *name).collect();
            let message = format!("`topics` is empty; name at least one of: {}", known.join(", "));
            return Ok(usage_result(&message, args.format));
        }
        if let Err(message) = applicable(raw, "topics", &names, INFO_TOPICS, &["format"]) {
            return Ok(usage_result(&message, args.format));
        }
        let config = effective_config(args.config.clone(), &self.config_override);
        let stale = self.stale();
        let mut answers = Map::new();
        let mut is_error = false;
        for topic in topics {
            if let (Some(message), false) = (&stale, topic == Topic::Version) {
                is_error = true;
                answers.insert(topic.name().to_owned(), failure_value_of(FailureKind::Stale, message));
                continue;
            }
            let answer = match topic {
                Topic::Version => Ok(self.version_report().await?),
                Topic::Config => self.host(config_topic(config.clone())).await?,
                Topic::Validate => self.host(validate_topic(args.paths.clone())).await?,
                Topic::Rules => {
                    self.host(rules_topic(RulesRequest {
                        dirs: args.dirs.clone(),
                        config: config.clone(),
                        test: args.test,
                    }))
                    .await?
                }
                Topic::Components => {
                    self.host(components_topic(ComponentsStatusRequest {
                        paths: args.paths.clone(),
                        config: config.clone(),
                        install: args.install,
                    }))
                    .await?
                }
                Topic::Cache => self.host(cache_topic(CacheRequest { config: config.clone() })).await?,
            };
            let answer = match answer {
                Ok(value) => {
                    // A failing rule test, or an invalid config, is the CLI's non-zero exit: the
                    // caller asked whether they pass.
                    is_error |=
                        value.get("ok") == Some(&Value::Bool(false)) || value.get("valid") == Some(&Value::Bool(false));
                    value
                }
                Err(error) => {
                    is_error = true;
                    failure_value(&error)
                }
            };
            answers.insert(topic.name().to_owned(), answer);
        }
        dto_result_with_status(&answers, args.format, is_error)
    }

    /// Run one host lookup on a blocking task. The outer error is a panic; the inner one is the
    /// host's answer, reported in the topic's key.
    async fn host<F>(&self, lookup: F) -> Result<anyhow::Result<Value>, ErrorData>
    where
        F: FnOnce(&Backend) -> anyhow::Result<Value> + Send + 'static,
    {
        let backend = self.backend.clone();
        run_blocking(move || Ok(lookup(&backend))).await
    }

    /// The `version` topic. The host lookups may fail exactly when this is needed to explain
    /// why; their failure is reported in the status instead of failing the topic.
    async fn version_report(&self) -> Result<Value, ErrorData> {
        let staleness = self.executable.check();
        let backend = self.backend.clone();
        let (engines, components, host_error) = run_blocking(move || {
            Ok(match (backend.engines(), backend.components()) {
                (Ok(engines), Ok(components)) => (engines.engines, components, None),
                (Err(error), _) | (_, Err(error)) => (Default::default(), Vec::new(), Some(format!("{error:#}"))),
            })
        })
        .await?;
        let executable_current = staleness.is_none();
        let mut executable_status = match staleness {
            Some(staleness) => self.executable.stale_message(staleness),
            None => "the executable serving this session is still the file on disk".to_string(),
        };
        if let Some(error) = host_error {
            executable_status.push_str(&format!("; the host could not report engines and components: {error}"));
        }
        serde_json::to_value(VersionReport {
            executable_current,
            executable_status,
            uptime_seconds: self.executable.uptime_seconds(),
            engines,
            components,
        })
        .map_err(|error| ErrorData::internal_error(format!("failed to serialize tool output: {error}"), None))
    }
}

fn to_value(value: impl serde::Serialize) -> anyhow::Result<Value> {
    Ok(serde_json::to_value(value)?)
}

fn config_topic(config: Option<String>) -> impl FnOnce(&Backend) -> anyhow::Result<Value> + Send + 'static {
    move |backend| backend.config_show(&ConfigRequest { config })
}

fn cache_topic(request: CacheRequest) -> impl FnOnce(&Backend) -> anyhow::Result<Value> + Send + 'static {
    move |backend| to_value(backend.cache_stats(&request)?)
}

fn validate_topic(paths: Vec<String>) -> impl FnOnce(&Backend) -> anyhow::Result<Value> + Send + 'static {
    move |backend| to_value(backend.config_validate(&ValidateRequest { paths })?)
}

fn rules_topic(request: RulesRequest) -> impl FnOnce(&Backend) -> anyhow::Result<Value> + Send + 'static {
    move |backend| to_value(backend.rules(&request)?)
}

fn components_topic(
    request: ComponentsStatusRequest,
) -> impl FnOnce(&Backend) -> anyhow::Result<Value> + Send + 'static {
    move |backend| to_value(backend.components_status(&request)?)
}
