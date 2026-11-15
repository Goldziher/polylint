//! Building the `oxc_formatter` / `oxc_formatter_json` option structs from a
//! resolved [`EngineConfig`].

use oxc_formatter::JsFormatOptions;
use oxc_formatter_json::{JsonFormatOptions, JsonVariant};

use poly_engine_api::config::EngineConfig;
use poly_engine_api::engine::SourceFile;
use poly_engine_api::language::Language;

use super::sort_imports::build_sort_imports;
use crate::core_options::{resolve_indent_style, resolve_indent_width, resolve_line_ending, resolve_line_width};

fn option_str<'a>(cfg: &'a EngineConfig, key: &str) -> Option<&'a str> {
    cfg.options.get(key).and_then(toml::Value::as_str)
}

fn option_bool(cfg: &EngineConfig, key: &str) -> Option<bool> {
    cfg.options.get(key).and_then(toml::Value::as_bool)
}

/// Build [`JsFormatOptions`] from a resolved [`EngineConfig`].
///
/// ## Layering order (Prettier-compatible defaults → poly overrides → user config)
///
/// | `cfg.options` key | Type | Values |
/// |---|---|---|
/// | `quote_style` | string | `"double"` (default) / `"single"` |
/// | `single_quote` | bool | `true` is `quote_style = "single"`; an explicit `quote_style` wins |
/// | `jsx_quote_style` | string | `"double"` (default) / `"single"` |
/// | `semicolons` | string | `"always"` (default) / `"as-needed"` |
/// | `trailing_commas` | string | `"all"` (default) / `"es5"` / `"none"` |
/// | `arrow_parentheses` | string | `"always"` (default) / `"as-needed"` |
/// | `bracket_spacing` | bool | `true` (default) |
/// | `bracket_same_line` | bool | `false` (default) |
/// | `indent_style` | string | `"space"` (default) / `"tab"` |
///
/// `line_width` and `indent_width` are always taken from `cfg.globals.line_length`
/// and `cfg.indent_width` respectively — user cannot override them here.
pub(super) fn build_js_options(cfg: &EngineConfig) -> JsFormatOptions {
    use oxc_formatter::{
        ArrowParentheses, AttributePosition, BracketSameLine, BracketSpacing, Expand, OperatorPosition,
        QuoteProperties, QuoteStyle, Semicolons, TrailingCommas as JsTrailingCommas,
    };

    let line_width = resolve_line_width(cfg);
    let indent_width = resolve_indent_width(cfg);
    let indent_style = resolve_indent_style(cfg);

    let quote_style = match option_str(cfg, "quote_style") {
        Some("single") => QuoteStyle::Single,
        Some(_) => QuoteStyle::Double,
        None if option_bool(cfg, "single_quote") == Some(true) => QuoteStyle::Single,
        None => QuoteStyle::default(),
    };

    let jsx_quote_style = cfg
        .options
        .get("jsx_quote_style")
        .and_then(|v| v.as_str())
        .map(|s| match s {
            "single" => QuoteStyle::Single,
            _ => QuoteStyle::Double,
        })
        .unwrap_or_default();

    let semicolons = cfg
        .options
        .get("semicolons")
        .and_then(|v| v.as_str())
        .and_then(|s| s.parse::<Semicolons>().ok())
        .unwrap_or_default();

    let trailing_commas = cfg
        .options
        .get("trailing_commas")
        .and_then(|v| v.as_str())
        .and_then(|s| s.parse::<JsTrailingCommas>().ok())
        .unwrap_or_default();

    let arrow_parentheses = cfg
        .options
        .get("arrow_parentheses")
        .and_then(|v| v.as_str())
        .and_then(|s| s.parse::<ArrowParentheses>().ok())
        .unwrap_or_default();

    let bracket_spacing = cfg
        .options
        .get("bracket_spacing")
        .and_then(|v| v.as_bool())
        .map(BracketSpacing::from)
        .unwrap_or_default();

    let bracket_same_line = cfg
        .options
        .get("bracket_same_line")
        .and_then(|v| v.as_bool())
        .map(BracketSameLine::from)
        .unwrap_or_default();

    let quote_properties = option_str(cfg, "quote_properties")
        .and_then(|s| match s {
            "preserve" => Some(QuoteProperties::Preserve),
            "consistent" => Some(QuoteProperties::Consistent),
            "as-needed" => Some(QuoteProperties::AsNeeded),
            _ => None,
        })
        .unwrap_or_default();
    let expand = match option_str(cfg, "object_wrap") {
        Some("collapse") => Expand::Never,
        _ => Expand::Auto,
    };
    let operator_position = match option_str(cfg, "operator_position") {
        Some("start") => OperatorPosition::Start,
        _ => OperatorPosition::End,
    };
    let attribute_position = if option_bool(cfg, "single_attribute_per_line") == Some(true) {
        AttributePosition::Multiline
    } else {
        AttributePosition::Auto
    };

    JsFormatOptions {
        line_width,
        line_ending: resolve_line_ending(cfg),
        indent_width,
        indent_style,
        quote_properties,
        expand,
        operator_position,
        attribute_position,
        sort_imports: build_sort_imports(cfg),
        quote_style,
        jsx_quote_style,
        semicolons,
        trailing_commas,
        arrow_parentheses,
        bracket_spacing,
        bracket_same_line,
        ..JsFormatOptions::default()
    }
}

/// Build [`JsonFormatOptions`] from a resolved [`EngineConfig`].
///
/// ## Layering order
///
/// | `cfg.options` key | Type | Values |
/// |---|---|---|
/// | `bracket_spacing` | bool | `true` (default) |
/// | `trailing_commas` | string | `"all"` / `"es5"` (default) or `"none"` / `"never"`; `"always"` is `"all"` |
///
/// `line_width` and `indent_width` are always sourced from `cfg.globals.line_length`
/// and `cfg.indent_width`. The variant (Json vs Jsonc) is derived from the file language
/// and cannot be overridden per-option.
pub(super) fn build_json_options(src: &SourceFile, cfg: &EngineConfig) -> JsonFormatOptions {
    use oxc_formatter_json::{
        BracketSpacing as JsonBracketSpacing, Expand as JsonExpand, QuoteProps as JsonQuoteProps,
        TrailingCommas as JsonTc,
    };

    let variant = json_variant(src);

    let line_width = resolve_line_width(cfg);
    let indent_width = resolve_indent_width(cfg);

    let bracket_spacing = cfg
        .options
        .get("bracket_spacing")
        .and_then(|v| v.as_bool())
        .map(JsonBracketSpacing::from)
        .unwrap_or_default();

    let trailing_commas = match option_str(cfg, "trailing_commas") {
        None => JsonTc::default(),
        Some(value) => parse_json_trailing_commas(value).unwrap_or_else(|| {
            warn_once("trailing_commas", value, "expected `all`, `es5`, `none` or `never`");
            JsonTc::default()
        }),
    };

    let expand = match option_str(cfg, "object_wrap") {
        Some("collapse") => JsonExpand::Never,
        _ => JsonExpand::Auto,
    };
    let quote_props = match option_str(cfg, "quote_properties") {
        Some("preserve") => JsonQuoteProps::Preserve,
        Some("consistent") => JsonQuoteProps::Consistent,
        _ => JsonQuoteProps::AsNeeded,
    };

    JsonFormatOptions {
        variant,
        line_width,
        line_ending: resolve_line_ending(cfg),
        indent_style: resolve_indent_style(cfg),
        indent_width,
        expand,
        quote_props,
        single_quote: option_bool(cfg, "single_quote").unwrap_or_default().into(),
        bracket_spacing,
        trailing_commas,
    }
}

/// JSON `trailing_commas`: the JS vocabulary (`all`, `es5`, `none`) plus the legacy `always` / `never`.
/// JSON has no distinction between `all` and `es5`, so both keep the commas.
fn parse_json_trailing_commas(value: &str) -> Option<oxc_formatter_json::TrailingCommas> {
    use oxc_formatter_json::TrailingCommas;
    match value {
        "all" | "es5" | "always" => Some(TrailingCommas::Always),
        "none" | "never" => Some(TrailingCommas::Never),
        _ => None,
    }
}

/// Warn about an invalid option value once per `(key, value)`, since the formatter runs per file.
fn warn_once(key: &str, value: &str, expected: &str) {
    static SEEN: std::sync::OnceLock<std::sync::Mutex<std::collections::HashSet<(String, String)>>> =
        std::sync::OnceLock::new();
    let seen = SEEN.get_or_init(Default::default);
    let first = seen
        .lock()
        .is_ok_and(|mut seen| seen.insert((key.to_string(), value.to_string())));
    if first {
        tracing::warn!(key, value, "invalid oxc option value, using the default: {expected}");
    }
}

/// The JSON dialect for `src`: `package.json`, `composer.json` and import maps are written by
/// tooling with `JSON.stringify` (every array expanded), which is what keeps `npm install` from
/// rewriting them; JSON with comments is its own variant; everything else is plain JSON.
fn json_variant(src: &SourceFile) -> JsonVariant {
    let name = src.path.file_name().and_then(|name| name.to_str()).unwrap_or_default();
    if matches!(name, "package.json" | "composer.json") || name.ends_with(".importmap") {
        return JsonVariant::JsonStringify;
    }
    match src.language {
        Language::Jsonc => JsonVariant::Jsonc,
        _ => JsonVariant::Json,
    }
}

/// `package.json` content with its keys put in `sort-package-json` order, when
/// `[fmt.json.oxc] sort_package_json` asks for it (`true`, or a table with `sort_scripts`).
/// `None` leaves the file as is — including when it is not valid JSON, which the formatter
/// then reports on its own.
pub(super) fn sorted_package_json(src: &SourceFile, cfg: &EngineConfig) -> Option<String> {
    if src.path.file_name().and_then(|name| name.to_str()) != Some("package.json") {
        return None;
    }
    let sort_scripts = match cfg.options.get("sort_package_json")? {
        toml::Value::Boolean(true) => false,
        toml::Value::Table(table) => table
            .get("sort_scripts")
            .and_then(toml::Value::as_bool)
            .unwrap_or(false),
        _ => return None,
    };
    let options = sort_package_json::SortOptions::new()
        .with_sort_scripts(sort_scripts)
        .with_pretty(false);
    sort_package_json::sort_package_json_with_options(&src.content, &options).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use poly_engine_api::config::GlobalDefaults;

    fn cfg(options: &str) -> EngineConfig {
        EngineConfig {
            globals: GlobalDefaults::default(),
            indent_width: 2,
            options: options.parse().unwrap(),
        }
    }

    #[test]
    fn single_quote_reaches_the_js_formatter() {
        let options = build_js_options(&cfg("single_quote = true"));
        assert_eq!(options.quote_style, oxc_formatter::QuoteStyle::Single);
        let options = build_js_options(&cfg("single_quote = false"));
        assert_eq!(options.quote_style, oxc_formatter::QuoteStyle::Double);
    }

    #[test]
    fn explicit_quote_style_wins_over_single_quote() {
        let options = build_js_options(&cfg("single_quote = true\nquote_style = \"double\""));
        assert_eq!(options.quote_style, oxc_formatter::QuoteStyle::Double);
    }

    #[test]
    fn json_trailing_commas_accepts_the_js_vocabulary() {
        use oxc_formatter_json::TrailingCommas::{Always, Never};
        for (value, expected) in [
            ("all", Always),
            ("es5", Always),
            ("always", Always),
            ("none", Never),
            ("never", Never),
        ] {
            assert_eq!(parse_json_trailing_commas(value), Some(expected), "{value}");
        }
        assert_eq!(parse_json_trailing_commas("bogus"), None);
    }

    #[test]
    fn json_trailing_commas_none_drops_the_commas() {
        let src = SourceFile {
            path: "a.jsonc".into(),
            language: Language::Jsonc,
            content: "".into(),
        };
        let options = build_json_options(&src, &cfg("trailing_commas = \"none\""));
        assert_eq!(options.trailing_commas, oxc_formatter_json::TrailingCommas::Never);
        let options = build_json_options(&src, &cfg("trailing_commas = \"bogus\""));
        assert_eq!(options.trailing_commas, oxc_formatter_json::TrailingCommas::default());
    }
}
