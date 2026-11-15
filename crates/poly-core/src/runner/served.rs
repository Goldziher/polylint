//! The files a project check is served, for a host operation that must act on the same set.

use std::path::Path;

use crate::Config;
use crate::engine::{ProjectDirs, ProjectFile};
use crate::language::Language;

use super::RunOptions;

/// The files a project-check engine that declared `declared` (and serves `served`) is handed for the
/// project at `root`, under `config` and `opts` (`--exclude`, `--config`, the `extends` resolver and
/// the generated-files override): the walk a run's project check makes, through the same code, so a
/// host operation over that set (`poly adr lock`, ADR 0061) never judges a file the check does not see
/// or misses one it does. `options` is the engine's resolved option table. Paths are absolute, sorted;
/// `root` should be canonical.
///
/// # Errors
///
/// When the config set cannot be built, or a directory option has the wrong type or leaves the root.
pub fn project_dir_files(
    root: &Path,
    config: &Config,
    opts: &RunOptions,
    declared: &ProjectDirs,
    options: &toml::Table,
    served: &[Language],
) -> anyhow::Result<Vec<ProjectFile>> {
    let configs = super::build_config_set(&[root.to_path_buf()], config, opts)?;
    let config_id = configs.config_id_for(&root.join(poly_config::CONFIG_FILE_NAMES[0]));
    super::project::served_files(declared, options, served, root, &configs, config_id, opts).map_err(anyhow::Error::msg)
}
