//! The inputs of [`ProjectAnalysis::project_check`](crate::engine::ProjectAnalysis::project_check): the file set,
//! the config and the host-resolved environment.

use std::path::PathBuf;
use std::sync::Arc;

use crate::config::EngineConfig;
use crate::language::Language;

/// One file handed to [`ProjectAnalysis::project_check`](crate::engine::ProjectAnalysis::project_check).
#[derive(Debug, Clone)]
pub struct ProjectFile {
    /// Path to the file on disk, as discovered (the key the findings are reported under).
    pub path: PathBuf,
    /// Detected language of the file.
    pub language: Language,
    /// Full file contents, shared rather than copied.
    pub content: Arc<str>,
}

/// A Python interpreter and the import roots it exposes, resolved by the host.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PythonEnvironment {
    /// The interpreter executable (a venv's `bin/python`, or `python3` from `PATH`).
    pub interpreter: PathBuf,
    /// Existing `site-packages` directories of the interpreter's environment.
    pub site_packages: Vec<PathBuf>,
    /// The interpreter's version (`3.12.4`) when it could be read without running it.
    pub version: Option<String>,
}

/// The toolchain environment a project check may see, resolved **by the host**.
///
/// A component never spawns a process or probes `PATH`: anything a check needs to know about the
/// machine is resolved by the host before the call and passed in here, so a check is a pure
/// function of `(files, config, environment)` and its result can be cached on those inputs.
/// Extensible: new toolchains add fields, and every field defaults to "not found".
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProjectEnvironment {
    /// The project's Python environment, when one was found.
    pub python: Option<PythonEnvironment>,
}

impl ProjectEnvironment {
    /// The wire form (`Request::ProjectCheck::environment`): flat `(key, value)` pairs.
    ///
    /// Keys: `python.interpreter`, `python.site_packages` (paths joined by a newline),
    /// `python.version`. Pairs are emitted in a fixed order so equal environments serialize
    /// identically.
    #[must_use]
    pub fn to_wire_pairs(&self) -> Vec<(String, String)> {
        let Some(python) = &self.python else {
            return Vec::new();
        };
        let mut pairs = vec![(
            "python.interpreter".to_string(),
            python.interpreter.display().to_string(),
        )];
        let sites: Vec<String> = python.site_packages.iter().map(|p| p.display().to_string()).collect();
        pairs.push(("python.site_packages".to_string(), sites.join("\n")));
        if let Some(version) = &python.version {
            pairs.push(("python.version".to_string(), version.clone()));
        }
        pairs
    }

    /// Rebuild an environment from [`ProjectEnvironment::to_wire_pairs`] output; unknown keys are ignored.
    #[must_use]
    pub fn from_wire_pairs(pairs: &[(String, String)]) -> Self {
        let get = |key: &str| pairs.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str());
        let Some(interpreter) = get("python.interpreter") else {
            return Self::default();
        };
        Self {
            python: Some(PythonEnvironment {
                interpreter: PathBuf::from(interpreter),
                site_packages: get("python.site_packages")
                    .map(|text| text.lines().filter(|l| !l.is_empty()).map(PathBuf::from).collect())
                    .unwrap_or_default(),
                version: get("python.version").map(str::to_string),
            }),
        }
    }
}

/// Everything [`ProjectAnalysis::project_check`](crate::engine::ProjectAnalysis::project_check) sees.
#[derive(Debug, Clone, Copy)]
pub struct ProjectRequest<'a> {
    /// The project root: the directory of the config governing the files.
    pub root: &'a std::path::Path,
    /// The files in scope for this engine, with contents.
    pub files: &'a [ProjectFile],
    /// The engine's resolved config.
    pub config: &'a EngineConfig,
    /// The host-resolved toolchain environment.
    pub environment: &'a ProjectEnvironment,
    /// The worker threads the host's run budgets for this check (`-j`, else every core). `None` leaves the
    /// engine to choose, as a caller that has no budget does.
    pub threads: Option<usize>,
    /// The files whose findings the run reports, spelled as in [`files`](Self::files); `None` means every file.
    ///
    /// The host sets it, for an engine that declares [`ProjectDirs::narrows_to_targets`], when the run named paths
    /// that leave some of the files out, as a pre-commit hook naming the staged files does, and discards the
    /// findings on every other file. An engine MAY restrict its work to these
    /// files, but its findings for each of them must equal those of the same request with `None`: the other files
    /// are still inputs (an import into one resolves as before), only their own findings may go uncomputed.
    /// Findings an engine still returns for other files are harmless.
    pub targets: Option<&'a [PathBuf]>,
    /// The host's cache, for an engine that keeps entries of its own there (skif's parsed modules), or `None` when
    /// the run caches nothing (`--no-cache`). An engine that keeps no cache ignores it (ADR 0080).
    pub cache: Option<&'a EngineCache>,
}

/// The host's cache, as an engine that keeps its own entries sees it (ADR 0080).
///
/// Never part of a project-check cache key: where the cache lives changes no finding, and the engine folds `salt`
/// into its own keys.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EngineCache {
    /// The result-cache root (`<cache home>/<repo key>`, or `[cache] dir`); the engine's entries go in its
    /// `results/engine/` directory.
    pub dir: PathBuf,
    /// Opaque bytes the engine adds to every key: the host's cache format version and build identity.
    pub salt: String,
}

impl EngineCache {
    /// The wire key of [`dir`](Self::dir) among `Request::ProjectCheck::environment`'s pairs.
    pub const DIR_KEY: &'static str = "poly.cache.dir";
    /// The wire key of [`salt`](Self::salt).
    pub const SALT_KEY: &'static str = "poly.cache.salt";

    /// The pairs this cache travels as, appended to the request's environment pairs. A component that predates them
    /// ignores keys it does not know, so they need no new field and no gate on the component's version.
    #[must_use]
    pub fn to_wire_pairs(&self) -> [(String, String); 2] {
        [
            (Self::DIR_KEY.to_owned(), self.dir.display().to_string()),
            (Self::SALT_KEY.to_owned(), self.salt.clone()),
        ]
    }

    /// The cache among a request's environment pairs, or `None` when the host sent none (`--no-cache`, or a host
    /// that predates it).
    #[must_use]
    pub fn from_wire_pairs(pairs: &[(String, String)]) -> Option<Self> {
        let get = |key: &str| pairs.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str());
        Some(Self {
            dir: PathBuf::from(get(Self::DIR_KEY)?),
            salt: get(Self::SALT_KEY).unwrap_or_default().to_owned(),
        })
    }
}

/// The language id of a listing entry: a file under a declared directory that the engine does not
/// serve, handed over by name with empty content (see [`ProjectDirs::list_unserved`]).
pub const LISTING_LANGUAGE: &str = "listing";

/// A file a project check reads besides the files it serves, named by an engine option.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectSideFile {
    /// The option naming the path, relative to the project root.
    pub option_key: String,
    /// The path used when the option is unset.
    pub default_path: String,
}

/// The directories a project check is about, named by engine options.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectDirs {
    /// The option naming one directory, relative to the project root.
    pub dir_key: String,
    /// The directory used when the option is unset.
    pub dir_default: String,
    /// An option holding a list of directories that replaces `dir_key`, when the engine has one.
    pub paths_key: Option<String>,
    /// A boolean option that extends the directories to their subdirectories, when the engine has one.
    pub recursive_key: Option<String>,
    /// Whether the directories extend to their subdirectories when `recursive_key` is unset or the
    /// engine has none: a tree-wide check such as a JavaScript workspace is about the whole tree.
    pub recursive_default: bool,
    /// Whether the check is about the whole tree: the walk then honors only `.gitignore` and the
    /// always-pruned directories, never `[discovery] exclude`, the run's `--exclude` or the split into
    /// nested `poly.toml` scopes. A verdict over a workspace graph (is this import resolvable, is this
    /// package used) is wrong when part of the graph is left out for reasons that concern linting, not
    /// the graph; the run's own file set then only decides which findings are reported.
    pub whole_tree: bool,
    /// File names (not paths) the walk hands over as inputs wherever it finds them, whatever their
    /// language, with the file name as `Language::Other`. For files the engine reads without serving them
    /// as sources, such as `.gitignore`; they enter the cache key like any served file.
    pub file_names: Vec<String>,
    /// Whether the host also lists the files in these directories that the engine does not serve
    /// (every file type, names only, empty content, language `Other("listing")`).
    pub list_unserved: bool,
    /// Whether the check computes findings only for the request's
    /// [`targets`](ProjectRequest::targets). The host sends targets, and keeps a result per set of
    /// targets, only for an engine that says so: a check that judges the whole tree whatever it is
    /// asked for gets `None` and its one result answers every run.
    pub narrows_to_targets: bool,
}

/// What a project-check engine needs from the host beyond the files of the languages it serves.
///
/// Declared by the engine, carried in its description (and so in the component index), and read
/// by the host: the core holds no table keyed by engine name. Everything here is resolved by the
/// host and handed over as content, because a component reads nothing itself.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProjectInputs {
    /// An option that must be set for the engine to be planned at all, so a project that never
    /// configured it pays nothing for the phase.
    pub gate_key: Option<String>,
    /// Side files, each loaded when it exists.
    pub files: Vec<ProjectSideFile>,
    /// When set, only served files under these directories are handed over.
    pub dirs: Option<ProjectDirs>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn python() -> ProjectEnvironment {
        ProjectEnvironment {
            python: Some(PythonEnvironment {
                interpreter: PathBuf::from("/venv/bin/python"),
                site_packages: vec![PathBuf::from("/venv/lib/site-packages")],
                version: Some("3.12.4".to_owned()),
            }),
        }
    }

    fn cache() -> EngineCache {
        EngineCache {
            dir: PathBuf::from("/cache/poly/repo"),
            salt: "6\0release-0.36.0".to_owned(),
        }
    }

    fn sent(environment: &ProjectEnvironment, cache: Option<&EngineCache>) -> Vec<(String, String)> {
        let mut pairs = environment.to_wire_pairs();
        pairs.extend(cache.into_iter().flat_map(EngineCache::to_wire_pairs));
        pairs
    }

    #[test]
    fn the_cache_round_trips_beside_the_environment() {
        let pairs = sent(&python(), Some(&cache()));
        assert_eq!(EngineCache::from_wire_pairs(&pairs), Some(cache()));
        assert_eq!(ProjectEnvironment::from_wire_pairs(&pairs), python());
    }

    /// A component that predates the cache reads the environment with `ProjectEnvironment::from_wire_pairs` alone,
    /// which ignores keys it does not know: the cache pairs change nothing it sees.
    #[test]
    fn a_component_that_predates_the_cache_reads_the_same_environment() {
        for environment in [python(), ProjectEnvironment::default()] {
            assert_eq!(
                ProjectEnvironment::from_wire_pairs(&sent(&environment, Some(&cache()))),
                ProjectEnvironment::from_wire_pairs(&sent(&environment, None)),
            );
        }
    }

    /// A host that predates the cache sends no cache pairs: the component runs without a store.
    #[test]
    fn a_host_that_predates_the_cache_sends_none() {
        assert_eq!(EngineCache::from_wire_pairs(&sent(&python(), None)), None);
        assert_eq!(EngineCache::from_wire_pairs(&[]), None);
    }
}
