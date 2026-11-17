//! Which programs to build: every judged file goes to the `tsconfig.json` or `jsconfig.json` TypeScript's own
//! discovery gives it (tsserver's rules, after tsrs-lint's `find_tsconfig`), each config is built once, and a file no
//! config includes stays untyped. The analysis builds no inferred program (skif ADR 0010).

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::{Path, PathBuf};

use crate::hash::HashSet;
use polylint_tsrs::{ConfigHost, ConfigReader, Parsed, tsrs_path};
use tsrs_core::{P, tspath};
use tsrs_tsoptions::{ParseConfigHost, ParsedCommandLine};

use crate::{TypeError, TypeInput, TypePackage};

/// The programs to build and where each judged file goes.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct ProgramPlan {
    /// The programs, ordered by config path.
    pub(crate) programs: Vec<PlannedProgram>,
    /// Every judged file, package by package in input order.
    pub(crate) files: Vec<PlannedFile>,
}

/// A config to build, with the judged files it holds.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct PlannedProgram {
    /// The config, by its canonical path as tsrs names files.
    pub(crate) config: String,
    /// Indices into [`ProgramPlan::files`].
    pub(crate) files: Vec<usize>,
}

/// A judged file.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct PlannedFile {
    /// The index of its package in [`TypeInput::packages`].
    pub(crate) package: usize,
    /// The path the host gave.
    pub(crate) path: PathBuf,
    /// The path as tsrs names it.
    pub(crate) name: String,
    /// Where it goes.
    pub(crate) placement: Placement,
}

/// Where a judged file goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Placement {
    /// Into this program of [`ProgramPlan::programs`].
    Program(usize),
    /// No `tsconfig.json` or `jsconfig.json` is above it.
    NoConfig,
    /// No config holds it, and one on the way cannot be parsed.
    ConfigError,
    /// The configs above it parse, but none includes it.
    Outside,
}

/// Where the search for a file's config led, as a planning keeps it: the config by its canonical path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Located {
    Config(String),
    NoConfig,
    ConfigError,
    Outside,
}

/// Where each file of a planning went, by its name as tsrs names files and whether the search started at the config
/// given for the root package. A file's place depends on nothing else, so a later planning of the same files over the
/// same file system reuses it instead of searching again.
pub(crate) type Lookups = BTreeMap<(String, bool), Located>;

/// Where the search for a file's config starts.
#[derive(Clone, Copy)]
enum Start<'a> {
    /// The nearest config above the file, then the configs above that one.
    Discover,
    /// This config and the configs it references, nothing above (`[js] tsconfig`).
    Given(&'a str),
}

/// Plans the programs of `input`, reading configs with `reader`: per judged file its config, deduplicated by canonical
/// path. A file `known` holds keeps the place it has there; the others are searched. Returns the plan with the places
/// of the searched files.
///
/// # Errors
/// [`TypeError::Config`] when the `tsconfig` given for the root package cannot be read or parsed.
pub(crate) fn plan_programs(
    input: &TypeInput<'_>,
    reader: &ConfigReader,
    known: &Lookups,
) -> Result<(ProgramPlan, Lookups), TypeError> {
    let given = input
        .tsconfig_override
        .map(|config| given_config(input.root, config, reader))
        .transpose()?;
    let given = given.as_deref();
    let mut configs = Configs::new(reader);
    configs.prefetch(input.packages.iter().flat_map(|package| {
        let start = start_of(package, input.root, given);
        package
            .files
            .iter()
            .map(move |path| (tsrs_path(path), start))
            .filter(move |(name, start)| !known.contains_key(&(name.clone(), matches!(start, Start::Given(_)))))
    }));
    let mut searched = Lookups::new();
    let mut by_config: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    let mut files = Vec::new();
    for (package_index, package) in input.packages.iter().enumerate() {
        let start = start_of(package, input.root, given);
        for path in &package.files {
            let name = tsrs_path(path);
            let key = (name, matches!(start, Start::Given(_)));
            let cached = known.get(&key).or_else(|| searched.get(&key)).cloned();
            let located = cached.unwrap_or_else(|| {
                let located = configs.locate(&key.0, start);
                searched.insert(key.clone(), located.clone());
                located
            });
            let (name, _) = key;
            let placement = match located {
                Located::Config(config) => {
                    by_config.entry(config).or_default().push(files.len());
                    // Replaced by the program index once every file is placed.
                    Placement::Program(usize::MAX)
                }
                Located::NoConfig => Placement::NoConfig,
                Located::ConfigError => Placement::ConfigError,
                Located::Outside => Placement::Outside,
            };
            files.push(PlannedFile {
                package: package_index,
                path: path.clone(),
                name,
                placement,
            });
        }
    }
    let programs = by_config
        .into_iter()
        .enumerate()
        .map(|(index, (config, members))| {
            for &file in &members {
                files[file].placement = Placement::Program(index);
            }
            PlannedProgram { config, files: members }
        })
        .collect();
    Ok((ProgramPlan { programs, files }, searched))
}

/// Where the search for the config of a file of `package` starts: at `given`, the config given for the package at
/// `root`, or at the nearest config above the file.
fn start_of<'g>(package: &TypePackage, root: &Path, given: Option<&'g str>) -> Start<'g> {
    match given {
        Some(config) if package.dir == root => Start::Given(config),
        _ => Start::Discover,
    }
}

/// The `tsconfig` given for the root package, absolute, once it parses.
fn given_config(root: &Path, config: &Path, reader: &ConfigReader) -> Result<String, TypeError> {
    let name = tsrs_path(&root.join(config));
    if let Some(message) = reader.config(&name).problem {
        return Err(TypeError::Config {
            path: config.to_path_buf(),
            message,
        });
    }
    Ok(name)
}

/// The configs one planning met, with how far the searches through them went.
///
/// A search for a file's config goes as tsserver's does: the nearest config, then breadth first through its project
/// references, then the next config above it (unless `disableSolutionSearching` is set), and the file goes to the
/// first config visited whose program holds it. Which configs a search visits, in which order, depends only on the
/// config it starts at, not on the file, so each search is walked once, only as far as a file needs, and a file first
/// looks up the configs that hold it among those visited: a workspace of many configs whose test files no config
/// holds would otherwise walk every config for each of those files.
struct Configs<'a> {
    reader: &'a ConfigReader,
    /// Each config met; `None` for one that cannot be read or parsed.
    parsed: Vec<Option<P<ParsedCommandLine>>>,
    /// The index of each config in `parsed`, by its path as tsrs keys files.
    by_path: crate::hash::HashMap<tspath::Path, usize>,
    /// The configs whose programs hold a file, by the file's path as tsrs keys files.
    holders: crate::hash::HashMap<tspath::Path, Vec<usize>>,
    /// The search from a config, by the config and whether it was given for the root package.
    searches: crate::hash::HashMap<(String, bool), Search>,
}

/// A search as far as it went, and how it goes on.
struct Search {
    /// Each config visited, by its path as the search reached it and its index in [`Configs::parsed`].
    visits: Vec<(String, usize)>,
    /// The first visit of each config visited.
    first: crate::hash::HashMap<usize, usize>,
    /// Whether a config on the way cannot be read or parsed.
    errored: bool,
    /// The config whose references the search walks; `None` once it walked them all.
    level: Option<String>,
    /// The configs queued at this level.
    queue: VecDeque<String>,
    /// Whether the config of this level stops the search at this level.
    disable_solution_searching: bool,
    /// Whether the search stops at its first level, as for the config given for the root package.
    given: bool,
    visited: HashSet<String>,
}

impl Search {
    fn new(first: String, given: bool) -> Self {
        Self {
            visits: Vec::new(),
            first: crate::hash::HashMap::default(),
            errored: false,
            queue: VecDeque::from([first.clone()]),
            level: Some(first),
            disable_solution_searching: false,
            given,
            visited: HashSet::default(),
        }
    }
}

impl<'a> Configs<'a> {
    fn new(reader: &'a ConfigReader) -> Self {
        Self {
            reader,
            parsed: Vec::new(),
            by_path: crate::hash::HashMap::default(),
            holders: crate::hash::HashMap::default(),
            searches: crate::hash::HashMap::default(),
        }
    }

    /// Reads, on the threads of the current rayon pool, the configs the searches of `files` (by name, with where
    /// their search starts) start at and those their project references reach, so that the searches find them read:
    /// one at a time, the planning would read every config of the workspace on one thread.
    fn prefetch<'s>(&mut self, files: impl Iterator<Item = (String, Start<'s>)>) {
        use rayon::prelude::*;

        let reader = self.reader;
        let host = reader.host();
        let mut seen: BTreeSet<String> = files
            .filter_map(|(name, start)| match start {
                Start::Discover => compute_config_file_name(&name, false, host),
                Start::Given(config) => Some(config.to_owned()),
            })
            .collect();
        let mut wave: Vec<String> = seen.iter().cloned().collect();
        while !wave.is_empty() {
            let read: Vec<(String, Option<Parsed>)> = wave
                .into_par_iter()
                .map(|path| {
                    let parsed = reader.prefetch(&path);
                    (path, parsed)
                })
                .collect();
            wave = Vec::new();
            for (path, parsed) in read {
                // Read by the search, on this thread, instead.
                let Some(parsed) = parsed else {
                    continue;
                };
                let config = parsed.usable();
                if let Some(config) = config {
                    wave.extend(
                        config
                            .resolved_project_reference_paths()
                            .iter()
                            .filter(|reference| seen.insert((*reference).clone()))
                            .cloned(),
                    );
                }
                self.insert(&path, config);
            }
        }
    }

    /// Where the search for the config of `file` from `start` leads.
    fn locate(&mut self, file: &str, start: Start<'_>) -> Located {
        let reader = self.reader;
        let host = reader.host();
        let first = match start {
            Start::Discover => compute_config_file_name(file, false, host),
            Start::Given(config) => Some(config.to_owned()),
        };
        let Some(first) = first else {
            return Located::NoConfig;
        };
        let given = matches!(start, Start::Given(_));
        let key = (first, given);
        let mut search = self
            .searches
            .remove(&key)
            .unwrap_or_else(|| Search::new(key.0.clone(), given));
        let found = self.find(&mut search, &host.to_path(file));
        let located = match found {
            Some(visit) => Located::Config(host.fs().realpath(&search.visits[visit].0)),
            None if search.errored => Located::ConfigError,
            None => Located::Outside,
        };
        self.searches.insert(key, search);
        located
    }

    /// The first visit of `search` to a config whose program holds `file`, walking the search on as far as needed.
    fn find(&mut self, search: &mut Search, file: &tspath::Path) -> Option<usize> {
        let visited = self
            .holders
            .get(file)
            .into_iter()
            .flatten()
            .filter_map(|config| search.first.get(config))
            .min();
        if let Some(&visit) = visited {
            return Some(visit);
        }
        while let Some(index) = self.step(search) {
            if self.parsed[index].is_some_and(|config| config.file_names_by_path().contains_key(file)) {
                return Some(search.visits.len() - 1);
            }
        }
        None
    }

    /// Visits the next config of `search`, reading it; returns its index, or `None` once the search is done.
    fn step(&mut self, search: &mut Search) -> Option<usize> {
        let reader = self.reader;
        loop {
            let level = search.level.clone()?;
            let Some(path) = search.queue.pop_front() else {
                search.level = if search.disable_solution_searching || search.given {
                    None
                } else {
                    compute_config_file_name(&level, true, reader.host())
                };
                search.disable_solution_searching = false;
                search.queue.extend(search.level.clone());
                continue;
            };
            if !search.visited.insert(path.clone()) {
                continue;
            }
            let index = self.config(&path);
            let Some(config) = self.parsed[index] else {
                search.errored = true;
                continue;
            };
            if path == level {
                search.disable_solution_searching = config
                    .compiler_options()
                    .is_some_and(|options| options.disable_solution_searching.is_true());
            }
            search.first.entry(index).or_insert(search.visits.len());
            search.visits.push((path, index));
            search
                .queue
                .extend(config.resolved_project_reference_paths().iter().cloned());
            return Some(index);
        }
    }

    /// The index of the config at `path`, read on its first visit.
    fn config(&mut self, path: &str) -> usize {
        if let Some(index) = self.index(path) {
            return index;
        }
        let config = self.reader.config(path).usable();
        self.insert(path, config)
    }

    fn index(&self, path: &str) -> Option<usize> {
        self.by_path.get(&self.reader.host().to_path(path)).copied()
    }

    /// Keeps `config`, read from `path`, unless a config of its path is kept already; returns its index. A config's
    /// program holds the files its `file_names` lists; a solution config (no files) holds none.
    fn insert(&mut self, path: &str, config: Option<P<ParsedCommandLine>>) -> usize {
        if let Some(index) = self.index(path) {
            return index;
        }
        let index = self.parsed.len();
        if let Some(config) = config {
            for file in config.file_names_by_path().keys() {
                self.holders.entry(file.clone()).or_default().push(index);
            }
        }
        self.parsed.push(config);
        self.by_path.insert(self.reader.host().to_path(path), index);
        index
    }
}

/// The nearest `tsconfig.json` or `jsconfig.json` above `file`; with `skip_directory`, above the config `file` (a
/// `jsconfig.json` beside a `tsconfig.json` is still found).
fn compute_config_file_name(file: &str, skip_directory: bool, host: &ConfigHost) -> Option<String> {
    let mut directory = tspath::get_directory_path(file);
    // Whether the `tsconfig.json` and the `jsconfig.json` of the first directory are skipped.
    let mut skipped = [skip_directory, skip_directory && !file.ends_with("/tsconfig.json")];
    loop {
        for (name, skip) in ["tsconfig.json", "jsconfig.json"].into_iter().zip(skipped) {
            if skip {
                continue;
            }
            let path = tspath::combine_paths(&directory, &[name]);
            if host.fs().file_exists(&path) {
                return Some(path);
            }
        }
        if directory.ends_with("/node_modules") {
            return None;
        }
        let parent = tspath::get_directory_path(&directory);
        if parent == directory {
            return None;
        }
        directory = parent;
        skipped = [false; 2];
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::{MemoryFs, TypePackage, TypeQuery};

    const ROOT: &str = "/ws";

    fn package(dir: &str, files: &[&str]) -> TypePackage {
        TypePackage {
            dir: PathBuf::from(dir),
            name: None,
            files: files.iter().map(PathBuf::from).collect(),
        }
    }

    /// A reader of the configs of `memory`, with configs of its own.
    fn reader(memory: MemoryFs) -> ConfigReader {
        let (fs, _) = crate::vfs::tsrs_fs(Arc::new(memory));
        ConfigReader::new(Arc::default(), Arc::default(), ConfigHost::new(fs, ROOT.to_owned()))
    }

    fn plan_with(memory: MemoryFs, packages: &[TypePackage], given: Option<&Path>) -> Result<ProgramPlan, TypeError> {
        plan_known(memory, packages, given, &Lookups::new()).map(|(plan, _)| plan)
    }

    fn plan_known(
        memory: MemoryFs,
        packages: &[TypePackage],
        given: Option<&Path>,
        known: &Lookups,
    ) -> Result<(ProgramPlan, Lookups), TypeError> {
        let reader = reader(memory);
        let query = TypeQuery::default();
        let mut input = TypeInput::new(Arc::new(MemoryFs::new()), Path::new(ROOT), packages, &query);
        if let Some(config) = given {
            input = input.with_tsconfig_override(config);
        }
        plan_programs(&input, &reader, known)
    }

    fn plan(memory: MemoryFs, packages: &[TypePackage]) -> ProgramPlan {
        plan_with(memory, packages, None).unwrap()
    }

    /// Per judged file, the config of its program or how it stays untyped.
    fn placements(plan: &ProgramPlan) -> Vec<(String, String)> {
        plan.files
            .iter()
            .map(|file| {
                let place = match file.placement {
                    Placement::Program(index) => plan.programs[index].config.clone(),
                    other => format!("{other:?}"),
                };
                (file.name.clone(), place)
            })
            .collect()
    }

    fn pairs(expected: &[(&str, &str)]) -> Vec<(String, String)> {
        expected
            .iter()
            .map(|&(file, place)| (file.to_owned(), place.to_owned()))
            .collect()
    }

    #[test]
    fn each_file_goes_to_its_nearest_config() {
        let memory = MemoryFs::new()
            .with_file("/ws/tsconfig.json", r#"{ "include": ["src"] }"#)
            .with_file("/ws/src/root.ts", "")
            .with_file("/ws/packages/a/tsconfig.json", r#"{ "include": ["src"] }"#)
            .with_file("/ws/packages/a/src/a.ts", "")
            .with_file("/ws/packages/b/jsconfig.json", r#"{ "include": ["lib"] }"#)
            .with_file("/ws/packages/b/lib/b.js", "");
        let packages = [
            package("/ws", &["/ws/src/root.ts"]),
            package("/ws/packages/a", &["/ws/packages/a/src/a.ts"]),
            package("/ws/packages/b", &["/ws/packages/b/lib/b.js"]),
        ];
        let plan = plan(memory, &packages);
        assert_eq!(
            placements(&plan),
            pairs(&[
                ("/ws/src/root.ts", "/ws/tsconfig.json"),
                ("/ws/packages/a/src/a.ts", "/ws/packages/a/tsconfig.json"),
                ("/ws/packages/b/lib/b.js", "/ws/packages/b/jsconfig.json"),
            ])
        );
        let configs: Vec<&str> = plan.programs.iter().map(|program| program.config.as_str()).collect();
        assert_eq!(
            configs,
            [
                "/ws/packages/a/tsconfig.json",
                "/ws/packages/b/jsconfig.json",
                "/ws/tsconfig.json"
            ]
        );
    }

    #[test]
    fn a_solution_config_is_followed_to_the_referenced_config_that_holds_the_file() {
        let memory = MemoryFs::new()
            .with_file(
                "/ws/tsconfig.json",
                r#"{ "files": [], "references": [{ "path": "./tsconfig.node.json" }, { "path": "./tsconfig.app.json" }] }"#,
            )
            .with_file("/ws/tsconfig.node.json", r#"{ "compilerOptions": { "composite": true }, "include": ["vite.config.ts"] }"#)
            .with_file("/ws/tsconfig.app.json", r#"{ "compilerOptions": { "composite": true }, "include": ["src"] }"#)
            .with_file("/ws/vite.config.ts", "")
            .with_file("/ws/src/main.ts", "");
        let plan = plan(memory, &[package("/ws", &["/ws/src/main.ts", "/ws/vite.config.ts"])]);
        assert_eq!(
            placements(&plan),
            pairs(&[
                ("/ws/src/main.ts", "/ws/tsconfig.app.json"),
                ("/ws/vite.config.ts", "/ws/tsconfig.node.json"),
            ])
        );
    }

    #[test]
    fn a_config_that_excludes_the_file_hands_it_to_the_config_above() {
        let memory = MemoryFs::new()
            .with_file("/ws/tsconfig.json", r#"{ "include": ["packages/*/test"] }"#)
            .with_file("/ws/packages/a/tsconfig.json", r#"{ "include": ["src"] }"#)
            .with_file("/ws/packages/a/src/a.ts", "")
            .with_file("/ws/packages/a/test/a.test.ts", "");
        let plan = plan(
            memory,
            &[package(
                "/ws/packages/a",
                &["/ws/packages/a/src/a.ts", "/ws/packages/a/test/a.test.ts"],
            )],
        );
        assert_eq!(
            placements(&plan),
            pairs(&[
                ("/ws/packages/a/src/a.ts", "/ws/packages/a/tsconfig.json"),
                ("/ws/packages/a/test/a.test.ts", "/ws/tsconfig.json"),
            ])
        );
    }

    #[test]
    fn files_without_a_config_or_outside_every_config_stay_untyped() {
        let memory = MemoryFs::new()
            .with_file("/ws/packages/a/tsconfig.json", r#"{ "include": ["src"] }"#)
            .with_file("/ws/packages/a/src/a.ts", "")
            .with_file("/ws/packages/a/scripts/build.ts", "")
            .with_file("/ws/packages/b/b.ts", "");
        let packages = [
            package(
                "/ws/packages/a",
                &["/ws/packages/a/src/a.ts", "/ws/packages/a/scripts/build.ts"],
            ),
            package("/ws/packages/b", &["/ws/packages/b/b.ts"]),
        ];
        let plan = plan(memory, &packages);
        assert_eq!(
            placements(&plan),
            pairs(&[
                ("/ws/packages/a/src/a.ts", "/ws/packages/a/tsconfig.json"),
                ("/ws/packages/a/scripts/build.ts", "Outside"),
                ("/ws/packages/b/b.ts", "NoConfig"),
            ])
        );
    }

    #[test]
    fn a_config_that_does_not_parse_is_a_config_error() {
        let memory = MemoryFs::new()
            .with_file("/ws/tsconfig.json", r#"{ "include": ["src"" }"#)
            .with_file("/ws/src/a.ts", "");
        let plan = plan(memory, &[package("/ws", &["/ws/src/a.ts"])]);
        assert_eq!(placements(&plan), pairs(&[("/ws/src/a.ts", "ConfigError")]));
        assert_eq!(plan.programs.len(), 0);
    }

    #[test]
    fn packages_that_share_a_config_share_one_program() {
        let memory = MemoryFs::new()
            .with_file("/ws/tsconfig.json", r#"{ "include": ["packages"] }"#)
            .with_file("/ws/packages/a/a.ts", "")
            .with_file("/ws/packages/b/b.ts", "")
            .with_file("/ws/packages/c/tsconfig.json", r#"{ "include": ["src"] }"#)
            .with_file("/ws/packages/c/src/c.ts", "");
        let packages = [
            package("/ws/packages/a", &["/ws/packages/a/a.ts"]),
            package("/ws/packages/b", &["/ws/packages/b/b.ts"]),
        ];
        let plan = plan(memory, &packages);
        assert_eq!(
            plan.programs,
            [PlannedProgram {
                config: "/ws/tsconfig.json".to_owned(),
                files: vec![0, 1],
            }]
        );
    }

    #[test]
    fn the_given_tsconfig_replaces_discovery_for_the_root_package_only() {
        let memory = MemoryFs::new()
            .with_file("/ws/tsconfig.json", r#"{ "include": ["src", "packages"] }"#)
            .with_file("/ws/tsconfig.build.json", r#"{ "include": ["src"] }"#)
            .with_file("/ws/src/a.ts", "")
            .with_file("/ws/scripts/x.ts", "")
            .with_file("/ws/packages/b/b.ts", "");
        let packages = [
            package("/ws", &["/ws/src/a.ts", "/ws/scripts/x.ts"]),
            package("/ws/packages/b", &["/ws/packages/b/b.ts"]),
        ];
        let plan = plan_with(memory, &packages, Some(Path::new("tsconfig.build.json"))).unwrap();
        assert_eq!(
            placements(&plan),
            pairs(&[
                ("/ws/src/a.ts", "/ws/tsconfig.build.json"),
                ("/ws/scripts/x.ts", "Outside"),
                ("/ws/packages/b/b.ts", "/ws/tsconfig.json"),
            ])
        );
    }

    #[test]
    fn a_given_tsconfig_that_cannot_be_read_is_an_error() {
        let memory = MemoryFs::new().with_file("/ws/src/a.ts", "");
        let error = plan_with(
            memory,
            &[package("/ws", &["/ws/src/a.ts"])],
            Some(Path::new("missing.json")),
        );
        assert!(
            matches!(&error, Err(TypeError::Config { path, .. }) if path == Path::new("missing.json")),
            "{error:?}"
        );
    }

    #[test]
    fn a_planning_keeps_the_places_it_is_given_and_returns_those_it_searched() {
        let memory = MemoryFs::new()
            .with_file("/ws/a/tsconfig.json", r#"{ "include": ["src"] }"#)
            .with_file("/ws/a/src/a.ts", "")
            .with_file("/ws/b/src/b.ts", "");
        let early = [package("/ws/a", &["/ws/a/src/a.ts"])];
        let (_, searched) = plan_known(memory, &early, None, &Lookups::new()).unwrap();
        assert_eq!(
            searched,
            Lookups::from([(
                ("/ws/a/src/a.ts".to_owned(), false),
                Located::Config("/ws/a/tsconfig.json".to_owned())
            )])
        );
        // A file system without the config: only a known place can still put `a.ts` into its program.
        let without_config = MemoryFs::new()
            .with_file("/ws/a/src/a.ts", "")
            .with_file("/ws/b/src/b.ts", "");
        let later = [
            package("/ws/a", &["/ws/a/src/a.ts"]),
            package("/ws/b", &["/ws/b/src/b.ts"]),
        ];
        let (plan, searched_later) = plan_known(without_config, &later, None, &searched).unwrap();
        assert_eq!(
            placements(&plan),
            pairs(&[
                ("/ws/a/src/a.ts", "/ws/a/tsconfig.json"),
                ("/ws/b/src/b.ts", "NoConfig"),
            ])
        );
        assert_eq!(
            searched_later,
            Lookups::from([(("/ws/b/src/b.ts".to_owned(), false), Located::NoConfig)]),
            "only the file the known places lack is searched"
        );
    }

    #[test]
    fn a_file_two_referenced_configs_hold_goes_to_the_one_the_search_visits_first_and_a_broken_one_only_errs_last() {
        let holding = r#"{ "include": ["../shared"] }"#;
        let cases: [(&[&str], &str, &str); 5] = [
            (&["./x", "./y"], "/ws/shared/f.ts", "/ws/x/tsconfig.json"),
            (&["./y", "./x"], "/ws/shared/f.ts", "/ws/y/tsconfig.json"),
            (&["./broken", "./x"], "/ws/shared/f.ts", "/ws/x/tsconfig.json"),
            (&["./broken", "./x"], "/ws/other/g.ts", "ConfigError"),
            (&["./x"], "/ws/other/g.ts", "Outside"),
        ];
        for (references, file, expected) in cases {
            let references: Vec<String> = references
                .iter()
                .map(|path| format!(r#"{{ "path": "{path}" }}"#))
                .collect();
            let memory = MemoryFs::new()
                .with_file(
                    "/ws/tsconfig.json",
                    format!(r#"{{ "files": [], "references": [{}] }}"#, references.join(", ")),
                )
                .with_file("/ws/x/tsconfig.json", holding)
                .with_file("/ws/y/tsconfig.json", holding)
                .with_file("/ws/broken/tsconfig.json", r#"{ "include": ["src"" }"#)
                .with_file("/ws/shared/f.ts", "")
                .with_file("/ws/other/g.ts", "");
            let plan = plan(memory, &[package("/ws", &[file])]);
            assert_eq!(
                placements(&plan),
                pairs(&[(file, expected)]),
                "references {references:?}"
            );
        }
    }

    #[test]
    fn a_planning_reads_each_config_once_and_a_later_planning_reads_them_from_the_run() {
        let memory = MemoryFs::new()
            .with_file(
                "/ws/tsconfig.json",
                r#"{ "files": [], "references": [{ "path": "./packages/a" }, { "path": "./packages/b" }] }"#,
            )
            .with_file("/ws/packages/a/tsconfig.json", r#"{ "include": ["src"] }"#)
            .with_file("/ws/packages/b/tsconfig.json", r#"{ "include": ["src"] }"#)
            .with_file("/ws/packages/a/src/a.ts", "");
        let tests: Vec<String> = (0..4).map(|index| format!("/ws/packages/a/test/{index}.ts")).collect();
        let mut files = vec!["/ws/packages/a/src/a.ts", "/ws/packages/b/test/b.ts"];
        files.extend(tests.iter().map(String::as_str));
        let memory = files.iter().fold(memory, |memory, file| memory.with_file(*file, ""));
        let packages = [package("/ws/packages/a", &files)];
        let reader = reader(memory);
        let query = TypeQuery::default();
        let input = TypeInput::new(Arc::new(MemoryFs::new()), Path::new(ROOT), &packages, &query);
        let (first, _) = plan_programs(&input, &reader, &Lookups::new()).unwrap();
        let mut expected = vec![
            ("/ws/packages/a/src/a.ts", "/ws/packages/a/tsconfig.json"),
            ("/ws/packages/b/test/b.ts", "Outside"),
        ];
        expected.extend(tests.iter().map(|test| (test.as_str(), "Outside")));
        assert_eq!(placements(&first), pairs(&expected));
        let counts = |parsed, reused| crate::SharedConfigCounts {
            parsed,
            reused,
            unshared: 0,
        };
        assert_eq!(
            crate::SharedConfigCounts::from(reader.counts().0),
            counts(3, 0),
            "the two package configs and the solution above them, each read once for every file outside them"
        );
        let (second, _) = plan_programs(&input, &reader, &Lookups::new()).unwrap();
        assert_eq!(second, first);
        assert_eq!(
            crate::SharedConfigCounts::from(reader.counts().0),
            counts(3, 3),
            "the later planning parses no config"
        );
    }
}
