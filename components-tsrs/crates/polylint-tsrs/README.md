# polylint-tsrs

Programs of the TypeScript checker of [tsrs](https://github.com/oxc-project/tsrs), the Rust port of the TypeScript 7
compiler, built over a host's file system. It is the layer [polylint](https://github.com/polylint/polylint) builds its
TypeScript type check on, published so that other hosts of tsrs can share it.

- `wrap_fs`: the file system tsrs reads through, over the host's own `tsrs_vfs::FS`, with the bundled `lib.*.d.ts`
  files; `BomFiles` records the files whose byte order mark the host dropped.
- `SharedFiles` and `SharedConfigs`: every file and config the programs of a run read is parsed once for the run.
- `MemoryFs`: a read-only, case-sensitive file system of exactly the files it was given.
- `ProgramHost`: builds a program in two steps (`load_config`, `program_options`) or in one call (`build`), so the
  host can change the parsed config (compiler options, config errors) before the program is built.
- `ProgramOwner`: lends the built program and its diagnostics, for all files or the files asked about, and frees the
  program's memory when dropped, on the host's `Executor` if it gives one.
- `SharedPrograms`, `Parallelism`, `Schedule`, `Cores`, `Limits`: a program several analyses of a run need is built
  once, and the programs built at once share the machine's cores within the host's limits.
- `Observer`: told what a program reads (file system answers, source files, configs).
- `CacheStore`, `CacheNamespace` and the `cache` envelope: the seam of a host's cache. Nothing is cached by the crate.
- `ModuleResolver`: resolves module specifiers under a config's options without building a program.

## Use

```toml
[dependencies]
polylint-tsrs = "0.1"
```

```rust
use std::sync::Arc;

use polylint_tsrs::{BuildRequest, MemoryFs, ProgramFiles, ProgramHost, SharedConfigs, SharedFiles};
use tsrs_core::arena::Region;

let (fs, boms) = MemoryFs::new()
    .with_file("/ws/tsconfig.json", r#"{ "compilerOptions": { "strict": true, "noEmit": true } }"#)
    .with_file("/ws/a.ts", "export const a: number = 'one';\n")
    .into_fs();
let (files, configs) = (Arc::<SharedFiles>::default(), Arc::<SharedConfigs>::default());
let host = ProgramHost::new(fs, boms, "/ws".to_owned(), Arc::clone(&files), configs);
let request = BuildRequest {
    config_path: "/ws/tsconfig.json",
    checkers: 1,
    single_threaded: true,
    list_files: false,
    diagnostics_in_checker_regions: true,
};

// tsrs allocates the config and the compiler host in the region current on the calling thread.
let program_files = Arc::new(ProgramFiles::default());
let scratch = Region::new_scratch(1 << 20);
let entered = scratch.enter();
let built = host
    .build(&request, Arc::clone(&program_files), None, |_config| {})
    .expect("the config parses");
// Copy what is needed out of the diagnostics: they point into the program.
let codes: Vec<i32> = built.owner.diagnostics(None).iter().map(|diagnostic| diagnostic.code()).collect();
assert_eq!(codes, [2322]);
// Drop order: the program, its loader regions, the scratch region, the program's files, then the run's.
drop(built);
drop(entered);
drop(scratch);
drop(program_files);
```

## tsrs version and features

The crate depends on the tsrs crates alone, held to one exact version (`=7.1.0-beta.tsrs2`), and tsrs types appear in
its API: a dependent pins the same version of the tsrs crates it names. It reads no environment variable and does no
I/O beyond the file system it is given.

The `plain-ptrs` feature, off by default, turns on `tsrs_core/plain-ptrs`: tsrs then uses plain pointers instead of
compressed ones, which reserve 32 GiB of address space for the process. The crate is correct either way; the arena
statistics (`arena_peak_bytes`) and the retiring of freed regions (`retire_freed_sources`) exist only with compressed
pointers.

## Origin

The crate was written as `skif-tsrs` in [skif](https://github.com/polylint/skif), polylint's JavaScript and TypeScript
dependency linter, and moved here at skif commit `50436ba0` when skif stopped using tsrs. Both projects are by the same
author and under the MIT licence.

## Licence

MIT. See `LICENSE`.
