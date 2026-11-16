//! A recipe's `config_defaults`: arguments poly passes only while the project's own
//! config leaves a setting unset, so a value the project chose wins (ADR 0066).
//!
//! The config consulted is the one `salt::config_fingerprint` hashes: a file named by
//! a config flag, else the first of the recipe's `config_files` found in poly's working
//! directory or an ancestor up to the repository root.

use std::path::{Path, PathBuf};

use poly_catalog::Recipe;

use super::{Role, salt};

/// `extra` with the recipe's applicable `config_defaults` in front, for `role`. Only the lint
/// role (whose fix passes share it) takes defaults. `command` is the argv the tool runs.
pub(super) fn with_defaults(
    recipe: &Recipe,
    role: Role,
    cwd: &Path,
    extra: &[String],
    command: &[String],
) -> Vec<String> {
    let mut args = if role == Role::Lint {
        default_args(recipe, cwd, &[extra, command])
    } else {
        Vec::new()
    };
    args.extend_from_slice(extra);
    args
}

/// The `config_defaults` arguments of `recipe` whose setting the governing config leaves unset.
/// `argvs` are the argument lists whose config flags name a file (the user's extra
/// arguments first, then the command's own).
fn default_args(recipe: &Recipe, cwd: &Path, argvs: &[&[String]]) -> Vec<String> {
    if recipe.config_defaults.is_empty() {
        return Vec::new();
    }
    let named: Vec<PathBuf> = argvs
        .iter()
        .flat_map(|argv| salt::config_flag_files(argv, &recipe.config_flags))
        .map(|path| if path.is_absolute() { path } else { cwd.join(path) })
        .collect();
    let config = governing(cwd, &named, &recipe.config_files)
        .and_then(|path| std::fs::read_to_string(&path).ok().map(|text| (path, text)));
    recipe
        .config_defaults
        .iter()
        .filter(|default| {
            !config
                .as_ref()
                .is_some_and(|(path, text)| sets(path, text, &default.key))
        })
        .map(|default| default.arg.clone())
        .collect()
}

/// The config file the tool reads: the first named one that exists, else the first of `names`
/// in `cwd` or an ancestor, stopping at the repository root.
fn governing(cwd: &Path, named: &[PathBuf], names: &[String]) -> Option<PathBuf> {
    if let Some(file) = named.iter().find(|file| file.is_file()) {
        return Some(file.clone());
    }
    for directory in cwd.ancestors() {
        if let Some(found) = names.iter().map(|name| directory.join(name)).find(|c| c.is_file()) {
            return Some(found);
        }
        if directory.join(".git").exists() {
            break;
        }
    }
    None
}

/// Whether `text`, the content of the config at `path`, sets the dotted `key`. TOML and JSON
/// are parsed; anything else is read as YAML by indentation.
fn sets(path: &Path, text: &str, key: &str) -> bool {
    let segments: Vec<&str> = key.split('.').collect();
    match path.extension().and_then(|ext| ext.to_str()) {
        Some("toml") => toml::from_str::<toml::Table>(text).is_ok_and(|table| {
            let root = toml::Value::Table(table);
            let mut value = Some(&root);
            for segment in &segments {
                value = value.and_then(|v| v.get(*segment));
            }
            value.is_some()
        }),
        Some("json") => serde_json::from_str::<serde_json::Value>(text).is_ok_and(|root| {
            let pointer: String = segments.iter().map(|segment| format!("/{segment}")).collect();
            root.pointer(&pointer).is_some()
        }),
        _ => yaml_sets(text, &segments),
    }
}

/// Block-mapping keys tracked by indentation, plus a one-level flow mapping
/// (`run: {timeout: 5m}`). Sequence entries and comments are skipped; no YAML parser is linked
/// into the core binary for this one question.
fn yaml_sets(text: &str, segments: &[&str]) -> bool {
    let Some((last, parents)) = segments.split_last() else {
        return false;
    };
    let mut stack: Vec<(usize, &str)> = Vec::new();
    for line in text.lines() {
        let trimmed = line.trim_start();
        if trimmed.is_empty() || trimmed.starts_with(['#', '-']) {
            continue;
        }
        let Some((raw_key, rest)) = trimmed.split_once(':') else {
            continue;
        };
        let indent = line.len() - trimmed.len();
        while stack.last().is_some_and(|&(depth, _)| depth >= indent) {
            stack.pop();
        }
        stack.push((indent, unquote(raw_key)));
        let path = stack.iter().map(|&(_, key)| key);
        if path.clone().eq(segments.iter().copied()) {
            return true;
        }
        if path.eq(parents.iter().copied()) && flow_has_key(rest, last) {
            return true;
        }
    }
    false
}

fn flow_has_key(rest: &str, key: &str) -> bool {
    let rest = rest.trim();
    let Some(inner) = rest.strip_prefix('{').and_then(|r| r.split('}').next()) else {
        return false;
    };
    inner
        .split(',')
        .filter_map(|entry| entry.split_once(':'))
        .any(|(candidate, _)| unquote(candidate) == key)
}

fn unquote(key: &str) -> &str {
    key.trim().trim_matches(['"', '\''])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn yaml(text: &str) -> bool {
        sets(Path::new(".golangci.yml"), text, "run.timeout")
    }

    #[test]
    fn a_yaml_run_timeout_is_found() {
        assert!(yaml("version: \"2\"\nrun:\n  timeout: 5m\n"));
        assert!(yaml("# lint config\nrun:\n  # slow CI\n  \"timeout\": 0\n"));
        assert!(yaml("run: {tests: false, timeout: 10m}\n"));
    }

    #[test]
    fn a_timeout_elsewhere_in_yaml_is_not_run_timeout() {
        assert!(!yaml("version: \"2\"\nrun:\n  tests: false\n"));
        assert!(!yaml("linters:\n  settings:\n    timeout: 5m\nrun:\n  tests: false\n"));
        assert!(!yaml("timeout: 5m\n"));
        assert!(!yaml("run:\n  tests: false\nissues:\n  timeout: 1\n"));
    }

    #[test]
    fn toml_and_json_configs_are_parsed() {
        assert!(sets(
            Path::new(".golangci.toml"),
            "[run]\ntimeout = \"5m\"\n",
            "run.timeout"
        ));
        assert!(!sets(
            Path::new(".golangci.toml"),
            "[run]\ntests = false\n",
            "run.timeout"
        ));
        assert!(sets(
            Path::new(".golangci.json"),
            r#"{"run":{"timeout":"5m"}}"#,
            "run.timeout"
        ));
        assert!(!sets(Path::new(".golangci.json"), r#"{"timeout":"5m"}"#, "run.timeout"));
    }

    fn recipe() -> Recipe {
        serde_json::from_value(serde_json::json!({
            "config_flags": ["-c"],
            "config_files": [".golangci.yml", ".golangci.toml"],
            "config_defaults": [{"arg": "--timeout=0", "key": "run.timeout"}]
        }))
        .expect("recipe")
    }

    fn repo() -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("dir");
        std::fs::create_dir(dir.path().join(".git")).expect("git");
        std::fs::create_dir(dir.path().join("sub")).expect("sub");
        dir
    }

    #[test]
    fn the_default_is_passed_only_while_the_project_sets_no_timeout() {
        let dir = repo();
        let cwd = dir.path().join("sub");
        assert_eq!(default_args(&recipe(), &cwd, &[]), ["--timeout=0"], "no config at all");
        std::fs::write(dir.path().join(".golangci.yml"), "run:\n  tests: false\n").expect("write");
        assert_eq!(
            default_args(&recipe(), &cwd, &[]),
            ["--timeout=0"],
            "a config without one"
        );
        std::fs::write(dir.path().join(".golangci.yml"), "run:\n  timeout: 5m\n").expect("write");
        assert!(
            default_args(&recipe(), &cwd, &[]).is_empty(),
            "the project's timeout wins"
        );
    }

    #[test]
    fn a_config_named_by_a_flag_governs_over_a_discovered_one() {
        let dir = repo();
        std::fs::write(dir.path().join(".golangci.yml"), "run:\n  timeout: 5m\n").expect("write");
        std::fs::write(dir.path().join("ci.toml"), "[run]\ntests = false\n").expect("write");
        let extra = vec!["-c".to_owned(), "ci.toml".to_owned()];
        assert_eq!(
            default_args(&recipe(), dir.path(), &[extra.as_slice()]),
            ["--timeout=0"]
        );
    }

    #[test]
    fn a_recipe_without_defaults_adds_nothing() {
        let dir = repo();
        assert!(default_args(&Recipe::default(), dir.path(), &[]).is_empty());
    }
}
