//! SARIF 2.1.0 logs, the report format zizmor, detekt, jarl and SwiftLint share.
//!
//! Only the fields a diagnostic needs are read: the rule, level and message of
//! each result and the first usable location's file and region. A log without a
//! `runs` list is not a SARIF document, so it is an `Err` rather than a clean run.

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
            findings.push(finding(result, &help, run));
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

fn finding(result: &Value, help: &BTreeMap<String, String>, run: &Value) -> Finding {
    let severity = match result.get("level").and_then(Value::as_str) {
        Some("error") => Severity::Error,
        Some("note") => Severity::Info,
        Some("none") => Severity::Hint,
        _ => Severity::Warning,
    };
    let message = ["/message/text", "/message/markdown"]
        .iter()
        .filter_map(|pointer| result.pointer(pointer).and_then(Value::as_str))
        .find(|text| !text.is_empty())
        .unwrap_or_default();
    let mut finding = Finding::new(severity, message);
    let rule = result.get("ruleId").and_then(Value::as_str);
    finding.code = rule.map(str::to_owned);
    finding.url = rule.and_then(|rule| help.get(rule)).cloned();
    finding.fixable = result
        .get("fixes")
        .and_then(Value::as_array)
        .is_some_and(|fixes| !fixes.is_empty());
    let located = result
        .get("locations")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|location| location.get("physicalLocation"))
        .find_map(|physical| Some((physical, artifact_path(physical, run)?)));
    if let Some((location, path)) = located {
        finding.path = Some(path);
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

/// The path a physical location names, with its `uriBaseId` applied.
fn artifact_path(physical: &Value, run: &Value) -> Option<String> {
    let artifact = physical.get("artifactLocation")?;
    let uri = artifact
        .get("uri")
        .and_then(Value::as_str)
        .filter(|uri| !uri.is_empty())?;
    let path = file_path(uri);
    if is_absolute(&path) {
        return Some(path);
    }
    let base = artifact
        .get("uriBaseId")
        .and_then(Value::as_str)
        .and_then(|id| base_path(run, id, 0));
    Some(match base {
        Some(base) => format!("{}/{path}", base.trim_end_matches('/')),
        None => path,
    })
}

/// The directory `originalUriBaseIds` binds `id` to. A base may itself be
/// relative to another; the chain is bounded so a cycle cannot loop.
fn base_path(run: &Value, id: &str, depth: usize) -> Option<String> {
    if depth > 4 {
        return None;
    }
    let entry = run.get("originalUriBaseIds")?.get(id)?;
    let path = file_path(entry.get("uri").and_then(Value::as_str)?);
    if is_absolute(&path) {
        return Some(path);
    }
    match entry.get("uriBaseId").and_then(Value::as_str) {
        Some(parent) => {
            let parent = base_path(run, parent, depth + 1)?;
            Some(format!("{}/{path}", parent.trim_end_matches('/')))
        }
        None => Some(path),
    }
}

fn is_absolute(path: &str) -> bool {
    path.starts_with('/') || drive_prefix(path)
}

fn drive_prefix(path: &str) -> bool {
    let bytes = path.as_bytes();
    bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':'
}

/// A SARIF URI reference as a filesystem path: `file:` scheme removed,
/// percent-escapes decoded, and `/C:/x` (the form of `file:///C:/x`) turned
/// into `C:/x` with forward slashes.
fn file_path(uri: &str) -> String {
    let rest = uri
        .strip_prefix("file://")
        .or_else(|| uri.strip_prefix("file:"))
        .unwrap_or(uri);
    let decoded = percent_decode(rest);
    let bytes = decoded.as_bytes();
    if bytes.len() >= 3 && bytes[0] == b'/' && bytes[1].is_ascii_alphabetic() && bytes[2] == b':' {
        return decoded[1..].replace('\\', "/");
    }
    if drive_prefix(&decoded) {
        return decoded.replace('\\', "/");
    }
    decoded
}

/// Decode `%XX` escapes as UTF-8. A malformed escape stays as written, and bytes
/// that do not form UTF-8 leave the whole string undecoded.
fn percent_decode(raw: &str) -> String {
    if !raw.contains('%') {
        return raw.to_owned();
    }
    let bytes = raw.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let escaped = (bytes[i] == b'%' && i + 2 < bytes.len())
            .then(|| hex_pair(bytes[i + 1], bytes[i + 2]))
            .flatten();
        match escaped {
            Some(byte) => {
                out.push(byte);
                i += 3;
            }
            None => {
                out.push(bytes[i]);
                i += 1;
            }
        }
    }
    String::from_utf8(out).unwrap_or_else(|_| raw.to_owned())
}

fn hex_pair(high: u8, low: u8) -> Option<u8> {
    let digit = |byte: u8| char::from(byte).to_digit(16);
    u8::try_from(digit(high)? * 16 + digit(low)?).ok()
}
