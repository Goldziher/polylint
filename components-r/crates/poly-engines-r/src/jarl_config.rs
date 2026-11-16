//! The configuration jarl lints a file with: the project's `jarl.toml`, then `[engines.jarl]`.
//!
//! jarl reads its settings from the nearest `jarl.toml` (or `.jarl.toml`) above the file. The options of
//! `[engines.jarl]` are laid over it as `jarl check --config` lays its overrides: `select`, `extend_select` and
//! `ignore` replace the file's lists, `check_roxygen` its switch, the keys of `rules.<name>` beside `level` are that
//! rule's `[lint.<name>]` options, and `min_r_version` plays the part of `--min-r-version`. Per-file ignores stay in
//! `jarl.toml`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::SystemTime;

use anyhow::Context;
use jarl_core::config::{ArgsConfig, Config, build_config};
use jarl_core::toml::{find_jarl_toml_in_directory, parse_config_overrides, parse_jarl_toml};
use poly_engine_api::config::EngineConfig;
use poly_engine_api::engine::OptionType;
use poly_engine_api::options::OptionsExt;
use poly_engine_api::rule_config::RuleSelection;

/// The keys of `[engines.jarl]` beside the shared rule selection (`select`, `extend_select`, `ignore`, `rules`).
pub(crate) const LINT_KEYS: &[(&str, OptionType)] = &[
    ("check_roxygen", OptionType::BOOLEAN),
    ("min_r_version", OptionType::STRING),
];

/// Absolute form of `path`, without resolving symbolic links.
pub(crate) fn absolute(path: &Path) -> PathBuf {
    std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf())
}

/// The nearest `jarl.toml` above `file`, if any.
pub(crate) fn find(file: &Path) -> anyhow::Result<Option<PathBuf>> {
    let file = absolute(file);
    for directory in file.ancestors().skip(1) {
        if let Some(found) = find_jarl_toml_in_directory(directory)? {
            return Ok(Some(found));
        }
    }
    Ok(None)
}

/// A file's identity for the config cache: its modification time and length.
fn stamp(path: &Path) -> Option<(SystemTime, u64)> {
    let metadata = std::fs::metadata(path).ok()?;
    Some((metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH), metadata.len()))
}

/// What a resolved config depends on: the file it came from, that file's identity, and the poly options.
type CacheKey = (Option<PathBuf>, Option<(SystemTime, u64)>, String);

/// The jarl config governing `file` under the options of `cfg`.
///
/// Resolving a config compiles every rule's options, so it is done once per `jarl.toml` and options table, not
/// once per file.
pub(crate) fn config_for(file: &Path, cfg: &EngineConfig) -> anyhow::Result<Arc<Config>> {
    static CACHE: OnceLock<Mutex<HashMap<CacheKey, Arc<Config>>>> = OnceLock::new();
    let toml_path = find(file)?;
    let options = toml::to_string(&cfg.options).unwrap_or_default();
    let key: CacheKey = (toml_path.clone(), toml_path.as_deref().and_then(stamp), options);
    let mut cache = CACHE
        .get_or_init(Mutex::default)
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(config) = cache.get(&key) {
        return Ok(Arc::clone(config));
    }
    let config = Arc::new(resolve(file, toml_path.as_deref(), cfg)?);
    cache.insert(key, Arc::clone(&config));
    Ok(config)
}

/// The `[lint]` overrides the poly options spell, in `jarl.toml`'s own vocabulary.
fn overrides(cfg: &EngineConfig) -> toml::Table {
    let selection = RuleSelection::from_options(cfg);
    let mut lint = toml::Table::new();
    for (key, codes) in [
        ("select", selection.select),
        ("extend-select", selection.extend_select),
        ("ignore", selection.ignore),
    ] {
        if !codes.is_empty() {
            lint.insert(
                key.into(),
                toml::Value::Array(codes.into_iter().map(toml::Value::String).collect()),
            );
        }
    }
    if let Some(check) = cfg.options.opt_bool("check_roxygen") {
        lint.insert("check-roxygen".into(), toml::Value::Boolean(check));
    }
    // `[engines.jarl.rules.<name>]` keys other than `level` are that rule's own options, `[lint.<name>]` in jarl.toml.
    for (rule, options) in selection.rules {
        if !options.params.is_empty() {
            lint.insert(rule, toml::Value::Table(options.params));
        }
    }
    let mut table = toml::Table::new();
    if !lint.is_empty() {
        table.insert("lint".into(), toml::Value::Table(lint));
    }
    table
}

fn resolve(file: &Path, toml_path: Option<&Path>, cfg: &EngineConfig) -> anyhow::Result<Config> {
    let overrides = [overrides(cfg)];
    let root = match toml_path.and_then(Path::parent) {
        Some(directory) => directory.to_path_buf(),
        None => absolute(file).parent().map(Path::to_path_buf).unwrap_or_default(),
    };
    let settings = match toml_path {
        Some(path) => parse_jarl_toml(path, &overrides)
            .map_err(|error| anyhow::anyhow!("{error}"))?
            .into_settings(&root)?,
        None => parse_config_overrides("", &overrides, &root)?,
    };
    let args = ArgsConfig {
        files: Vec::new(),
        fix: false,
        unsafe_fixes: false,
        fix_only: false,
        select: String::new(),
        extend_select: String::new(),
        ignore: String::new(),
        min_r_version: cfg.options.opt_str("min_r_version").map(str::to_owned),
        allow_dirty: true,
        allow_no_vcs: true,
    };
    build_config(&args, Some(&settings), Vec::new()).context("invalid jarl configuration")
}

/// A digest of the project `jarl.toml` governing `file`, for the cache key; `None` without one.
pub(crate) fn digest(file: &Path) -> Option<String> {
    let path = find(file).ok().flatten()?;
    let text = std::fs::read(&path).ok()?;
    let mut hasher = blake3::Hasher::new();
    hasher.update(path.to_string_lossy().as_bytes());
    hasher.update(&[0]);
    hasher.update(&text);
    Some(hasher.finalize().to_hex().to_string())
}
