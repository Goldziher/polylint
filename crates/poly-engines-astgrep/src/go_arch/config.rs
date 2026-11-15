//! Parsing and compiling the Go architecture settings out of the engine's options table.

use std::collections::HashSet;

use anyhow::{Context, bail};
use globset::{Glob, GlobBuilder, GlobMatcher, GlobSet, GlobSetBuilder};
use poly_engine_api::engine::Severity;
use serde::Deserialize;

/// A `go_forbidden_calls` entry as written.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawCall {
    call: String,
    #[serde(default)]
    paths: Vec<String>,
    #[serde(default)]
    allow_in: Vec<String>,
    message: Option<String>,
    level: Option<String>,
    #[serde(default)]
    include_tests: bool,
}

/// A `go_forbidden_imports` entry as written.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawImport {
    import: String,
    #[serde(default)]
    paths: Vec<String>,
    #[serde(default)]
    allow_in: Vec<String>,
    message: Option<String>,
    level: Option<String>,
    #[serde(default)]
    include_tests: bool,
}

/// A `go_layers` entry as written.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawLayer {
    name: String,
    paths: Vec<String>,
    #[serde(default)]
    may_import: Vec<String>,
    message: Option<String>,
    level: Option<String>,
    #[serde(default)]
    include_tests: bool,
}

/// The files a rule speaks about: inside `paths` (everywhere when empty), outside `allow_in`, and
/// `_test.go` files only when asked for.
#[derive(Debug)]
pub(super) struct Scope {
    paths: Option<GlobSet>,
    allow_in: Option<GlobSet>,
    include_tests: bool,
}

impl Scope {
    fn new(key: &str, paths: &[String], allow_in: &[String], include_tests: bool) -> anyhow::Result<Self> {
        Ok(Self {
            paths: glob_set(&format!("{key}.paths"), paths)?,
            allow_in: glob_set(&format!("{key}.allow_in"), allow_in)?,
            include_tests,
        })
    }

    /// Whether the rule applies to the file at module-relative path `rel`.
    pub(super) fn applies(&self, rel: &str) -> bool {
        if !self.include_tests && rel.ends_with("_test.go") {
            return false;
        }
        if self.paths.as_ref().is_some_and(|paths| !paths.is_match(rel)) {
            return false;
        }
        !self.allow_in.as_ref().is_some_and(|allowed| allowed.is_match(rel))
    }
}

/// A forbidden `<import path>.<Name>` selector.
#[derive(Debug)]
pub(super) struct CallRule {
    pub(super) call: String,
    pub(super) package: GlobMatcher,
    pub(super) name: GlobMatcher,
    pub(super) scope: Scope,
    pub(super) message: Option<String>,
    pub(super) level: Option<Severity>,
}

/// A forbidden import path.
#[derive(Debug)]
pub(super) struct ImportRule {
    pub(super) import: GlobMatcher,
    pub(super) pattern: String,
    pub(super) scope: Scope,
    pub(super) message: Option<String>,
    pub(super) level: Option<Severity>,
}

/// A layer of the module: the directories it covers and the layers it may import.
#[derive(Debug)]
pub(super) struct Layer {
    pub(super) name: String,
    pub(super) dirs: GlobSet,
    pub(super) may_import: HashSet<String>,
    pub(super) include_tests: bool,
    pub(super) message: Option<String>,
    pub(super) level: Option<Severity>,
}

/// The compiled Go architecture settings of one config.
#[derive(Debug)]
pub(crate) struct GoArch {
    pub(super) calls: Vec<CallRule>,
    pub(super) imports: Vec<ImportRule>,
    pub(super) layers: Vec<Layer>,
}

impl GoArch {
    /// Compile the three option arrays; `None` when none of them holds an entry.
    ///
    /// # Errors
    /// An entry that is not a table, has an unknown or missing field, carries an invalid glob or
    /// level, or (for a layer) names a layer that does not exist. The message names the key and
    /// the entry's index.
    pub(crate) fn from_values(
        calls: Option<&Vec<toml::Value>>,
        imports: Option<&Vec<toml::Value>>,
        layers: Option<&Vec<toml::Value>>,
    ) -> anyhow::Result<Option<Self>> {
        let arch = Self {
            calls: entries("go_forbidden_calls", calls, compile_call)?,
            imports: entries("go_forbidden_imports", imports, compile_import)?,
            layers: entries("go_layers", layers, compile_layer)?,
        };
        arch.validate_layers()?;
        let empty = arch.calls.is_empty() && arch.imports.is_empty() && arch.layers.is_empty();
        Ok((!empty).then_some(arch))
    }

    fn validate_layers(&self) -> anyhow::Result<()> {
        let mut names = HashSet::new();
        for layer in &self.layers {
            if !names.insert(layer.name.as_str()) {
                bail!("go_layers: layer `{}` is declared twice", layer.name);
            }
        }
        for layer in &self.layers {
            if let Some(unknown) = layer.may_import.iter().find(|name| !names.contains(name.as_str())) {
                bail!(
                    "go_layers: layer `{}` may_import `{unknown}`, which is not a declared layer",
                    layer.name
                );
            }
        }
        Ok(())
    }
}

/// Deserialize and compile each entry of `values`, prefixing a failure with `key[index]`.
fn entries<Raw, Out>(
    key: &str,
    values: Option<&Vec<toml::Value>>,
    compile: impl Fn(&str, Raw) -> anyhow::Result<Out>,
) -> anyhow::Result<Vec<Out>>
where
    Raw: serde::de::DeserializeOwned,
{
    let Some(values) = values else {
        return Ok(Vec::new());
    };
    values
        .iter()
        .enumerate()
        .map(|(index, value)| {
            let at = format!("{key}[{index}]");
            if !value.is_table() {
                bail!("{at}: expected a table, found {}", value.type_str());
            }
            let raw: Raw = value
                .clone()
                .try_into()
                .map_err(|e: toml::de::Error| anyhow::anyhow!("{at}: {}", e.to_string().trim_end()))?;
            compile(&at, raw)
        })
        .collect()
}

fn compile_call(at: &str, raw: RawCall) -> anyhow::Result<CallRule> {
    let (package, name) = raw
        .call
        .rsplit_once('.')
        .filter(|(package, name)| !package.is_empty() && !name.is_empty())
        .with_context(|| {
            format!(
                "{at}: `call` must be `<import path>.<Name>` (such as `os.Exit`), got `{}`",
                raw.call
            )
        })?;
    Ok(CallRule {
        package: matcher(&format!("{at}.call"), package)?,
        name: matcher(&format!("{at}.call"), name)?,
        scope: Scope::new(at, &raw.paths, &raw.allow_in, raw.include_tests)?,
        level: level(at, raw.level.as_deref())?,
        message: raw.message,
        call: raw.call,
    })
}

fn compile_import(at: &str, raw: RawImport) -> anyhow::Result<ImportRule> {
    Ok(ImportRule {
        import: matcher(&format!("{at}.import"), &raw.import)?,
        scope: Scope::new(at, &raw.paths, &raw.allow_in, raw.include_tests)?,
        level: level(at, raw.level.as_deref())?,
        message: raw.message,
        pattern: raw.import,
    })
}

fn compile_layer(at: &str, raw: RawLayer) -> anyhow::Result<Layer> {
    if raw.name.trim().is_empty() {
        bail!("{at}: `name` must not be empty");
    }
    let Some(dirs) = glob_set(&format!("{at}.paths"), &raw.paths)? else {
        bail!("{at}: layer `{}` needs at least one entry in `paths`", raw.name);
    };
    Ok(Layer {
        level: level(at, raw.level.as_deref())?,
        name: raw.name,
        dirs,
        may_import: raw.may_import.into_iter().collect(),
        include_tests: raw.include_tests,
        message: raw.message,
    })
}

/// A glob where `*` stays within one path element and `**` crosses elements.
fn glob(at: &str, pattern: &str) -> anyhow::Result<Glob> {
    GlobBuilder::new(pattern)
        .literal_separator(true)
        .build()
        .with_context(|| format!("{at}: invalid glob `{pattern}`"))
}

fn matcher(at: &str, pattern: &str) -> anyhow::Result<GlobMatcher> {
    Ok(glob(at, pattern)?.compile_matcher())
}

/// The globs as one set; `None` for no patterns. A trailing `/**` also matches the directory it
/// names, so `internal/domain/**` covers `internal/domain` itself.
fn glob_set(at: &str, patterns: &[String]) -> anyhow::Result<Option<GlobSet>> {
    if patterns.is_empty() {
        return Ok(None);
    }
    let mut builder = GlobSetBuilder::new();
    for pattern in patterns {
        builder.add(glob(at, pattern)?);
        if let Some(directory) = pattern.strip_suffix("/**").filter(|directory| !directory.is_empty()) {
            builder.add(glob(at, directory)?);
        }
    }
    builder
        .build()
        .map(Some)
        .with_context(|| format!("{at}: invalid glob set"))
}

fn level(at: &str, level: Option<&str>) -> anyhow::Result<Option<Severity>> {
    Ok(match level {
        None => None,
        Some("error") => Some(Severity::Error),
        Some("warning" | "warn") => Some(Severity::Warning),
        Some("info" | "information") => Some(Severity::Info),
        Some("hint" | "help") => Some(Severity::Hint),
        Some(other) => bail!("{at}: unknown level `{other}` (expected error, warning, info or hint)"),
    })
}
