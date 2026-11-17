//! `[cache]` configuration table from `poly.toml`.
//!
//! The `[cache]` table is **fully optional**: all fields carry defaults so an
//! absent table is equivalent to the defaults shown below.
//!
//! ```toml
//! [cache]
//! enabled = true           # master enable/disable switch (default true)
//! # dir = "..."            # optional repo-local root override; absent → anchor walk
//!
//! [cache.results]
//! hooks = "safe"           # Off | Safe (default) | Aggressive
//!
//! [cache.sccache]
//! enabled = false          # opt-in; off by default
//! # bin = "sccache"
//! # dir = "~/.sccache"
//! # max_size = "10G"
//! ```
//!
//! `[cache]` belongs to the root config of a cascade (ADR 0018): one run opens one cache, so a
//! nested `poly.toml` that sets it is ignored with a warning ([`ignore_nested`]). A relative `dir`
//! is anchored at the directory of the `poly.toml` declaring it ([`anchor_dir_in_table`]), so every
//! working directory, `poly cache` and the hook runner name the same cache.

use std::collections::BTreeSet;
use std::path::Path;
use std::sync::Mutex;

use serde::Deserialize;

/// Default `sccache` binary name, resolved on `$PATH` when `bin` is unset.
pub const DEFAULT_SCCACHE_BIN: &str = "sccache";

/// Controls when hook results are served from the result cache.
///
/// The default is [`Safe`] — hook results are cached only when the hook
/// explicitly declares its inputs via `cache = { inputs = [...] }` in the
/// job definition.
///
/// [`Safe`]: HookCacheMode::Safe
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Default, schemars::JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum HookCacheMode {
    /// Never cache hook results.
    Off,
    /// Cache hook results only when the hook declares its inputs (default).
    ///
    /// An inline command is cached only when it carries a
    /// `cache = { inputs = [...] }` declaration; otherwise it always reruns.
    /// The `lint`/`fmt` builtins are never hook-cached in any mode: their verdict
    /// depends on configuration outside the matched files, and `poly lint` /
    /// `poly fmt` keep their own per-file cache, which keys it.
    #[default]
    Safe,
    /// Cache based on matched files only, regardless of declared inputs.
    ///
    /// May produce stale results for commands whose behaviour depends on
    /// inputs outside the matched file set (e.g. environment variables,
    /// generated files).  **Use with care.**
    Aggressive,
}

/// Configuration for the tier-1 result cache.
#[derive(Debug, Clone, Deserialize, Default, schemars::JsonSchema)]
#[serde(default)]
pub struct ResultsCacheConfig {
    /// Cache mode for hook results.
    pub hooks: HookCacheMode,
}

/// Configuration for the tier-2 sccache integration (opt-in, off by default).
///
/// When enabled, `poly hooks` starts the shared sccache server (idempotent,
/// long-lived) before running hooks, and injects `RUSTC_WRAPPER` /
/// `SCCACHE_DIR` / `SCCACHE_CACHE_SIZE` only into hooks that declare
/// `cache.compiler = true`.
///
/// Sccache is **not implemented in this crate** — this is only the config
/// surface.  Wire-up happens in `crates/poly-hooks` (Workstream B/C).
#[derive(Debug, Clone, Deserialize, Default, schemars::JsonSchema)]
#[serde(default)]
pub struct SccacheConfig {
    /// Enable sccache for compiler hooks (default `false`).
    pub enabled: bool,
    /// Path to the `sccache` binary.  When absent, `$PATH` is searched.
    pub bin: Option<String>,
    /// Override sccache storage directory.
    pub dir: Option<String>,
    /// Maximum cache size string understood by sccache (e.g. `"10G"`).
    pub max_size: Option<String>,
}

impl SccacheConfig {
    /// Resolve and validate the `sccache` binary to invoke as `RUSTC_WRAPPER`.
    ///
    /// Because a repository's checked-in `poly.toml` controls this value and it
    /// becomes `RUSTC_WRAPPER`, only two shapes are accepted:
    ///
    /// - a **bare command name** (no path separators) resolved on `$PATH`
    ///   — the default `sccache` when `bin` is unset; or
    /// - an **absolute path**.
    ///
    /// A relative path containing separators (e.g. `./evil`, `a/b`) is rejected
    /// so a hostile repo cannot silently point the compiler wrapper at a
    /// repo-relative binary. The consumer (`poly-hooks` runner) MUST call this
    /// rather than reading [`bin`][Self::bin] directly.
    ///
    /// # Errors
    ///
    /// Returns `Err` when `bin` is empty or is a relative path with separators.
    pub fn validated_bin(&self) -> anyhow::Result<&str> {
        let Some(bin) = self.bin.as_deref() else {
            return Ok(DEFAULT_SCCACHE_BIN);
        };
        if bin.is_empty() {
            anyhow::bail!("[cache.sccache] bin must not be empty");
        }
        let is_bare_name = Path::new(bin).components().count() == 1 && !bin.contains('/') && !bin.contains('\\');
        if Path::new(bin).is_absolute() || is_bare_name {
            Ok(bin)
        } else {
            anyhow::bail!(
                "[cache.sccache] bin = {bin:?} must be a bare command name (resolved on \
                 $PATH) or an absolute path; a relative path with separators is rejected \
                 to avoid executing a repo-relative binary as RUSTC_WRAPPER"
            );
        }
    }
}

/// Anchor a relative `[cache] dir` in a raw config `table` at `dir`, leaving an absolute (or, on
/// Windows, drive-relative rooted) path as written.
pub(crate) fn anchor_dir_in_table(table: &mut toml::Table, dir: &Path) {
    let Some(value) = table
        .get_mut("cache")
        .and_then(toml::Value::as_table_mut)
        .and_then(|cache| cache.get_mut("dir"))
    else {
        return;
    };
    if let Some(relative) = value.as_str().map(Path::new).filter(|path| !path.has_root()) {
        *value = toml::Value::String(dir.join(relative).to_string_lossy().into_owned());
    }
}

/// Configs already warned about this process; a nested config is resolved once per directory a
/// run plans, and a long-lived `poly mcp` reloads it on every request.
static WARNED_NESTED: Mutex<BTreeSet<std::path::PathBuf>> = Mutex::new(BTreeSet::new());

/// Drop `[cache]` from the raw table of the nested config in `config_dir`, warning once that it is
/// ignored: only the root config of the cascade chooses the run's cache.
pub(crate) fn ignore_nested(table: &mut toml::Table, config_dir: &Path) {
    if table.remove("cache").is_none() {
        return;
    }
    let file = config_dir.join(crate::CONFIG_FILE_NAMES[0]);
    let first = WARNED_NESTED
        .lock()
        .map_or(true, |mut warned| warned.insert(file.clone()));
    if first {
        eprintln!(
            "warning: {} sets [cache], which only the root poly.toml of a repository (or of a \
             `[workspace] root = true` project) can set; it is ignored",
            file.display()
        );
    }
}

fn default_cache_enabled() -> bool {
    true
}

/// Configuration for the `[cache]` table in `poly.toml`.
///
/// All fields default so `[cache]` is entirely optional. Root-only: a nested `poly.toml` that sets
/// `[cache]` is ignored with a warning, since one run opens one cache.
#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
#[serde(default)]
pub struct CacheConfig {
    /// Master enable/disable switch for both cache tiers (default `true`).
    ///
    /// Equivalent to `--no-cache` on the CLI.
    #[serde(default = "default_cache_enabled")]
    pub enabled: bool,
    /// Override the cache root directory. A relative path is resolved against the directory of
    /// the `poly.toml` that declares it.
    ///
    /// When absent the cache lives in the per-user cache home (see
    /// `poly_cache::root_from`): `<platform-cache>/poly/<repo-key>`, keyed by the
    /// git common dir of the nearest `.git` ancestor (so every worktree of one
    /// repository shares it), else by the nearest `poly.toml` ancestor.
    pub dir: Option<String>,
    /// Tier-1 result-cache configuration.
    pub results: ResultsCacheConfig,
    /// Tier-2 sccache configuration.
    pub sccache: SccacheConfig,
}

impl Default for CacheConfig {
    fn default() -> Self {
        Self {
            enabled: default_cache_enabled(),
            dir: None,
            results: ResultsCacheConfig::default(),
            sccache: SccacheConfig::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with_bin(bin: Option<&str>) -> SccacheConfig {
        SccacheConfig {
            bin: bin.map(str::to_owned),
            ..SccacheConfig::default()
        }
    }

    #[test]
    fn validated_bin_defaults_to_sccache_when_unset() {
        assert_eq!(with_bin(None).validated_bin().unwrap(), DEFAULT_SCCACHE_BIN);
    }

    #[test]
    fn validated_bin_accepts_bare_command_name() {
        assert_eq!(with_bin(Some("sccache")).validated_bin().unwrap(), "sccache");
    }

    #[test]
    fn validated_bin_accepts_absolute_path() {
        let abs = if cfg!(windows) {
            r"C:\tools\sccache.exe"
        } else {
            "/usr/local/bin/sccache"
        };
        assert_eq!(with_bin(Some(abs)).validated_bin().unwrap(), abs);
    }

    #[test]
    fn validated_bin_rejects_relative_path_with_separator() {
        assert!(with_bin(Some("./evil")).validated_bin().is_err());
        assert!(with_bin(Some("a/b")).validated_bin().is_err());
        assert!(with_bin(Some(r"a\b")).validated_bin().is_err());
    }

    #[test]
    fn validated_bin_rejects_empty() {
        assert!(with_bin(Some("")).validated_bin().is_err());
    }
}
