//! Cross-file link-fragment checking (`MD051` across files) as a project check.
//!
//! rumdl's per-file `lint` validates `#fragment` links against the file's own headings only; a
//! `other.md#heading` link needs the headings of `other.md`, which rumdl keeps in a workspace
//! index built from every file. This module builds that index from the Markdown the host hands
//! over and runs rumdl's own `cross_file_check`, so the verdicts are rumdl's.
//!
//! The host caches the result on the digest of every file in the request, so editing, adding or
//! removing any Markdown file in the tree re-runs the check. The walk is the whole tree (see
//! [`project_inputs`]), because a link may point anywhere in it.
//!
//! `MD057` (relative target exists) stays in the per-file pass: it reads the filesystem for
//! non-Markdown targets, which no project input can model.

use std::collections::BTreeMap;
use std::path::PathBuf;

use poly_engine_api::engine::{Diagnostic, ProjectDirs, ProjectFile, ProjectInputs, ProjectRequest};
use poly_engine_api::language::Language;
use poly_engine_api::template::contains_go_template_markdown;
use rumdl_lib::config::Config as RumdlConfig;
use rumdl_lib::rule::Rule;
use rumdl_lib::rules::{all_rules, filter_rules};
use rumdl_lib::workspace_index::{FileIndex, WorkspaceIndex};

use super::{build_rumdl_config, map_warning};

/// The option that switches the cross-file check on; also the host's gate for the project phase.
pub(super) const ENABLE_KEY: &str = "cross_file_links";

/// The option naming the directory the check is about, relative to the project root.
pub(super) const DIR_KEY: &str = "cross_file_links_dir";

/// The rule whose cross-file half this check runs.
const FRAGMENT_RULE: &str = "MD051";

/// Whether the cross-file check is switched on. A present-but-false key is off.
pub(super) fn enabled(cfg: &poly_engine_api::config::EngineConfig) -> bool {
    cfg.options.get("cross_file_links").and_then(toml::Value::as_bool) == Some(true)
}

/// The whole Markdown tree under `cross_file_links_dir` (default the project root), unaffected by
/// `[discovery] exclude`, so a link into an excluded file still resolves.
pub(super) fn project_inputs() -> ProjectInputs {
    ProjectInputs {
        gate_key: Some(ENABLE_KEY.to_owned()),
        files: Vec::new(),
        dirs: Some(ProjectDirs {
            dir_key: DIR_KEY.to_owned(),
            dir_default: ".".to_owned(),
            paths_key: None,
            recursive_key: None,
            recursive_default: true,
            whole_tree: true,
            file_names: Vec::new(),
            list_unserved: false,
        }),
    }
}

/// The active rules for one Markdown flavor, narrowed to the fragment rule, with its config.
struct Flavor {
    config: RumdlConfig,
    rules: Vec<Box<dyn Rule>>,
}

impl Flavor {
    fn new(req: &ProjectRequest<'_>, language: &Language) -> Self {
        let config = build_rumdl_config(req.config, language);
        let mut rules = filter_rules(&all_rules(&config), &config.global);
        rules.retain(|rule| rule.name() == FRAGMENT_RULE);
        Self { config, rules }
    }
}

/// Below this many files the index build stays on the calling thread: spawning costs more than it saves.
const PARALLEL_MIN_FILES: usize = 32;

/// One [`FileIndex`] per served file, in `served` order, built on up to `threads` worker threads
/// (every core when `None`). Falls back to the calling thread when a worker cannot be spawned.
fn build_indexes(served: &[(&ProjectFile, &Flavor)], threads: Option<usize>) -> Vec<FileIndex> {
    let index_of = |(file, flavor): &(&ProjectFile, &Flavor)| {
        rumdl_lib::build_file_index_only_with_config(
            &file.content,
            &flavor.rules,
            flavor.config.markdown_flavor(),
            Some(file.path.clone()),
            &flavor.config,
        )
    };
    let workers = threads
        .or_else(|| std::thread::available_parallelism().ok().map(Into::into))
        .unwrap_or(1)
        .clamp(1, served.len().max(1));
    if workers == 1 || served.len() < PARALLEL_MIN_FILES {
        return served.iter().map(index_of).collect();
    }
    let chunk = served.len().div_ceil(workers);
    std::thread::scope(|scope| {
        let handles: Vec<_> = served
            .chunks(chunk)
            .map(|files| {
                let spawned = std::thread::Builder::new()
                    .spawn_scoped(scope, move || files.iter().map(index_of).collect::<Vec<_>>());
                (files, spawned)
            })
            .collect();
        handles
            .into_iter()
            .flat_map(
                |(files, spawned)| match spawned.map(std::thread::ScopedJoinHandle::join) {
                    Ok(Ok(indexes)) => indexes,
                    _ => files.iter().map(index_of).collect(),
                },
            )
            .collect()
    })
}

/// Runs `MD051`'s cross-file check over the Markdown in `req.files`, grouped by the linking file.
pub(super) fn check(req: &ProjectRequest<'_>) -> anyhow::Result<Vec<(PathBuf, Vec<Diagnostic>)>> {
    if !enabled(req.config) {
        return Ok(Vec::new());
    }
    let markdown = Flavor::new(req, &Language::Markdown);
    let mdx = Flavor::new(req, &Language::Mdx);
    let mut served: Vec<(&ProjectFile, &Flavor)> = Vec::new();
    for file in req.files {
        let flavor = match file.language {
            Language::Markdown => &markdown,
            Language::Mdx => &mdx,
            _ => continue,
        };
        if !contains_go_template_markdown(&file.content) {
            served.push((file, flavor));
        }
    }

    let indexes = build_indexes(&served, req.threads);
    let mut workspace = WorkspaceIndex::new();
    for ((file, _), index) in served.iter().zip(indexes) {
        workspace.insert_file(file.path.clone(), index);
    }

    let mut grouped: BTreeMap<PathBuf, Vec<Diagnostic>> = BTreeMap::new();
    for (file, flavor) in &served {
        let Some(index) = workspace.get_file(&file.path) else {
            continue;
        };
        let warnings =
            rumdl_lib::run_cross_file_checks(&file.path, index, &flavor.rules, &workspace, Some(&flavor.config))
                .map_err(|e| anyhow::anyhow!("rumdl cross-file check: {e}"))?;
        if !warnings.is_empty() {
            grouped
                .entry(file.path.clone())
                .or_default()
                .extend(warnings.iter().map(|w| map_warning(w, "rumdl")));
        }
    }
    Ok(grouped.into_iter().collect())
}
