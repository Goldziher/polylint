//! The result-cache maintenance both `poly cache` and the MCP server run, and the documents both
//! print.

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use poly_cache::{CacheStats, ResultCache, root_from_cwd};
use poly_frontend_api::dto::{CacheFreedReport, CacheNamespace, CacheStatsReport};

const SECONDS_PER_DAY: u64 = 86_400;

/// The cache root for a run in the working directory: `explicit` when given, else `[cache] dir`
/// from `configured`, else the default per-repository anchor walk (which respects
/// `POLY_CACHE_HOME`).
///
/// # Errors
///
/// When the default anchor cannot be resolved.
pub fn root(explicit: Option<&Path>, configured: Option<&str>) -> Result<PathBuf> {
    if let Some(dir) = explicit {
        return Ok(dir.to_path_buf());
    }
    match configured {
        Some(dir) => Ok(PathBuf::from(dir)),
        None => root_from_cwd().context("failed to resolve the cache directory"),
    }
}

/// Open the result cache at `root`.
///
/// # Errors
///
/// When the cache cannot be opened.
pub fn open(root: PathBuf) -> Result<ResultCache> {
    ResultCache::open(root, true).context("failed to open the result cache")
}

/// The `poly cache stats --format json` document.
///
/// # Errors
///
/// When the cache cannot be read.
pub fn stats(cache: &ResultCache) -> Result<CacheStatsReport> {
    Ok(stats_report(&cache.stats()?))
}

/// The document for already-read `stats`.
#[must_use]
pub fn stats_report(stats: &CacheStats) -> CacheStatsReport {
    CacheStatsReport {
        format_version: stats.format_version.clone(),
        on_disk_version: stats.on_disk_version.clone(),
        namespaces: stats
            .per_namespace
            .iter()
            .map(|namespace| CacheNamespace {
                namespace: namespace.namespace.as_dir().to_owned(),
                entries: namespace.entries,
                bytes: namespace.bytes,
            })
            .collect(),
        total_entries: stats.per_namespace.iter().map(|namespace| namespace.entries).sum(),
        total_bytes: stats.total_bytes,
    }
}

/// Remove every entry: the `poly cache clean --format json` document.
///
/// # Errors
///
/// When the cache cannot be cleaned.
pub fn clean(cache: &ResultCache) -> Result<CacheFreedReport> {
    Ok(CacheFreedReport {
        command: "clean".to_owned(),
        freed_bytes: cache.clean()?,
    })
}

/// Evict entries older than `max_age_days` and, oldest first, beyond `max_size` (`500M`, `2G`):
/// the `poly cache gc --format json` document.
///
/// # Errors
///
/// When `max_size` does not parse or the cache cannot be collected.
pub fn gc(cache: &ResultCache, max_age_days: Option<u64>, max_size: Option<&str>) -> Result<CacheFreedReport> {
    let max_age = max_age_days.map(max_age_duration);
    let max_size = max_size.map(parse_size).transpose()?;
    Ok(CacheFreedReport {
        command: "gc".to_owned(),
        freed_bytes: cache.gc(max_age, max_size)?,
    })
}

/// `max_age` days as a duration; a huge value saturates instead of overflowing.
#[must_use]
pub fn max_age_duration(days: u64) -> Duration {
    Duration::from_secs(days.saturating_mul(SECONDS_PER_DAY))
}

/// Parse a human size string (`512`, `64K`, `500M`, `2G`, `1T`) into bytes.
///
/// A bare number is bytes; a `K`/`M`/`G`/`T` suffix (case-insensitive) scales by the
/// corresponding power of 1024.
///
/// # Errors
///
/// When the value is empty, not a number, or overflows a 64-bit byte count.
pub fn parse_size(value: &str) -> Result<u64> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        bail!("empty size value");
    }
    let (number, scale) = match trimmed.as_bytes()[trimmed.len() - 1] {
        b'k' | b'K' => (&trimmed[..trimmed.len() - 1], 1024u64),
        b'm' | b'M' => (&trimmed[..trimmed.len() - 1], 1024 * 1024),
        b'g' | b'G' => (&trimmed[..trimmed.len() - 1], 1024 * 1024 * 1024),
        b't' | b'T' => (&trimmed[..trimmed.len() - 1], 1024u64.pow(4)),
        _ => (trimmed, 1u64),
    };
    let number: u64 = number
        .trim()
        .parse()
        .with_context(|| format!("invalid size `{value}` (expected e.g. `500M`, `2G`)"))?;
    number
        .checked_mul(scale)
        .with_context(|| format!("size `{value}` overflows a 64-bit byte count"))
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::{max_age_duration, parse_size};

    #[test]
    fn parse_size_handles_suffixes_and_bare_bytes() {
        assert_eq!(parse_size("512").unwrap(), 512);
        assert_eq!(parse_size("64K").unwrap(), 64 * 1024);
        assert_eq!(parse_size("500M").unwrap(), 500 * 1024 * 1024);
        assert_eq!(parse_size("2g").unwrap(), 2 * 1024 * 1024 * 1024);
    }

    #[test]
    fn max_age_saturates_instead_of_overflowing() {
        assert_eq!(max_age_duration(2), Duration::from_secs(172_800));
        assert_eq!(max_age_duration(u64::MAX), Duration::from_secs(u64::MAX));
    }

    #[test]
    fn parse_size_rejects_garbage() {
        assert!(parse_size("").is_err());
        assert!(parse_size("abc").is_err());
        assert!(parse_size("12X").is_err());
    }
}
