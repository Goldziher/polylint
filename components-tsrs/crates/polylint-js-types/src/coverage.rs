//! The type coverage of a package, from what happened to each of its judged files.

use std::path::Path;

use crate::{CoverageReason, CoverageState, PackageCoverage, TypePackage};

/// What happened to one judged file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FileState {
    /// A built program holds it; this many of its imports resolve to no module.
    Covered { unresolved_modules: u32 },
    /// No config is above it.
    NoConfig,
    /// The config that would hold it cannot be parsed.
    ConfigError,
    /// No program holds it.
    Outside,
    /// The checker failed on the program that holds it.
    CheckerFailed,
}

/// The extensions of the files TypeScript checks without `checkJs`.
const TYPESCRIPT_EXTENSIONS: [&str; 4] = ["ts", "tsx", "mts", "cts"];

fn is_typescript(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            TYPESCRIPT_EXTENSIONS
                .iter()
                .any(|typescript| extension.eq_ignore_ascii_case(typescript))
        })
}

fn count(states: &[FileState], predicate: impl Fn(FileState) -> bool) -> u32 {
    u32::try_from(states.iter().filter(|&&state| predicate(state)).count()).unwrap_or(u32::MAX)
}

/// The coverage of `package`, whose judged files ended as `states` (in the order of [`TypePackage::files`]).
pub(crate) fn package_coverage(package: &TypePackage, states: &[FileState]) -> PackageCoverage {
    let covered = count(states, |state| matches!(state, FileState::Covered { .. }));
    let untyped_files = u32::try_from(states.len()).unwrap_or(u32::MAX).saturating_sub(covered);
    let unresolved_modules = states
        .iter()
        .map(|state| match state {
            FileState::Covered { unresolved_modules } => *unresolved_modules,
            _ => 0,
        })
        .fold(0_u32, u32::saturating_add);
    let reason = reason(package, states, covered, untyped_files, unresolved_modules);
    let state = match (&reason, covered) {
        (None, _) => CoverageState::Typed,
        (Some(_), 0) => CoverageState::Untyped,
        (Some(_), _) => CoverageState::Partial,
    };
    PackageCoverage {
        dir: package.dir.clone(),
        name: package.name.clone(),
        state,
        reason,
        untyped_files,
    }
}

/// Why the package is not fully typed, the most telling cause first.
fn reason(
    package: &TypePackage,
    states: &[FileState],
    covered: u32,
    untyped_files: u32,
    unresolved_modules: u32,
) -> Option<CoverageReason> {
    if states.contains(&FileState::CheckerFailed) {
        return Some(CoverageReason::CheckerFailed);
    }
    if covered == 0 && !package.files.iter().any(|file| is_typescript(file)) {
        return Some(CoverageReason::NotTypescript);
    }
    if states.contains(&FileState::ConfigError) {
        return Some(CoverageReason::TsconfigError);
    }
    if covered == 0 && states.iter().all(|&state| state == FileState::NoConfig) {
        return Some(CoverageReason::NoTsconfig);
    }
    if untyped_files > 0 {
        return Some(CoverageReason::FilesOutsidePrograms(untyped_files));
    }
    (unresolved_modules > 0).then_some(CoverageReason::UnresolvedModules(unresolved_modules))
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    const TYPED: FileState = FileState::Covered { unresolved_modules: 0 };

    fn package(files: &[&str]) -> TypePackage {
        TypePackage {
            dir: PathBuf::from("/ws/packages/a"),
            name: Some("a".to_owned()),
            files: files.iter().map(PathBuf::from).collect(),
        }
    }

    fn coverage(files: &[&str], states: &[FileState]) -> (CoverageState, Option<CoverageReason>, u32) {
        let coverage = package_coverage(&package(files), states);
        (coverage.state, coverage.reason, coverage.untyped_files)
    }

    #[test]
    fn a_package_whose_files_are_all_covered_and_resolve_is_typed() {
        assert_eq!(
            coverage(&["a.ts", "b.tsx"], &[TYPED, TYPED]),
            (CoverageState::Typed, None, 0)
        );
        let full = package_coverage(&package(&["a.ts"]), &[TYPED]);
        assert_eq!(
            (full.dir, full.name),
            (PathBuf::from("/ws/packages/a"), Some("a".to_owned()))
        );
    }

    #[test]
    fn unresolved_imports_make_a_covered_package_partial() {
        let states = [
            FileState::Covered { unresolved_modules: 2 },
            FileState::Covered { unresolved_modules: 1 },
        ];
        assert_eq!(
            coverage(&["a.ts", "b.ts"], &states),
            (CoverageState::Partial, Some(CoverageReason::UnresolvedModules(3)), 0)
        );
    }

    #[test]
    fn files_outside_programs_make_a_package_partial() {
        let states = [TYPED, FileState::Outside, FileState::NoConfig];
        assert_eq!(
            coverage(&["a.ts", "b.ts", "c.ts"], &states),
            (CoverageState::Partial, Some(CoverageReason::FilesOutsidePrograms(2)), 2)
        );
    }

    #[test]
    fn a_package_without_a_config_is_untyped() {
        assert_eq!(
            coverage(&["a.ts", "b.js"], &[FileState::NoConfig, FileState::NoConfig]),
            (CoverageState::Untyped, Some(CoverageReason::NoTsconfig), 2)
        );
    }

    #[test]
    fn a_package_of_plain_javascript_outside_every_program_is_not_typescript() {
        assert_eq!(
            coverage(&["a.js", "b.MJS"], &[FileState::NoConfig, FileState::Outside]),
            (CoverageState::Untyped, Some(CoverageReason::NotTypescript), 2)
        );
        assert_eq!(coverage(&["a.js"], &[TYPED]), (CoverageState::Typed, None, 0));
    }

    #[test]
    fn typescript_extensions_are_recognized_in_any_case() {
        assert!(is_typescript(Path::new("a.TS")) && is_typescript(Path::new("a.d.mts")));
        assert!(!is_typescript(Path::new("a.js")) && !is_typescript(Path::new("ts")));
    }

    #[test]
    fn a_config_error_is_named_before_files_outside_programs() {
        assert_eq!(
            coverage(&["a.ts", "b.ts"], &[TYPED, FileState::ConfigError]),
            (CoverageState::Partial, Some(CoverageReason::TsconfigError), 1)
        );
        assert_eq!(
            coverage(&["a.ts"], &[FileState::ConfigError]),
            (CoverageState::Untyped, Some(CoverageReason::TsconfigError), 1)
        );
    }

    #[test]
    fn a_failed_checker_is_named_first() {
        assert_eq!(
            coverage(
                &["a.ts", "b.ts"],
                &[FileState::CheckerFailed, FileState::Covered { unresolved_modules: 4 }]
            ),
            (CoverageState::Partial, Some(CoverageReason::CheckerFailed), 1)
        );
        assert_eq!(
            coverage(&["a.js"], &[FileState::CheckerFailed]),
            (CoverageState::Untyped, Some(CoverageReason::CheckerFailed), 1)
        );
    }
}
