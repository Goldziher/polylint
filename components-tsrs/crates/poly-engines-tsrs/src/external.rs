//! Findings that only say "this package is not installed".
//!
//! The run is hermetic: the host hands over no `node_modules`, so every import of an installed package fails to
//! resolve and every global an `@types` package declares is missing. A finding is dropped as such when the
//! project itself declares the dependency; one the project does not declare is a real error and stays.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

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

/// The `react` packages: when the project declares one, a JSX finding that says the types are missing is the same
/// "not installed" finding as an unresolved module.
const REACT_PACKAGES: &[&str] = &["react", "@types/react"];

/// `JSX.{0}` has no interface: with `react` declared, `@types/react` is what would provide it.
const JSX_INTERFACE_CODE: i32 = 7026;

/// `This JSX tag requires the module path '{0}' to exist`: the argument is the jsx runtime, such as
/// `react/jsx-runtime`.
const JSX_RUNTIME_CODE: i32 = 2875;

/// The dependencies one `package.json` declares.
#[derive(Debug)]
struct Manifest {
    dir: PathBuf,
    names: BTreeSet<String>,
}

/// The package names the project's `package.json` files declare, kept per manifest: an import is judged against
/// the manifests of the directories above the importing file, so a sibling package's dependencies declare nothing
/// for it.
#[derive(Debug, Default)]
pub(crate) struct Declared(Vec<Manifest>);

impl Declared {
    /// Reads every `package.json` among `files`; one that does not parse declares nothing. A dependency is
    /// declared whatever its version spec is, so `workspace:*` and `link:` dependencies count.
    pub(crate) fn from_files(files: &[ProjectFile]) -> Self {
        let mut manifests = Vec::new();
        for file in files
            .iter()
            .filter(|file| file.path.file_name().is_some_and(|name| name == "package.json"))
        {
            let Ok(manifest) = serde_json::from_str::<serde_json::Value>(&file.content) else {
                continue;
            };
            let mut names = BTreeSet::new();
            for field in DEPENDENCY_FIELDS {
                if let Some(table) = manifest.get(field).and_then(serde_json::Value::as_object) {
                    names.extend(table.keys().cloned());
                }
            }
            manifests.push(Manifest {
                dir: file.path.parent().map(Path::to_path_buf).unwrap_or_default(),
                names,
            });
        }
        Self(manifests)
    }

    /// The manifests that govern `file`: those in its directory and above, as Node resolves packages up the tree.
    /// A file below none of them falls back to the root-most manifest.
    fn scope(&self, file: &Path) -> Scope<'_> {
        let mut names: Vec<&BTreeSet<String>> = self
            .0
            .iter()
            .filter(|manifest| file.starts_with(&manifest.dir))
            .map(|manifest| &manifest.names)
            .collect();
        if names.is_empty()
            && let Some(root) = self.0.iter().min_by_key(|manifest| manifest.dir.components().count())
        {
            names.push(&root.names);
        }
        Scope(names)
    }

    /// Whether the manifests governing `file` declare the package `name`.
    pub(crate) fn declares(&self, file: &Path, name: &str) -> bool {
        self.scope(file).has(name)
    }

    /// Whether the finding `code` with its first message argument `argument`, in `file`, only reports a package the
    /// project declares but the hermetic run does not have. `jsx` is whether the program sets the `jsx` option.
    pub(crate) fn is_missing_install(&self, file: &Path, code: i32, argument: Option<&str>, jsx: bool) -> bool {
        self.scope(file).is_missing_install(code, argument, jsx)
    }
}

/// The declared names that govern one file.
struct Scope<'a>(Vec<&'a BTreeSet<String>>);

impl Scope<'_> {
    fn has(&self, name: &str) -> bool {
        self.0.iter().any(|names| names.contains(name))
    }

    /// Whether the package, or its `@types` package, is declared.
    fn has_package(&self, name: &str) -> bool {
        self.has(name) || self.has(&types_package(name))
    }

    fn is_missing_install(&self, code: i32, argument: Option<&str>, jsx: bool) -> bool {
        if let Some((_, packages)) = INSTALL_HINT_CODES.iter().find(|(known, _)| *known == code) {
            return packages.iter().any(|package| self.has(package));
        }
        if jsx && code == JSX_INTERFACE_CODE {
            return REACT_PACKAGES.iter().any(|package| self.has(package));
        }
        let Some(argument) = argument else {
            return false;
        };
        if jsx && code == JSX_RUNTIME_CODE {
            return package_name(argument) == Some("react") && REACT_PACKAGES.iter().any(|package| self.has(package));
        }
        if code == TYPE_FILE_CODE {
            // `types="vite/client"` names a file inside the package `vite`.
            return package_name(argument).is_some_and(|name| self.has_package(name)) || self.has_package(argument);
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
pub(crate) fn package_name(specifier: &str) -> Option<&str> {
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
pub(crate) fn types_package(package: &str) -> String {
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

    const FILE: &str = "/w/src/a.ts";

    fn declared(names: &[&str]) -> Declared {
        Declared(vec![Manifest {
            dir: PathBuf::from("/w"),
            names: names.iter().map(|name| (*name).to_owned()).collect(),
        }])
    }

    impl Declared {
        fn is_missing(&self, code: i32, argument: Option<&str>) -> bool {
            self.is_missing_install(Path::new(FILE), code, argument, false)
        }
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
        assert!(declared.is_missing(2307, Some("left-pad")));
        assert!(declared.is_missing(2307, Some("left-pad/lib/x")));
        assert!(declared.is_missing(7016, Some("left-pad")));
        assert!(declared.is_missing(2792, Some("@acme/ui/button")));
        assert!(
            declared.is_missing(2307, Some("react")),
            "its @types package is declared"
        );
        assert!(!declared.is_missing(2307, Some("typo-pad")));
        assert!(
            !declared.is_missing(2307, Some("./left-pad")),
            "a relative path is a real error"
        );
        assert!(!declared.is_missing(2307, None));
        assert!(!declared.is_missing(2322, Some("left-pad")), "only the module codes");
    }

    #[test]
    fn node_builtins_are_missing_only_with_node_types_declared() {
        let with = declared(&["@types/node"]);
        for specifier in ["fs", "node:fs", "fs/promises", "node:test"] {
            assert!(with.is_missing(2307, Some(specifier)), "{specifier}");
        }
        assert!(!declared(&[]).is_missing(2307, Some("fs")));
    }

    #[test]
    fn install_hints_and_type_files_follow_the_declared_packages() {
        let node = declared(&["@types/node"]);
        assert!(node.is_missing(2591, None));
        assert!(!node.is_missing(2582, None));
        assert!(declared(&["vitest"]).is_missing(2582, None));
        assert!(node.is_missing(2688, Some("node")));
        assert!(!declared(&[]).is_missing(2688, Some("node")));
    }

    #[test]
    fn type_file_references_resolve_to_their_package() {
        let found = declared(&["vite", "vitest", "@scope/pkg"]);
        for argument in ["vite/client", "vitest/globals", "@scope/pkg/sub", "vite"] {
            assert!(found.is_missing(2688, Some(argument)), "{argument}");
        }
        assert!(
            !found.is_missing(2688, Some("foo/bar")),
            "an undeclared package is reported"
        );
        assert!(!found.is_missing(2688, Some("@other/pkg/sub")));
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
        let has = |file: &str, name: &str| found.declares(Path::new(file), name);
        assert!(has("/w/x.ts", "a") && !has("/w/x.ts", "b"));
        assert!(has("/w/pkg/x.ts", "b") && has("/w/pkg/x.ts", "c"));
        assert!(!has("/w/pkg/x.ts", "d"));
    }

    #[test]
    fn a_sibling_package_declares_nothing_for_a_file() {
        let manifest = |path: &str, text: &str| ProjectFile {
            path: path.into(),
            language: poly_engine_api::language::Language::Other("package.json".into()),
            content: text.into(),
        };
        let found = Declared::from_files(&[
            manifest("/w/package.json", r#"{ "devDependencies": { "typescript": "5" } }"#),
            manifest("/w/a/package.json", r#"{ "dependencies": { "only-a": "1" } }"#),
            manifest(
                "/w/b/package.json",
                r#"{ "dependencies": { "only-b": "1", "@w/a": "workspace:*" } }"#,
            ),
        ]);
        let missing =
            |file: &str, specifier: &str| found.is_missing_install(Path::new(file), 2307, Some(specifier), false);
        assert!(missing("/w/a/src/x.ts", "only-a"));
        assert!(
            !missing("/w/a/src/x.ts", "only-b"),
            "B's dependency does not hide A's import"
        );
        assert!(missing("/w/b/src/x.ts", "only-b"));
        assert!(missing("/w/b/src/x.ts", "@w/a"), "a workspace dependency is declared");
        assert!(
            missing("/w/b/src/x.ts", "typescript"),
            "the root manifest is above both"
        );
        assert!(
            missing("/elsewhere/x.ts", "typescript"),
            "a file below no manifest uses the root-most one"
        );
    }

    #[test]
    fn react_jsx_findings_are_missing_installs_only_with_react_declared_and_jsx_set() {
        let react = declared(&["react"]);
        let file = Path::new(FILE);
        assert!(react.is_missing_install(file, 7026, Some("IntrinsicElements"), true));
        assert!(react.is_missing_install(file, 2875, Some("react/jsx-runtime"), true));
        assert!(
            !react.is_missing_install(file, 7026, Some("IntrinsicElements"), false),
            "no jsx option"
        );
        assert!(!react.is_missing_install(file, 2875, Some("preact/jsx-runtime"), true));
        assert!(!declared(&["left-pad"]).is_missing_install(file, 7026, Some("IntrinsicElements"), true));
        assert!(declared(&["@types/react"]).is_missing_install(file, 2875, Some("react/jsx-dev-runtime"), true));
    }
}
