//! Where a file's JSON Schema comes from, and the digest of it that keys the lint cache.
//!
//! The `jsonschema` engine validates JSON, YAML and TOML against a schema that is never fetched:
//! it is a file under the project root or in the local schema cache. This module owns the lookup
//! the engine and the host-side cache hook ([`HostHook::JsonSchema`](super::HostHook::JsonSchema))
//! must agree on, so a schema edit invalidates exactly the results it changes.
//!
//! A schema is named, in order, by the file's own reference (`$schema` in JSON, a `#:schema` or
//! `# yaml-language-server: $schema=` comment in YAML and TOML) and then by the first entry of
//! `[engines.jsonschema] schemas = [{ glob, schema }]` whose glob matches the project-relative path.
//! A reference is a path (relative to the file for an in-file reference, to the project root for a
//! mapping), a `file://` URI, or an `http(s)://` URL that must already sit in the schema cache
//! (`<cache_dir>/<host>/<path>`, `.poly/schemas` by default).

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::SystemTime;

use crate::config::EngineConfig;
use crate::options::OptionsExt;

const DEFAULT_CACHE_DIR: &str = ".poly/schemas";
const CONFIG_FILE: &str = "poly.toml";
/// Most local files (the schema and the files it `$ref`s) one digest covers.
const MAX_CLOSURE: usize = 64;
/// Most leading comment lines scanned for a schema directive.
const DIRECTIVE_SCAN_LINES: usize = 64;

/// What a file is validated against.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SchemaSource {
    /// Nothing names a schema for the file.
    None,
    /// A schema file on disk, canonical, inside the project root or the schema cache.
    Local(PathBuf),
    /// A schema was named but cannot be read here: it is remote and not cached, missing, or
    /// outside the project root. Reported, never silently passed.
    Unavailable {
        /// The reference as written.
        reference: String,
        /// Why it cannot be used.
        reason: String,
    },
}

/// The directories a run may read schemas from, for one file.
#[derive(Debug, Clone)]
pub struct Scope {
    /// The canonical file the scope was built for.
    pub file: PathBuf,
    /// The project root: the nearest ancestor holding a `poly.toml`, else the working directory.
    pub root: PathBuf,
    /// The directory remote schemas are looked up in.
    pub cache_dir: PathBuf,
}

impl Scope {
    /// The scope for `file` under `cfg`.
    #[must_use]
    pub fn for_file(file: &Path, cfg: &EngineConfig) -> Scope {
        let absolute = if file.is_absolute() {
            file.to_path_buf()
        } else {
            std::env::current_dir().unwrap_or_default().join(file)
        };
        let file = canonical(&absolute).unwrap_or(absolute);
        let root = project_root(&file);
        let cache_dir = match cfg.options.opt_str("cache_dir") {
            Some(dir) if Path::new(dir).is_absolute() => PathBuf::from(dir),
            Some(dir) => root.join(dir),
            None => root.join(DEFAULT_CACHE_DIR),
        };
        Scope { file, root, cache_dir }
    }

    /// Whether the canonical `path` is inside the project root or the schema cache.
    #[must_use]
    pub fn allows(&self, path: &Path) -> bool {
        let cache = canonical(&self.cache_dir).unwrap_or_else(|_| self.cache_dir.clone());
        path.starts_with(&self.root) || path.starts_with(cache)
    }

    /// Resolves `reference` to a readable schema file: a path relative to `base_dir`, a `file://`
    /// URI, or an `http(s)://` URL found in the schema cache.
    ///
    /// # Errors
    ///
    /// The reason the reference cannot be used: unsupported scheme, not cached, missing, or outside
    /// the allowed directories.
    pub fn locate(&self, reference: &str, base_dir: &Path) -> Result<PathBuf, String> {
        let reference = reference.split('#').next().unwrap_or_default().trim();
        if reference.is_empty() {
            return Err("empty schema reference".to_owned());
        }
        let candidate = match scheme_of(reference) {
            Some("http" | "https") => self.cache_path(reference)?,
            Some("file") => uri_to_path(reference)?,
            Some(other) => return Err(format!("unsupported schema scheme `{other}`")),
            None => {
                let path = Path::new(reference);
                if path.is_absolute() {
                    path.to_path_buf()
                } else {
                    base_dir.join(path)
                }
            }
        };
        let located = canonical(&candidate).map_err(|_| {
            if scheme_of(reference).is_some_and(|scheme| scheme.starts_with("http")) {
                format!(
                    "remote schema is not in the local schema cache; expected at `{}`",
                    candidate.display()
                )
            } else {
                format!("`{}` does not exist", candidate.display())
            }
        })?;
        if self.allows(&located) {
            Ok(located)
        } else {
            Err(format!(
                "`{}` is outside the project root and the schema cache",
                located.display()
            ))
        }
    }

    /// The cache location of a remote schema URL: `<cache_dir>/<host>/<path>`.
    fn cache_path(&self, url: &str) -> Result<PathBuf, String> {
        let rest = url.split_once("://").map_or(url, |(_, rest)| rest);
        let rest = rest.split(['?', '#']).next().unwrap_or_default();
        let mut path = self.cache_dir.clone();
        let mut segments = 0;
        for segment in rest.split('/').filter(|segment| !segment.is_empty()) {
            if matches!(segment, "." | "..") || segment.contains([':', '\\']) {
                return Err(format!("`{url}` cannot be mapped into the schema cache"));
            }
            path.push(segment);
            segments += 1;
        }
        if segments < 2 || rest.ends_with('/') {
            return Err(format!("`{url}` names no schema file the cache could hold"));
        }
        Ok(path)
    }

    /// The schema of the first `schemas` mapping whose glob matches this file's project-relative path.
    #[must_use]
    pub fn mapped(&self, cfg: &EngineConfig) -> Option<String> {
        let relative = self.file.strip_prefix(&self.root).unwrap_or(&self.file);
        let relative = relative.to_string_lossy().replace('\\', "/");
        compiled_mappings(cfg)
            .iter()
            .find_map(|(matcher, schema)| matcher.is_match(&relative).then(|| schema.clone()))
    }

    /// The directory an in-file reference is relative to: the file's own.
    #[must_use]
    pub fn base_dir(&self) -> &Path {
        self.file.parent().unwrap_or(&self.root)
    }
}

/// Most directories whose project root is remembered.
const MAX_ROOTS: usize = 4096;
/// Most distinct configs whose compiled `schemas` mappings are remembered.
const MAX_MAPPINGS: usize = 64;
/// Most schema closures remembered.
const MAX_CLOSURES: usize = 64;

type RootMemo = Mutex<HashMap<PathBuf, Option<PathBuf>>>;

fn root_memo() -> &'static RootMemo {
    static MEMO: OnceLock<RootMemo> = OnceLock::new();
    MEMO.get_or_init(Mutex::default)
}

/// Forgets the project roots found so far; called at the start of a run, so a `poly.toml` created
/// or removed between runs of one process is seen.
pub fn forget() {
    root_memo()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clear();
}

/// The nearest ancestor of the canonical `file` holding a `poly.toml`, else the working directory.
/// The ancestor walk is remembered per directory: every file of a directory shares one answer.
fn project_root(file: &Path) -> PathBuf {
    let found = match file.parent() {
        Some(dir) => {
            let cached = root_memo()
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .get(dir)
                .cloned();
            cached.unwrap_or_else(|| {
                let found = find_root(file);
                let mut memo = root_memo().lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                if memo.len() >= MAX_ROOTS {
                    memo.clear();
                }
                memo.insert(dir.to_path_buf(), found.clone());
                found
            })
        }
        None => None,
    };
    found.unwrap_or_else(|| {
        let cwd = std::env::current_dir().unwrap_or_default();
        canonical(&cwd).unwrap_or(cwd)
    })
}

fn find_root(file: &Path) -> Option<PathBuf> {
    file.ancestors()
        .skip(1)
        .find(|dir| dir.join(CONFIG_FILE).is_file())
        .map(Path::to_path_buf)
}

type Matchers = Arc<Vec<(globset::GlobMatcher, String)>>;

/// The `schemas` mappings of `cfg` with their globs compiled, remembered per config so a run does
/// not rebuild every matcher for every file.
fn compiled_mappings(cfg: &EngineConfig) -> Matchers {
    static MEMO: OnceLock<Mutex<HashMap<[u8; 32], Matchers>>> = OnceLock::new();
    let key = cfg.fingerprint();
    let memo = MEMO.get_or_init(Mutex::default);
    if let Some(found) = memo.lock().unwrap_or_else(std::sync::PoisonError::into_inner).get(&key) {
        return Arc::clone(found);
    }
    let built: Matchers = Arc::new(
        mappings(cfg)
            .into_iter()
            .filter_map(|(glob, schema)| {
                let matcher = globset::Glob::new(&glob)
                    .inspect_err(|error| tracing::warn!(%glob, %error, "skipping invalid `schemas` glob"))
                    .ok()?
                    .compile_matcher();
                Some((matcher, schema))
            })
            .collect(),
    );
    let mut memo = memo.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    if memo.len() >= MAX_MAPPINGS {
        memo.clear();
    }
    memo.insert(key, Arc::clone(&built));
    built
}

/// The `(glob, schema)` pairs of `[engines.jsonschema] schemas`, in the order written; entries
/// that are not tables carrying both strings are ignored.
#[must_use]
pub fn mappings(cfg: &EngineConfig) -> Vec<(String, String)> {
    cfg.options
        .get("schemas")
        .and_then(toml::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|entry| {
            let table = entry.as_table()?;
            Some((
                table.get("glob")?.as_str()?.to_owned(),
                table.get("schema")?.as_str()?.to_owned(),
            ))
        })
        .collect()
}

/// The schema `file` is validated against. `in_file` is the file's own reference, when it has one.
///
/// An in-file reference wins; a remote one that is not cached yields to a matching `schemas`
/// mapping, anything else that cannot be used is [`SchemaSource::Unavailable`].
#[must_use]
pub fn resolve(file: &Path, in_file: Option<&str>, cfg: &EngineConfig) -> SchemaSource {
    resolve_in(&Scope::for_file(file, cfg), in_file, cfg)
}

/// [`resolve`] for a file whose [`Scope`] the caller already holds.
#[must_use]
pub fn resolve_in(scope: &Scope, in_file: Option<&str>, cfg: &EngineConfig) -> SchemaSource {
    if let Some(reference) = in_file.map(str::trim).filter(|reference| !reference.is_empty()) {
        match scope.locate(reference, scope.base_dir()) {
            Ok(path) => return SchemaSource::Local(path),
            Err(reason) => {
                let remote = scheme_of(reference).is_some_and(|scheme| scheme.starts_with("http"));
                if !remote || scope.mapped(cfg).is_none() {
                    return SchemaSource::Unavailable {
                        reference: reference.to_owned(),
                        reason,
                    };
                }
            }
        }
    }
    match scope.mapped(cfg) {
        Some(schema) => match scope.locate(&schema, &scope.root) {
            Ok(path) => SchemaSource::Local(path),
            Err(reason) => SchemaSource::Unavailable {
                reference: schema,
                reason,
            },
        },
        None => SchemaSource::None,
    }
}

/// The schema a `#:schema <ref>` (TOML) or `# yaml-language-server: $schema=<ref>` (YAML) comment in
/// the leading comment block of `content` names. Other comments are ignored, whatever they contain.
#[must_use]
pub fn directive_ref(content: &str) -> Option<String> {
    for line in content.lines().take(DIRECTIVE_SCAN_LINES) {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if !line.starts_with('#') {
            return None;
        }
        if let Some(reference) = line
            .strip_prefix("#:schema")
            .filter(|rest| rest.starts_with(char::is_whitespace))
        {
            let reference = reference.trim();
            if !reference.is_empty() {
                return Some(reference.to_owned());
            }
        }
        let comment = line.trim_start_matches('#').trim_start();
        if let Some(rest) = comment
            .strip_prefix("yaml-language-server:")
            .and_then(|rest| rest.trim_start().strip_prefix("$schema="))
        {
            let reference = rest.split_whitespace().next().unwrap_or_default();
            if !reference.is_empty() {
                return Some(reference.to_owned());
            }
        }
    }
    None
}

/// The `$schema` string values written anywhere in JSON or JSONC `content`, meta-schema URLs
/// excluded: a textual scan the engine uses to decide whether a file can name a schema before it
/// parses it.
#[must_use]
pub fn declared_json_refs(content: &str) -> Vec<String> {
    let mut references = quoted_values_after(content, "$schema");
    references.retain(|reference| !is_metaschema(reference));
    references
}

/// Whether `reference` names a JSON Schema meta-schema: a file carrying such a `$schema` is itself a
/// schema, not a document to validate against one.
#[must_use]
pub fn is_metaschema(reference: &str) -> bool {
    scheme_of(reference).is_some() && reference.contains("://json-schema.org/")
}

/// The cache digest of the schemas that govern `path`: the resolved schema file and every local file
/// it `$ref`s, plus the references that could not be resolved, so a schema that appears later
/// invalidates the result recorded for its absence. `None` when nothing governs the file.
#[must_use]
pub fn salt(cfg: &EngineConfig, path: &Path) -> Option<String> {
    let scope = Scope::for_file(path, cfg);
    let content = std::fs::read_to_string(&scope.file).ok()?;
    let mut references: Vec<String> = directive_ref(&content).into_iter().collect();
    references.extend(declared_json_refs(&content));

    let mut unresolved = BTreeSet::new();
    let mut seeds: BTreeSet<PathBuf> = BTreeSet::new();
    for reference in &references {
        note(&scope, reference, scope.base_dir(), &mut seeds, &mut unresolved);
    }
    if let Some(schema) = scope.mapped(cfg) {
        note(&scope, &schema, &scope.root, &mut seeds, &mut unresolved);
    }
    let closure = ref_closure(&scope, seeds);
    let seen = &closure.seen;
    unresolved.extend(closure.unresolved.iter().cloned());

    if seen.is_empty() && unresolved.is_empty() {
        return None;
    }
    let mut hasher = blake3::Hasher::new();
    for line in &unresolved {
        hasher.update(line.as_bytes());
        hasher.update(b"\0");
    }
    for file in seen {
        hasher.update(file.to_string_lossy().as_bytes());
        hasher.update(b"\0");
        hasher.update(file_digest(file).as_bytes());
    }
    Some(format!("jsonschema:{}", hasher.finalize().to_hex()))
}

/// Resolves `reference` against `base`: the file into `seen`, or the reason it is missing into
/// `unresolved`. Whether it resolved.
fn note(
    scope: &Scope,
    reference: &str,
    base: &Path,
    seen: &mut BTreeSet<PathBuf>,
    unresolved: &mut BTreeSet<String>,
) -> bool {
    match scope.locate(reference, base) {
        Ok(found) => {
            seen.insert(found);
            true
        }
        Err(_) => {
            unresolved.insert(format!("unresolved:{reference}"));
            false
        }
    }
}

type Stamp = Option<(SystemTime, u64)>;

fn stamp(path: &Path) -> Stamp {
    let meta = std::fs::metadata(path).ok()?;
    Some((meta.modified().ok()?, meta.len()))
}

/// The local files the schemas of one file reach through `$ref`, with what each looked like when
/// the closure was built.
struct Closure {
    seen: BTreeSet<PathBuf>,
    unresolved: BTreeSet<String>,
    /// Every file read to build the closure, and its stamp then.
    read: Vec<(PathBuf, Stamp)>,
    /// References that did not resolve, with the directory they were relative to.
    missing: Vec<(String, PathBuf)>,
}

impl Closure {
    /// Whether nothing the closure was built from has changed: each file read still has its stamp
    /// and each missing reference is still missing.
    fn is_current(&self, scope: &Scope) -> bool {
        self.read.iter().all(|(path, was)| stamp(path) == *was)
            && self
                .missing
                .iter()
                .all(|(reference, base)| scope.locate(reference, base).is_err())
    }
}

type ClosureKey = (BTreeSet<PathBuf>, PathBuf, PathBuf);

/// The closure of `seeds` under `$ref`, remembered while every file it read is unchanged, so a
/// run linting many files against one schema reads that schema once.
fn ref_closure(scope: &Scope, seeds: BTreeSet<PathBuf>) -> Arc<Closure> {
    static MEMO: OnceLock<Mutex<HashMap<ClosureKey, Arc<Closure>>>> = OnceLock::new();
    let memo = MEMO.get_or_init(Mutex::default);
    let key = (seeds, scope.root.clone(), scope.cache_dir.clone());
    let cached = memo
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get(&key)
        .cloned();
    if let Some(closure) = cached
        && closure.is_current(scope)
    {
        return closure;
    }
    let built = Arc::new(build_closure(scope, key.0.clone()));
    if built.read.iter().all(|(_, was)| was.is_some()) {
        let mut memo = memo.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if memo.len() >= MAX_CLOSURES {
            memo.clear();
        }
        memo.insert(key, Arc::clone(&built));
    }
    built
}

fn build_closure(scope: &Scope, mut seen: BTreeSet<PathBuf>) -> Closure {
    let mut unresolved = BTreeSet::new();
    let mut read = Vec::new();
    let mut missing = Vec::new();
    let mut queue: Vec<PathBuf> = seen.iter().cloned().collect();
    while let Some(file) = queue.pop() {
        if seen.len() >= MAX_CLOSURE {
            break;
        }
        let was = stamp(&file);
        let text = std::fs::read_to_string(&file);
        read.push((file.clone(), was));
        let Ok(text) = text else {
            continue;
        };
        let base = file.parent().unwrap_or(&scope.root).to_path_buf();
        let mut refs = quoted_values_after(&text, "$ref");
        if is_yaml_file(&file) {
            refs.extend(yaml_values_after(&text, "$ref"));
        }
        for reference in refs {
            if reference.starts_with('#') {
                continue;
            }
            let before = seen.clone();
            if !note(scope, &reference, &base, &mut seen, &mut unresolved) {
                missing.push((reference, base.clone()));
            }
            queue.extend(seen.difference(&before).cloned());
        }
    }
    Closure {
        seen,
        unresolved,
        read,
        missing,
    }
}

/// Digest of one file's bytes, memoized on its modification time and length so a warm run over
/// many files does not re-read a large schema for each.
fn file_digest(path: &Path) -> blake3::Hash {
    type Stamped = (Option<SystemTime>, u64, blake3::Hash);
    static MEMO: OnceLock<Mutex<HashMap<PathBuf, Stamped>>> = OnceLock::new();
    let stamp = std::fs::metadata(path)
        .map(|meta| (meta.modified().ok(), meta.len()))
        .ok();
    let memo = MEMO.get_or_init(Mutex::default);
    if let Some((modified, len)) = stamp
        && let Some(&(seen_modified, seen_len, hash)) =
            memo.lock().unwrap_or_else(std::sync::PoisonError::into_inner).get(path)
        && seen_modified == modified
        && seen_len == len
        && modified.is_some()
    {
        return hash;
    }
    let hash = match std::fs::read(path) {
        Ok(bytes) => blake3::hash(&bytes),
        Err(_) => blake3::hash(b"unreadable"),
    };
    if let Some((modified, len)) = stamp {
        memo.lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(path.to_path_buf(), (modified, len, hash));
    }
    hash
}

/// Every string value that follows a `"<key>"` member name in `text`: a textual scan, so it covers
/// JSON and JSONC without parsing them and over-approximates (a nested occurrence counts too).
fn quoted_values_after(text: &str, key: &str) -> Vec<String> {
    let needle = format!("\"{key}\"");
    let mut values = Vec::new();
    let mut from = 0;
    while let Some(at) = text[from..].find(&needle) {
        let start = from + at + needle.len();
        from = start;
        let rest = text[start..].trim_start();
        let Some(rest) = rest.strip_prefix(':') else {
            continue;
        };
        let Some(rest) = rest.trim_start().strip_prefix('"') else {
            continue;
        };
        let mut value = String::new();
        let mut chars = rest.chars();
        while let Some(ch) = chars.next() {
            match ch {
                '"' | '\n' => break,
                '\\' => {
                    if let Some(escaped) = chars.next() {
                        value.push(escaped);
                    }
                }
                other => value.push(other),
            }
        }
        values.push(value);
    }
    values
}

fn is_yaml_file(path: &Path) -> bool {
    path.extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("yaml") || ext.eq_ignore_ascii_case("yml"))
}

/// Every value written as `<key>: value` (plain, or single- or double-quoted) in YAML `text`, as a
/// block entry or inside a one-line flow mapping (`{ $ref: x }`, `{"$ref": "x"}`): a line scan that
/// over-approximates, like [`quoted_values_after`].
fn yaml_values_after(text: &str, key: &str) -> Vec<String> {
    let mut found = Vec::new();
    for line in text.lines() {
        let block = line.trim_start().trim_start_matches("- ");
        if let Some(value) = value_after_key(block, key) {
            found.push(unquote(value.split(" #").next().unwrap_or_default()));
        }
        for (at, _) in line.match_indices(key) {
            let head = line[..at].trim_end_matches(['"', '\'']).trim_end();
            if !head.ends_with(['{', ',']) {
                continue;
            }
            if let Some(value) = value_after_key(&line[at..], key) {
                let value = value.split(['}', ',']).next().unwrap_or_default();
                found.push(unquote(value));
            }
        }
    }
    found.retain(|value| !value.is_empty());
    found
}

/// The text after `key:` when `line` starts with the key (bare or quoted), else `None`.
fn value_after_key<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    let rest = line
        .strip_prefix(key)
        .or_else(|| {
            line.strip_prefix('"')
                .or_else(|| line.strip_prefix('\''))
                .and_then(|quoted| quoted.strip_prefix(key))
        })
        .map(|after| after.trim_start_matches(['"', '\'']))?;
    Some(rest.trim_start().strip_prefix(':')?.trim())
}

fn unquote(value: &str) -> String {
    value.trim().trim_matches(['"', '\'']).to_owned()
}

/// The scheme of an absolute URI-looking `reference` (`https` in `https://x`), if it has one.
#[must_use]
pub fn scheme_of(reference: &str) -> Option<&str> {
    let (scheme, _) = reference.split_once("://")?;
    let valid = !scheme.is_empty()
        && scheme.starts_with(|ch: char| ch.is_ascii_alphabetic())
        && scheme
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '+' | '-' | '.'));
    valid.then_some(scheme)
}

/// The `file://` URI of an absolute path.
#[must_use]
pub fn file_uri(path: &Path) -> String {
    let text = path.to_string_lossy().replace('\\', "/");
    let mut uri = String::from("file://");
    if !text.starts_with('/') {
        uri.push('/');
    }
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~' | b'/' | b':') {
            uri.push(char::from(byte));
        } else {
            uri.push_str(&format!("%{byte:02X}"));
        }
    }
    uri
}

/// The path a `file://` URI names.
fn uri_to_path(uri: &str) -> Result<PathBuf, String> {
    let rest = uri
        .strip_prefix("file://")
        .ok_or_else(|| format!("`{uri}` is not a file URI"))?;
    if !rest.starts_with('/') {
        return Err(format!("`{uri}` names a remote host"));
    }
    let mut bytes = Vec::with_capacity(rest.len());
    let raw = rest.as_bytes();
    let mut index = 0;
    while index < raw.len() {
        if raw[index] == b'%'
            && let Some(hex) = rest.get(index + 1..index + 3)
            && let Ok(byte) = u8::from_str_radix(hex, 16)
        {
            bytes.push(byte);
            index += 3;
        } else {
            bytes.push(raw[index]);
            index += 1;
        }
    }
    let decoded = String::from_utf8(bytes).map_err(|_| format!("`{uri}` is not valid UTF-8"))?;
    let drive = decoded.as_bytes();
    let path = if drive.len() >= 3 && drive[0] == b'/' && drive[1].is_ascii_alphabetic() && drive[2] == b':' {
        decoded[1..].to_owned()
    } else {
        decoded
    };
    Ok(PathBuf::from(path))
}

/// `std::fs::canonicalize` without the Windows verbatim prefix, so paths compare and print as the
/// user wrote them.
fn canonical(path: &Path) -> std::io::Result<PathBuf> {
    let resolved = std::fs::canonicalize(path)?;
    let text = resolved.to_string_lossy();
    Ok(match text.strip_prefix(r"\\?\") {
        Some(plain) => PathBuf::from(plain),
        None => resolved,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::GlobalDefaults;

    fn cfg(options: &str) -> EngineConfig {
        EngineConfig {
            globals: GlobalDefaults::default(),
            indent_width: 2,
            options: toml::from_str(options).expect("options parse"),
        }
    }

    fn project() -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("poly.toml"), "").expect("poly.toml");
        dir
    }

    #[test]
    fn directive_ref_ignores_other_comments_naming_a_schema() {
        assert_eq!(
            directive_ref(
                "# see $schema=./a.json for details
k: v
"
            ),
            None
        );
        assert_eq!(
            directive_ref(
                "#:schemas ./a.json
k = 1
"
            ),
            None
        );
        assert_eq!(
            directive_ref(
                "# note
# yaml-language-server: $schema=./a.json
"
            )
            .as_deref(),
            Some("./a.json")
        );
    }

    #[test]
    fn yaml_schema_refs_are_scanned() {
        let text = "a:\n  $ref: ./x.yaml#/d\nb:\n  - \"$ref\": 'y.yaml'\nc: {$refs: z}\n";
        assert_eq!(yaml_values_after(text, "$ref"), vec!["./x.yaml#/d", "y.yaml"]);
    }

    #[test]
    fn yaml_flow_mapping_refs_are_scanned() {
        let text = "a: { $ref: ./x.yaml }\nb: [{\"$ref\": \"y.yaml\"}, {$ref: z.yaml, x: 1}]\n- - $ref: n.yaml\nc: {$refs: w}\n";
        assert_eq!(
            yaml_values_after(text, "$ref"),
            vec!["./x.yaml", "y.yaml", "z.yaml", "n.yaml"]
        );
    }

    #[test]
    fn salt_follows_refs_of_a_yaml_schema() {
        let dir = project();
        std::fs::write(dir.path().join("s.yaml"), "properties:\n  a:\n    $ref: part.yaml\n").expect("s");
        std::fs::write(dir.path().join("part.yaml"), "type: string\n").expect("part");
        std::fs::write(
            dir.path().join("f.yaml"),
            "# yaml-language-server: $schema=s.yaml\na: x\n",
        )
        .expect("f");
        let before = salt(&cfg(""), &dir.path().join("f.yaml")).expect("salt");
        std::fs::write(dir.path().join("part.yaml"), "type: integer\n").expect("part");
        let after = salt(&cfg(""), &dir.path().join("f.yaml")).expect("salt");
        assert_ne!(before, after);
    }

    #[test]
    fn directive_ref_reads_both_comment_forms_and_stops_at_content() {
        assert_eq!(directive_ref("#:schema ./a.json\nx = 1\n").as_deref(), Some("./a.json"));
        assert_eq!(
            directive_ref("# yaml-language-server: $schema=../s.json extra\nk: v\n").as_deref(),
            Some("../s.json")
        );
        assert_eq!(directive_ref("x = 1\n#:schema ./late.json\n"), None);
    }

    #[test]
    fn metaschema_urls_are_not_references() {
        assert!(is_metaschema("https://json-schema.org/draft/2020-12/schema"));
        assert!(is_metaschema("http://json-schema.org/draft-07/schema#"));
        assert!(!is_metaschema("https://example.com/schema.json"));
        assert!(!is_metaschema("schemas/json-schema.org/x.json"));
    }

    #[test]
    fn remote_schemas_map_into_the_cache_by_host_and_path() {
        let dir = project();
        let scope = Scope::for_file(&dir.path().join("a.json"), &cfg(""));
        let path = scope
            .cache_path("https://example.com/schemas/x.json?v=1#frag")
            .expect("path");
        assert_eq!(path, scope.cache_dir.join("example.com").join("schemas").join("x.json"));
        assert!(scope.cache_path("https://example.com/../x.json").is_err());
        assert!(scope.cache_path("https://example.com/").is_err());
    }

    #[test]
    fn a_cached_remote_schema_resolves_and_an_uncached_one_is_unavailable() {
        let dir = project();
        let file = dir.path().join("doc.json");
        std::fs::write(&file, "{}").expect("doc");
        let url = "https://example.com/s.json";
        match resolve(&file, Some(url), &cfg("")) {
            SchemaSource::Unavailable { reference, reason } => {
                assert_eq!(reference, url);
                assert!(reason.contains("not in the local schema cache"), "{reason}");
            }
            other => panic!("expected Unavailable, got {other:?}"),
        }
        let cached = dir.path().join(".poly/schemas/example.com/s.json");
        std::fs::create_dir_all(cached.parent().unwrap()).expect("cache dir");
        std::fs::write(&cached, "{}").expect("cached");
        assert_eq!(
            resolve(&file, Some(url), &cfg("")),
            SchemaSource::Local(canonical(&cached).unwrap())
        );
    }

    #[test]
    fn a_schema_outside_the_project_root_is_unavailable() {
        let dir = project();
        let outside = tempfile::tempdir().expect("outside");
        let schema = outside.path().join("s.json");
        std::fs::write(&schema, "{}").expect("schema");
        let file = dir.path().join("doc.json");
        std::fs::write(&file, "{}").expect("doc");
        let source = resolve(&file, Some(&schema.to_string_lossy()), &cfg(""));
        assert!(
            matches!(&source, SchemaSource::Unavailable { reason, .. } if reason.contains("outside the project root")),
            "{source:?}"
        );
    }

    #[test]
    fn the_first_matching_mapping_names_the_schema() {
        let dir = project();
        std::fs::create_dir_all(dir.path().join("cfg")).expect("cfg dir");
        std::fs::write(dir.path().join("a.json"), "{}").expect("a");
        std::fs::write(dir.path().join("b.json"), "{}").expect("b");
        let file = dir.path().join("cfg/app.yaml");
        std::fs::write(&file, "k: v\n").expect("yaml");
        let options = r#"schemas = [
            { glob = "cfg/*.yaml", schema = "a.json" },
            { glob = "**/*.yaml", schema = "b.json" },
        ]"#;
        assert_eq!(
            resolve(&file, None, &cfg(options)),
            SchemaSource::Local(canonical(&dir.path().join("a.json")).unwrap())
        );
        assert_eq!(
            resolve(&dir.path().join("other.txt"), None, &cfg(options)),
            SchemaSource::None
        );
    }

    #[test]
    fn the_salt_follows_the_schema_and_the_files_it_refs() {
        let dir = project();
        let main = dir.path().join("main.json");
        let part = dir.path().join("part.json");
        std::fs::write(&main, r#"{"$ref": "part.json"}"#).expect("main");
        std::fs::write(&part, r#"{"type": "object"}"#).expect("part");
        let doc = dir.path().join("doc.yaml");
        std::fs::write(&doc, "#:schema main.json\nk: v\n").expect("doc");
        let first = salt(&cfg(""), &doc).expect("salt");
        assert_eq!(salt(&cfg(""), &doc).as_deref(), Some(first.as_str()));
        std::fs::write(&part, r#"{"type": "array", "minItems": 1}"#).expect("edit part");
        let second = salt(&cfg(""), &doc).expect("salt");
        assert_ne!(first, second, "editing a $ref'd file must change the salt");
        assert_eq!(salt(&cfg(""), &dir.path().join("plain.yaml")), None);
    }

    #[test]
    fn the_scope_root_matches_an_uncached_walk_and_follows_a_forget() {
        let dir = project();
        let nested = dir.path().join("a/b/c");
        std::fs::create_dir_all(&nested).expect("nested");
        let file = nested.join("doc.json");
        std::fs::write(&file, "{}").expect("doc");
        let canonical_file = canonical(&file).expect("canonical");
        let uncached = find_root(&canonical_file).expect("root");
        assert_eq!(Scope::for_file(&file, &cfg("")).root, uncached);
        assert_eq!(Scope::for_file(&file, &cfg("")).root, uncached);
        assert_eq!(uncached, canonical(dir.path()).expect("canonical root"));

        std::fs::write(dir.path().join("a/b/poly.toml"), "").expect("inner poly.toml");
        forget();
        let inner = canonical(&dir.path().join("a/b")).expect("canonical inner");
        assert_eq!(Scope::for_file(&file, &cfg("")).root, inner);
    }

    #[test]
    fn compiled_mappings_match_the_configured_globs_and_skip_bad_ones() {
        let dir = project();
        let options = r#"schemas = [
            { glob = "[", schema = "bad.json" },
            { glob = "**/*.yaml", schema = "good.json" },
        ]"#;
        let scope = Scope::for_file(&dir.path().join("x/app.yaml"), &cfg(options));
        assert_eq!(scope.mapped(&cfg(options)).as_deref(), Some("good.json"));
        assert_eq!(scope.mapped(&cfg(options)).as_deref(), Some("good.json"));
        assert_eq!(scope.mapped(&cfg("")), None);
    }

    fn bump(path: &Path, seconds: u64) {
        let file = std::fs::OpenOptions::new().write(true).open(path).expect("open");
        let modified = std::fs::metadata(path).expect("meta").modified().expect("mtime");
        file.set_modified(modified + std::time::Duration::from_secs(seconds))
            .expect("set mtime");
    }

    #[test]
    fn the_ref_closure_memo_is_invalidated_when_a_referenced_file_changes() {
        let dir = project();
        let main = dir.path().join("main.json");
        let part = dir.path().join("part.json");
        let extra = dir.path().join("extra.json");
        std::fs::write(&main, r#"{"$ref": "part.json"}"#).expect("main");
        std::fs::write(&part, r#"{"type": "object"}"#).expect("part");
        std::fs::write(&extra, r#"{"type": "string"}"#).expect("extra");
        let doc = dir.path().join("doc.yaml");
        std::fs::write(&doc, "#:schema main.json\nk: v\n").expect("doc");
        let first = salt(&cfg(""), &doc).expect("salt");
        assert_eq!(salt(&cfg(""), &doc).as_deref(), Some(first.as_str()));

        // Same length, so only the modification time tells the memos the file changed.
        std::fs::write(&part, r#"{"type": "string"}"#).expect("edit part");
        bump(&part, 5);
        let second = salt(&cfg(""), &doc).expect("salt");
        assert_ne!(first, second);

        std::fs::write(&part, r#"{"$ref": "extra.json"}"#).expect("part refs extra");
        bump(&part, 10);
        let third = salt(&cfg(""), &doc).expect("salt");
        assert_ne!(second, third);
        std::fs::write(&extra, r#"{"type": "number"}"#).expect("edit extra");
        bump(&extra, 15);
        assert_ne!(
            third,
            salt(&cfg(""), &doc).expect("salt"),
            "a file only reached through a $ref counts"
        );
    }

    #[test]
    fn a_ref_that_was_missing_invalidates_the_memo_when_it_appears() {
        let dir = project();
        std::fs::write(dir.path().join("main.json"), r#"{"$ref": "later.json"}"#).expect("main");
        let doc = dir.path().join("doc.yaml");
        std::fs::write(&doc, "#:schema main.json\nk: v\n").expect("doc");
        let before = salt(&cfg(""), &doc).expect("salt");
        assert_eq!(salt(&cfg(""), &doc).as_deref(), Some(before.as_str()));
        std::fs::write(dir.path().join("later.json"), "{}").expect("later");
        assert_ne!(before, salt(&cfg(""), &doc).expect("salt"));
    }

    #[test]
    fn file_uris_round_trip() {
        let path = Path::new("/tmp/a b/c#d.json");
        let uri = file_uri(path);
        assert_eq!(uri, "file:///tmp/a%20b/c%23d.json");
        assert_eq!(uri_to_path(&uri).expect("path"), path);
        assert_eq!(
            uri_to_path("file:///C:/x/y.json").expect("drive"),
            Path::new("C:/x/y.json")
        );
        assert!(uri_to_path("file://host/x").is_err());
    }
}
