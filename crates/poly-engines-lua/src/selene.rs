//! Lua linter backend, wrapping `selene-lib`.
//!
//! Lint-only: selene reports; it does not rewrite. A file selene cannot parse reports a `syntax` error, since
//! the formatter leaves such a file alone and something has to say why.
//!
//! # Configuration
//!
//! selene's own `selene.toml` is honoured: the one nearest the linted file (see
//! [`selene_config`](poly_engine_api::host_hooks::selene_config)) supplies `std`, `[lints]` (or `[rules]`) and
//! `[config]`. `[engines.selene]` takes the same keys and wins over the file, lint by lint:
//!
//! | Key | Meaning |
//! |-----|---------|
//! | `std` | Standard library, `+`-joined: `lua51` (default), `lua52`, `lua53`, `luau` |
//! | `lints` | Table of lint name to `allow`, `warn` or `deny` |
//! | `config` | Table of lint name to that lint's own settings |
//! | `ignore` | Lints to switch off (`allow`) |
//!
//! Only selene's built-in standard libraries are available. A `std` that names a library file (`roblox`, a
//! project's own `.yml`) is reported as an `unsupported-std` warning on each file rather than linted against
//! the wrong globals. `-- selene: allow(lint)` comments work as in selene.

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
use std::rc::Rc;

use poly_engine_api::config::EngineConfig;
use poly_engine_api::engine::{
    Capabilities, Diagnostic, Engine, OptionKeys, OptionTable, OptionType, Severity, SourceFile, Span,
};
use poly_engine_api::engine_guard::refuse_pathological_nesting;
use poly_engine_api::host_hooks::{HostHook, selene_config};
use poly_engine_api::language::Language;
use poly_engine_api::line_index::LineIndex;
use poly_engine_api::options::OptionsExt;
use selene_lib::standard_library::StandardLibrary;
use selene_lib::{Checker, CheckerConfig, LintVariation};

/// Cache-key version: the `selene-lib` crate version plus a marker for this backend's config mapping. Bump
/// when the crate is updated or the mapping changes.
const VERSION: &str = "selene-lib-0.31.0+lint1";

static LANGUAGES: &[Language] = &[Language::Lua];

const DEFAULT_STD: &str = "lua51";

/// What selene runs with for one file: the standard library, per-lint levels and per-lint settings.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct Settings {
    /// The standard library as written; `None` is selene's default, which it treats differently from `lua51`.
    std: Option<String>,
    lints: BTreeMap<String, String>,
    config: BTreeMap<String, String>,
}

impl Settings {
    /// Layers `selene.toml` under the options table.
    fn resolve(src: &SourceFile, cfg: &EngineConfig) -> Self {
        let mut std = None;
        let mut lints: BTreeMap<String, String> = BTreeMap::new();
        let mut config: BTreeMap<String, String> = BTreeMap::new();
        let mut absorb = |layer: &toml::Table, levels: Option<&toml::Table>| {
            if let Some(name) = layer.opt_str("std") {
                std = Some(name.to_owned());
            }
            if let Some(levels) = levels {
                lints.extend(levels.iter().map(|(name, level)| (name.clone(), level_text(level))));
            }
            if let Some(settings) = layer.opt_table("config") {
                config.extend(
                    settings
                        .iter()
                        .map(|(name, value)| (name.clone(), serde_json::to_string(value).unwrap_or_default())),
                );
            }
        };
        if let Some(found) = selene_config::governing(&src.path).filter(|found| found.parse_error.is_none()) {
            // `[rules]` is selene's older spelling of `[lints]`.
            let table = &found.table;
            absorb(table, table.opt_table("lints").or_else(|| table.opt_table("rules")));
        }
        absorb(&cfg.options, cfg.options.opt_table("lints"));
        for name in cfg.options.opt_string_list("ignore") {
            lints.insert(name, "allow".to_owned());
        }
        Self { std, lints, config }
    }
}

/// A lint level as the text it is parsed back from: a string as is, anything else as its TOML form so the
/// error names what the user wrote.
fn level_text(level: &toml::Value) -> String {
    level.as_str().map_or_else(|| level.to_string(), str::to_owned)
}

/// A checker built for one [`Settings`], with the Lua version its standard library parses.
struct Prepared {
    checker: Checker<serde_json::Value>,
    lua_version: full_moon_2::LuaVersion,
}

thread_local! {
    /// Building a checker parses the standard library, so each worker thread keeps the ones it has built.
    static PREPARED: RefCell<HashMap<Settings, Result<Rc<Prepared>, String>>> = RefCell::new(HashMap::new());
}

fn standard_library(spec: &str) -> Result<StandardLibrary, String> {
    let mut names = spec.split('+').map(str::trim);
    let load = |name: &str| {
        StandardLibrary::from_name(name)
            .ok_or_else(|| format!("selene std `{name}` is not built in; only lua51, lua52, lua53 and luau are"))
    };
    let mut library = load(names.next().unwrap_or(DEFAULT_STD))?;
    for name in names {
        library.extend(load(name)?);
    }
    Ok(library)
}

fn prepare(settings: &Settings) -> Result<Prepared, String> {
    let library = standard_library(settings.std.as_deref().unwrap_or(DEFAULT_STD))?;
    let mut lints = HashMap::new();
    for (name, level) in &settings.lints {
        let level = match level.as_str() {
            "allow" => LintVariation::Allow,
            "warn" => LintVariation::Warn,
            "deny" => LintVariation::Deny,
            other => {
                return Err(format!(
                    "selene lint `{name}` is set to `{other}`; expected allow, warn or deny"
                ));
            }
        };
        lints.insert(name.clone(), level);
    }
    let mut config = HashMap::new();
    for (name, json) in &settings.config {
        let value: serde_json::Value =
            serde_json::from_str(json).map_err(|error| format!("selene config `{name}`: {error}"))?;
        config.insert(name.clone(), value);
    }
    let (lua_version, _) = library.lua_version();
    let checker = Checker::new(
        CheckerConfig {
            config,
            lints,
            std: settings.std.clone(),
            ..CheckerConfig::default()
        },
        library,
    )
    .map_err(|error| error.to_string())?;
    Ok(Prepared { checker, lua_version })
}

fn prepared(settings: &Settings) -> Result<Rc<Prepared>, String> {
    PREPARED.with(|cache| {
        if let Some(hit) = cache.borrow().get(settings) {
            return hit.clone();
        }
        let built = prepare(settings).map(Rc::new);
        cache.borrow_mut().insert(settings.clone(), built.clone());
        built
    })
}

fn diagnostic(code: &str, severity: Severity, title: String, span: Option<Span>) -> Diagnostic {
    Diagnostic {
        engine: "selene".to_owned(),
        code: Some(code.to_owned()),
        severity,
        title,
        description: None,
        span,
        url: None,
        fix: Vec::new(),
        metadata: Default::default(),
    }
}

fn span(index: &LineIndex<'_>, start: usize, end: usize) -> Span {
    let (start_line, start_col) = index.line_col(start);
    let (end_line, end_col) = index.line_col(end.max(start));
    Span {
        start_line,
        start_col,
        end_line,
        end_col,
    }
}

/// selene Lua linter backend. See the module docs.
pub struct SeleneEngine;

impl Engine for SeleneEngine {
    fn overridden_hooks(&self) -> u32 {
        0
    }

    fn name(&self) -> &'static str {
        "selene"
    }

    fn languages(&self) -> &'static [Language] {
        LANGUAGES
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            lint: true,
            format: false,
            fix: false,
            project: false,
        }
    }

    fn option_keys(&self, table: OptionTable) -> OptionKeys {
        match table {
            OptionTable::Lint => OptionKeys::declared(&[
                ("std", OptionType::STRING),
                ("lints", OptionType::TABLE),
                ("config", OptionType::TABLE),
                ("ignore", OptionType::ARRAY),
            ]),
            OptionTable::Format | OptionTable::CrossCuttingLint => OptionKeys::declared(&[]),
        }
    }

    fn version(&self) -> &str {
        VERSION
    }

    fn host_hook(&self) -> HostHook {
        HostHook::SeleneConfig
    }

    fn lint(&self, src: &SourceFile, cfg: &EngineConfig) -> anyhow::Result<Vec<Diagnostic>> {
        refuse_pathological_nesting(&src.path, &src.content)?;
        let settings = Settings::resolve(src, cfg);
        let ready = match prepared(&settings) {
            Ok(ready) => ready,
            Err(message) => return Ok(vec![diagnostic("unsupported-std", Severity::Warning, message, None)]),
        };
        let index = LineIndex::new(&src.content);
        let ast = match full_moon_2::parse_fallible(&src.content, ready.lua_version).into_result() {
            Ok(ast) => ast,
            Err(errors) => {
                return Ok(errors
                    .iter()
                    .map(|error| {
                        let (start, end) = error.range();
                        diagnostic(
                            "syntax",
                            Severity::Error,
                            error.to_string(),
                            Some(span(&index, start.bytes(), end.bytes())),
                        )
                    })
                    .collect());
            }
        };
        let mut found = ready.checker.test_on(&ast);
        found.sort_by_key(|found| found.diagnostic.start_position());
        Ok(found
            .into_iter()
            .filter_map(|found| {
                let severity = match found.severity {
                    selene_lib::lints::Severity::Allow => return None,
                    selene_lib::lints::Severity::Error => Severity::Error,
                    selene_lib::lints::Severity::Warning => Severity::Warning,
                };
                let lint = found.diagnostic;
                let (start, end) = lint.primary_label.range;
                let mut out = diagnostic(
                    lint.code,
                    severity,
                    lint.message,
                    Some(span(&index, start as usize, end as usize)),
                );
                if !lint.notes.is_empty() {
                    out.description = Some(lint.notes.join("\n"));
                }
                Some(out)
            })
            .collect())
    }
}
