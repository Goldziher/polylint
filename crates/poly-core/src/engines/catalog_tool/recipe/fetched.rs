//! Projects whose dependencies are not fetched yet.
//!
//! `mix format` resolves `import_deps` against the project's `deps/` directory and fails with
//! "Unknown dependency" when it is empty. That is the state of a fresh checkout, not a defect in
//! the file being formatted, so such files are skipped with a reason naming the fix instead of
//! being reported as errors.

use std::path::{Path, PathBuf};

use poly_catalog::{Fetched, Group};

use super::memo::Memo;
use super::{RecipeTool, paths};

/// The answer per project root and fetch directory, for the run: every file of a project asks.
pub(super) static UNFETCHED: Memo<(PathBuf, String), bool> = Memo::new();

impl RecipeTool {
    /// Why `path` cannot be handled until its project's dependencies are fetched, when that is so.
    ///
    /// The reason is identical for every file of a project, so the run's skip notes aggregate it.
    pub(in crate::engines::catalog_tool) fn unfetched_reason(&self, path: &Path) -> Option<String> {
        let fetched = self.recipe.fetched.as_ref()?;
        // A file outside the working directory is mirrored with none of the project's context
        // (see `Member::is_outside`), so the project's dependencies are not its concern.
        paths::relative(&self.cwd, path)?;
        let absolute = paths::absolute(&self.cwd, path);
        let root = match &self.recipe.group {
            Group::Marker(markers) => paths::project_root(&absolute, markers)?,
            Group::None | Group::Dir => self.cwd.clone(),
        };
        self.is_unfetched(&root, fetched).then(|| {
            format!(
                "{}: dependencies not fetched (run `{}` in {})",
                self.tool.name,
                fetched.command,
                display_root(&root)
            )
        })
    }

    fn is_unfetched(&self, root: &Path, fetched: &Fetched) -> bool {
        let key = (root.to_path_buf(), fetched.dir.clone());
        if let Some(known) = UNFETCHED.get(&key) {
            return known;
        }
        let answer = !root.join(&fetched.dir).is_dir() && self.needs_dependencies(root, fetched);
        UNFETCHED.insert(key, answer);
        answer
    }

    fn needs_dependencies(&self, root: &Path, fetched: &Fetched) -> bool {
        let Some(needle) = &fetched.config_mentions else {
            return true;
        };
        self.recipe.config_files.iter().any(|name| {
            std::fs::read_to_string(root.join(name)).is_ok_and(|text| {
                text.lines()
                    .filter(|line| !line.trim_start().starts_with('#'))
                    .any(|line| line.contains(needle.as_str()))
            })
        })
    }
}

/// `root` as the user knows it: relative to the directory poly runs from.
fn display_root(root: &Path) -> String {
    let shown = std::env::current_dir()
        .ok()
        .and_then(|cwd| paths::relative(&cwd, root))
        .unwrap_or_else(|| root.to_path_buf());
    if shown.as_os_str().is_empty() {
        ".".to_owned()
    } else {
        paths::slashed(&shown)
    }
}
