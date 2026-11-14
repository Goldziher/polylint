//! SARIF 2.1.0 logs, the report format zizmor and detekt share.
//!
//! Only the fields a diagnostic needs are read: the rule, level and message of
//! each result and the first location's file and region. A log without a `runs`
//! list is not a SARIF document, so it is an `Err` rather than a clean run.

use std::collections::BTreeMap;

use anyhow::{Context, Result};
use serde_json::Value;

use super::{Finding, position};
use crate::engine::Severity;

pub(super) fn parse(stdout: &str) -> Result<Vec<Finding>> {
    let log: Value = serde_json::from_str(stdout.trim()).context("the tool did not print a SARIF log")?;
    let runs = log
        .get("runs")
        .and_then(Value::as_array)
        .context("the SARIF log has no runs list")?;
    let mut findings = Vec::new();
    for run in runs {
        let help = help_uris(run);
        for result in run.get("results").and_then(Value::as_array).into_iter().flatten() {
            findings.push(finding(result, &help));
        }
    }
    Ok(findings)
}

/// `ruleId` to the rule's `helpUri`, from the run's `tool.driver.rules`.
fn help_uris(run: &Value) -> BTreeMap<String, String> {
    run.pointer("/tool/driver/rules")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|rule| {
            let id = rule.get("id")?.as_str()?;
            let uri = rule.get("helpUri")?.as_str()?;
            Some((id.to_owned(), uri.to_owned()))
        })
        .collect()
}

fn finding(result: &Value, help: &BTreeMap<String, String>) -> Finding {
    let severity = match result.get("level").and_then(Value::as_str) {
        Some("error") => Severity::Error,
        Some("note") => Severity::Info,
        Some("none") => Severity::Hint,
        _ => Severity::Warning,
    };
    let message = result
        .pointer("/message/text")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let mut finding = Finding::new(severity, message);
    let rule = result.get("ruleId").and_then(Value::as_str);
    finding.code = rule.map(str::to_owned);
    finding.url = rule.and_then(|rule| help.get(rule)).cloned();
    if let Some(location) = result.pointer("/locations/0/physicalLocation") {
        finding.path = location
            .pointer("/artifactLocation/uri")
            .and_then(Value::as_str)
            .map(|uri| uri.strip_prefix("file://").unwrap_or(uri).to_owned());
        if let Some(region) = location.get("region") {
            let number = |key: &str| position(region.get(key).and_then(Value::as_u64));
            finding.line = number("startLine");
            finding.column = number("startColumn");
            finding.end_line = number("endLine");
            finding.end_column = number("endColumn");
        }
    }
    finding
}
