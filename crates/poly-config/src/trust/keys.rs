//! Which config keys can make poly run a program, and their digest.
//!
//! The list is the trust model's attack surface (ADR 0059): every key here either
//! names an executable, feeds one its argv or environment, picks where it runs,
//! fetches one, or relocates a directory poly later executes from or trusts.

use std::path::Path;

/// `[tools.<name>]` keys withheld from an untrusted config. `bin` is handled
/// apart: a bare name only selects a program from the user's own `PATH`.
const TOOL_KEYS: &[&str] = &[
    "url",
    "sha256",
    "env",
    "args",
    "extra_args",
    "format_extra_args",
    "lint_extra_args",
    "new_from_rev",
    "root",
];

/// oxc options that hand files to an `oxlint`/`oxfmt` found in the repository's
/// own `node_modules/.bin`, which then loads the repository's JavaScript plugins.
const OXC_KEYS: &[&str] = &["js_plugins", "external"];

/// One exec-capable key a config sets, as its path of table keys and its value.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Entry {
    pub(crate) path: Vec<String>,
    pub(crate) value: toml::Value,
}

impl Entry {
    fn new(path: &[&str], value: &toml::Value) -> Self {
        Entry {
            path: path.iter().map(|segment| (*segment).to_owned()).collect(),
            value: value.clone(),
        }
    }

    /// The dotted key, as a note names it.
    pub(crate) fn display(&self) -> String {
        self.path.join(".")
    }

    /// Whether this entry is the `[hooks]` table, which is withheld rather than removed.
    pub(crate) fn is_hooks(&self) -> bool {
        self.path.len() == 1 && self.path[0] == "hooks"
    }
}

/// Whether `bin` names a program to look up on `PATH` rather than a file.
#[must_use]
pub fn is_bare_program(bin: &str) -> bool {
    !bin.is_empty() && !bin.contains(['/', '\\'])
}

/// Every exec-capable key `table` (a raw, merged config table) sets, in a stable order.
pub(crate) fn exec_entries(table: &toml::Table) -> Vec<Entry> {
    let mut entries = Vec::new();
    if let Some(tools) = table.get("tools").and_then(toml::Value::as_table) {
        for (name, tool) in tools {
            let Some(tool) = tool.as_table() else { continue };
            if let Some(bin) = tool.get("bin")
                && !bin.as_str().is_some_and(is_bare_program)
            {
                entries.push(Entry::new(&["tools", name, "bin"], bin));
            }
            for key in TOOL_KEYS {
                if let Some(value) = tool.get(*key) {
                    entries.push(Entry::new(&["tools", name, key], value));
                }
            }
        }
    }
    if let Some(hooks) = table.get("hooks") {
        entries.push(Entry::new(&["hooks"], hooks));
    }
    if let Some(cache) = table.get("cache").and_then(toml::Value::as_table) {
        if let Some(dir) = cache.get("dir") {
            entries.push(Entry::new(&["cache", "dir"], dir));
        }
        if let Some(sccache) = cache.get("sccache").and_then(toml::Value::as_table) {
            if let Some(dir) = sccache.get("dir") {
                entries.push(Entry::new(&["cache", "sccache", "dir"], dir));
            }
            if let Some(bin) = sccache.get("bin")
                && !bin.as_str().is_some_and(is_bare_program)
            {
                entries.push(Entry::new(&["cache", "sccache", "bin"], bin));
            }
        }
    }
    for section in ["lint", "fmt", "engines"] {
        if let Some(value) = table.get(section).and_then(toml::Value::as_table) {
            let mut path = vec![section.to_owned()];
            collect_oxc(value, &mut path, false, &mut entries);
        }
    }
    entries.sort_by(|a, b| a.path.cmp(&b.path));
    entries
}

/// Collect the [`OXC_KEYS`] anywhere beneath an `oxc` table of `table`.
fn collect_oxc(table: &toml::Table, path: &mut Vec<String>, in_oxc: bool, entries: &mut Vec<Entry>) {
    for (key, value) in table {
        path.push(key.clone());
        if in_oxc && OXC_KEYS.contains(&key.as_str()) {
            entries.push(Entry {
                path: path.clone(),
                value: value.clone(),
            });
        } else if let Some(nested) = value.as_table() {
            collect_oxc(nested, path, in_oxc || key == "oxc", entries);
        }
        path.pop();
    }
}

/// Remove `entries` from `table`, except the `[hooks]` table, which stays so the
/// whole-project phase can report what it withheld.
pub(crate) fn strip(table: &mut toml::Table, entries: &[Entry]) {
    for entry in entries.iter().filter(|entry| !entry.is_hooks()) {
        remove_path(table, &entry.path);
    }
}

fn remove_path(table: &mut toml::Table, path: &[String]) {
    match path {
        [] => {}
        [last] => {
            table.remove(last);
        }
        [head, rest @ ..] => {
            if let Some(nested) = table.get_mut(head).and_then(toml::Value::as_table_mut) {
                remove_path(nested, rest);
            }
        }
    }
}

/// The digest a trust record stores for the config governing `config_dir`: the
/// directory relative to `root` and every exec-capable entry, length-prefixed so
/// no two different inputs share an encoding.
pub(crate) fn digest(root: &Path, config_dir: &Path, entries: &[Entry]) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"poly-trust-v1\0");
    let dir = super::canonical(config_dir);
    let relative = match dir.strip_prefix(root) {
        Ok(relative) => relative
            .components()
            .map(|component| component.as_os_str().to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join("/"),
        Err(_) => dir.to_string_lossy().into_owned(),
    };
    feed_str(&mut hasher, &relative);
    for entry in entries {
        hasher.update(&(entry.path.len() as u64).to_le_bytes());
        for segment in &entry.path {
            feed_str(&mut hasher, segment);
        }
        feed_value(&mut hasher, &entry.value);
    }
    hasher.finalize().to_hex().to_string()
}

fn feed_str(hasher: &mut blake3::Hasher, text: &str) {
    hasher.update(&(text.len() as u64).to_le_bytes());
    hasher.update(text.as_bytes());
}

/// Feed `value` with a type tag per node and table keys in sorted order, so the
/// digest does not depend on how the file ordered its keys.
fn feed_value(hasher: &mut blake3::Hasher, value: &toml::Value) {
    match value {
        toml::Value::String(text) => {
            hasher.update(b"s");
            feed_str(hasher, text);
        }
        toml::Value::Integer(number) => {
            hasher.update(b"i");
            hasher.update(&number.to_le_bytes());
        }
        toml::Value::Float(number) => {
            hasher.update(b"f");
            hasher.update(&number.to_bits().to_le_bytes());
        }
        toml::Value::Boolean(flag) => {
            hasher.update(if *flag { b"t" } else { b"n" });
        }
        toml::Value::Datetime(datetime) => {
            hasher.update(b"d");
            feed_str(hasher, &datetime.to_string());
        }
        toml::Value::Array(items) => {
            hasher.update(b"a");
            hasher.update(&(items.len() as u64).to_le_bytes());
            for item in items {
                feed_value(hasher, item);
            }
        }
        toml::Value::Table(table) => {
            hasher.update(b"m");
            hasher.update(&(table.len() as u64).to_le_bytes());
            let mut keys: Vec<&String> = table.keys().collect();
            keys.sort();
            for key in keys {
                feed_str(hasher, key);
                feed_value(hasher, &table[key.as_str()]);
            }
        }
    }
}
