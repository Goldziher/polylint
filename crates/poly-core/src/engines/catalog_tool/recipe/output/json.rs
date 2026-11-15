//! The JSON reports: actionlint, buf, hadolint, golangci-lint, buildifier,
//! tflint, npm-groovy-lint, gh-actions-updater (`gau --lint-pins`), shellcheck, kubeconform, ktlint, credo, checkmake, PMD and RuboCop-compatible Ruby linters.
//!
//! Each reader pulls only the fields it needs through `serde_json::Value`, so a
//! tool adding a field never breaks it; a field it needs going missing, or the
//! document being something other than JSON, is an `Err`.

use anyhow::{Context, Result, bail};
use serde_json::Value;

use super::yaml_locate::Locus;
use super::{Finding, position};
use crate::engine::Severity;

/// The first JSON value in `text`. Some tools follow their report with a
/// plain-text summary (golangci-lint prints `7 issues:` after the document).
fn document(text: &str, tool: &str) -> Result<Value> {
    serde_json::Deserializer::from_str(text.trim())
        .into_iter::<Value>()
        .next()
        .with_context(|| format!("{tool} printed no JSON report"))?
        .with_context(|| format!("{tool} did not print a JSON report"))
}

fn text(value: &Value, key: &str) -> Option<String> {
    value.get(key).and_then(Value::as_str).map(str::to_owned)
}

fn number(value: &Value, key: &str) -> Option<u32> {
    position(value.get(key).and_then(Value::as_u64))
}

pub(super) fn actionlint(stdout: &str) -> Result<Vec<Finding>> {
    if stdout.trim().is_empty() {
        return Ok(Vec::new());
    }
    let report = document(stdout, "actionlint")?;
    let Some(items) = report.as_array() else {
        bail!("actionlint printed a JSON value that is not a list of findings");
    };
    Ok(items
        .iter()
        .map(|item| {
            let message = text(item, "message").unwrap_or_default();
            let kind = text(item, "kind");
            let severity = actionlint_severity(kind.as_deref(), &message);
            let mut finding = Finding::new(severity, message);
            finding.path = text(item, "filepath");
            finding.line = number(item, "line");
            finding.column = number(item, "column");
            finding.end_column = number(item, "end_column");
            finding.code = kind;
            finding
        })
        .collect())
}

/// actionlint has no severities — every finding fails it — so poly assigns one
/// per `kind`.
///
/// - `runner-label` is a warning: whether a label exists depends on the runners
///   the repository has registered, which a file cannot show. Self-hosted and
///   ARC labels are flagged until declared in `.github/actionlint.yaml`
///   (`self-hosted-runner.labels`), and before 0.30 this was a warning too.
/// - `shellcheck` takes the level shellcheck printed in the message
///   (`SC2086:info:…`), mapped as poly's own shellcheck backend maps it, so an
///   `info` or `style` note in a `run:` block does not fail the run.
/// - Every other kind (`syntax-check`, `expression`, `action`, `job-needs`, …)
///   is a defect in the workflow itself and stays an error.
fn actionlint_severity(kind: Option<&str>, message: &str) -> Severity {
    match kind {
        Some("runner-label") => Severity::Warning,
        Some("shellcheck") => shellcheck_level(message),
        _ => Severity::Error,
    }
}

/// The level in actionlint's `shellcheck reported issue in this script:
/// SC2086:info:1:6: …` message; `Warning` when the message has no level.
fn shellcheck_level(message: &str) -> Severity {
    let level = message
        .split_whitespace()
        .find(|word| {
            word.strip_prefix("SC")
                .is_some_and(|rest| rest.starts_with(|c: char| c.is_ascii_digit()))
        })
        .and_then(|code| code.split(':').nth(1))
        .unwrap_or_default();
    match level {
        "error" => Severity::Error,
        "info" => Severity::Info,
        "style" => Severity::Hint,
        _ => Severity::Warning,
    }
}

/// buf prints one JSON object per line.
pub(super) fn buf(stdout: &str) -> Result<Vec<Finding>> {
    stdout
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            let item = document(line, "buf")?;
            let mut finding = Finding::new(Severity::Error, text(&item, "message").unwrap_or_default());
            finding.path = text(&item, "path");
            finding.line = number(&item, "start_line");
            finding.column = number(&item, "start_column");
            finding.end_line = number(&item, "end_line");
            finding.end_column = number(&item, "end_column");
            finding.code = text(&item, "type");
            Ok(finding)
        })
        .collect()
}

pub(super) fn hadolint(stdout: &str) -> Result<Vec<Finding>> {
    if stdout.trim().is_empty() {
        return Ok(Vec::new());
    }
    let report = document(stdout, "hadolint")?;
    let Some(items) = report.as_array() else {
        bail!("hadolint printed a JSON value that is not a list of findings");
    };
    Ok(items
        .iter()
        .map(|item| {
            let severity = match text(item, "level").as_deref() {
                Some("error") => Severity::Error,
                Some("warning") => Severity::Warning,
                Some("info") => Severity::Info,
                _ => Severity::Hint,
            };
            let mut finding = Finding::new(severity, text(item, "message").unwrap_or_default());
            finding.path = text(item, "file");
            finding.line = number(item, "line");
            finding.column = number(item, "column");
            finding.code = text(item, "code");
            finding
        })
        .collect())
}

pub(super) fn golangci(stdout: &str) -> Result<Vec<Finding>> {
    let report = document(stdout, "golangci-lint")?;
    let issues = report
        .get("Issues")
        .context("golangci-lint's JSON report has no Issues field")?;
    let Some(items) = issues.as_array() else {
        return Ok(Vec::new());
    };
    Ok(items
        .iter()
        .map(|item| {
            let severity = match text(item, "Severity").as_deref() {
                Some("warning") => Severity::Warning,
                Some("info") => Severity::Info,
                _ => Severity::Error,
            };
            let mut finding = Finding::new(severity, text(item, "Text").unwrap_or_default());
            finding.code = text(item, "FromLinter");
            if let Some(pos) = item.get("Pos") {
                finding.path = text(pos, "Filename");
                finding.line = number(pos, "Line");
                finding.column = number(pos, "Column");
            }
            finding.fixable = item
                .get("SuggestedFixes")
                .and_then(Value::as_array)
                .is_some_and(|fixes| !fixes.is_empty())
                || item
                    .get("Replacement")
                    .is_some_and(|replacement| !replacement.is_null());
            finding
        })
        .collect())
}

/// buildifier's `-format=json`. A file with a syntax error is reported as
/// `valid: false` with the message on stderr (`path:line:col: message`).
pub(super) fn buildifier(stdout: &str, stderr: &str) -> Result<Vec<Finding>> {
    let report = document(stdout, "buildifier")?;
    let files = report
        .get("files")
        .and_then(Value::as_array)
        .context("buildifier's JSON report has no files list")?;
    let mut findings = Vec::new();
    for file in files {
        let path = text(file, "filename");
        if file.get("valid").and_then(Value::as_bool) == Some(false) {
            findings.push(syntax_error(path.as_deref(), stderr));
        }
        for warning in file.get("warnings").and_then(Value::as_array).into_iter().flatten() {
            let mut finding = Finding::new(Severity::Error, text(warning, "message").unwrap_or_default());
            finding.path = path.clone();
            finding.code = text(warning, "category");
            finding.url = text(warning, "url");
            finding.fixable = warning.get("autoFixable").and_then(Value::as_bool).unwrap_or(false);
            if let Some(start) = warning.get("start") {
                finding.line = number(start, "line");
                finding.column = number(start, "column");
            }
            if let Some(end) = warning.get("end") {
                finding.end_line = number(end, "line");
                finding.end_column = number(end, "column");
            }
            findings.push(finding);
        }
    }
    Ok(findings)
}

/// The `path:line:col: message` line buildifier wrote for `path`, or a generic
/// message when stderr does not carry one.
fn syntax_error(path: Option<&str>, stderr: &str) -> Finding {
    let mut finding = Finding::new(Severity::Error, "syntax error");
    finding.path = path.map(str::to_owned);
    finding.code = Some("syntax".to_owned());
    let located = path.and_then(|path| {
        stderr
            .lines()
            .find_map(|line| line.strip_prefix(path)?.strip_prefix(':'))
    });
    if let Some(rest) = located {
        let mut parts = rest.splitn(3, ':');
        finding.line = parts.next().and_then(|n| n.trim().parse().ok());
        finding.column = parts.next().and_then(|n| n.trim().parse().ok());
        if let Some(message) = parts.next() {
            finding.message = message.trim().to_owned();
        }
    }
    finding
}

pub(super) fn tflint(stdout: &str) -> Result<Vec<Finding>> {
    let report = document(stdout, "tflint")?;
    let mut findings = Vec::new();
    for issue in report.get("issues").and_then(Value::as_array).into_iter().flatten() {
        let rule = issue.get("rule");
        let severity = match rule.and_then(|rule| text(rule, "severity")).as_deref() {
            Some("warning") => Severity::Warning,
            Some("notice") => Severity::Info,
            _ => Severity::Error,
        };
        let mut finding = Finding::new(severity, text(issue, "message").unwrap_or_default());
        finding.code = rule.and_then(|rule| text(rule, "name"));
        finding.url = rule.and_then(|rule| text(rule, "link"));
        apply_range(&mut finding, issue.get("range"));
        findings.push(finding);
    }
    for error in report.get("errors").and_then(Value::as_array).into_iter().flatten() {
        let message = text(error, "message")
            .or_else(|| text(error, "summary"))
            .unwrap_or_else(|| "tflint failed".to_owned());
        let mut finding = Finding::new(Severity::Error, message);
        finding.code = Some("tflint-error".to_owned());
        apply_range(&mut finding, error.get("range"));
        findings.push(finding);
    }
    Ok(findings)
}

fn apply_range(finding: &mut Finding, range: Option<&Value>) {
    let Some(range) = range else { return };
    finding.path = text(range, "filename");
    if let Some(start) = range.get("start") {
        finding.line = number(start, "line");
        finding.column = number(start, "column");
    }
    if let Some(end) = range.get("end") {
        finding.end_line = number(end, "line");
        finding.end_column = number(end, "column");
    }
}

/// npm-groovy-lint's `--output json`: `files` maps an absolute path to its
/// `errors`, each with a 1-based line and a 0-based `range` column.
pub(super) fn groovy(stdout: &str) -> Result<Vec<Finding>> {
    let start = stdout
        .find('{')
        .context("npm-groovy-lint did not print a JSON report")?;
    let report = document(&stdout[start..], "npm-groovy-lint")?;
    let files = report
        .get("files")
        .and_then(Value::as_object)
        .context("npm-groovy-lint's JSON report has no files map")?;
    let mut findings = Vec::new();
    for (path, file) in files {
        for error in file.get("errors").and_then(Value::as_array).into_iter().flatten() {
            let severity = match text(error, "severity").as_deref() {
                Some("error") => Severity::Error,
                Some("warning") => Severity::Warning,
                _ => Severity::Info,
            };
            let mut finding = Finding::new(severity, text(error, "msg").unwrap_or_default());
            finding.path = Some(path.clone());
            finding.line = number(error, "line");
            finding.code = text(error, "rule");
            if let Some(range) = error.get("range") {
                if let Some(start) = range.get("start") {
                    finding.column = start.get("character").and_then(Value::as_u64).map(|c| c as u32 + 1);
                }
                if let Some(end) = range.get("end") {
                    finding.end_line = number(end, "line");
                    finding.end_column = end.get("character").and_then(Value::as_u64).map(|c| c as u32 + 1);
                }
            }
            findings.push(finding);
        }
    }
    Ok(findings)
}

/// gh-actions-updater's `--lint-pins --format json`: a `summary` with counts,
/// one `findings` entry per remote `uses:` ref that is not a bare `vN` major
/// tag (exit 1), and `diagnostics` for files it could not parse (exit 3).
///
/// The summary's finding count must match the list, so a report cut short is
/// an error rather than fewer findings.
pub(super) fn gau(stdout: &str) -> Result<Vec<Finding>> {
    let report = document(stdout, "gau")?;
    let expected = report
        .get("summary")
        .and_then(|summary| summary.get("findings"))
        .and_then(Value::as_u64)
        .context("gau's JSON report has no summary.findings count")?;
    let items = report
        .get("findings")
        .and_then(Value::as_array)
        .context("gau's JSON report has no findings list")?;
    if usize::try_from(expected).ok() != Some(items.len()) {
        bail!(
            "gau's summary counts {expected} finding(s) but its report lists {}",
            items.len()
        );
    }
    let mut findings: Vec<Finding> = items
        .iter()
        .map(|item| {
            let uses = text(item, "uses").unwrap_or_default();
            let reason = text(item, "reason").unwrap_or_default();
            let mut finding = Finding::new(Severity::Error, format!("{uses}: {reason}"));
            finding.path = text(item, "file");
            finding.line = number(item, "line");
            finding.code = text(item, "kind");
            finding
        })
        .collect();
    for diagnostic in report
        .get("diagnostics")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let mut finding = Finding::new(Severity::Error, text(diagnostic, "message").unwrap_or_default());
        finding.path = text(diagnostic, "file");
        finding.line = number(diagnostic, "line");
        finding.code = text(diagnostic, "code");
        findings.push(finding);
    }
    Ok(findings)
}

/// shellcheck's `--format=json1`: `{"comments": [...]}`, each comment with a
/// 1-based line and column, an end position, a numeric `code` and a `fix` when
/// shellcheck can suggest one. Levels map as poly's own shellcheck backend does:
/// `style` is a hint, so a style note does not fail the run.
pub(super) fn shellcheck(stdout: &str) -> Result<Vec<Finding>> {
    let report = document(stdout, "shellcheck")?;
    let comments = report
        .get("comments")
        .and_then(Value::as_array)
        .context("shellcheck's JSON report has no comments list")?;
    Ok(comments
        .iter()
        .map(|item| {
            let severity = match text(item, "level").as_deref() {
                Some("error") => Severity::Error,
                Some("info") => Severity::Info,
                Some("style") => Severity::Hint,
                _ => Severity::Warning,
            };
            let mut finding = Finding::new(severity, text(item, "message").unwrap_or_default());
            finding.path = text(item, "file");
            finding.line = number(item, "line");
            finding.column = number(item, "column");
            finding.end_line = number(item, "endLine");
            finding.end_column = number(item, "endColumn");
            if let Some(code) = item.get("code").and_then(Value::as_u64) {
                finding.code = Some(format!("SC{code}"));
                finding.url = Some(format!("https://www.shellcheck.net/wiki/SC{code}"));
            }
            finding.fixable = item.get("fix").is_some_and(|fix| !fix.is_null());
            finding
        })
        .collect())
}

/// kubeconform's `-output json`: one `resources` entry per manifest document.
/// It reports no line numbers, so each finding names the resource and the
/// JSON path of the invalid field instead. A valid, empty or skipped resource
/// is not a finding; a schema kubeconform could not find is a warning, since
/// the manifest was not checked rather than found wrong.
pub(super) fn kubeconform(stdout: &str) -> Result<Vec<Finding>> {
    let report = document(stdout, "kubeconform")?;
    let resources = report
        .get("resources")
        .and_then(Value::as_array)
        .context("kubeconform's JSON report has no resources list")?;
    let mut findings = Vec::new();
    for resource in resources {
        let status = text(resource, "status").unwrap_or_default();
        if !matches!(status.as_str(), "statusInvalid" | "statusError") {
            continue;
        }
        let subject = match (text(resource, "kind"), text(resource, "name")) {
            (Some(kind), Some(name)) if !name.is_empty() => format!("{kind}/{name}"),
            (Some(kind), _) => kind,
            _ => "resource".to_owned(),
        };
        let message = text(resource, "msg").unwrap_or_default();
        let severity = if message.contains("could not find schema") {
            Severity::Warning
        } else {
            Severity::Error
        };
        let code = text(resource, "kind");
        let details: Vec<&Value> = resource
            .get("validationErrors")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .collect();
        let mut push = |detail: String, pointer: &str| {
            let mut finding = Finding::new(severity, format!("{subject}: {detail}"));
            finding.path = text(resource, "filename");
            finding.code = code.clone();
            finding.locus = locus(resource, pointer);
            findings.push(finding);
        };
        if details.is_empty() {
            push(message, "");
        }
        for detail in details {
            let path = text(detail, "path").unwrap_or_default();
            let reason = text(detail, "msg").unwrap_or_default();
            let shown = if path.is_empty() {
                reason
            } else {
                format!("{path}: {reason}")
            };
            push(shown, &path);
        }
    }
    Ok(findings)
}

fn locus(resource: &Value, pointer: &str) -> Option<Locus> {
    Some(Locus {
        kind: text(resource, "kind").filter(|kind| !kind.is_empty())?,
        name: text(resource, "name").unwrap_or_default(),
        pointer: pointer.to_owned(),
    })
}

/// PMD's `check -f json`: violations per file, plus `processingErrors` (a file
/// PMD could not parse, which it did not check) and `configurationErrors`.
pub(super) fn pmd(stdout: &str) -> Result<Vec<Finding>> {
    let report = document(stdout, "PMD")?;
    let files = report
        .get("files")
        .and_then(Value::as_array)
        .context("PMD's JSON report has no files list")?;
    let mut findings = Vec::new();
    for file in files {
        let path = text(file, "filename");
        for violation in file.get("violations").and_then(Value::as_array).into_iter().flatten() {
            // PMD priority 1 is the most severe, 5 the least.
            let severity = match violation.get("priority").and_then(Value::as_u64) {
                Some(1 | 2) => Severity::Error,
                Some(3) | None => Severity::Warning,
                Some(_) => Severity::Info,
            };
            let mut finding = Finding::new(severity, text(violation, "description").unwrap_or_default());
            finding.path.clone_from(&path);
            finding.line = number(violation, "beginline");
            finding.column = number(violation, "begincolumn");
            finding.end_line = number(violation, "endline");
            finding.end_column = number(violation, "endcolumn");
            finding.code = text(violation, "rule");
            finding.url = text(violation, "externalInfoUrl");
            findings.push(finding);
        }
    }
    for failure in report
        .get("processingErrors")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let message = text(failure, "message").unwrap_or_default();
        let mut finding = Finding::new(Severity::Error, message.clone());
        finding.path = text(failure, "filename");
        (finding.line, finding.column) = parse_error_position(&message);
        findings.push(finding);
    }
    for problem in report
        .get("configurationErrors")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let rule = text(problem, "rule").unwrap_or_default();
        let issue = text(problem, "issue").unwrap_or_default();
        findings.push(Finding::new(Severity::Error, format!("{rule}: {issue}")));
    }
    Ok(findings)
}

/// The `at line L, column C` a PMD parse failure names, when it does.
fn parse_error_position(message: &str) -> (Option<u32>, Option<u32>) {
    let number_after = |marker: &str| {
        let rest = message.split_once(marker)?.1;
        let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
        position(digits.parse().ok())
    };
    (number_after("at line "), number_after(", column "))
}

/// ktlint's `--reporter=json`: a list of `{file, errors}` objects, each error
/// with a 1-based line and column, a `standard:`-prefixed rule id and a message.
pub(super) fn ktlint(stdout: &str) -> Result<Vec<Finding>> {
    let report = document(stdout, "ktlint")?;
    let Some(files) = report.as_array() else {
        bail!("ktlint printed a JSON value that is not a list of files");
    };
    let mut findings = Vec::new();
    for file in files {
        let path = text(file, "file");
        for error in file.get("errors").and_then(Value::as_array).into_iter().flatten() {
            let mut finding = Finding::new(Severity::Error, text(error, "message").unwrap_or_default());
            finding.path = path.clone();
            finding.line = number(error, "line");
            finding.column = number(error, "column");
            finding.code = text(error, "rule");
            findings.push(finding);
        }
    }
    Ok(findings)
}

/// credo's `--format json`: `{"issues": [...]}`, each issue with the failed
/// `check`, a 1-based `line_no` and a `column` that is `null` when the check
/// reports a whole line. Every issue fails a `--strict` run, so every one is an
/// error.
pub(super) fn credo(stdout: &str) -> Result<Vec<Finding>> {
    let report = document(stdout, "credo")?;
    let issues = report
        .get("issues")
        .and_then(Value::as_array)
        .context("credo's JSON report has no issues list")?;
    Ok(issues
        .iter()
        .map(|issue| {
            let mut finding = Finding::new(Severity::Error, text(issue, "message").unwrap_or_default());
            finding.path = text(issue, "filename");
            finding.line = number(issue, "line_no");
            finding.column = number(issue, "column");
            finding.end_column = number(issue, "column_end");
            finding.code = text(issue, "check");
            finding
        })
        .collect())
}

/// checkmake's `--output json`: a list of `{rule, violation, file_name,
/// line_number}`, printed only when there are violations, so a clean run has an
/// empty stdout. An empty stdout next to an error on stderr is a run that did
/// not lint (an unreadable Makefile), never a clean one.
pub(super) fn checkmake(stdout: &str, stderr: &str) -> Result<Vec<Finding>> {
    if stdout.trim().is_empty() {
        if stderr.contains("Error:") && !stderr.contains("violations found") {
            bail!("checkmake failed: {}", stderr.trim());
        }
        return Ok(Vec::new());
    }
    let report = document(stdout, "checkmake")?;
    let Some(violations) = report.as_array() else {
        bail!("checkmake printed a JSON value that is not a list of violations");
    };
    Ok(violations
        .iter()
        .map(|violation| {
            let mut finding = Finding::new(Severity::Error, text(violation, "violation").unwrap_or_default());
            finding.path = text(violation, "file_name");
            finding.line = number(violation, "line_number");
            finding.code = text(violation, "rule");
            finding
        })
        .collect())
}

/// RuboCop's `--format json` (as printed by rbcop too): `{"files": [{"path",
/// "offenses": [...]}]}`. An offense's `message` repeats the cop name in front,
/// which the finding carries as its code. `fatal`, `error` and `warning`
/// offenses are errors; `convention` and `refactor` (style) are warnings.
pub(super) fn rubocop(stdout: &str) -> Result<Vec<Finding>> {
    let report = document(stdout, "rubocop")?;
    let files = report
        .get("files")
        .and_then(Value::as_array)
        .context("the RuboCop JSON report has no files list")?;
    let mut findings = Vec::new();
    for file in files {
        let path = text(file, "path");
        for offense in file.get("offenses").and_then(Value::as_array).into_iter().flatten() {
            let severity = match offense.get("severity").and_then(Value::as_str) {
                Some("fatal" | "error" | "warning") => Severity::Error,
                Some("info") => Severity::Info,
                _ => Severity::Warning,
            };
            let code = text(offense, "cop_name");
            let message = text(offense, "message").unwrap_or_default();
            let message = code
                .as_deref()
                .and_then(|cop| message.strip_prefix(cop))
                .and_then(|rest| rest.strip_prefix(": "))
                .map_or_else(|| message.clone(), str::to_owned);
            let mut finding = Finding::new(severity, message);
            finding.path = path.clone();
            finding.code = code;
            finding.fixable = offense.get("correctable").and_then(Value::as_bool).unwrap_or(false);
            if let Some(location) = offense.get("location") {
                finding.line = number(location, "start_line");
                finding.column = number(location, "start_column");
                finding.end_line = number(location, "last_line");
                finding.end_column = number(location, "last_column");
            }
            findings.push(finding);
        }
    }
    Ok(findings)
}
