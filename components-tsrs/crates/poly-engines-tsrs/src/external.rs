//! Findings that only say "this package is not installed".
//!
//! The run is hermetic: the host hands over no `node_modules`, so every import of an installed package fails to
//! resolve and every global an `@types` package declares is missing. A finding is dropped as such when the
//! project itself declares the dependency; one the project does not declare is a real error and stays.

use std::collections::BTreeSet;

use poly_engine_api::engine::ProjectFile;

/// `Cannot find module '{0}' or its corresponding type declarations`, its `moduleResolution` variant, and
/// `Could not find a declaration file for module '{0}'`: the argument is the specifier.
const MODULE_CODES: &[i32] = &[2307, 2792, 7016];

/// `Cannot find type definition file for '{0}'`: the argument is a `types` entry.
const TYPE_FILE_CODE: i32 = 2688;

/// `Cannot find name '{x}'. Do you need to install type definitions for ...`, with the packages that provide
/// them. They are suggestions only when the package is missing, so they are the same finding as the module codes.
const INSTALL_HINT_CODES: &[(i32, &[&str])] = &[
    (2580, &["@types/node"]),
    (2591, &["@types/node"]),
    (2581, &["@types/jquery"]),
    (2592, &["@types/jquery"]),
    (
        2582,
        &[
            "@types/jest",
            "@types/mocha",
            "@types/jasmine",
            "jest",
            "mocha",
            "vitest",
        ],
    ),
    (
        2593,
        &[
            "@types/jest",
            "@types/mocha",
            "@types/jasmine",
            "jest",
            "mocha",
            "vitest",
        ],
    ),
];

const DEPENDENCY_FIELDS: &[&str] = &[
    "dependencies",
    "devDependencies",
    "peerDependencies",
    "optionalDependencies",
];

/// Node's built-in modules (`module.builtinModules`): `@types/node` provides them.
const NODE_BUILTINS: &[&str] = &[
    "assert",
    "async_hooks",
    "buffer",
    "child_process",
    "cluster",
    "console",
    "constants",
    "crypto",
    "dgram",
    "diagnostics_channel",
    "dns",
    "domain",
    "events",
    "fs",
    "http",
    "http2",
    "https",
    "inspector",
    "module",
    "net",
    "os",
    "path",
    "perf_hooks",
    "process",
    "punycode",
    "querystring",
    "readline",
    "repl",
    "stream",
    "string_decoder",
    "sys",
    "timers",
    "tls",
    "trace_events",
    "tty",
    "url",
    "util",
    "v8",
    "vm",
    "wasi",
    "worker_threads",
    "zlib",
];

/// The package names the project's `package.json` files declare.
#[derive(Debug, Default)]
pub(crate) struct Declared(BTreeSet<String>);

impl Declared {
    /// Reads every `package.json` among `files`; one that does not parse declares nothing.
    pub(crate) fn from_files(files: &[ProjectFile]) -> Self {
        let mut names = BTreeSet::new();
        for file in files
            .iter()
            .filter(|file| file.path.file_name().is_some_and(|name| name == "package.json"))
        {
            let Ok(manifest) = serde_json::from_str::<serde_json::Value>(&file.content) else {
                continue;
            };
            for field in DEPENDENCY_FIELDS {
                if let Some(table) = manifest.get(field).and_then(serde_json::Value::as_object) {
                    names.extend(table.keys().cloned());
                }
            }
        }
        Self(names)
    }

    fn has(&self, name: &str) -> bool {
        self.0.contains(name)
    }

    /// Whether the package, or its `@types` package, is declared.
    fn has_package(&self, name: &str) -> bool {
        self.has(name) || self.has(&types_package(name))
    }

    /// Whether the finding `code` with its first message argument `argument` only reports a package the project
    /// declares but the hermetic run does not have.
    pub(crate) fn is_missing_install(&self, code: i32, argument: Option<&str>) -> bool {
        if let Some((_, packages)) = INSTALL_HINT_CODES.iter().find(|(known, _)| *known == code) {
            return packages.iter().any(|package| self.has(package));
        }
        let Some(argument) = argument else {
            return false;
        };
        if code == TYPE_FILE_CODE {
            return self.has_package(argument);
        }
        if !MODULE_CODES.contains(&code) {
            return false;
        }
        if is_node_builtin(argument) && self.has("@types/node") {
            return true;
        }
        package_name(argument).is_some_and(|name| self.has_package(name))
    }
}

fn is_node_builtin(specifier: &str) -> bool {
    let name = specifier.strip_prefix("node:").unwrap_or(specifier);
    let root = name.split('/').next().unwrap_or(name);
    specifier.starts_with("node:") || NODE_BUILTINS.contains(&root)
}

/// The package a specifier imports from: `lodash/fp` is `lodash`, `@scope/pkg/x` is `@scope/pkg`. Relative and
/// absolute paths and `#` imports name no package.
fn package_name(specifier: &str) -> Option<&str> {
    if specifier.starts_with(['.', '/', '#']) {
        return None;
    }
    let mut parts = specifier.splitn(3, '/');
    let first = parts.next().filter(|first| !first.is_empty())?;
    if !first.starts_with('@') {
        return Some(first);
    }
    let second = parts.next()?;
    Some(&specifier[..first.len() + 1 + second.len()])
}

/// The DefinitelyTyped name of `package`: `@scope/pkg` is `@types/scope__pkg`.
fn types_package(package: &str) -> String {
    match package.strip_prefix('@') {
        Some(scoped) => format!("@types/{}", scoped.replacen('/', "__", 1)),
        None => format!("@types/{package}"),
    }
}

/// The first single-quoted text of `message`, for a finding whose message arguments were not kept.
pub(crate) fn quoted(message: &str) -> Option<&str> {
    let start = message.find('\'')? + 1;
    let len = message[start..].find('\'')?;
    Some(&message[start..start + len])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn declared(names: &[&str]) -> Declared {
        Declared(names.iter().map(|name| (*name).to_owned()).collect())
    }

    #[test]
    fn package_names_come_from_the_specifier() {
        assert_eq!(package_name("lodash"), Some("lodash"));
        assert_eq!(package_name("lodash/fp"), Some("lodash"));
        assert_eq!(package_name("@scope/pkg"), Some("@scope/pkg"));
        assert_eq!(package_name("@scope/pkg/deep/file"), Some("@scope/pkg"));
        assert_eq!(package_name("@scope"), None);
        for relative in ["./a", "../a", "/abs", "#internal"] {
            assert_eq!(package_name(relative), None, "{relative}");
        }
    }

    #[test]
    fn types_packages_follow_definitelytyped_naming() {
        assert_eq!(types_package("react"), "@types/react");
        assert_eq!(types_package("@babel/core"), "@types/babel__core");
    }

    #[test]
    fn a_declared_package_is_missing_but_an_undeclared_one_is_an_error() {
        let declared = declared(&["left-pad", "@types/react", "@acme/ui"]);
        assert!(declared.is_missing_install(2307, Some("left-pad")));
        assert!(declared.is_missing_install(2307, Some("left-pad/lib/x")));
        assert!(declared.is_missing_install(7016, Some("left-pad")));
        assert!(declared.is_missing_install(2792, Some("@acme/ui/button")));
        assert!(
            declared.is_missing_install(2307, Some("react")),
            "its @types package is declared"
        );
        assert!(!declared.is_missing_install(2307, Some("typo-pad")));
        assert!(
            !declared.is_missing_install(2307, Some("./left-pad")),
            "a relative path is a real error"
        );
        assert!(!declared.is_missing_install(2307, None));
        assert!(
            !declared.is_missing_install(2322, Some("left-pad")),
            "only the module codes"
        );
    }

    #[test]
    fn node_builtins_are_missing_only_with_node_types_declared() {
        let with = declared(&["@types/node"]);
        for specifier in ["fs", "node:fs", "fs/promises", "node:test"] {
            assert!(with.is_missing_install(2307, Some(specifier)), "{specifier}");
        }
        assert!(!declared(&[]).is_missing_install(2307, Some("fs")));
    }

    #[test]
    fn install_hints_and_type_files_follow_the_declared_packages() {
        let node = declared(&["@types/node"]);
        assert!(node.is_missing_install(2591, None));
        assert!(!node.is_missing_install(2582, None));
        assert!(declared(&["vitest"]).is_missing_install(2582, None));
        assert!(node.is_missing_install(2688, Some("node")));
        assert!(!declared(&[]).is_missing_install(2688, Some("node")));
    }

    #[test]
    fn quoted_takes_the_first_quoted_text() {
        assert_eq!(quoted("Cannot find module 'a/b' or its types."), Some("a/b"));
        assert_eq!(quoted("none"), None);
    }

    #[test]
    fn declarations_are_read_from_every_manifest() {
        let manifest = |path: &str, text: &str| ProjectFile {
            path: path.into(),
            language: poly_engine_api::language::Language::Other("package.json".into()),
            content: text.into(),
        };
        let files = [
            manifest("/w/package.json", r#"{ "dependencies": { "a": "1" } }"#),
            manifest(
                "/w/pkg/package.json",
                r#"{ "devDependencies": { "b": "1" }, "peerDependencies": { "c": "1" } }"#,
            ),
            manifest("/w/bad/package.json", "{ not json"),
        ];
        let found = Declared::from_files(&files);
        assert!(found.has("a") && found.has("b") && found.has("c"));
        assert!(!found.has("d"));
    }
}
