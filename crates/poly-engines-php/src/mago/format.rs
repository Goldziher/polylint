//! PHP format pass via [`mago_formatter::Formatter`].
//!
//! ## Settings layering
//!
//! | Priority | Source |
//! |----------|--------|
//! | 3 (highest) | user `[fmt.php.mago]` table keys |
//! | 2 | poly opinionated defaults (`line_length` → `print_width`, `indent_width` → `tab_width`) |
//! | 1 (lowest) | mago's own `FormatSettings::default()` |
//!
//! The user can override any key that [`mago_formatter::settings::FormatSettings`]
//! exposes (kebab-case).  Unknown keys are silently ignored.
//!
//! Returns [`FormatOutput::Unchanged`] on parse failure (the lint pass reports
//! the syntax error) and when the formatted output is identical to the input.

use std::borrow::Cow;
use std::cell::RefCell;
use std::collections::HashSet;
use std::sync::Mutex;

use mago_formatter::Formatter;
use mago_formatter::settings::{FormatSettings, RawFormatSettings};
use mago_php_version::PHPVersion;

use poly_engine_api::config::EngineConfig;
use poly_engine_api::engine::{FormatOutput, SourceFile};

/// Polylint-default PHP version for formatting rules.
const PHP_VERSION: PHPVersion = PHPVersion::PHP84;

/// Runs `f` with this thread's arena, reset first, so a file does not pay for a fresh allocation.
///
/// Nothing `f` allocates may escape it. A nested call on the same thread gets a fresh arena instead
/// of a second borrow.
pub(super) fn with_arena<R>(f: impl FnOnce(&mago_allocator::LocalArena) -> R) -> R {
    thread_local! {
        static ARENA: RefCell<mago_allocator::LocalArena> = const { RefCell::new(mago_allocator::LocalArena::new()) };
    }
    ARENA.with(|cell| match cell.try_borrow_mut() {
        Ok(mut arena) => {
            arena.reset();
            f(&arena)
        }
        Err(_) => f(&mago_allocator::LocalArena::new()),
    })
}

/// Format a single PHP source file.
///
/// Uses a per-thread arena and a per-call `Formatter`, avoiding any shared state so this
/// function is safe to call from multiple rayon threads concurrently.
pub(super) fn format_php(src: &SourceFile, cfg: &EngineConfig) -> anyhow::Result<FormatOutput> {
    let php_version = super::rules::parse_php_version(cfg)?.unwrap_or(PHP_VERSION);
    let settings = build_format_settings(cfg);

    let name: Cow<'static, [u8]> = Cow::Borrowed(b"input.php");
    let code: Cow<'static, [u8]> = Cow::Owned(src.content.as_bytes().to_vec());

    let formatted = with_arena(|arena| {
        let formatter = Formatter::new(arena, php_version, settings);
        let bytes = formatter.format_code(name, code).ok()?;
        std::str::from_utf8(bytes).ok().map(str::to_owned)
    });
    let Some(formatted) = formatted else {
        return Ok(FormatOutput::Unchanged);
    };

    if formatted == *src.content {
        Ok(FormatOutput::Unchanged)
    } else {
        Ok(FormatOutput::Formatted(formatted))
    }
}

/// Build [`FormatSettings`] by layering user options over poly opinionated
/// defaults over mago's own defaults.
///
/// 1. Start with `FormatSettings::default()` (mago's defaults).
/// 2. Apply poly's `line_length` and `indent_width` as the opinionated base.
/// 3. Deserialise each `cfg.options` entry on its own into [`RawFormatSettings`]
///    and merge it over the settings so far, so one invalid value drops only
///    that field (with a warning) instead of every user option.
fn build_format_settings(cfg: &EngineConfig) -> FormatSettings {
    let base = FormatSettings {
        print_width: cfg.globals.line_length,
        tab_width: cfg.indent_width,
        ..FormatSettings::default()
    };

    if cfg.options.is_empty() {
        return base;
    }

    let mut settings = base;
    for (key, value) in &cfg.options {
        let single = toml::Table::from_iter([(key.clone(), value.clone())]);
        match toml::Value::Table(single).try_into::<RawFormatSettings>() {
            Ok(raw) => settings = raw.merge_with(settings),
            Err(error) => warn_invalid_option(key, &error.to_string()),
        }
    }
    settings
}

/// Warn once per process for each distinct invalid option, not once per file.
fn warn_invalid_option(key: &str, error: &str) {
    static WARNED: Mutex<Option<HashSet<String>>> = Mutex::new(None);
    let mut guard = WARNED.lock().unwrap_or_else(|e| e.into_inner());
    if guard.get_or_insert_with(HashSet::new).insert(format!("{key}\0{error}")) {
        tracing::warn!(option = key, %error, "[fmt.php.mago] option ignored: invalid value");
    }
}
