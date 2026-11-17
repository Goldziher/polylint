//! A small project and the two steps of building its program through `polylint-tsrs`.

#![allow(dead_code, clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;

use polylint_tsrs::{Executor, MemoryFs, ProgramFiles, ProgramHost, ProgramOwner, SharedConfigs, SharedFiles};
use tsrs_compiler::ProgramRegions;
use tsrs_core::arena::{Region, RegionScope};
use tsrs_core::{CompilerOptions, P, Tristate};
use tsrs_tsoptions::ParsedCommandLine;

pub const ROOT: &str = "/ws";
pub const CONFIG: &str = "/ws/tsconfig.json";
pub const BASE: &str = "/ws/tsconfig.base.json";
pub const A: &str = "/ws/src/a.ts";
pub const B: &str = "/ws/src/b.ts";
pub const C: &str = "/ws/src/c.ts";

/// `Type '{0}' is not assignable to type '{1}'.`
pub const NOT_ASSIGNABLE: i32 = 2322;
/// `Parameter '{0}' implicitly has an '{1}' type.`
pub const IMPLICIT_ANY: i32 = 7006;

/// A project whose config extends a base that is not strict: `a.ts` and `b.ts` each hold a type error, and `c.ts` one
/// that only a strict program reports.
pub fn workspace() -> MemoryFs {
    MemoryFs::new()
        .with_file(BASE, r#"{ "compilerOptions": { "strict": false, "noEmit": true } }"#)
        .with_file(CONFIG, r#"{ "extends": "./tsconfig.base.json", "include": ["src"] }"#)
        .with_file(A, "export const a: number = 'one';\n")
        .with_file(B, "import { a } from './a';\nexport const b: string = a;\n")
        .with_file(C, "export function c(value) {\n  return value;\n}\n")
}

/// A host of the programs of `memory`, sharing `files` and `configs`.
pub fn host(memory: &MemoryFs, files: &Arc<SharedFiles>, configs: &Arc<SharedConfigs>) -> ProgramHost {
    let (fs, boms) = memory.clone().into_fs();
    ProgramHost::new(fs, boms, ROOT.to_owned(), Arc::clone(files), Arc::clone(configs))
}

/// A built program with what it points into. The fields drop in order: the program, its loader regions, the region
/// of its config and compiler host, then its hold on the project files.
pub struct Built {
    pub owner: ProgramOwner,
    _regions: Arc<ProgramRegions>,
    region: Region,
    _files: Arc<ProgramFiles>,
}

impl Built {
    /// Enters the program's region, where a check of the program allocates.
    pub fn enter(&self) -> RegionScope {
        self.region.enter()
    }

    /// The file and code of every diagnostic of the program, or of `files` alone, as owned data.
    pub fn diagnostics(&self, files: Option<&[&str]>) -> Vec<(Option<String>, i32)> {
        let _scope = self.enter();
        self.owner
            .diagnostics(files)
            .iter()
            .map(|diagnostic| {
                (
                    diagnostic.file().map(|file| file.file_name().to_owned()),
                    diagnostic.code(),
                )
            })
            .collect()
    }
}

/// Sets `strict` in the compiler options of `config`, as a host overrides an option of the parsed config.
pub fn set_strict(config: &mut ParsedCommandLine, strict: bool) {
    let base = config.compiler_options().expect("a config has compiler options");
    config.parsed_config.compiler_options = Some(P::new(CompilerOptions {
        strict: if strict { Tristate::True } else { Tristate::False },
        ..(*base).clone()
    }));
}

/// Builds the program of [`CONFIG`] through `host` on one thread, `change` changing the parsed config in between;
/// it is freed on `executor`.
pub fn build(
    host: &ProgramHost,
    change: impl FnOnce(&mut ParsedCommandLine),
    executor: Option<Arc<dyn Executor>>,
) -> Built {
    let files = Arc::new(ProgramFiles::default());
    let region = Region::new_scratch(1 << 20);
    let regions: Arc<ProgramRegions> = Arc::default();
    let owner = {
        let _scope = region.enter();
        let mut config = host.load_config(CONFIG).expect("the config parses");
        change(&mut config);
        let base = config.compiler_options().expect("a config has compiler options");
        config.parsed_config.compiler_options = Some(P::new(CompilerOptions {
            checkers: Some(1),
            single_threaded: Tristate::True,
            ..(*base).clone()
        }));
        let mut options = host.program_options(CONFIG, config, Arc::clone(&files), Arc::clone(&regions));
        options.single_threaded = Tristate::True;
        options.checker_diagnostic_regions = true;
        ProgramOwner::new(options, Arc::clone(&files), executor)
    };
    Built {
        owner,
        _regions: regions,
        region,
        _files: files,
    }
}
