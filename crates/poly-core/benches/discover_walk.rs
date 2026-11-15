//! File-discovery bench: the directory walk, language detection and ignore handling over a
//! synthetic tree of 50k tiny files in nested directories, with and without `.gitignore` churn.
//!
//! `poly_core::discover::discover` is `#[doc(hidden)]` but public, which is what the walk
//! measured here goes through; no engine runs.
//!
//! Run with: `cargo bench -p poly-core --bench discover_walk`.

mod common;

use std::hint::black_box;
use std::path::{Path, PathBuf};

use criterion::{Criterion, criterion_group, criterion_main};
use poly_core::config::Config;
use poly_core::discover::discover;
use poly_core::resolve::ConfigSet;

const FILE_COUNT: usize = 50_000;
const FILES_PER_DIR: usize = 50;
const DIRS_PER_LEVEL: usize = 10;

/// `FILE_COUNT` files in three directory levels (`g*/p*/m*`), `FILES_PER_DIR` per leaf directory.
fn nested_tree(root: &Path) {
    for dir in 0..FILE_COUNT / FILES_PER_DIR {
        let path = root
            .join(format!("g{}", dir / (DIRS_PER_LEVEL * DIRS_PER_LEVEL)))
            .join(format!("p{}", dir / DIRS_PER_LEVEL % DIRS_PER_LEVEL))
            .join(format!("m{}", dir % DIRS_PER_LEVEL));
        std::fs::create_dir_all(&path).expect("create dir");
        for file in 0..FILES_PER_DIR {
            let n = dir * FILES_PER_DIR + file;
            let ext = if n % 2 == 0 { "rs" } else { "py" };
            std::fs::write(path.join(format!("f{file}.{ext}")), "x = 1\n").expect("write fixture");
        }
    }
}

/// Every `.gitignore` ignores half the files and one directory, so the walk must evaluate rules at
/// each level instead of finding none.
fn add_gitignores(root: &Path) {
    std::fs::create_dir_all(root.join(".git")).expect("fake repo marker");
    std::fs::write(root.join(".gitignore"), "*.py\ng9/\n").expect("write gitignore");
    for g in 0..FILE_COUNT / FILES_PER_DIR / (DIRS_PER_LEVEL * DIRS_PER_LEVEL) {
        let dir = root.join(format!("g{g}"));
        std::fs::write(dir.join(".gitignore"), "p1/\n*.tmp\n").expect("write nested gitignore");
        for p in 0..DIRS_PER_LEVEL {
            std::fs::write(dir.join(format!("p{p}")).join(".gitignore"), "f1*.rs\n").expect("write nested gitignore");
        }
    }
}

fn bench_discover(c: &mut Criterion) {
    let config = Config::default();
    let mut group = c.benchmark_group("discover_walk");
    group.sample_size(10);

    for (name, with_ignores) in [("50k_no_gitignore", false), ("50k_gitignore_churn", true)] {
        let dir = tempfile::tempdir().expect("tempdir");
        nested_tree(dir.path());
        if with_ignores {
            add_gitignores(dir.path());
        }
        let paths = vec![PathBuf::from(dir.path())];
        let configs = ConfigSet::single(config.clone());
        group.bench_function(name, |b| {
            b.iter(|| black_box(discover(black_box(&paths), &configs, &[])));
        });
    }
    group.finish();
}

criterion_group!(benches, bench_discover);
criterion_main!(benches);
