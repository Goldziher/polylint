//! The surface a host builds on: the config step between parse and build, the lent program and its diagnostics, the
//! observer of what a program reads, the executor a program is freed on and module resolution without a program.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use common::{A, B, BASE, C, CONFIG, IMPLICIT_ANY, NOT_ASSIGNABLE, ROOT, build, host, set_strict, workspace};
use polylint_tsrs::{
    BuildRequest, Executor, MemoryFs, ModuleResolver, Observer, ProgramFiles, ProgramHost, SharedConfigs, SharedFiles,
};
use tsrs_core::arena::Region;

fn own_host(memory: &MemoryFs) -> ProgramHost {
    host(memory, &Arc::default(), &Arc::default())
}

fn found(file: &str, code: i32) -> (Option<String>, i32) {
    (Some(file.to_owned()), code)
}

#[test]
fn the_host_changes_the_parsed_config_before_the_program_is_built() {
    let memory = workspace();
    let host = own_host(&memory);
    let as_written = build(&host, |_| {}, None);
    assert!(!as_written.owner.program().options().strict.is_true());
    assert_eq!(
        as_written.diagnostics(None),
        [found(A, NOT_ASSIGNABLE), found(B, NOT_ASSIGNABLE)],
        "the config's own options"
    );
    let strict = build(&host, |config| set_strict(config, true), None);
    assert!(strict.owner.program().options().strict.is_true());
    assert_eq!(
        strict.diagnostics(None),
        [
            found(A, NOT_ASSIGNABLE),
            found(B, NOT_ASSIGNABLE),
            found(C, IMPLICIT_ANY)
        ],
        "the options the host set"
    );
    let shared = host.configs().config(CONFIG).usable().expect("the config parses");
    assert!(
        !shared.compiler_options().unwrap().strict.is_true(),
        "the run's parse of the config is unchanged"
    );
}

#[test]
fn the_diagnostics_of_a_program_are_those_of_the_files_asked_about() {
    let memory = workspace();
    let host = own_host(&memory);
    let built = build(&host, |config| set_strict(config, true), None);
    assert_eq!(built.diagnostics(Some(&[B])), [found(B, NOT_ASSIGNABLE)]);
    assert_eq!(
        built.diagnostics(Some(&[C, A])),
        [found(A, NOT_ASSIGNABLE), found(C, IMPLICIT_ANY)]
    );
    assert_eq!(
        built.diagnostics(Some(&["/ws/src/none.ts"])),
        [],
        "a file it does not hold"
    );
    assert_eq!(built.diagnostics(Some(&[])), []);
    assert!(
        built.owner.program().get_source_file(B).is_some(),
        "the program is lent"
    );
}

#[test]
fn a_config_that_cannot_be_used_is_an_error_with_its_diagnostics() {
    let memory = workspace().with_file("/ws/broken/tsconfig.json", r#"{ "include": ["src"" }"#);
    let host = own_host(&memory);
    let region = Region::new_scratch(1 << 16);
    let _scope = region.enter();
    let broken = host
        .load_config("/ws/broken/tsconfig.json")
        .err()
        .expect("syntax errors");
    assert!(!broken.is_empty(), "the syntax errors of the file");
    assert!(host.load_config("/ws/missing/tsconfig.json").is_err());
    assert!(host.load_config(CONFIG).is_ok());
}

/// What a program read, as an observer records it.
#[derive(Default)]
struct Recorder {
    reads: Mutex<BTreeSet<String>>,
    sources: Mutex<BTreeMap<String, Option<String>>>,
    configs: Mutex<BTreeMap<String, Option<Vec<String>>>>,
    lookups: AtomicUsize,
}

impl Observer for Recorder {
    fn file_read(&self, path: &str, text: Option<&str>) {
        if text.is_some() {
            self.reads.lock().unwrap().insert(path.to_owned());
        }
    }

    fn file_exists(&self, _path: &str, _exists: bool) {
        self.lookups.fetch_add(1, Ordering::Relaxed);
    }

    fn directory_exists(&self, _path: &str, _exists: bool) {
        self.lookups.fetch_add(1, Ordering::Relaxed);
    }

    fn source_file(&self, file_name: &str, text: Option<&str>) {
        self.sources
            .lock()
            .unwrap()
            .insert(file_name.to_owned(), text.map(str::to_owned));
    }

    fn config(&self, file_name: &str, extended: Option<&[String]>) {
        self.configs
            .lock()
            .unwrap()
            .insert(file_name.to_owned(), extended.map(<[String]>::to_vec));
    }
}

impl Recorder {
    fn project_sources(&self) -> Vec<String> {
        let sources = self.sources.lock().unwrap();
        sources.keys().filter(|name| name.starts_with(ROOT)).cloned().collect()
    }

    fn project_reads(&self) -> Vec<String> {
        let reads = self.reads.lock().unwrap();
        reads
            .iter()
            .filter(|name| name.starts_with("/ws/src/"))
            .cloned()
            .collect()
    }
}

#[test]
fn an_observer_is_told_what_a_program_reads() {
    let memory = workspace();
    let (files, configs) = (Arc::<SharedFiles>::default(), Arc::<SharedConfigs>::default());
    let first = Arc::new(Recorder::default());
    let observed = host(&memory, &files, &configs).with_observer(Arc::clone(&first) as Arc<dyn Observer>);
    let built = build(&observed, |_| {}, None);
    assert_eq!(first.project_sources(), [A, B, C], "the source files it asked for");
    assert_eq!(
        first.sources.lock().unwrap().get(A).cloned().flatten().as_deref(),
        Some("export const a: number = 'one';\n"),
        "with the text each was parsed from"
    );
    assert!(
        first
            .sources
            .lock()
            .unwrap()
            .keys()
            .any(|name| name.starts_with(&polylint_tsrs::lib_path())),
        "the lib files among them"
    );
    assert_eq!(first.project_reads(), [A, B, C], "the files it read to parse them");
    assert_eq!(
        *first.configs.lock().unwrap(),
        BTreeMap::from([(CONFIG.to_owned(), Some(vec![BASE.to_owned()]))]),
        "its config with the config that one extends"
    );
    assert!(
        first.lookups.load(Ordering::Relaxed) > 0,
        "the lookups of its module resolution"
    );

    // A second program of the run takes the parsed files from the first: it asks for them and reads none.
    let second = Arc::new(Recorder::default());
    let reusing = host(&memory, &files, &configs).with_observer(Arc::clone(&second) as Arc<dyn Observer>);
    let rebuilt = build(&reusing, |_| {}, None);
    assert_eq!(second.project_sources(), [A, B, C]);
    assert_eq!(second.project_reads(), Vec::<String>::new());
    assert_eq!(rebuilt.diagnostics(None), built.diagnostics(None));
}

#[test]
fn a_program_built_with_an_observer_is_the_program_built_without_one() {
    let memory = workspace();
    let plain = build(&own_host(&memory), |config| set_strict(config, true), None);
    let observed_host = own_host(&memory).with_observer(Arc::new(Recorder::default()));
    let observed = build(&observed_host, |config| set_strict(config, true), None);
    assert_eq!(observed.diagnostics(None), plain.diagnostics(None));
}

#[test]
fn one_call_builds_and_binds_a_program_with_the_hosts_changes() {
    let memory = workspace();
    let host = own_host(&memory);
    let executor = Arc::new(Counting::default());
    let files = Arc::new(ProgramFiles::default());
    let region = Region::new_scratch(1 << 20);
    let request = BuildRequest {
        config_path: CONFIG,
        checkers: 1,
        single_threaded: true,
        list_files: true,
        diagnostics_in_checker_regions: true,
    };
    let built = {
        let _scope = region.enter();
        host.build(
            &request,
            Arc::clone(&files),
            Some(&(Arc::clone(&executor) as Arc<dyn Executor>)),
            |config| set_strict(config, true),
        )
        .unwrap_or_else(|_| panic!("the config parses"))
    };
    assert_eq!(built.files, [A, B, C], "the files the config includes");
    assert_eq!(
        executor.0.load(Ordering::Relaxed),
        2,
        "created and bound on the executor"
    );
    let options = built.owner.program().options();
    assert!(options.strict.is_true(), "the host's change");
    assert_eq!(options.checkers, Some(1), "the request's checkers");
    let codes: Vec<i32> = {
        let _scope = region.enter();
        built
            .owner
            .diagnostics(None)
            .iter()
            .map(|diagnostic| diagnostic.code())
            .collect()
    };
    assert_eq!(codes, [NOT_ASSIGNABLE, NOT_ASSIGNABLE, IMPLICIT_ANY]);
    let missing = BuildRequest {
        config_path: "/ws/missing/tsconfig.json",
        ..request
    };
    assert!(host.build(&missing, Arc::clone(&files), None, |_| {}).is_err());
    let (owner, regions) = (built.owner, built.loader_regions);
    drop(owner);
    drop(regions);
    assert_eq!(executor.0.load(Ordering::Relaxed), 3, "freed on the executor");
}

/// An executor that counts the work it ran.
#[derive(Default)]
struct Counting(AtomicUsize);

impl Executor for Counting {
    fn run(&self, work: &mut (dyn FnMut() + Send)) {
        self.0.fetch_add(1, Ordering::Relaxed);
        work();
    }
}

#[test]
fn a_program_is_freed_on_its_executor_when_its_owner_is_dropped() {
    let memory = workspace();
    let executor = Arc::new(Counting::default());
    let built = build(
        &own_host(&memory),
        |_| {},
        Some(Arc::clone(&executor) as Arc<dyn Executor>),
    );
    assert_eq!(executor.0.load(Ordering::Relaxed), 0);
    drop(built);
    assert_eq!(executor.0.load(Ordering::Relaxed), 1);
}

#[test]
fn a_poisoned_program_is_left_to_the_process() {
    let memory = workspace();
    let executor = Arc::new(Counting::default());
    let built = build(
        &own_host(&memory),
        |_| {},
        Some(Arc::clone(&executor) as Arc<dyn Executor>),
    );
    built.owner.poison();
    assert!(built.owner.poisoned());
    drop(built);
    assert_eq!(executor.0.load(Ordering::Relaxed), 0, "never freed");
}

#[test]
fn modules_resolve_without_a_program() {
    let memory = workspace()
        .with_file(
            "/ws/node_modules/shapes/package.json",
            r#"{ "name": "shapes", "types": "index.d.ts" }"#,
        )
        .with_file("/ws/node_modules/shapes/index.d.ts", "export interface Shape {}\n");
    let host = own_host(&memory);
    let region = Region::new_scratch(1 << 16);
    let _scope = region.enter();
    let config = host.configs().config(CONFIG).usable().expect("the config parses");
    let resolver = ModuleResolver::new(host.configs().host(), config.compiler_options().unwrap());
    let mode = tsrs_core::RESOLUTION_MODE_NONE;
    let local = resolver.resolve("./a", B, mode).expect("a relative module");
    assert_eq!((local.file_name.as_str(), local.external_library), (A, false));
    assert_eq!(local.extension, ".ts");
    let package = resolver.resolve("shapes", B, mode).expect("a package");
    assert_eq!(
        (package.file_name.as_str(), package.external_library),
        ("/ws/node_modules/shapes/index.d.ts", true)
    );
    assert_eq!(resolver.resolve("./missing", B, mode), None);
    assert_eq!(resolver.resolve("unknown-package", B, mode), None);
}
