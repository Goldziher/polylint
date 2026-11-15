//! Deterministic synthetic corpora shared by the runner and discovery benches.
//!
//! Everything is generated from the file index, with no RNG and no network, so two runs of a bench
//! see byte-identical trees.
#![allow(dead_code)] // each bench binary uses a different subset

use std::path::Path;

use poly_core::runner::RunOptions;
use tempfile::TempDir;

/// Route the result cache into a directory owned by the bench, so a warm or cold run never touches
/// the developer's real cache.
pub fn isolate_cache(dir: &Path) {
    // SAFETY: called from `main`-time bench setup before any worker thread is spawned.
    unsafe { std::env::set_var(poly_cache::CACHE_HOME_ENV, dir) };
}

/// Run options that stay offline: a component that is not linked into the default build is skipped
/// instead of downloaded.
pub fn offline_options(no_cache: bool, jobs: Option<usize>) -> RunOptions {
    RunOptions {
        no_cache,
        jobs,
        explicit_config: true,
        on_missing: Some(poly_config::v2::OnMissing::Skip),
        install: Some(poly_config::v2::ComponentInstallPolicy::Never),
        ..RunOptions::default()
    }
}

/// A short Rust file; `i` keeps every file distinct.
pub fn rust_file(i: usize) -> String {
    format!(
        "pub struct Item{i} {{\nid: usize,\nlabel: String,\n}}\n\nimpl Item{i} {{\n\
         pub fn new(id: usize) -> Self {{\nSelf {{\nid,\nlabel: String::new(),\n}}\n}}\n\
         pub fn bump(&mut self, by: usize) -> usize {{\nfor _ in 0..by {{\nself.id += 1;\n}}\nself.id\n}}\n}}\n"
    )
}

/// A short Python file with a docstring.
pub fn python_file(i: usize) -> String {
    format!(
        "import os\n\n\nclass Item{i}:\n    \"\"\"Item {i}.\"\"\"\n\n    def __init__(self, id):\n        self.id = id\n\n    \
         def bump(self, by):\n        for _ in range(by):\n            self.id += 1\n        return os.getpid() + self.id\n"
    )
}

/// A small JSON document.
pub fn json_file(i: usize) -> String {
    format!("{{\"id\": {i}, \"name\": \"item-{i}\", \"tags\": [\"a\", \"b\", \"c\"], \"nested\": {{\"ok\": true}}}}\n")
}

/// A small YAML document.
pub fn yaml_file(i: usize) -> String {
    format!("id: {i}\nname: item-{i}\ntags:\n  - a\n  - b\nnested:\n  ok: true\n")
}

/// A small Markdown document.
pub fn markdown_file(i: usize) -> String {
    format!("# Item {i}\n\nSome prose for item {i}.\n\n- one\n- two\n\n```rust\nfn main() {{}}\n```\n")
}

/// A small TOML document.
pub fn toml_file(i: usize) -> String {
    format!("[package]\nname = \"item-{i}\"\nversion = \"0.{i}.0\"\n\n[dependencies]\nserde = \"1\"\n")
}

/// A small shell script.
pub fn shell_file(i: usize) -> String {
    format!("#!/usr/bin/env bash\nset -euo pipefail\n\nfor n in 1 2 3; do\n  echo \"item {i} $n\"\ndone\n")
}

/// `count` files of one kind, fanned out over directories so no directory holds more than 100.
pub fn write_flat(root: &Path, count: usize, ext: &str, content: impl Fn(usize) -> String) {
    for i in 0..count {
        let dir = root.join(format!("d{}", i / 100));
        std::fs::create_dir_all(&dir).expect("create dir");
        std::fs::write(dir.join(format!("file_{i}.{ext}")), content(i)).expect("write fixture");
    }
}

/// A tree of `count` small Rust files.
pub fn tiny_tree(count: usize) -> TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    write_flat(dir.path(), count, "rs", rust_file);
    dir
}

/// A single Rust file of about `bytes` bytes made of repeated, distinct items.
pub fn large_file_tree(bytes: usize) -> TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut src = String::with_capacity(bytes + 512);
    let mut i = 0;
    while src.len() < bytes {
        src.push_str(&rust_file(i));
        i += 1;
    }
    std::fs::write(dir.path().join("big.rs"), src).expect("write fixture");
    dir
}

/// `per_kind` files of each of Rust, Python, JSON, YAML, Markdown, TOML and shell.
pub fn mixed_tree(per_kind: usize) -> TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    write_flat(&root.join("rs"), per_kind, "rs", rust_file);
    write_flat(&root.join("py"), per_kind, "py", python_file);
    write_flat(&root.join("json"), per_kind, "json", json_file);
    write_flat(&root.join("yaml"), per_kind, "yaml", yaml_file);
    write_flat(&root.join("md"), per_kind, "md", markdown_file);
    write_flat(&root.join("toml"), per_kind, "toml", toml_file);
    write_flat(&root.join("sh"), per_kind, "sh", shell_file);
    dir
}
