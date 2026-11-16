//! Which cargo target directory a staged run points its hooks at.
//!
//! A staged run executes hooks inside a snapshot, so cargo would otherwise
//! build into `<snapshot>/target` and start cold. poly pins `CARGO_TARGET_DIR`
//! to the directory the live repository would have used anyway, following
//! cargo's own precedence so a shared or relocated target directory survives.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// Resolve the target directory cargo would use for the repository at `root`.
///
/// Precedence: `CARGO_TARGET_DIR`, `CARGO_BUILD_TARGET_DIR`, `build.target-dir`
/// from the nearest `.cargo/config.toml` (or `.cargo/config`) walking up from
/// `root` and then `$CARGO_HOME`, else `<root>/target`. An empty variable counts
/// as unset. A relative environment value resolves against `root`; a relative
/// config value resolves against the directory containing that `.cargo/`.
///
/// `env` looks a variable up, so callers and tests control the environment.
#[must_use]
pub fn resolve_target_dir(root: &Path, env: &dyn Fn(&str) -> Option<OsString>) -> PathBuf {
    let lookup = |key: &str| env(key).filter(|value| !value.is_empty());
    for key in ["CARGO_TARGET_DIR", "CARGO_BUILD_TARGET_DIR"] {
        if let Some(value) = lookup(key) {
            return root.join(value);
        }
    }
    let cargo_home = lookup("CARGO_HOME").map(PathBuf::from).or_else(|| {
        lookup("HOME")
            .or_else(|| lookup("USERPROFILE"))
            .map(|home| PathBuf::from(home).join(".cargo"))
    });
    let config_dirs = root.ancestors().map(|dir| dir.join(".cargo")).chain(cargo_home);
    for config_dir in config_dirs {
        if let Some(value) = config_target_dir(&config_dir) {
            let base = config_dir.parent().unwrap_or(root);
            return base.join(value);
        }
    }
    root.join("target")
}

/// The `build.target-dir` value of the config in `config_dir`, preferring
/// `config.toml` over the legacy extensionless `config`, as cargo does.
fn config_target_dir(config_dir: &Path) -> Option<String> {
    ["config.toml", "config"].into_iter().find_map(|name| {
        let text = std::fs::read_to_string(config_dir.join(name)).ok()?;
        let table: toml::Table = text.parse().ok()?;
        let value = table.get("build")?.as_table()?.get("target-dir")?.as_str()?;
        (!value.is_empty()).then(|| value.to_owned())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env_of(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<OsString> + use<> {
        let pairs: Vec<(String, OsString)> = pairs
            .iter()
            .map(|(name, value)| ((*name).to_owned(), OsString::from(*value)))
            .collect();
        move |key| {
            pairs
                .iter()
                .find(|(name, _)| name == key)
                .map(|(_, value)| value.clone())
        }
    }

    fn write_config(dir: &Path, body: &str) {
        std::fs::create_dir_all(dir.join(".cargo")).unwrap();
        std::fs::write(dir.join(".cargo/config.toml"), body).unwrap();
    }

    #[test]
    fn falls_back_to_the_repository_target() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let got = resolve_target_dir(tmp.path(), &env_of(&[("CARGO_HOME", home.to_str().unwrap())]));
        assert_eq!(got, tmp.path().join("target"));
    }

    #[test]
    fn process_env_target_dir_wins_over_everything() {
        let tmp = tempfile::tempdir().unwrap();
        write_config(tmp.path(), "[build]\ntarget-dir = \"from-config\"\n");
        let shared = tmp.path().join("shared/t");
        let other = tmp.path().join("other");
        let env = env_of(&[
            ("CARGO_TARGET_DIR", shared.to_str().unwrap()),
            ("CARGO_BUILD_TARGET_DIR", other.to_str().unwrap()),
        ]);
        assert_eq!(resolve_target_dir(tmp.path(), &env), shared);
    }

    #[test]
    fn build_target_dir_env_is_honoured() {
        let tmp = tempfile::tempdir().unwrap();
        write_config(tmp.path(), "[build]\ntarget-dir = \"from-config\"\n");
        let build = tmp.path().join("build/t");
        let env = env_of(&[("CARGO_BUILD_TARGET_DIR", build.to_str().unwrap())]);
        assert_eq!(resolve_target_dir(tmp.path(), &env), build);
    }

    #[test]
    fn empty_variable_counts_as_unset() {
        let tmp = tempfile::tempdir().unwrap();
        let env = env_of(&[("CARGO_TARGET_DIR", ""), ("CARGO_HOME", tmp.path().to_str().unwrap())]);
        assert_eq!(resolve_target_dir(tmp.path(), &env), tmp.path().join("target"));
    }

    #[test]
    fn relative_config_value_resolves_against_the_cargo_dir_parent() {
        let tmp = tempfile::tempdir().unwrap();
        write_config(tmp.path(), "[build]\ntarget-dir = \"out/build\"\n");
        let nested = tmp.path().join("crates/a");
        std::fs::create_dir_all(&nested).unwrap();
        assert_eq!(resolve_target_dir(&nested, &env_of(&[])), tmp.path().join("out/build"));
    }

    #[test]
    fn absolute_config_value_is_used_as_is() {
        let tmp = tempfile::tempdir().unwrap();
        let shared = tmp.path().join("shared");
        write_config(
            tmp.path(),
            &format!("[build]\ntarget-dir = {:?}\n", shared.to_str().unwrap()),
        );
        assert_eq!(resolve_target_dir(tmp.path(), &env_of(&[])), shared);
    }

    #[test]
    fn cargo_home_config_applies_when_the_repository_has_none() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("cargo-home");
        std::fs::create_dir_all(&home).unwrap();
        let shared = tmp.path().join("home-target");
        std::fs::write(
            home.join("config.toml"),
            format!("[build]\ntarget-dir = {:?}\n", shared.to_str().unwrap()),
        )
        .unwrap();
        let repo = tmp.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        let env = env_of(&[("CARGO_HOME", home.to_str().unwrap())]);
        assert_eq!(resolve_target_dir(&repo, &env), shared);
    }
}
