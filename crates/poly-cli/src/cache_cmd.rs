//! `poly cache` — maintenance commands over the tier-1 result cache.
//!
//! Subcommands operate on the per-user result cache
//! (`<platform-cache>/poly/<repo-key>`, or an explicit `--cache-dir`):
//!
//! - `poly cache stats` — entry counts, sizes, and format version (default).
//! - `poly cache gc [--max-age <days>] [--max-size <500M|2G|…>]` — evict stale
//!   and/or oversized entries (and wipe a tree from an incompatible layout).
//! - `poly cache clean` — remove every cached entry.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{Context, Result};
use clap::{Args, Subcommand};
use poly_cache::CacheStats;
use poly_config::PolyConfig;
use poly_host_services::cache;
use serde::Serialize;

use crate::OutputFormat;

/// `poly cache` arguments — an optional subcommand (defaulting to `stats`).
#[derive(Args)]
pub struct CacheArgs {
    /// The maintenance operation to perform (default: `stats`).
    #[command(subcommand)]
    pub command: Option<CacheCommand>,

    /// Operate on this cache directory instead of the resolved repo cache.
    #[arg(long, global = true)]
    pub cache_dir: Option<PathBuf>,

    /// Output format.
    #[arg(long, value_enum, default_value_t = OutputFormat::Pretty, global = true)]
    pub format: OutputFormat,
}

/// The `poly cache` subcommands.
#[derive(Subcommand)]
pub enum CacheCommand {
    /// Show entry counts, sizes, and the cache format version.
    Stats,
    /// Evict stale or oversized entries (and wipe an incompatible-layout tree).
    Gc {
        /// Evict entries older than this many days.
        #[arg(long)]
        max_age: Option<u64>,
        /// Evict oldest-first until under this budget (e.g. `500M`, `2G`).
        #[arg(long)]
        max_size: Option<String>,
    },
    /// Remove every cached entry, keeping the directory layout.
    Clean,
}

/// Run `poly cache`, mapping any error to exit code 2.
pub fn run_cache(args: CacheArgs) -> ExitCode {
    match run(args) {
        Ok(code) => code,
        Err(error) => {
            eprintln!("poly cache: {error:#}");
            ExitCode::from(2)
        }
    }
}

fn run(args: CacheArgs) -> Result<ExitCode> {
    let root = resolve_root(args.cache_dir.as_deref())?;
    let cache = cache::open(root)?;

    match args.command.unwrap_or(CacheCommand::Stats) {
        CacheCommand::Stats => {
            let stats = cache.stats()?;
            emit(args.format, &cache::stats_report(&stats), |_| print_stats(&stats))?;
        }
        CacheCommand::Gc { max_age, max_size } => {
            let document = cache::gc(&cache, max_age, max_size.as_deref())?;
            emit(args.format, &document, |d| {
                println!("freed {} ({} bytes)", format_iec(d.freed_bytes), d.freed_bytes);
            })?;
        }
        CacheCommand::Clean => {
            let document = cache::clean(&cache)?;
            emit(args.format, &document, |d| {
                println!("cleaned {} ({} bytes)", format_iec(d.freed_bytes), d.freed_bytes);
            })?;
        }
    }
    Ok(ExitCode::SUCCESS)
}

fn emit<T: Serialize>(format: OutputFormat, document: &T, pretty: impl FnOnce(&T)) -> Result<()> {
    match format {
        OutputFormat::Pretty => pretty(document),
        OutputFormat::Json => println!("{}", poly_core::report::render_json(document)?),
        OutputFormat::Toon => println!("{}", poly_core::report::render_toon(document)?),
    }
    Ok(())
}

/// Resolve the cache root: `--cache-dir` if given, else `[cache] dir` from the
/// nearest config, else the default anchor walk from the current directory.
pub(crate) fn resolve_root(cache_dir: Option<&Path>) -> Result<PathBuf> {
    if let Some(dir) = cache_dir {
        return Ok(dir.to_path_buf());
    }
    let cwd = std::env::current_dir().context("failed to resolve the working directory")?;
    let config = PolyConfig::load_with(&cwd, &crate::config_sources::resolver()?).context("failed to load config")?;
    cache::root(None, config.cache.dir.as_deref())
}

/// A cache entry count, grouped and pluralised the way every other poly count
/// is: `1 entry`, `20,889 entries`.
fn entries(count: u64) -> String {
    poly_core::report::quantity(usize::try_from(count).unwrap_or(usize::MAX), "entry", "entries")
}

/// Render a [`CacheStats`] as a human-readable summary.
fn print_stats(stats: &CacheStats) {
    println!("cache format version: {}", stats.format_version);
    match &stats.on_disk_version {
        Some(version) if version == &stats.format_version => {
            println!("on-disk version:      {version}");
        }
        Some(version) => println!("on-disk version:      {version} (stale; gc will wipe)"),
        None => println!("on-disk version:      (none)"),
    }
    // Width measured from the rows themselves: a fixed column silently
    // collapses the moment one count outgrows it, which is how an aligned table
    // turns into a ragged one on exactly the repositories big enough to need it.
    let counts: Vec<String> = stats.per_namespace.iter().map(|n| entries(n.entries)).collect();
    let column = counts.iter().map(|c| c.chars().count()).max().unwrap_or(0);
    for (namespace, count) in stats.per_namespace.iter().zip(&counts) {
        println!(
            "  {:<6} {count:>column$}  {:>10}",
            namespace.namespace.as_dir(),
            format_iec(namespace.bytes),
        );
    }
    println!(
        "total: {}, {} ({} bytes)",
        entries(stats.per_namespace.iter().map(|n| n.entries).sum::<u64>()),
        format_iec(stats.total_bytes),
        stats.total_bytes,
    );
}

/// Format a byte count using IEC binary units (`B`, `KiB`, `MiB`, …).
fn format_iec(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    format!("{value:.1} {}", UNITS[unit])
}

#[cfg(test)]
mod tests {
    use super::format_iec;

    #[test]
    fn format_iec_uses_binary_units() {
        assert_eq!(format_iec(512), "512 B");
        assert_eq!(format_iec(1024), "1.0 KiB");
        assert_eq!(format_iec(1536), "1.5 KiB");
        assert_eq!(format_iec(1024 * 1024), "1.0 MiB");
    }
}
