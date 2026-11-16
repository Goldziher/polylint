//! The pinned `shellcheck` download behind `install = "auto"`.
//!
//! The built-in engine runs the host's `shellcheck` and never downloads by
//! itself: `install` defaults to `"never"`. With `install = "auto"` and no
//! `shellcheck` on `PATH`, the engine takes the catalog's pinned, checksummed
//! release through the recipe tier's resolver (never offline). The `install`
//! read lives in this module so the option-key audit attributes it to
//! `shellcheck` alone, not to every tool sharing `native_tool/mod.rs`. ~keep

use std::path::PathBuf;
use std::sync::OnceLock;

use poly_engine_api::options::OptionsExt;
use tracing::warn;

use super::probe::probe_at;
use super::spec::ToolSpec;
use crate::config::EngineConfig;

/// Resolves the pinned download: the program and an identity that changes with
/// the build. A function pointer so tests substitute a fake without a network.
pub(super) type Provider = fn() -> Result<(PathBuf, String), String>;

/// A downloaded `shellcheck`.
#[derive(Debug)]
pub(super) struct Provisioned {
    binary: String,
    version: String,
    identity: String,
}

/// The `shellcheck` a run executes.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct Selected<'a> {
    pub(super) binary: &'a str,
    /// The cache-key part for a downloaded binary; empty for the host's, whose
    /// version the engine's `version()` already folds in.
    pub(super) salt: String,
}

/// Whether the config asks for the pinned download when `shellcheck` is not on
/// `PATH`. Anything but `"auto"` keeps today's zero-download behavior.
pub(super) fn install_auto(cfg: &EngineConfig) -> bool {
    match cfg.options.opt_str("install") {
        Some("auto") => true,
        Some("never") | None => false,
        Some(other) => {
            warn!("shellcheck: `install = \"{other}\"` is not \"never\" or \"auto\"; not downloading");
            false
        }
    }
}

/// The production [`Provider`]: the catalog recipe's pinned release.
pub(super) fn catalog_download() -> Result<(PathBuf, String), String> {
    crate::engines::catalog_tool::locate_or_install("shellcheck", poly_config::InstallPolicy::Auto)
}

/// Pick the executable: the host's (`host_version` is `Some` when it is on
/// `PATH`), else, under `auto`, the download `provider` yields. The download is
/// resolved once into `memo`; a failure is warned about once and remembered.
pub(super) fn select<'a>(
    spec: &'a ToolSpec,
    host_version: Option<&str>,
    auto: bool,
    memo: &'a OnceLock<Option<Provisioned>>,
    provider: Provider,
) -> Option<Selected<'a>> {
    if host_version.is_some() {
        return Some(Selected {
            binary: spec.lint_binary?,
            salt: String::new(),
        });
    }
    if !auto {
        return None;
    }
    let downloaded = memo
        .get_or_init(|| match provider() {
            Ok((path, identity)) => {
                let binary = path.to_string_lossy().into_owned();
                let version = probe_at(spec, &binary, &binary)?;
                Some(Provisioned {
                    binary,
                    version,
                    identity,
                })
            }
            Err(message) => {
                warn!("shellcheck is not on PATH and the pinned download is unavailable: {message}");
                None
            }
        })
        .as_ref()?;
    Some(Selected {
        binary: &downloaded.binary,
        salt: format!("downloaded:{}:{}", downloaded.version, downloaded.identity),
    })
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::engines::native_tool::spec::SHELLCHECK_SPEC;

    fn cfg(install: Option<&str>) -> EngineConfig {
        let mut options = toml::Table::new();
        if let Some(value) = install {
            options.insert("install".into(), toml::Value::String(value.into()));
        }
        EngineConfig {
            globals: crate::config::GlobalDefaults::default(),
            indent_width: 4,
            options,
        }
    }

    fn fake_download() -> Result<(PathBuf, String), String> {
        let held = tempfile::tempdir().expect("dir");
        let dir = held.path().to_path_buf();
        std::mem::forget(held);
        let tool = crate::tool_run::fake_tool::script(
            &dir,
            "shellcheck",
            "case \"$1\" in --version) echo 'version: 9.9.9';; *) cat >/dev/null; echo '{\"comments\":[]}';; esac",
        );
        Ok((tool, "fake-build".into()))
    }

    fn offline_download() -> Result<(PathBuf, String), String> {
        Err("poly is offline; not downloading shellcheck".into())
    }

    fn unreachable_provider() -> Result<(PathBuf, String), String> {
        panic!("the download must not be consulted");
    }

    #[test]
    fn install_defaults_to_never_and_only_auto_downloads() {
        assert!(!install_auto(&cfg(None)));
        assert!(!install_auto(&cfg(Some("never"))));
        assert!(!install_auto(&cfg(Some("always"))));
        assert!(install_auto(&cfg(Some("auto"))));
    }

    #[test]
    fn absent_with_install_never_is_not_available_and_never_downloads() {
        let memo = OnceLock::new();
        assert_eq!(select(&SHELLCHECK_SPEC, None, false, &memo, unreachable_provider), None);
        assert!(memo.get().is_none(), "nothing was resolved");
    }

    #[test]
    fn the_host_binary_wins_over_a_download() {
        let memo = OnceLock::new();
        let chosen = select(&SHELLCHECK_SPEC, Some("0.10.0"), true, &memo, unreachable_provider).expect("host");
        assert_eq!(
            chosen,
            Selected {
                binary: "shellcheck",
                salt: String::new()
            }
        );
    }

    #[test]
    fn absent_with_install_auto_runs_the_resolved_binary() {
        let memo = OnceLock::new();
        let chosen = select(&SHELLCHECK_SPEC, None, true, &memo, fake_download).expect("downloaded");
        assert!(chosen.binary.ends_with("/shellcheck"), "{}", chosen.binary);
        assert!(
            chosen.salt.contains("9.9.9") && chosen.salt.contains("fake-build"),
            "{}",
            chosen.salt
        );
    }

    #[test]
    fn a_failed_download_leaves_the_tool_unavailable_and_is_not_retried() {
        let memo = OnceLock::new();
        assert_eq!(select(&SHELLCHECK_SPEC, None, true, &memo, offline_download), None);
        assert_eq!(select(&SHELLCHECK_SPEC, None, true, &memo, unreachable_provider), None);
    }

    #[test]
    fn the_resolved_binary_lints_a_file() {
        let memo = OnceLock::new();
        let chosen = select(&SHELLCHECK_SPEC, None, true, &memo, fake_download).expect("downloaded");
        let src = crate::engine::SourceFile {
            path: "x.sh".into(),
            language: crate::language::Language::Shell,
            content: "echo hi\n".into(),
        };
        let found = crate::engines::native_tool::lint::lint_via_shellcheck_at(&SHELLCHECK_SPEC, chosen.binary, &src)
            .expect("runs");
        assert!(found.is_empty());
    }
}
