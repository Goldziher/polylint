//! Layout options every oxfmt-equivalent formatter shares: line width, line ending, indentation.
//!
//! Read from the engine's `[fmt.<lang>.<engine>]` table (where `.oxfmtrc.json[c]` defaults land),
//! falling back to the `[defaults]` globals.

use std::collections::HashSet;
use std::sync::{Mutex, OnceLock};

use oxc_formatter_core::{CoreFormatOptions, IndentStyle, IndentWidth, LineEnding, LineWidth};

use poly_engine_api::config::{EngineConfig, LineEnding as ConfiguredLineEnding};

/// An option value the formatters cannot use; a default is substituted and the user warned.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct InvalidOption {
    key: &'static str,
    value: String,
    expected: &'static str,
}

impl InvalidOption {
    /// Warn once per `(key, value)`: the options are resolved for every file of a run.
    fn warn_once(&self) {
        static SEEN: OnceLock<Mutex<HashSet<(&'static str, String)>>> = OnceLock::new();
        let first = SEEN
            .get_or_init(Default::default)
            .lock()
            .is_ok_and(|mut seen| seen.insert((self.key, self.value.clone())));
        if first {
            tracing::warn!(
                key = self.key,
                value = %self.value,
                "invalid `{}` value, using the default; expected {}",
                self.key,
                self.expected
            );
        }
    }
}

fn warn_and_default<T>(parsed: Result<T, InvalidOption>, default: impl FnOnce() -> T) -> T {
    parsed.unwrap_or_else(|invalid| {
        invalid.warn_once();
        default()
    })
}

fn default_line_width() -> LineWidth {
    LineWidth::try_from(120u16).expect("120 is a valid LineWidth")
}

/// `line_width` from the table when set, else `[defaults] line_length`.
fn parse_line_width(cfg: &EngineConfig) -> Result<LineWidth, InvalidOption> {
    let invalid = |value: String| InvalidOption {
        key: "line_width",
        value,
        expected: "an integer from 1 to 320",
    };
    let width = match cfg.options.get("line_width").and_then(toml::Value::as_integer) {
        Some(explicit) => u16::try_from(explicit).map_err(|_| invalid(explicit.to_string()))?,
        None => {
            return Ok(u16::try_from(cfg.globals.line_length)
                .ok()
                .and_then(|width| LineWidth::try_from(width).ok())
                .unwrap_or_else(default_line_width));
        }
    };
    LineWidth::try_from(width).map_err(|_| invalid(width.to_string()))
}

/// `line_width` from the table when set, else `[defaults] line_length`, else 120.
pub(crate) fn resolve_line_width(cfg: &EngineConfig) -> LineWidth {
    warn_and_default(parse_line_width(cfg), default_line_width)
}

fn parse_line_ending(cfg: &EngineConfig) -> Result<LineEnding, InvalidOption> {
    match cfg.options.get("line_ending").and_then(toml::Value::as_str) {
        Some("crlf") => Ok(LineEnding::Crlf),
        Some("cr") => Ok(LineEnding::Cr),
        Some("lf") => Ok(LineEnding::Lf),
        Some(other) => Err(InvalidOption {
            key: "line_ending",
            value: other.to_string(),
            expected: "`lf`, `crlf` or `cr`",
        }),
        None => Ok(global_line_ending(cfg)),
    }
}

fn global_line_ending(cfg: &EngineConfig) -> LineEnding {
    match cfg.globals.line_ending {
        ConfiguredLineEnding::Crlf => LineEnding::Crlf,
        ConfiguredLineEnding::Lf => LineEnding::Lf,
    }
}

/// `line_ending` (`lf` / `crlf` / `cr`) from the table, else the `[defaults]` one.
pub(crate) fn resolve_line_ending(cfg: &EngineConfig) -> LineEnding {
    warn_and_default(parse_line_ending(cfg), || global_line_ending(cfg))
}

fn parse_indent_style(cfg: &EngineConfig) -> Result<IndentStyle, InvalidOption> {
    match cfg.options.get("indent_style").and_then(toml::Value::as_str) {
        Some("tab") => Ok(IndentStyle::Tab),
        Some("space") | None => Ok(IndentStyle::Space),
        Some(other) => Err(InvalidOption {
            key: "indent_style",
            value: other.to_string(),
            expected: "`space` or `tab`",
        }),
    }
}

pub(crate) fn resolve_indent_style(cfg: &EngineConfig) -> IndentStyle {
    warn_and_default(parse_indent_style(cfg), || IndentStyle::Space)
}

pub(crate) fn resolve_indent_width(cfg: &EngineConfig) -> IndentWidth {
    u8::try_from(cfg.indent_width)
        .ok()
        .and_then(|width| IndentWidth::try_from(width).ok())
        .unwrap_or_default()
}

/// All four shared options resolved for `cfg`.
pub(crate) fn core_format_options(cfg: &EngineConfig) -> CoreFormatOptions {
    CoreFormatOptions {
        indent_style: resolve_indent_style(cfg),
        indent_width: resolve_indent_width(cfg),
        line_width: resolve_line_width(cfg),
        line_ending: resolve_line_ending(cfg),
    }
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
    fn unknown_indent_style_is_reported() {
        let invalid = parse_indent_style(&cfg("indent_style = \"tabs\"")).unwrap_err();
        assert_eq!((invalid.key, invalid.value.as_str()), ("indent_style", "tabs"));
        assert_eq!(parse_indent_style(&cfg("indent_style = \"tab\"")), Ok(IndentStyle::Tab));
        assert_eq!(
            resolve_indent_style(&cfg("indent_style = \"tabs\"")),
            IndentStyle::Space
        );
    }

    #[test]
    fn unknown_line_ending_is_reported() {
        let invalid = parse_line_ending(&cfg("line_ending = \"windows\"")).unwrap_err();
        assert_eq!((invalid.key, invalid.value.as_str()), ("line_ending", "windows"));
        assert_eq!(parse_line_ending(&cfg("line_ending = \"crlf\"")), Ok(LineEnding::Crlf));
    }

    #[test]
    fn out_of_range_line_width_is_reported() {
        for bad in ["0", "70000", "-5", "321"] {
            let invalid = parse_line_width(&cfg(&format!("line_width = {bad}"))).unwrap_err();
            assert_eq!((invalid.key, invalid.value.as_str()), ("line_width", bad));
        }
        assert_eq!(resolve_line_width(&cfg("line_width = 0")), default_line_width());
        assert_eq!(
            parse_line_width(&cfg("line_width = 80")),
            Ok(LineWidth::try_from(80u16).unwrap())
        );
    }
}
