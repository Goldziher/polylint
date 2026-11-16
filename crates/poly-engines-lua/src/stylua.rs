//! Lua formatter backend, wrapping the `stylua` library (`stylua_lib`).
//!
//! Format-only: StyLua has no lint rules (selene is the linter). A file that does not parse is returned
//! [`FormatOutput::Unchanged`]; selene reports the syntax error.
//!
//! # Options
//!
//! `[engines.stylua]` takes StyLua's own `stylua.toml` keys with StyLua's own spellings (`quote_style =
//! "AutoPreferDouble"`): `syntax`, `column_width`, `line_endings`, `indent_type`, `indent_width`,
//! `quote_style`, `call_parentheses`, `collapse_simple_statement`, `space_after_function_names`,
//! `block_newline_gaps` and `sort_requires`. The nearest `stylua.toml` or `.stylua.toml` is laid under that
//! table by the [`StyluaConfig`](poly_engine_api::host_hooks::HostHook::StyluaConfig) host hook, so a file
//! checked into a project is honoured without repeating it in `poly.toml`.
//!
//! Where nothing sets a key, StyLua's defaults apply, except the two layout globals poly owns:
//! `column_width` is [`GlobalDefaults::line_length`](poly_engine_api::config::GlobalDefaults::line_length)
//! (120, which is also StyLua's default) and `line_endings` follows the global `line_ending`. Indentation
//! stays StyLua's default (tabs, `indent_width` 4 as the wrapping heuristic), because that is what the Lua
//! ecosystem's `.stylua.toml` files assume; set `indent_type = "Spaces"` to indent with `indent_width` spaces.
//!
//! The output is verified: StyLua reparses what it printed and compares the syntax tree to the input's, so a
//! formatter bug surfaces as an error on that file and never as silently changed code.

use poly_engine_api::config::{EngineConfig, LineEnding as PolyLineEnding};
use poly_engine_api::engine::{Capabilities, Engine, FormatOutput, OptionKeys, OptionTable, SourceFile};
use poly_engine_api::engine_guard::refuse_pathological_nesting;
use poly_engine_api::host_hooks::HostHook;
use poly_engine_api::language::Language;
use serde::Deserialize;
use stylua_lib::{
    BlockNewlineGaps, CallParenType, CollapseSimpleStatement, Config, IndentType, LineEndings, LuaVersion,
    OutputVerification, QuoteStyle, SortRequiresConfig, SpaceAfterFunctionNames,
};

/// Cache-key version: the `stylua` crate version plus a marker for this backend's option mapping. Bump when
/// the crate is updated or the mapping changes.
const VERSION: &str = "stylua-2.6.0+fmt1";

static LANGUAGES: &[Language] = &[Language::Lua];

/// The keys of `[engines.stylua]`. Mirrors `stylua_lib::Config` field for field, but with every field
/// optional and unknown keys tolerated: a poly table also carries poly's own keys (`enabled`), which
/// `Config`'s `deny_unknown_fields` would reject, and the option-key check derives the recognised keys from
/// this type.
#[derive(Debug, Default, Deserialize)]
struct StyluaOptions {
    syntax: Option<LuaVersion>,
    column_width: Option<usize>,
    line_endings: Option<LineEndings>,
    indent_type: Option<IndentType>,
    indent_width: Option<usize>,
    quote_style: Option<QuoteStyle>,
    call_parentheses: Option<CallParenType>,
    collapse_simple_statement: Option<CollapseSimpleStatement>,
    space_after_function_names: Option<SpaceAfterFunctionNames>,
    block_newline_gaps: Option<BlockNewlineGaps>,
    sort_requires: Option<SortRequiresConfig>,
}

/// StyLua Lua formatter backend. See the module docs.
pub struct StyluaEngine;

impl StyluaEngine {
    /// The StyLua config for `cfg`: StyLua's defaults, then poly's layout globals, then the options table.
    fn config(cfg: &EngineConfig) -> anyhow::Result<Config> {
        let options: StyluaOptions = toml::Value::Table(cfg.options.clone())
            .try_into()
            .map_err(|error| anyhow::anyhow!("the stylua options are invalid: {error}"))?;
        let defaults = Config::default();
        Ok(Config {
            syntax: options.syntax.unwrap_or(defaults.syntax),
            column_width: options.column_width.unwrap_or(cfg.globals.line_length),
            line_endings: options.line_endings.unwrap_or(match cfg.globals.line_ending {
                PolyLineEnding::Crlf => LineEndings::Windows,
                PolyLineEnding::Lf => LineEndings::Unix,
            }),
            indent_type: options.indent_type.unwrap_or(defaults.indent_type),
            indent_width: options.indent_width.unwrap_or(cfg.indent_width),
            quote_style: options.quote_style.unwrap_or(defaults.quote_style),
            call_parentheses: options.call_parentheses.unwrap_or(defaults.call_parentheses),
            collapse_simple_statement: options
                .collapse_simple_statement
                .unwrap_or(defaults.collapse_simple_statement),
            space_after_function_names: options
                .space_after_function_names
                .unwrap_or(defaults.space_after_function_names),
            block_newline_gaps: options.block_newline_gaps.unwrap_or(defaults.block_newline_gaps),
            sort_requires: options.sort_requires.unwrap_or(defaults.sort_requires),
            ..defaults
        })
    }
}

impl Engine for StyluaEngine {
    fn overridden_hooks(&self) -> u32 {
        0
    }

    fn name(&self) -> &'static str {
        "stylua"
    }

    fn languages(&self) -> &'static [Language] {
        LANGUAGES
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            lint: false,
            format: true,
            fix: false,
            project: false,
        }
    }

    /// The whole table is deserialized into [`StyluaOptions`], so the recognised keys are derived from it.
    fn option_keys(&self, table: OptionTable) -> OptionKeys {
        match table {
            OptionTable::Format => OptionKeys::declared(&[])
                .with_derived(poly_engine_api::option_probe::recognized_by_type_probe::<StyluaOptions>),
            OptionTable::Lint | OptionTable::CrossCuttingLint => OptionKeys::declared(&[]),
        }
    }

    fn version(&self) -> &str {
        VERSION
    }

    fn host_hook(&self) -> HostHook {
        HostHook::StyluaConfig
    }

    fn format(&self, src: &SourceFile, cfg: &EngineConfig) -> anyhow::Result<FormatOutput> {
        refuse_pathological_nesting(&src.path, &src.content)?;
        let config = Self::config(cfg)?;
        match stylua_lib::format_code(&src.content, config, None, OutputVerification::Full) {
            Ok(formatted) if formatted != *src.content => Ok(FormatOutput::Formatted(formatted)),
            Ok(_) | Err(stylua_lib::Error::ParseError(_)) => Ok(FormatOutput::Unchanged),
            Err(error) => Err(anyhow::anyhow!("{error}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use poly_engine_api::config::GlobalDefaults;

    fn cfg(options: &str) -> EngineConfig {
        EngineConfig {
            globals: GlobalDefaults::default(),
            indent_width: 4,
            options: options.parse().unwrap(),
        }
    }

    fn src(content: &str) -> SourceFile {
        SourceFile {
            path: "a.lua".into(),
            language: Language::Lua,
            content: content.into(),
        }
    }

    fn formatted(options: &str, content: &str) -> Option<String> {
        match StyluaEngine.format(&src(content), &cfg(options)).unwrap() {
            FormatOutput::Formatted(text) => Some(text),
            FormatOutput::Unchanged => None,
        }
    }

    #[test]
    fn the_layout_globals_are_poly_s_and_the_rest_is_stylua_s() {
        let config = StyluaEngine::config(&cfg("")).unwrap();
        assert_eq!(config.column_width, 120);
        assert_eq!(config.indent_width, 4);
        assert_eq!(config.indent_type, IndentType::Tabs);
        assert_eq!(config.line_endings, LineEndings::Unix);
    }

    #[test]
    fn options_override_the_defaults() {
        let config =
            StyluaEngine::config(&cfg("column_width = 80\nindent_type = \"Spaces\"\nenabled = true\n")).unwrap();
        assert_eq!(config.column_width, 80);
        assert_eq!(config.indent_type, IndentType::Spaces);
    }

    #[test]
    fn an_invalid_option_value_is_an_error_not_a_silent_default() {
        assert!(StyluaEngine::config(&cfg("quote_style = \"Backticks\"\n")).is_err());
    }

    #[test]
    fn already_formatted_and_unparsable_input_is_unchanged() {
        assert_eq!(formatted("", "local a = 1\n"), None);
        assert_eq!(formatted("", "local = = 1\n"), None);
    }

    #[test]
    fn the_quote_style_option_reaches_the_formatter() {
        assert_eq!(formatted("", "local a = 'x'\n").as_deref(), Some("local a = \"x\"\n"));
        assert_eq!(
            formatted("quote_style = \"ForceSingle\"\n", "local a = \"x\"\n").as_deref(),
            Some("local a = 'x'\n")
        );
    }
}
