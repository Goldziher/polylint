//! `poly lint` gives the adrlint tree-wide and lock rules the same verdict however the run spells its
//! paths (ADR 0041): a bare `docs/adr`, `./docs/adr`, an absolute path, one record, and any argument
//! order, a root-level file first included, which is what a pre-commit hook passes.
#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::process::Command;

use adrlint::{adr, lock};
use tempfile::TempDir;

const POLY: &str = env!("CARGO_BIN_EXE_poly");

const RECORD: &str = "\
# Use Rust

* Status: accepted
* Date: 2026-10-31

## Context and Problem Statement

We need a language for a new service.

## Decision Drivers

* Performance
* Safety

## Considered Options

* Rust
* Go

## Decision Outcome

Chosen option: Rust, because it is fast and safe.

### Consequences

Good.

### Confirmation

The service compiles and its benchmarks pass.

## More Information

* [Rust](https://www.rust-lang.org/)
";

struct Project {
    dir: TempDir,
    /// The project root with symlinks resolved, as a shell's working directory spells it.
    root: PathBuf,
    cache: TempDir,
}

impl Project {
    fn new(records: &[&str]) -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = std::fs::canonicalize(dir.path()).expect("canonicalize");
        let project = Self {
            dir,
            root,
            cache: tempfile::tempdir().expect("tempdir"),
        };
        project.write(
            "poly.toml",
            "[workspace]\nroot = true\n\n[lint.markdown.adrlint]\nenabled = true\n",
        );
        project.write("README.md", "# Readme\n");
        project.write("CHANGELOG.md", "# Changelog\n");
        for record in records {
            project.write(&format!("docs/adr/{record}"), RECORD);
        }
        project
    }

    fn write(&self, name: &str, content: &str) {
        let path = self.root.join(name);
        std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        std::fs::write(path, content).expect("write");
    }

    /// Lock every `(record, number)` at its current content.
    fn lock(&self, records: &[(&str, u32)]) {
        let mut manifest = lock::LockFile::default();
        for (record, number) in records {
            let key = format!("docs/adr/{record}");
            let content = std::fs::read_to_string(self.root.join(&key)).expect("read record");
            manifest.insert(key, lock::entry(&adr::parse(&content), *number));
        }
        self.write("adrlint.lock", &lock::render(&manifest).expect("render lock"));
    }

    /// `(result path, code)` of every tree-wide or lock finding of `poly lint <paths>`, sorted.
    fn tree_findings(&self, paths: &[&str]) -> Vec<(String, String)> {
        self.tree_findings_in("", paths)
    }

    /// [`tree_findings`](Self::tree_findings) with `poly` run from `below`, a directory of the project.
    fn tree_findings_in(&self, below: &str, paths: &[&str]) -> Vec<(String, String)> {
        let output = Command::new(POLY)
            .args(["lint", "--no-cache", "--workspace", "never", "--format", "json"])
            .args(paths)
            .env("POLY_CACHE_HOME", self.cache.path())
            .current_dir(self.dir.path().join(below))
            .output()
            .expect("run poly");
        let document: serde_json::Value = serde_json::from_slice(&output.stdout)
            .unwrap_or_else(|error| panic!("{paths:?}: {error}: {}", String::from_utf8_lossy(&output.stderr)));
        assert_eq!(
            document["errors"].as_array().map(Vec::len),
            Some(0),
            "{paths:?}: {document}"
        );
        let mut found = Vec::new();
        for result in document["results"].as_array().expect("results") {
            let path = result["path"].as_str().expect("path");
            for diagnostic in result["diagnostics"].as_array().expect("diagnostics") {
                let code = diagnostic["code"].as_str().unwrap_or_default();
                if diagnostic["engine"] == "adrlint" && (code.starts_with("MADR4") || code.starts_with("ADR5")) {
                    found.push((path.to_owned(), code.to_owned()));
                }
            }
        }
        found.sort();
        found
    }
}

/// `code` on each of `records` under `prefix`, the spelling the run gave the ADR directory.
fn expected(prefix: &str, records: &[&str], code: &str) -> Vec<(String, String)> {
    let mut expected: Vec<_> = records
        .iter()
        .map(|record| {
            (
                Path::new(prefix).join(record).to_string_lossy().into_owned(),
                code.to_owned(),
            )
        })
        .collect();
    expected.sort();
    expected
}

#[test]
fn a_duplicate_number_is_reported_however_the_directory_is_spelled() {
    let project = Project::new(&["0001-use-rust.md", "0001-use-go.md"]);
    let absolute = project.root.join("docs/adr").to_string_lossy().into_owned();
    let both = ["0001-use-rust.md", "0001-use-go.md"];
    let runs: &[(&[&str], &str)] = &[
        (&["docs/adr"], "docs/adr"),
        (&["./docs/adr"], "./docs/adr"),
        (&[&absolute], &absolute),
        (&["."], "./docs/adr"),
        (&["README.md", "docs/adr"], "docs/adr"),
        (&["docs/adr", "README.md"], "docs/adr"),
        (&["README.md", &absolute], &absolute),
    ];
    for (paths, prefix) in runs {
        assert_eq!(
            project.tree_findings(paths),
            expected(prefix, &both, "MADR43"),
            "poly lint {paths:?}"
        );
    }
    let one = ["0001-use-rust.md"];
    let files: &[&[&str]] = &[
        &["docs/adr/0001-use-rust.md"],
        &["README.md", "docs/adr/0001-use-rust.md"],
        &["docs/adr/0001-use-rust.md", "CHANGELOG.md"],
    ];
    for paths in files {
        assert_eq!(
            project.tree_findings(paths),
            expected("docs/adr", &one, "MADR43"),
            "poly lint {paths:?}"
        );
    }
    // From a subdirectory, where a relative path's ancestor chain never reaches the `poly.toml`.
    for (paths, prefix) in [(["adr"], "adr"), (["./adr"], "./adr"), (["."], "./adr")] {
        assert_eq!(
            project.tree_findings_in("docs", &paths),
            expected(prefix, &both, "MADR43"),
            "in docs/: poly lint {paths:?}"
        );
    }
    assert_eq!(
        project.tree_findings_in("docs/adr", &["0001-use-rust.md"]),
        expected("", &one, "MADR43"),
        "in docs/adr/: poly lint 0001-use-rust.md"
    );
}

#[test]
fn an_edited_locked_record_is_adr50_and_never_a_false_adr51() {
    let project = Project::new(&["0001-use-rust.md", "0002-use-go.md"]);
    project.lock(&[("0001-use-rust.md", 1), ("0002-use-go.md", 2)]);
    let absolute = project.root.join("docs/adr").to_string_lossy().into_owned();
    let directories: &[&[&str]] = &[
        &["docs/adr"],
        &["./docs/adr"],
        &[&absolute],
        &["."],
        &["README.md", "docs/adr"],
        &["docs/adr", "README.md"],
        &["CHANGELOG.md", "docs/adr/0001-use-rust.md"],
        &["docs/adr/0001-use-rust.md", "CHANGELOG.md"],
        &["docs/adr/0002-use-go.md"],
    ];
    for paths in directories {
        assert_eq!(project.tree_findings(paths), [], "an intact log: poly lint {paths:?}");
    }

    project.write(
        "docs/adr/0001-use-rust.md",
        &RECORD.replace("Rust, because", "Zig, because"),
    );
    let edited = ["0001-use-rust.md"];
    let runs: &[(&[&str], &str)] = &[
        (&["docs/adr"], "docs/adr"),
        (&["./docs/adr"], "./docs/adr"),
        (&[&absolute], &absolute),
        (&["."], "./docs/adr"),
        (&["README.md", "docs/adr"], "docs/adr"),
        (&["docs/adr", "README.md"], "docs/adr"),
        (&["docs/adr/0001-use-rust.md"], "docs/adr"),
        (&["CHANGELOG.md", "docs/adr/0001-use-rust.md"], "docs/adr"),
        (&["docs/adr/0001-use-rust.md", "CHANGELOG.md"], "docs/adr"),
    ];
    for (paths, prefix) in runs {
        assert_eq!(
            project.tree_findings(paths),
            expected(prefix, &edited, "ADR50"),
            "poly lint {paths:?}"
        );
    }
    assert_eq!(
        project.tree_findings(&["README.md", "docs/adr/0002-use-go.md"]),
        [],
        "a run naming only the intact record reports nothing"
    );
}
