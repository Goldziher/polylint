//! Config importers: each reads a foreign tool's config file(s) and produces
//! `poly.toml` fragments plus an [`Absorb`] verdict describing how completely
//! poly can honor the source. The driver merges fragments into an existing
//! `poly.toml` (comment-preserving via `toml_edit`) and the deletion policy uses
//! the verdict to decide whether the source may be removed.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use toml_edit::{Array, DocumentMut, InlineTable, Item, Table, Value};

pub mod hooks;
pub mod markdownlint;
pub mod ruff;
pub mod taplo;
pub mod typos;

/// How completely poly can absorb a source config.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Absorb {
    /// Every meaningful setting maps onto a poly key — the source may be deleted.
    Full,
    /// Some settings are not representable in poly; the carried keys are the
    /// leftovers. The source must be kept.
    Partial(Vec<String>),
    /// Nothing was absorbed (no recognized config present).
    None,
}

/// A single `poly.toml` table (identified by its header path) and the key/value
/// entries to insert under it.
#[derive(Debug, Clone)]
pub struct Fragment {
    /// Table header path, e.g. `["lint", "python", "ruff"]` renders as
    /// `[lint.python.ruff]`.
    pub path: Vec<String>,
    /// Ordered key → value entries.
    pub entries: Vec<(String, Item)>,
}

impl Fragment {
    /// Build a fragment at `path` from an ordered list of entries.
    pub fn new(path: &[&str], entries: Vec<(String, Item)>) -> Self {
        Fragment {
            path: path.iter().map(|s| (*s).to_string()).collect(),
            entries,
        }
    }
}

/// The outcome of running one importer against a directory.
#[derive(Debug, Clone)]
pub struct ImportResult {
    /// Tool name for reporting, e.g. `"ruff"`.
    pub tool: &'static str,
    /// Source files that were read (absolute paths).
    pub sources: Vec<PathBuf>,
    /// `poly.toml` fragments to merge.
    pub fragments: Vec<Fragment>,
    /// Human-readable notes (caveats, dropped settings).
    pub notes: Vec<String>,
    /// Completeness verdict driving the deletion policy.
    pub absorb: Absorb,
}

impl ImportResult {
    /// Whether this result carries any table to merge.
    pub fn has_fragments(&self) -> bool {
        self.fragments.iter().any(|f| !f.entries.is_empty())
    }
}

/// What merging an importer's fragments into a `poly.toml` did.
#[derive(Debug, Default)]
pub struct Merge {
    /// One line per key whose existing value differs from the imported one.
    pub conflicts: Vec<String>,
    /// The dotted names of those keys.
    pub keys: Vec<String>,
    /// Set when nothing could be merged, with the reason.
    pub refused: Option<String>,
    /// Caveats from converting the fragments to the target's schema.
    pub notes: Vec<String>,
}

/// Merge every fragment into `doc`. A key absent from `doc` is inserted; a key
/// already set to the same value is left alone, so re-runs are idempotent. A key
/// set to a *different* value (arrays included) keeps the existing value and is
/// reported in [`Merge::conflicts`]: the imported value is not honored, so the
/// caller must keep the source file.
///
/// When `doc` already uses the v2 schema (`[engines]`), the fragments are
/// rewritten to v2 first; a v2 file ignores v1 `[lint.*]` / `[fmt.*]` tables.
pub fn apply(doc: &mut DocumentMut, fragments: &[Fragment]) -> Merge {
    merge(doc, fragments, false)
}

/// [`apply`] for one importer result, then downgrade the result to
/// [`Absorb::Partial`] when anything conflicted or the merge was refused, so the
/// deletion policy keeps its source files. Returns the conflict lines.
pub fn apply_result(doc: &mut DocumentMut, result: &mut ImportResult) -> Vec<String> {
    let merge = merge(doc, &result.fragments, result.tool == "typos");
    let sources: Vec<String> = result.sources.iter().map(|p| p.display().to_string()).collect();
    result.notes.extend(merge.notes);
    if let Some(reason) = merge.refused {
        result.fragments.clear();
        result
            .notes
            .push(format!("{reason}; nothing was merged and {} kept", sources.join(", ")));
        result.absorb = Absorb::Partial(vec![reason]);
        return Vec::new();
    }
    if merge.keys.is_empty() {
        return merge.conflicts;
    }
    result.notes.push(format!(
        "conflicting keys (poly.toml value kept, imported value not applied): {}; {} kept",
        merge.keys.join(", "),
        sources.join(", ")
    ));
    let keys = merge.keys.iter().map(|key| format!("conflicting key {key}"));
    result.absorb = match std::mem::replace(&mut result.absorb, Absorb::None) {
        Absorb::Partial(mut left) => {
            left.extend(keys);
            Absorb::Partial(left)
        }
        _ => Absorb::Partial(keys.collect()),
    };
    merge.conflicts
}

fn merge(doc: &mut DocumentMut, fragments: &[Fragment], enable_typos: bool) -> Merge {
    let mut out = Merge::default();
    let mut incoming = DocumentMut::new();
    for fragment in fragments.iter().filter(|f| !f.entries.is_empty()) {
        let table = ensure_table(incoming.as_table_mut(), &fragment.path);
        for (key, item) in &fragment.entries {
            table.insert(key, item.clone());
        }
    }
    if incoming.as_table().is_empty() {
        return out;
    }
    if doc.contains_key("engines") {
        let options = super::v2::V2Options {
            required_version: None,
            keep_typos_default: enable_typos,
        };
        match super::v2::migrate_document(&mut incoming, &options) {
            Ok(outcome) => out.notes.extend(outcome.notes),
            Err(error) => {
                out.refused = Some(format!(
                    "poly.toml uses the v2 schema and the imported settings could not be converted: {error:#}"
                ));
                return out;
            }
        }
    }
    merge_table(doc.as_table_mut(), incoming.as_table(), &mut Vec::new(), &mut out);
    out
}

fn merge_table(target: &mut Table, incoming: &Table, path: &mut Vec<String>, out: &mut Merge) {
    for (key, item) in incoming.iter() {
        path.push(key.to_string());
        if !target.contains_key(key) {
            let mut fresh = item.clone();
            clear_positions(&mut fresh);
            target.insert(key, fresh);
        } else if let (Some(existing), Item::Table(inner)) = (target.get_mut(key).and_then(Item::as_table_mut), item) {
            merge_table(existing, inner, path, out);
        } else if let Some(existing) = target.get(key)
            && !same_value(existing, item)
        {
            let dotted = path.join(".");
            out.conflicts.push(format!(
                "{dotted}: poly.toml has {}, the source has {}",
                render(existing),
                render(item)
            ));
            out.keys.push(dotted);
        }
        path.pop();
    }
}

fn render(item: &Item) -> String {
    item.to_string().trim().to_string()
}

/// Whether two items hold the same TOML value, ignoring formatting.
fn same_value(a: &Item, b: &Item) -> bool {
    let parse = |item: &Item| {
        let rendered = render(item);
        format!("v = {rendered}")
            .parse::<toml::Table>()
            .ok()
            .and_then(|mut t| t.remove("v"))
    };
    match (parse(a), parse(b)) {
        (Some(left), Some(right)) => left == right,
        _ => render(a) == render(b),
    }
}

fn clear_positions(item: &mut Item) {
    match item {
        Item::Table(table) => {
            table.set_position(None);
            for (_, child) in table.iter_mut() {
                clear_positions(child);
            }
        }
        Item::ArrayOfTables(array) => {
            for table in array.iter_mut() {
                table.set_position(None);
                for (_, child) in table.iter_mut() {
                    clear_positions(child);
                }
            }
        }
        _ => {}
    }
}

/// Navigate (creating as needed) the table at `path`, marking freshly created
/// intermediate tables implicit so only the leaf renders a `[header]`.
fn ensure_table<'a>(root: &'a mut Table, path: &[String]) -> &'a mut Table {
    let mut current = root;
    let last = path.len().saturating_sub(1);
    for (index, segment) in path.iter().enumerate() {
        let entry = current.entry(segment).or_insert_with(|| {
            let mut table = Table::new();
            if index < last {
                table.set_implicit(true);
            }
            Item::Table(table)
        });
        current = entry
            .as_table_mut()
            .expect("migrate fragment path segment must be a table");
    }
    current
}

/// A TOML array of strings.
pub fn str_array(items: &[String]) -> Item {
    let mut array = Array::new();
    for item in items {
        array.push(item.as_str());
    }
    Item::Value(Value::Array(array))
}

/// A single-line inline table of string → string (keys sorted for determinism).
pub fn inline_map(map: &BTreeMap<String, String>) -> Item {
    let mut table = InlineTable::new();
    for (key, value) in map {
        table.insert(key, Value::from(value.as_str()));
    }
    Item::Value(Value::InlineTable(table))
}

/// A TOML string value.
pub fn str_item(value: &str) -> Item {
    Item::Value(Value::from(value))
}

/// A TOML integer value.
pub fn int_item(value: i64) -> Item {
    Item::Value(Value::from(value))
}

/// A TOML boolean value.
pub fn bool_item(value: bool) -> Item {
    Item::Value(Value::from(value))
}

/// Read and parse a TOML file, returning `None` when missing or malformed.
pub fn load_toml(path: &Path) -> Option<toml::Table> {
    let text = std::fs::read_to_string(path).ok()?;
    toml::from_str(&text).ok()
}

/// Read and parse a JSON / JSONC file into a [`serde_json::Value`]. Uses `json5`
/// so comments and trailing commas (common in `.jsonc`) parse cleanly.
pub fn load_json(path: &Path) -> Option<serde_json::Value> {
    let text = std::fs::read_to_string(path).ok()?;
    json5::from_str(&text).ok()
}

/// Read and parse a YAML file into a [`serde_json::Value`].
pub fn load_yaml(path: &Path) -> Option<serde_json::Value> {
    let text = std::fs::read_to_string(path).ok()?;
    serde_saphyr::from_str(&text).ok()
}

/// Why `path` cannot be parsed as the config format its extension names, or
/// `None` when it parses (or does not exist).
pub fn parse_error(path: &Path) -> Option<String> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return None,
        Err(error) => return Some(error.to_string()),
    };
    match path.extension().and_then(|e| e.to_str()) {
        Some("toml") => toml::from_str::<toml::Table>(&text).err().map(|e| e.to_string()),
        Some("json" | "jsonc") => json5::from_str::<serde_json::Value>(&text).err().map(|e| e.to_string()),
        Some("yml" | "yaml") => serde_saphyr::from_str::<serde_json::Value>(&text)
            .err()
            .map(|e| e.to_string()),
        _ => None,
    }
}

/// Source config files in `dir` that exist but cannot be parsed and that no
/// importer read, as `could not parse <file>: <reason>; kept` notes.
pub fn unparsable_sources(dir: &Path, results: &[ImportResult], candidates: &[&str]) -> Vec<String> {
    candidates
        .iter()
        .map(|name| dir.join(name))
        .filter(|path| path.is_file() && !results.iter().any(|r| r.sources.contains(path)))
        .filter_map(|path| {
            let reason = parse_error(&path)?;
            Some(format!(
                "could not parse {}: {}; kept",
                path.display(),
                reason.trim().replace('\n', " ")
            ))
        })
        .collect()
}

/// Find the first existing file among `names` in `dir`.
pub fn first_existing(dir: &Path, names: &[&str]) -> Option<PathBuf> {
    names
        .iter()
        .map(|name| dir.join(name))
        .find(|candidate| candidate.is_file())
}

/// Collect the string elements of a TOML array value, dropping non-strings.
pub fn toml_string_list(value: Option<&toml::Value>) -> Vec<String> {
    value
        .and_then(toml::Value::as_array)
        .map(|arr| arr.iter().filter_map(|v| v.as_str().map(str::to_string)).collect())
        .unwrap_or_default()
}
