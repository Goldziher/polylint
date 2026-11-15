//! The references one run shares across every file, bundled so the per-file and
//! per-phase steps take one argument instead of a dozen.

use std::path::PathBuf;

use poly_cache::ResultCache;

use super::batch::{BatchPrefill, LintPrefill};
use super::fast::FastPath;
use super::plan::RunPlan;
use super::selection::EngineSelection;
use super::skips::SizeLimits;
use super::types::RunOptions;
use crate::discover::DiscoveredFile;
use crate::filter::PerFileIgnores;
use crate::language::Language;
use crate::resolve::ConfigSet;

/// What the run-level steps (planning, the project phase, result assembly) read.
pub(super) struct RunEnv<'a> {
    pub(super) paths: &'a [PathBuf],
    pub(super) files: &'a [DiscoveredFile],
    pub(super) configs: &'a ConfigSet,
    pub(super) opts: &'a RunOptions,
    pub(super) plan: &'a RunPlan,
    pub(super) cache: &'a ResultCache,
    pub(super) selection: EngineSelection<'a>,
}

/// Per-config lint inputs resolved once per run rather than once per file.
pub(super) struct LintSetup {
    pub(super) ignores: Vec<PerFileIgnores>,
    pub(super) ignore_bases: Vec<Vec<PathBuf>>,
    pub(super) act_on_generated: Vec<bool>,
    pub(super) size_limits: SizeLimits,
}

/// What `lint_one` shares across files.
pub(super) struct LintCtx<'a, 'f> {
    pub(super) plan: &'a RunPlan,
    pub(super) fix: bool,
    pub(super) fix_generated: bool,
    pub(super) collect_debug: bool,
    pub(super) setup: &'a LintSetup,
    pub(super) externally_linted: &'a [Language],
    pub(super) prefilled: &'a LintPrefill,
    pub(super) fast: Option<&'a FastPath<'f>>,
}

/// What `format_one` shares across files.
pub(super) struct FormatCtx<'a, 'f> {
    pub(super) plan: &'a RunPlan,
    pub(super) cache: &'a ResultCache,
    pub(super) write: bool,
    pub(super) fix_generated: bool,
    pub(super) collect_debug: bool,
    pub(super) act_on_generated: &'a [bool],
    pub(super) limits: &'a SizeLimits,
    pub(super) prefilled: &'a BatchPrefill,
    pub(super) fast: Option<&'a FastPath<'f>>,
}
