use std::path::PathBuf;

use poly_engine_api::config::GlobalDefaults;
use poly_engine_api::language::Language;

use super::*;

fn config(options: &[(&str, toml::Value)]) -> EngineConfig {
    EngineConfig {
        globals: GlobalDefaults::default(),
        indent_width: 2,
        options: options.iter().cloned().map(|(k, v)| (k.to_owned(), v)).collect(),
    }
}

fn markdown(path: &str, content: &str) -> ProjectFile {
    ProjectFile {
        path: PathBuf::from(path),
        language: Language::Markdown,
        content: content.into(),
    }
}

fn manifest_file(path: &str, manifest: &LockFile) -> ProjectFile {
    ProjectFile {
        path: PathBuf::from(path),
        language: Language::Other("adrlint.lock".to_owned()),
        content: lock::render(manifest).expect("render").into(),
    }
}

fn record(status: &str, title: &str) -> String {
    format!("---\nstatus: {status}\ndate: 2026-01-01\n---\n\n# {title}\n\n## Context and Problem Statement\n\nText.\n")
}

fn lock_op(all: bool) -> WireAdrOp {
    WireAdrOp::Lock { all }
}

fn parsed(plan: &WireAdrPlan) -> LockFile {
    assert_eq!(plan.writes.len(), 1, "{plan:?}");
    lock::parse_manifest(&plan.writes[0].content).expect("the written manifest parses")
}

#[test]
fn lock_records_only_accepted_and_rejected_records() {
    let accepted = record("accepted", "Use Rust");
    let files = [
        markdown("docs/adr/0001-use-rust.md", &accepted),
        markdown("docs/adr/0002-use-go.md", &record("proposed", "Use Go")),
        markdown("docs/adr/0003-use-zig.md", &record("rejected", "Use Zig")),
        markdown("docs/adr/README.md", "# Decisions\n"),
    ];
    let plan = plan(&lock_op(false), &config(&[]), &files).expect("lock");

    assert_eq!(plan.writes[0].path, "adrlint.lock");
    assert!(!plan.writes[0].create);
    let written = parsed(&plan);
    assert_eq!(
        written.adrs.keys().collect::<Vec<_>>(),
        ["docs/adr/0001-use-rust.md", "docs/adr/0003-use-zig.md"]
    );
    assert_eq!(
        written.entry("docs/adr/0001-use-rust.md"),
        Some(&lock::entry(&adr::parse(&accepted), 1))
    );
    assert_eq!(
        plan.changes,
        ["add docs/adr/0001-use-rust.md", "add docs/adr/0003-use-zig.md"]
    );
    assert_eq!(plan.summary, "locked 2 ADR(s) in adrlint.lock");
}

#[test]
fn lock_writes_nothing_when_the_manifest_is_current() {
    let accepted = record("accepted", "Use Rust");
    let mut manifest = LockFile::default();
    manifest.insert("docs/adr/0001-use-rust.md", lock::entry(&adr::parse(&accepted), 1));
    let files = [
        markdown("docs/adr/0001-use-rust.md", &accepted),
        manifest_file("adrlint.lock", &manifest),
    ];
    let plan = plan(&lock_op(false), &config(&[]), &files).expect("lock");
    assert!(plan.writes.is_empty(), "{plan:?}");
    assert!(plan.changes.is_empty());
    assert_eq!(plan.summary, "adrlint.lock is up to date");
}

#[test]
fn lock_honours_the_configured_manifest_path() {
    let files = [markdown("adr/0001-use-rust.md", &record("accepted", "Use Rust"))];
    let cfg = config(&[("lock", toml::Value::String("./docs/adr.lock".into()))]);
    let plan = plan(&lock_op(false), &cfg, &files).expect("lock");
    assert_eq!(plan.writes[0].path, "docs/adr.lock");
}

#[test]
fn lock_refuses_an_empty_lock_unless_all_is_passed() {
    let files = [markdown("docs/adr/0001-use-go.md", &record("proposed", "Use Go"))];
    let error = plan(&lock_op(false), &config(&[]), &files).expect_err("nothing to lock");
    assert!(error.to_string().contains("--all"), "{error}");

    let plan = plan(&lock_op(true), &config(&[]), &files).expect("lock --all");
    assert_eq!(
        parsed(&plan).adrs.keys().collect::<Vec<_>>(),
        ["docs/adr/0001-use-go.md"]
    );
}

#[test]
fn lock_never_locks_the_template() {
    let files = [
        markdown("docs/adr/0000-template.md", &record("accepted", "Template")),
        markdown("docs/adr/0000-use-rust.md", &record("accepted", "Use Rust")),
    ];
    let plan = plan(&lock_op(true), &config(&[]), &files).expect("lock");
    assert_eq!(
        parsed(&plan).adrs.keys().collect::<Vec<_>>(),
        ["docs/adr/0000-use-rust.md"]
    );
}

#[test]
fn lock_requires_all_to_refresh_a_stale_manifest() {
    let manifest = LockFile {
        canonical: lock::CANONICAL_VERSION + 1,
        ..LockFile::default()
    };
    let files = [
        markdown("docs/adr/0001-use-rust.md", &record("accepted", "Use Rust")),
        manifest_file("adrlint.lock", &manifest),
    ];
    let error = plan(&lock_op(false), &config(&[]), &files).expect_err("stale");
    assert!(error.to_string().contains("canonical version"), "{error}");

    let refreshed = parsed(&plan(&lock_op(true), &config(&[]), &files).expect("lock --all"));
    assert_eq!(refreshed.canonical, lock::CANONICAL_VERSION);
}

#[test]
fn unlock_removes_one_entry() {
    let rust = record("accepted", "Use Rust");
    let go = record("accepted", "Use Go");
    let mut manifest = LockFile::default();
    manifest.insert("docs/adr/0001-use-rust.md", lock::entry(&adr::parse(&rust), 1));
    manifest.insert("docs/adr/0002-use-go.md", lock::entry(&adr::parse(&go), 2));
    let files = [manifest_file("adrlint.lock", &manifest)];
    let op = WireAdrOp::Unlock {
        path: "docs/adr/0001-use-rust.md".into(),
    };
    let plan = plan(&op, &config(&[]), &files).expect("unlock");
    assert_eq!(
        parsed(&plan).adrs.keys().collect::<Vec<_>>(),
        ["docs/adr/0002-use-go.md"]
    );
    assert_eq!(plan.changes, ["remove docs/adr/0001-use-rust.md"]);
    assert_eq!(plan.summary, "unlocked docs/adr/0001-use-rust.md");
}

#[test]
fn unlock_reports_a_record_that_is_not_locked() {
    let files = [manifest_file("adrlint.lock", &LockFile::default())];
    let op = WireAdrOp::Unlock {
        path: "docs/adr/0009-missing.md".into(),
    };
    let error = plan(&op, &config(&[]), &files).expect_err("not locked");
    assert!(
        error.to_string().contains("no entry for docs/adr/0009-missing.md"),
        "{error}"
    );

    let error = plan(&op, &config(&[]), &[]).expect_err("no manifest");
    assert!(error.to_string().contains("does not exist"), "{error}");
}

fn new(dir: &str, title: &str) -> WireAdrOp {
    WireAdrOp::New {
        dir: dir.into(),
        title: title.into(),
        date: "2026-11-15".into(),
    }
}

#[test]
fn new_takes_the_next_number_and_fills_the_template() {
    let template = "---\nstatus: accepted\ndate: 2000-01-01\ndecision-makers: []\n---\n\n# Template\n\n## Context and Problem Statement\n\nWhy?\n";
    let files = [
        markdown("docs/adr/0000-template.md", template),
        markdown("docs/adr/0000-use-rust.md", &record("accepted", "Use Rust")),
        markdown("docs/adr/0001-use-go.md", &record("accepted", "Use Go")),
        markdown("other/0007-elsewhere.md", &record("accepted", "Elsewhere")),
    ];
    let plan = plan(&new("docs/adr", "Choose a Database!"), &config(&[]), &files).expect("new");
    assert_eq!(plan.writes.len(), 1);
    let write = &plan.writes[0];
    assert_eq!(write.path, "docs/adr/0002-choose-a-database.md");
    assert!(write.create);
    assert_eq!(
        write.content,
        "---\nstatus: proposed\ndate: 2026-11-15\ndecision-makers: []\n---\n\n# Choose a Database!\n\n## Context and Problem Statement\n\nWhy?\n"
    );
    assert_eq!(plan.summary, "created docs/adr/0002-choose-a-database.md");
}

#[test]
fn new_starts_an_empty_log_at_the_configured_number_with_the_madr_template() {
    let cfg = config(&[("start", toml::Value::Integer(0))]);
    let started = plan(&new("docs/adr", "Use Rust"), &cfg, &[]).expect("new");
    let write = &started.writes[0];
    assert_eq!(write.path, "docs/adr/0000-use-rust.md");
    assert!(write.content.starts_with("---\nstatus: proposed\ndate: 2026-11-15\n"));
    assert!(write.content.contains("\n# Use Rust\n"));

    let unconfigured = plan(&new("docs/adr", "Use Rust"), &config(&[]), &[]).expect("new");
    assert_eq!(unconfigured.writes[0].path, "docs/adr/0001-use-rust.md");
}

#[test]
fn new_rejects_an_unusable_title() {
    for title in ["", "   ", "!!!", "a\u{7}b"] {
        assert!(
            plan(&new("docs/adr", title), &config(&[]), &[]).is_err(),
            "{title:?} must be rejected"
        );
    }
}

#[test]
fn a_template_is_recognised_by_name_alone() {
    assert!(is_template(Path::new("docs/adr/0000-template.md")));
    assert!(is_template(Path::new("docs/adr/template.md")));
    assert!(is_template(Path::new("docs/adr/0000-TEMPLATE.md")));
    assert!(!is_template(Path::new("docs/adr/0001-use-a-template.md")));
    assert!(!is_template(Path::new("docs/adr/x-template.md")));
}

#[test]
fn a_long_title_is_cut_at_a_word_boundary() {
    let slug = slugify(&"word ".repeat(30));
    let cut = truncate_slug(&slug);
    assert!(cut.len() <= MAX_SLUG_LEN);
    assert!(!cut.ends_with('-'));
    assert!(cut.ends_with("word"));
}
