//! The files a catalog tool without a recipe takes: the catalog's `path_globs`, or the user's
//! `[tools.<name>] files` in their place, less the user's `exclude`. A recipe tool has its own
//! filter, relative to its working directory (`recipe::Filter`).

use std::path::Path;

use globset::GlobSet;
use poly_config::ToolConfig;

/// Include and exclude globs, matched against a path relative to the run root.
#[derive(Default)]
pub(super) struct FileFilter {
    include: Option<GlobSet>,
    exclude: Option<GlobSet>,
    /// The patterns, for the cache key: a result cached under other patterns must not be replayed.
    key: String,
}

impl FileFilter {
    /// The catalog's `path_globs` alone; they are poly's data, so a malformed one is dropped, not refused.
    pub(super) fn catalog(path_globs: &[String]) -> Self {
        FileFilter {
            include: (!path_globs.is_empty()).then(|| poly_engine_api::glob_set::build_glob_set_lossy(path_globs)),
            exclude: None,
            key: format!("path_globs={path_globs:?}"),
        }
    }

    /// `path_globs` with the user's `files` and `exclude` applied. A malformed user pattern is an
    /// error: an exclude that matched nothing would hand the tool the files it was meant to skip.
    pub(super) fn for_config(path_globs: &[String], config: &ToolConfig) -> Result<Self, String> {
        if config.files.is_none() && config.exclude.is_none() {
            return Ok(Self::catalog(path_globs));
        }
        let include: Vec<String> = config
            .files
            .as_ref()
            .map_or_else(|| path_globs.to_vec(), |files| files.as_slice().to_vec());
        let exclude: Vec<String> = config
            .exclude
            .as_ref()
            .map(|patterns| patterns.as_slice().to_vec())
            .unwrap_or_default();
        let build = |patterns: &[String]| -> Result<Option<GlobSet>, String> {
            if patterns.is_empty() {
                return Ok(None);
            }
            poly_engine_api::glob_set::build_glob_set(patterns)
                .map(Some)
                .map_err(|error| format!("`files`/`exclude`: {error}"))
        };
        Ok(FileFilter {
            include: build(&include)?,
            exclude: build(&exclude)?,
            key: format!("files={include:?}:exclude={exclude:?}"),
        })
    }

    /// Whether the tool takes `path`.
    pub(super) fn matches(&self, path: &Path) -> bool {
        let relative = poly_engine_api::run_root::relative(path);
        self.include.as_ref().is_none_or(|set| set.is_match(relative))
            && !self.exclude.as_ref().is_some_and(|set| set.is_match(relative))
    }

    /// The patterns as a cache-key fragment.
    pub(super) fn key(&self) -> &str {
        &self.key
    }
}
