//! ADR log maintenance behind `poly adr`: lock, unlock and new (ADR 0061).
//!
//! The same operations the `adrlint` CLI's `lock`, `unlock` and `new` commands perform, over the
//! bundled adrlint library and the `[engines.adrlint]` settings. [`plan`] is a pure function of its
//! input, like [`ProjectAnalysis::project_check`](poly_engine_api::engine::ProjectAnalysis::project_check): the host
//! hands over the files as content, with paths relative to the project root, and writes the answer's
//! [`WireAdrPlan::writes`] itself. That is what lets the web component answer it as
//! `Request::Adr` and a bundled build call it in-process with the same result.
//!
//! The lint gate `adrlint lock` applies before writing (no error in the log) is the host's: it runs
//! `poly lint` over the log first, so `[per-file-ignores]` and every other suppression apply.

use std::path::Path;

use ::adrlint::adr;
use ::adrlint::config::{self as adr_config, AdrConfig, LockConfig};
use ::adrlint::discovery;
use ::adrlint::lock::{self, LockFile};
use anyhow::{Context, bail};
use poly_component_protocol::{WireAdrOp, WireAdrPlan, WireAdrWrite};
use poly_engine_api::config::EngineConfig;
use poly_engine_api::engine::ProjectFile;
use poly_engine_api::options::OptionsExt;

use super::project::DEFAULT_LOCK;

/// The highest number a four-digit ADR file name can carry.
const MAX_NUMBER: u32 = 9999;

/// Longest slug a new record's file name gets, as `adrlint new` truncates it.
const MAX_SLUG_LEN: usize = 80;

/// The status a new record starts with.
const NEW_STATUS: &str = "proposed";

/// Answers one `poly adr` operation over `files`. See the module docs.
///
/// # Errors
///
/// When the manifest does not parse, the operation has nothing to act on (no locked record, no
/// entry to unlock), or the title of a new record is unusable.
pub fn plan(op: &WireAdrOp, cfg: &EngineConfig, files: &[ProjectFile]) -> anyhow::Result<WireAdrPlan> {
    match op {
        WireAdrOp::Lock { all, check } => lock_log(cfg, files, *all, *check),
        WireAdrOp::Unlock { path } => unlock_record(cfg, files, path),
        WireAdrOp::New { dir, title, date } => new_record(cfg, files, dir, title, date),
    }
}

/// The manifest key of `path`: relative to the project root, `/` separated.
fn key_of(path: &Path) -> String {
    lock::key(Path::new(""), path)
}

/// The manifest's key, from the `lock` option (default `adrlint.lock`), as the project check reads it.
fn lock_key(cfg: &EngineConfig) -> String {
    let configured = cfg.options.opt_str("lock").unwrap_or(DEFAULT_LOCK);
    key_of(Path::new(configured))
}

/// The parsed manifest among `files`, or `None` when the host handed over none (it does not exist).
fn manifest(files: &[ProjectFile], lock_key: &str) -> anyhow::Result<Option<LockFile>> {
    files
        .iter()
        .find(|file| key_of(&file.path) == lock_key)
        .map(|file| lock::parse_manifest(&file.content).with_context(|| format!("invalid lock file {lock_key}")))
        .transpose()
}

/// The number of a numbered ADR file (`NNNN-*.md`), or `None` for any other file.
fn record_number(path: &Path) -> Option<u32> {
    let name = path.file_name()?.to_str()?;
    if !discovery::is_numbered(name) {
        return None;
    }
    discovery::parse_number(name)
}

/// Plans the manifest. With `check` it never refuses what `adrlint lock --check` reports instead: a
/// stale canonical version comes back as a pending write (the host exits 1 on it), and a log with no
/// manifest and nothing to lock as an empty plan (exit 0), so an always-run hook passes on a log that
/// has not opted in.
fn lock_log(cfg: &EngineConfig, files: &[ProjectFile], all: bool, check: bool) -> anyhow::Result<WireAdrPlan> {
    let lock_key = lock_key(cfg);
    let existing = manifest(files, &lock_key)?;
    let current = existing.clone().unwrap_or_default();
    let stale = !current.is_current();
    if stale && !all && !check {
        bail!(
            "{lock_key} uses canonical version {}; run `poly adr lock --all` to re-lock every ADR",
            current.canonical
        );
    }

    let statuses = LockConfig::default();
    let mut candidate = current.clone();
    candidate.version = lock::LOCK_VERSION;
    candidate.canonical = lock::CANONICAL_VERSION;
    let mut locked = 0usize;
    for file in files {
        let key = key_of(&file.path);
        if key == lock_key || is_template(&file.path) {
            continue;
        }
        let Some(number) = record_number(&file.path) else {
            continue;
        };
        let document = adr::parse(&file.content);
        if !all && !adr_config::is_locked_status(&statuses, &lock::status(&document)) {
            continue;
        }
        candidate.insert(key, lock::entry(&document, number));
        locked += 1;
    }

    if locked == 0 && current.adrs.is_empty() {
        if check && existing.is_none() {
            return Ok(WireAdrPlan {
                writes: Vec::new(),
                changes: Vec::new(),
                summary: format!("no ADR has a locked status and {lock_key} does not exist; nothing to lock"),
            });
        }
        bail!(
            "no ADR has a locked status ({}); pass --all to lock every ADR",
            statuses.statuses.join(", ")
        );
    }

    let mut changes = lock_changes(&current, &candidate);
    if stale {
        changes.push(format!(
            "re-lock {lock_key}, which uses canonical version {} (`poly adr lock --all`)",
            current.canonical
        ));
    }
    if existing.as_ref() == Some(&candidate) {
        return Ok(WireAdrPlan {
            writes: Vec::new(),
            changes,
            summary: format!("{lock_key} is up to date"),
        });
    }
    Ok(WireAdrPlan {
        writes: vec![WireAdrWrite {
            path: lock_key.clone(),
            content: lock::render(&candidate)?,
            create: false,
        }],
        changes,
        summary: format!("locked {locked} ADR(s) in {lock_key}"),
    })
}

/// `add <key>` / `update <key>` for every entry `candidate` adds to or changes in `current`.
fn lock_changes(current: &LockFile, candidate: &LockFile) -> Vec<String> {
    candidate
        .adrs
        .iter()
        .filter_map(|(key, entry)| match current.entry(key) {
            None => Some(format!("add {key}")),
            Some(existing) if existing != entry => Some(format!("update {key}")),
            Some(_) => None,
        })
        .collect()
}

fn unlock_record(cfg: &EngineConfig, files: &[ProjectFile], path: &str) -> anyhow::Result<WireAdrPlan> {
    let lock_key = lock_key(cfg);
    let Some(mut manifest) = manifest(files, &lock_key)? else {
        bail!("{lock_key} does not exist, so no ADR is locked");
    };
    let key = key_of(Path::new(path));
    if !lock::unlock(&mut manifest, &key) {
        bail!("{lock_key} has no entry for {key}");
    }
    Ok(WireAdrPlan {
        writes: vec![WireAdrWrite {
            path: lock_key,
            content: lock::render(&manifest)?,
            create: false,
        }],
        changes: vec![format!("remove {key}")],
        summary: format!("unlocked {key}"),
    })
}

fn new_record(
    cfg: &EngineConfig,
    files: &[ProjectFile],
    dir: &str,
    title: &str,
    date: &str,
) -> anyhow::Result<WireAdrPlan> {
    let title = validate_title(title)?;
    let dir = Path::new(dir);
    let in_dir = |file: &&&ProjectFile| file.path.parent().unwrap_or_else(|| Path::new("")) == dir;

    let mut members: Vec<&ProjectFile> = files.iter().collect();
    members.sort_by(|a, b| a.path.cmp(&b.path));
    // The template comes from the target directory; the number from every configured directory, so
    // two logs never hand out the same number.
    let template = members.iter().filter(in_dir).find(|file| is_template(&file.path));
    let highest = members
        .iter()
        .filter(|file| !is_template(&file.path))
        .filter_map(|file| record_number(&file.path))
        .max();
    let next = highest.map_or_else(|| start(cfg), |number| number.saturating_add(1));
    if next > MAX_NUMBER {
        bail!("the ADR directory has reached the maximum number, {MAX_NUMBER}");
    }

    let name = format!("{next:04}-{}.md", truncate_slug(&slugify(title)));
    let path = key_of(&dir.join(&name));
    if files.iter().any(|file| key_of(&file.path) == path) {
        bail!("{path} already exists");
    }
    let content = match template {
        Some(template) => from_template(&template.content, title, date),
        None => madr_template(title, date),
    };
    Ok(WireAdrPlan {
        writes: vec![WireAdrWrite {
            path: path.clone(),
            content,
            create: true,
        }],
        changes: vec![format!("create {path}")],
        summary: format!("created {path}"),
    })
}

/// The first number of an empty log: the `start` option, else adrlint's own default.
fn start(cfg: &EngineConfig) -> u32 {
    cfg.options
        .opt_i64("start")
        .and_then(|start| u32::try_from(start).ok())
        .unwrap_or_else(|| AdrConfig::default().start)
}

/// Whether `path` is the log's template: `template.md`, or a numbered `NNNN-template.md`.
///
/// A numbered template shares its number with a record when the log starts at the same number, which
/// is why a repository exempts that pair from `MADR43` in `[per-file-ignores]`. It is never locked
/// and never counts toward the next number.
fn is_template(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    let name = name.to_ascii_lowercase();
    let rest = name
        .split_once('-')
        .filter(|(number, _)| !number.is_empty() && number.bytes().all(|byte| byte.is_ascii_digit()))
        .map_or(name.as_str(), |(_, rest)| rest);
    rest == "template.md"
}

/// Rejects a title that cannot name a record, as `adrlint new` does.
fn validate_title(raw: &str) -> anyhow::Result<&str> {
    let title = raw.trim();
    if title.is_empty() {
        bail!("the ADR title must not be empty");
    }
    if title.chars().any(char::is_control) {
        bail!("the ADR title must not contain control characters");
    }
    if !title.chars().any(|character| character.is_ascii_alphanumeric()) {
        bail!("the ADR title must contain at least one ASCII letter or digit");
    }
    Ok(title)
}

/// Lower-case ASCII words joined by single hyphens; `adr` when nothing is left.
fn slugify(title: &str) -> String {
    let mut slug = String::with_capacity(title.len());
    let mut previous_hyphen = false;
    for character in title.chars() {
        if character.is_ascii_alphanumeric() {
            slug.push(character.to_ascii_lowercase());
            previous_hyphen = false;
        } else if !previous_hyphen && !slug.is_empty() {
            slug.push('-');
            previous_hyphen = true;
        }
    }
    while slug.ends_with('-') {
        slug.pop();
    }
    if slug.is_empty() { "adr".to_owned() } else { slug }
}

/// Cuts a slug to [`MAX_SLUG_LEN`] bytes, at a hyphen when there is one.
fn truncate_slug(slug: &str) -> &str {
    if slug.len() <= MAX_SLUG_LEN {
        return slug;
    }
    let boundary = slug[..MAX_SLUG_LEN]
        .rfind('-')
        .filter(|index| *index > 0)
        .unwrap_or(MAX_SLUG_LEN);
    &slug[..boundary]
}

/// The repository's template with the new title, status and date: the front matter's `status:` and
/// `date:` lines are rewritten and the first `# ` heading becomes the title (inserted after the front
/// matter when the template has none).
fn from_template(template: &str, title: &str, date: &str) -> String {
    let mut out = String::with_capacity(template.len() + title.len());
    let mut lines = template.split_inclusive('\n').peekable();
    if lines.peek().is_some_and(|line| line.trim_end() == "---") {
        out.push_str(lines.next().unwrap_or_default());
        for line in lines.by_ref() {
            let ending = &line[line.trim_end_matches(['\r', '\n']).len()..];
            match front_matter_key(line) {
                Some("status") => out.push_str(&format!("status: {NEW_STATUS}{ending}")),
                Some("date") => out.push_str(&format!("date: {date}{ending}")),
                _ => out.push_str(line),
            }
            if line.trim_end() == "---" {
                break;
            }
        }
    }
    let mut titled = false;
    let mut body = String::new();
    for line in lines {
        if !titled && line.starts_with("# ") {
            let ending = &line[line.trim_end_matches(['\r', '\n']).len()..];
            body.push_str(&format!("# {title}{ending}"));
            titled = true;
        } else {
            body.push_str(line);
        }
    }
    if !titled {
        out.push_str(&format!("# {title}\n\n"));
    }
    out.push_str(&body);
    out
}

/// The key of a top-level `key: value` front matter line.
fn front_matter_key(line: &str) -> Option<&str> {
    let (key, _) = line.split_once(':')?;
    (!key.is_empty() && !key.starts_with([' ', '\t', '#', '-'])).then(|| key.trim_end())
}

/// MADR's template, as `adrlint new` writes it.
fn madr_template(title: &str, date: &str) -> String {
    format!(
        r#"---
status: {NEW_STATUS}
date: {date}
decision-makers: []
consulted: []
informed: []
---

# {title}

## Context and Problem Statement

Describe the context and the problem statement here, in two or three sentences.

## Decision Drivers

* The first decision driver
* The second decision driver

## Considered Options

* The first option
* The second option

## Decision Outcome

Chosen option: "The first option", because it best satisfies the decision drivers.

### Consequences

* Good, because the first positive consequence
* Bad, because the first negative consequence

### Confirmation

Describe how compliance with the decision will be confirmed.

## Pros and Cons of the Options

### The first option

* Good, because the first advantage
* Bad, because the first disadvantage

### The second option

* Good, because the first advantage
* Bad, because the first disadvantage

## More Information

* [MADR](https://adr.github.io/madr/)
"#
    )
}

#[cfg(test)]
mod tests;
