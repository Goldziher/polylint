//! Runner bench across the dimensions a single `format` run hides: lint versus format, cold versus
//! warm result cache, many tiny files versus one large file, mixed languages, and one worker thread
//! versus all cores.
//!
//! Trees are generated into a temp dir at start-up (see `common`). Engines that the default build
//! does not link (the web family) are skipped rather than downloaded, so the bench runs offline and
//! the mixed tree measures routing plus whatever engines are linked.
//!
//! Run with: `cargo bench -p poly-core --bench runner_modes`.

mod common;

use std::hint::black_box;
use std::path::PathBuf;

use criterion::{BatchSize, BenchmarkId, Criterion, criterion_group, criterion_main};
use poly_core::config::Config;
use poly_core::runner::{RunOptions, format, lint};

const TINY_FILES: usize = 5_000;
const LARGE_FILE_BYTES: usize = 2 * 1024 * 1024;
const MIXED_PER_KIND: usize = 100;

fn run_lint(paths: &[PathBuf], config: &Config, opts: &RunOptions) {
    black_box(lint(black_box(paths), config, opts, false, false).expect("lint run"));
}

fn run_format(paths: &[PathBuf], config: &Config, opts: &RunOptions) {
    black_box(format(black_box(paths), config, opts, false, false).expect("format run"));
}

fn bench_modes(c: &mut Criterion) {
    let cache_home = tempfile::tempdir().expect("cache home");
    common::isolate_cache(cache_home.path());
    let config = Config::default();

    let tiny = common::tiny_tree(TINY_FILES);
    let large = common::large_file_tree(LARGE_FILE_BYTES);
    let mixed = common::mixed_tree(MIXED_PER_KIND);

    let mut group = c.benchmark_group("runner_modes");
    group.sample_size(10);

    for (name, dir) in [("5k_tiny", &tiny), ("one_2mb_file", &large), ("mixed_700", &mixed)] {
        let paths = vec![PathBuf::from(dir.path())];
        let opts = common::offline_options(true, None);
        group.bench_function(BenchmarkId::new("lint_no_cache", name), |b| {
            b.iter(|| run_lint(&paths, &config, &opts));
        });
        group.bench_function(BenchmarkId::new("fmt_no_cache", name), |b| {
            b.iter(|| run_format(&paths, &config, &opts));
        });
    }

    let paths = vec![PathBuf::from(mixed.path())];
    for (label, jobs) in [("jobs_1", Some(1)), ("jobs_default", None)] {
        let opts = common::offline_options(true, jobs);
        group.bench_function(BenchmarkId::new("lint_mixed_700", label), |b| {
            b.iter(|| run_lint(&paths, &config, &opts));
        });
    }

    for (name, dir) in [("5k_tiny", &tiny), ("mixed_700", &mixed)] {
        let paths = vec![PathBuf::from(dir.path())];
        let opts = common::offline_options(false, None);
        let wipe = || {
            let _ = std::fs::remove_dir_all(cache_home.path());
            std::fs::create_dir_all(cache_home.path()).expect("recreate cache home");
        };
        group.bench_function(BenchmarkId::new("lint_cache_cold", name), |b| {
            b.iter_batched(wipe, |()| run_lint(&paths, &config, &opts), BatchSize::PerIteration);
        });
        wipe();
        run_lint(&paths, &config, &opts);
        group.bench_function(BenchmarkId::new("lint_cache_warm", name), |b| {
            b.iter(|| run_lint(&paths, &config, &opts));
        });
        wipe();
        run_format(&paths, &config, &opts);
        group.bench_function(BenchmarkId::new("fmt_cache_warm", name), |b| {
            b.iter(|| run_format(&paths, &config, &opts));
        });
    }
    group.finish();
}

criterion_group!(benches, bench_modes);
criterion_main!(benches);
