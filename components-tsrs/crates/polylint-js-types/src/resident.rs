//! The programs the analyses of a run over several roots share ([`TypePrograms`], skif ADR 0010): the registry of
//! `polylint-tsrs` under this crate's name and count types.

use std::path::{Path, PathBuf};

use polylint_tsrs::SharedPrograms;

use crate::{SharedConfigCounts, SharedFileCounts};

/// The programs the analyses of one run share, so that a program the judged files of several roots go to is built
/// once instead of once per root: an outer root's `tsconfig.json` that also includes the files of the roots nested
/// below it without a config of their own.
///
/// Give the same value (or a clone, which shares it) to every analysis of the run with
/// [`TypeBuild::with_programs`](crate::TypeBuild::with_programs) or
/// [`TypeInput::with_programs`](crate::TypeInput::with_programs), and only to analyses that read the same files: an
/// analysis walks a shared program as the analysis that built it read it. A built program stays while a root of the
/// run that has not finished holds a file its config includes, and is freed once none does; an analysis marks its
/// root finished when its facts are returned or it is given up, and a host marks a root it analyses without types
/// with [`TypePrograms::done`]. The programs of the run also share the files they read, and the analyses the configs
/// their plannings and programs read: each is parsed once for the run. A bundled lib file, a declaration file below
/// `node_modules` or a config is kept until these programs (every clone) and the last program that read it are
/// dropped; a project file until the last program that read it is dropped. The facts do not depend on it.
#[derive(Clone)]
pub struct TypePrograms {
    shared: SharedPrograms,
}

impl TypePrograms {
    /// The programs shared by the analyses of `roots`, the roots of the run's analyses, absolute.
    #[must_use]
    pub fn new(roots: impl IntoIterator<Item = PathBuf>) -> Self {
        Self {
            shared: SharedPrograms::new(roots),
        }
    }

    /// How the programs of the run got the lib and `node_modules` declaration files they read, so far: the files
    /// parsed for the run, those taken parsed from it, and those parsed for one program alone.
    #[must_use]
    pub fn shared_files(&self) -> SharedFileCounts {
        self.shared.shared_files().into()
    }

    /// How many project files the run's programs hold now, for tests that check each is freed with the last program
    /// that read it.
    #[doc(hidden)]
    #[must_use]
    pub fn project_files_held(&self) -> usize {
        self.shared.project_files_held()
    }

    /// How the plannings and programs of the run got the configs they read, so far: the configs, then the configs
    /// they extend.
    #[must_use]
    pub fn shared_configs(&self) -> (SharedConfigCounts, SharedConfigCounts) {
        let (configs, extended) = self.shared.shared_configs();
        (configs.into(), extended.into())
    }

    /// Marks the analysis of `root` finished: a program kept only for it is freed.
    pub fn done(&self, root: &Path) {
        self.shared.done(root);
    }

    /// How many programs were built through these programs, for tests that check each is built once.
    #[doc(hidden)]
    #[must_use]
    pub fn builds(&self) -> usize {
        self.shared.builds()
    }

    /// How many built programs are kept for roots that have not finished, for tests that check none outlives them.
    #[doc(hidden)]
    #[must_use]
    pub fn kept(&self) -> usize {
        self.shared.kept()
    }

    /// The registry the analyses claim and publish programs through.
    pub(crate) fn shared(&self) -> &SharedPrograms {
        &self.shared
    }
}

impl std::fmt::Debug for TypePrograms {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("TypePrograms")
            .field("roots", self.shared.roots())
            .finish_non_exhaustive()
    }
}
