//! Python type checking with ty, as a project-check engine.
//!
//! ty is consumed as the published `ty_*` crates at the exact version that ships with the pinned
//! ruff (`=0.0.21`, see the workspace `Cargo.toml`). `ty_project` is unpublished, so the minimum of
//! it is ported: the salsa database (`db`), the program settings (`settings`) and the system
//! the check reads through (`system`). The crate version is folded into `TyEngine::version`, so
//! bumping it invalidates cached results.
//!
//! # Configuration
//!
//! The engine is opt-in and named `python-types`:
//!
//! ```toml
//! [engines.python-types]
//! backend = "ty"
//! python_version = "3.12"      # default: the interpreter's, else ty's latest
//! src = ["src"]                # first-party roots; default: the project root and `src/`
//! extra_paths = ["stubs"]      # searched before first-party code
//! ```
//!
//! Third-party imports resolve through the `site-packages` directories of the interpreter the
//! host resolved (`ProjectEnvironment`); the check never executes it. ty's own `ty.toml` and
//! `[tool.ty]` are not read.

mod db;
mod diagnostics;
mod order;
mod settings;
mod system;

use std::panic::AssertUnwindSafe;
use std::path::PathBuf;

use anyhow::{Context, anyhow, bail};
use order::merge_by_index;
use poly_engine_api::engine::{
    Capabilities, Diagnostic, Engine, OptionKeys, OptionTable, OptionType, ProjectAnalysis, ProjectFile, ProjectInputs,
    ProjectRequest, WireSelection,
};
use poly_engine_api::language::Language;
use ruff_db::Db as SourceDb;
use ruff_db::files::{File, system_path_to_file};
use ruff_db::system::SystemPathBuf;
use rustc_hash::{FxHashMap, FxHashSet};
use ty_python_semantic::Db;

use crate::ruff::LANGUAGES;
use db::TyDb;
use system::OverlaySystem;

/// The engine id, and so the `[engines.<id>]` table that configures it.
pub const ENGINE_NAME: &str = "python-types";

/// The `backend` value that switches this engine on.
const BACKEND: &str = "ty";

/// The `backend` value served by the `pyrefly` component instead.
const OTHER_BACKEND: &str = "pyrefly";

const OPTION_KEYS: &[(&str, OptionType)] = &[
    ("backend", OptionType::STRING),
    ("python_version", OptionType::STRING),
    ("src", OptionType::ARRAY),
    ("extra_paths", OptionType::ARRAY),
];

/// The ty-backed Python type checker.
#[derive(Debug, Default, Clone, Copy)]
pub struct TyEngine;

impl Engine for TyEngine {
    // No per-file hook is overridden, so a component host never loads the component to ask one.
    fn overridden_hooks(&self) -> u32 {
        0
    }

    fn name(&self) -> &'static str {
        ENGINE_NAME
    }

    fn languages(&self) -> &'static [Language] {
        LANGUAGES
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            lint: false,
            format: false,
            fix: false,
            project: true,
        }
    }

    fn option_keys(&self, table: OptionTable) -> OptionKeys {
        match table {
            OptionTable::Lint => OptionKeys::declared(OPTION_KEYS),
            OptionTable::Format => OptionKeys::declared(&[]),
            OptionTable::CrossCuttingLint => OptionKeys::UNCHECKED,
        }
    }

    fn version(&self) -> &str {
        "ty-0.0.21+proj1"
    }

    fn project_analysis(&self) -> Option<&dyn ProjectAnalysis> {
        Some(self)
    }
}

impl ProjectAnalysis for TyEngine {
    /// Planned only once `backend` is set, so a Python project that never mentions it pays nothing.
    fn project_inputs(&self) -> ProjectInputs {
        ProjectInputs {
            gate_key: Some("backend".to_owned()),
            ..ProjectInputs::default()
        }
    }

    fn selection(&self) -> Option<WireSelection> {
        Some(WireSelection {
            table: ENGINE_NAME.to_owned(),
            key: "backend".to_owned(),
            value: BACKEND.to_owned(),
            default: true,
        })
    }

    fn project_check(&self, req: &ProjectRequest<'_>) -> anyhow::Result<Vec<(PathBuf, Vec<Diagnostic>)>> {
        match req.config.options.get("backend") {
            None => return Ok(Vec::new()),
            Some(value) if value.as_str() == Some(BACKEND) => {}
            Some(other) => bail!(
                "unknown `backend` {other} for `{ENGINE_NAME}`; the backends are \"{BACKEND}\" and \"{OTHER_BACKEND}\""
            ),
        }
        let files: Vec<&ProjectFile> = req
            .files
            .iter()
            .filter(|file| file.language == Language::Python)
            .collect();
        if files.is_empty() {
            return Ok(Vec::new());
        }
        // ty's inference recurses deeply; the 16 MiB it asks for is more than a caller's thread has.
        let outcome = std::thread::scope(|scope| {
            std::thread::Builder::new()
                .name("poly-ty".to_string())
                .stack_size(ruff_db::STACK_SIZE)
                .spawn_scoped(scope, || check(req, &files))
                .context("cannot start the ty worker thread")
                .map(std::thread::ScopedJoinHandle::join)
        })?;
        outcome.unwrap_or_else(|panic| std::panic::resume_unwind(panic))
    }
}

/// Type-check `files` with one database, reporting under each file's path as the host gave it.
fn check(req: &ProjectRequest<'_>, files: &[&ProjectFile]) -> anyhow::Result<Vec<(PathBuf, Vec<Diagnostic>)>> {
    let root = settings::system_path(req.root)?;
    let mut overlay: FxHashMap<SystemPathBuf, std::sync::Arc<str>> = FxHashMap::default();
    let mut targets = Vec::with_capacity(files.len());
    for file in files {
        let path = settings::system_path(&file.path)?;
        overlay.insert(path.clone(), file.content.clone());
        targets.push((path, &file.path));
    }
    let mut db = TyDb::new(OverlaySystem::new(&root, overlay));
    let program = settings::program_settings(req, &root, db.system(), db.vendored())?;
    db.configure(program, &root);

    let mut by_file: FxHashMap<File, PathBuf> = FxHashMap::default();
    let mut order: Vec<File> = Vec::with_capacity(targets.len());
    for (path, original) in &targets {
        match system_path_to_file(&db, path) {
            Ok(file) => {
                by_file.insert(file, (*original).clone());
                order.push(file);
            }
            Err(error) => tracing::warn!(path = %path, %error, "ty: file is not readable"),
        }
    }

    let mut seen = FxHashSet::default();
    order.retain(|file| seen.insert(*file));

    let threads = req.threads.unwrap_or(1).min(order.len());
    let per_file: Vec<Vec<Reported>> = if threads <= 1 {
        order
            .iter()
            .map(|file| check_one(&db, *file, &by_file))
            .collect::<anyhow::Result<_>>()?
    } else {
        check_parallel(&db, &order, &by_file, threads)?
    };
    Ok(per_file.into_iter().flatten().collect())
}

/// One reported entry: the host's path for a file and the findings anchored there.
type Reported = (PathBuf, Vec<Diagnostic>);

/// Check `file` and report its findings, preceded by any anchored in another project file.
fn check_one(db: &TyDb, file: File, by_file: &FxHashMap<File, PathBuf>) -> anyhow::Result<Vec<Reported>> {
    let mut reported = Vec::new();
    let mut diagnostics = Vec::new();
    for diagnostic in db.check_file(file) {
        match diagnostics::convert(db, &diagnostic) {
            Some((anchor, converted)) if anchor == file => diagnostics.push(converted),
            Some((anchor, converted)) => match by_file.get(&anchor) {
                Some(path) => reported.push((path.clone(), vec![converted])),
                None => tracing::debug!(code = ?converted.code, "ty: diagnostic anchored outside the project"),
            },
            None => tracing::debug!(message = %diagnostic.concise_message(), "ty: diagnostic without a file"),
        }
    }
    if !diagnostics.is_empty() {
        let path = by_file.get(&file).ok_or_else(|| anyhow!("ty: unknown file"))?;
        reported.push((path.clone(), diagnostics));
    }
    Ok(reported)
}

/// Check `order` on a pool of `threads` workers, each over its own salsa clone of `db`.
///
/// Like `ty_project`, every task runs under `Cancelled::catch`. Nothing in a one-shot check writes
/// an input, so a cancellation is not expected; it is reported as an error rather than a panic.
fn check_parallel(
    db: &TyDb,
    order: &[File],
    by_file: &FxHashMap<File, PathBuf>,
    threads: usize,
) -> anyhow::Result<Vec<Vec<Reported>>> {
    let next = std::sync::atomic::AtomicUsize::new(0);
    let clones: Vec<TyDb> = (0..threads).map(|_| db.clone()).collect();
    let mut results: Vec<(usize, anyhow::Result<Vec<Reported>>)> = Vec::with_capacity(order.len());
    std::thread::scope(|scope| -> anyhow::Result<()> {
        let next = &next;
        let mut workers = Vec::with_capacity(threads);
        for (index, worker_db) in clones.into_iter().enumerate() {
            let worker = std::thread::Builder::new()
                .name(format!("poly-ty-{index}"))
                .stack_size(ruff_db::STACK_SIZE)
                .spawn_scoped(scope, move || {
                    let mut done = Vec::new();
                    loop {
                        let position = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        let Some(file) = order.get(position) else { break };
                        let checked =
                            salsa::Cancelled::catch(AssertUnwindSafe(|| check_one(&worker_db, *file, by_file)))
                                .unwrap_or_else(|cancelled| Err(anyhow!("ty: the check was cancelled: {cancelled}")));
                        done.push((position, checked));
                    }
                    done
                })
                .context("cannot start a ty worker")?;
            workers.push(worker);
        }
        for worker in workers {
            let done = worker.join().map_err(|_| anyhow!("a ty worker panicked"))?;
            results.extend(done);
        }
        Ok(())
    })?;
    merge_by_index(results).into_iter().collect::<anyhow::Result<Vec<_>>>()
}
