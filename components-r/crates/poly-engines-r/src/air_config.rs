//! The configuration air formats a file with: the project's `air.toml` or poly's defaults, then `[engines.air]`.
//!
//! air reads its settings from the nearest `air.toml` (or `.air.toml`) above the file. A project that has one is
//! formatted exactly as `air format` formats it, so the table is taken as it is and poly's own `line_length` is not
//! laid under it: a key the file leaves out keeps air's default. A project without one gets poly's opinionated
//! defaults (the line width, the line ending). Keys set under `[engines.air]` are the last layer in both cases.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::SystemTime;

use air_workspace::settings::Settings;
use air_workspace::toml::find_air_toml;
use air_workspace::toml_options::TomlOptions;
use anyhow::Context;
use poly_engine_api::config::{EngineConfig, LineEnding};
use poly_engine_api::engine::OptionType;

/// The `[engines.air]` keys, each the snake_case spelling of the `[format]` key of `air.toml` it overrides.
pub(crate) const FORMAT_KEYS: &[(&str, OptionType)] = &[
    ("line_width", OptionType::INTEGER),
    ("indent_width", OptionType::INTEGER),
    ("indent_style", OptionType::STRING),
    ("line_ending", OptionType::STRING),
    ("persistent_line_breaks", OptionType::BOOLEAN),
    ("assignment_style", OptionType::STRING),
    ("skip", OptionType::ARRAY),
    ("table", OptionType::ARRAY),
    ("default_table", OptionType::BOOLEAN),
];

/// An `air.toml` found above a file, with its text.
#[derive(Debug, Clone)]
pub(crate) struct AirToml {
    /// Absolute path of the file.
    pub(crate) path: PathBuf,
    /// Its contents.
    pub(crate) text: Arc<str>,
}

impl AirToml {
    /// The directory the file's relative patterns resolve against.
    fn root(&self) -> Option<&Path> {
        self.path.parent()
    }
}

/// Absolute form of `path`, without resolving symbolic links.
fn absolute(path: &Path) -> PathBuf {
    std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf())
}

type TomlCache = Mutex<HashMap<PathBuf, ((SystemTime, u64), Arc<str>)>>;

/// The nearest `air.toml` above `file`, read once per change of the file (its modification time and length).
pub(crate) fn find(file: &Path) -> anyhow::Result<Option<AirToml>> {
    static CACHE: OnceLock<TomlCache> = OnceLock::new();
    let file = absolute(file);
    let Some(directory) = file.parent() else {
        return Ok(None);
    };
    let Some(path) = find_air_toml(directory) else {
        return Ok(None);
    };
    let metadata = std::fs::metadata(&path).with_context(|| format!("reading {}", path.display()))?;
    let stamp = (metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH), metadata.len());
    let mut cache = CACHE
        .get_or_init(Mutex::default)
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some((cached, text)) = cache.get(&path)
        && *cached == stamp
    {
        return Ok(Some(AirToml {
            path,
            text: Arc::clone(text),
        }));
    }
    let text: Arc<str> = std::fs::read_to_string(&path)
        .with_context(|| format!("reading {}", path.display()))?
        .into();
    cache.insert(path.clone(), (stamp, Arc::clone(&text)));
    Ok(Some(AirToml { path, text }))
}

/// The table the options of `cfg` and the project config resolve to, as `air.toml` would spell it.
fn layered_table(cfg: &EngineConfig, project: Option<&AirToml>) -> anyhow::Result<toml::Table> {
    let mut table = match project {
        Some(found) => found
            .text
            .parse::<toml::Table>()
            .with_context(|| format!("parsing {}", found.path.display()))?,
        None => defaults(cfg),
    };
    let overrides: Vec<(&str, &toml::Value)> = FORMAT_KEYS
        .iter()
        .filter_map(|(key, _)| cfg.options.get(*key).map(|value| (*key, value)))
        .collect();
    if overrides.is_empty() {
        return Ok(table);
    }
    let format = table
        .entry("format")
        .or_insert_with(|| toml::Value::Table(toml::Table::new()));
    let toml::Value::Table(format) = format else {
        anyhow::bail!("`format` in the air configuration is not a table");
    };
    for (key, value) in overrides {
        format.insert(key.replace('_', "-"), value.clone());
    }
    Ok(table)
}

/// poly's opinionated layer for a project without an `air.toml`: the line width and the line ending, and the indent
/// width the language fixes.
fn defaults(cfg: &EngineConfig) -> toml::Table {
    let mut format = toml::Table::new();
    format.insert(
        "line-width".into(),
        toml::Value::Integer(to_integer(cfg.globals.line_length)),
    );
    format.insert(
        "indent-width".into(),
        toml::Value::Integer(to_integer(cfg.indent_width)),
    );
    let ending = match cfg.globals.line_ending {
        LineEnding::Lf => "lf",
        LineEnding::Crlf => "crlf",
    };
    format.insert("line-ending".into(), toml::Value::String(ending.into()));
    let mut table = toml::Table::new();
    table.insert("format".into(), toml::Value::Table(format));
    table
}

fn to_integer(value: usize) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

/// air's resolved settings for a file, with the config file that supplied them.
pub(crate) fn resolve(cfg: &EngineConfig, project: Option<&AirToml>) -> anyhow::Result<Settings> {
    let table = layered_table(cfg, project)?;
    let origin = project.map_or_else(
        || "the air options".to_owned(),
        |found| found.path.display().to_string(),
    );
    let options: TomlOptions = toml::Value::Table(table)
        .try_into()
        .with_context(|| format!("invalid air configuration ({origin})"))?;
    options
        .into_settings(project.and_then(AirToml::root))
        .with_context(|| format!("invalid air configuration ({origin})"))
}

/// The settings of the project's own `air.toml` alone, which decide what air leaves out of a run.
pub(crate) fn project_settings(project: &AirToml) -> anyhow::Result<Settings> {
    let options: TomlOptions = toml::from_str(&project.text)
        .with_context(|| format!("invalid air configuration ({})", project.path.display()))?;
    options
        .into_settings(project.root())
        .with_context(|| format!("invalid air configuration ({})", project.path.display()))
}

/// Why air's `exclude` and `default-exclude` patterns leave `file` out of a run, if they do.
///
/// The patterns are air's own, matched against the absolute path like `air format` matches the paths it is given.
pub(crate) fn excluded_by(settings: &Settings, file: &Path) -> Option<String> {
    let file = absolute(file);
    let format = &settings.format;
    if let Some(patterns) = &format.exclude
        && let Some(glob) = patterns.matched_path_or_any_parents(&file, false)
    {
        return Some(glob.original().to_owned());
    }
    if let Some(patterns) = &format.default_exclude
        && let Some(glob) = patterns.matched_path_or_any_parents(&file, false)
    {
        return Some(glob.original().to_owned());
    }
    None
}

/// A digest of the project `air.toml` governing `file`, for the cache key; `None` without one.
pub(crate) fn digest(file: &Path) -> Option<String> {
    let found = find(file).ok().flatten()?;
    let mut hasher = blake3::Hasher::new();
    hasher.update(found.path.to_string_lossy().as_bytes());
    hasher.update(&[0]);
    hasher.update(found.text.as_bytes());
    Some(hasher.finalize().to_hex().to_string())
}
