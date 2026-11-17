# 0078 — The tsrs Program Layer Is the Published Crate `polylint-tsrs`

- Status: Accepted
- Date: 2026-11-17
- Amends: ADR 0003, ADR 0010, ADR 0051
- Related: ADR 0040, ADR 0044, ADR 0074

## Context

ADR 0051 made `poly-engines-tsrs` the one crate that touches tsrs, and it adapted two files of skif's tsrs boundary
by copy: the in-memory file system and the guard that frees a program. skif (ADR 0044) later extracted its whole
tsrs-facing layer into a self-contained crate, `skif-tsrs`, so that both projects could share one implementation,
and then stopped using tsrs. The layer has no other home, and poly's engine is its remaining user.

ADR 0003 and ADR 0010 say poly publishes no crate to crates.io. The reason given there is mechanical: poly's
crates depend on pinned git dependencies (oxc, biome, rubyfmt), which crates.io refuses in a published crate, and a
binary distributed as release archives needs no source channel.

Embedding tsrs has constraints that are easy to get wrong and are the same for every host: a program lives in arenas
that only an unsafe call gives back, the config and the compiler host are allocated in the region current on the
calling thread, a file shared between programs must outlive all of them, and `bundled::default_lib_path` reads
`TSRS_LIB_PATH` from the environment.

## Decision

- **The layer is a crate of its own, `polylint-tsrs`**, at `components-tsrs/crates/polylint-tsrs`, imported from
  skif's `crates/skif-tsrs` at skif commit `50436ba0` (MIT, same author; `ATTRIBUTIONS.md`). `poly-engines-tsrs`
  builds its programs through it, so the tsrs workspace has two crates that depend on tsrs, not one.
- **What it owns.**
  - Program construction over a host file system: `wrap_fs` (the host's `tsrs_vfs::FS` with the bundled
    `lib.*.d.ts` files), `MemoryFs`, and `ProgramHost`, which reads the config, lets the host change the parsed
    command line, and builds and binds the program (`load_config` and `program_options`, or `build`). The library
    path is always passed explicitly, so `TSRS_LIB_PATH` is never read.
  - Shared parses: `SharedFiles` and `SharedConfigs` parse each file and config once for a run. The bundled lib files
    and the declaration files below `node_modules` stay for the run; a project's own files stay while a program that
    read them lives. Sharing is sound because a parse is a function of the text and the parse options that key the
    cache, the binder reads the file alone, and a checker writes only into its own stores or into caches set once in
    the region that owns the file, which is how tsrs shares declaration files between the projects of `tsc -b`.
  - The owner: `ProgramOwner` lends the program and its diagnostics (`tsc` order, sorted and deduplicated, for all
    files or the named ones) and frees the program when dropped. tsrs leaks each program from a box, and
    `free_unshared_program` is the only way to give its arenas back: it is the crate's single unsafe call, allowed
    at that call under `unsafe_code = "deny"`. A program dropped during a panic, or poisoned by one, is left to the
    process, since a second panic while freeing would abort the host.
  - The observer hook (`Observer`): what a program asks of the file system, its source files and its configs.
  - The cache seam (`CacheStore`, `CacheNamespace`, the `cache` envelope). The crate caches nothing itself.
  - Scheduling for hosts that build several programs: `SharedPrograms`, `Parallelism`, `Schedule`, `Cores`.
- **It is published to crates.io and versioned on its own**, starting at `0.1.0`. This is the one exception to the
  rule of ADR 0003 and ADR 0010, by the maintainer's decision: the layer is reusable by any host of tsrs, and it can
  be published because its only dependencies are the nine `tsrs_*` crates at an exact registry version
  (`=7.1.0-beta.tsrs2`), with no git dependency. Its version is not part of poly's lock-step release: it moves when
  its API or the tsrs pin moves. tsrs types appear in its API, so a release that changes the tsrs pin is a breaking
  one. Every other poly crate stays unpublished.
- **`plain-ptrs` is a feature of the crate, off by default**, forwarding to `tsrs_core/plain-ptrs`. A published
  crate cannot rely on a consuming workspace's feature unification, and reserving address space or not is the choice
  of the binary. The crate is correct either way and its tests run in both modes. poly's tsrs workspace turns the
  feature on, as ADR 0051 decided for `tsrs_core`.
- **The measured constants are defaults a host can replace.** They were tuned on skif's corpus of large TypeScript
  monorepos:
  - `CORES_PER_PROGRAM = 4` and `MOST_SHARED_CHECKERS = 8`, the defaults of `Limits`
    (`Parallelism::with_limits`, `Cores::shared_with`).
  - `RETIRE_BELOW = 8 GiB`, the bound of `retire_freed_sources`, an opt-in for a process that exits after one run;
    `retire_freed_sources_below` takes another bound.
  - `kept_for_run`: a bundled lib file or a `.d.ts` below `/node_modules/` is kept for the run. This one is fixed.

## Consequences

- Good, because the engine no longer carries a copy of the file system and the owner, and the soundness argument for
  freeing a program is written once.
- Good, because a path-scoped check (ADR 0074) and a later incremental one build on the shared parses, the observer
  and the cache seam instead of growing them inside the engine.
- Bad, because poly now has a crates.io release to make by hand or by trusted publishing, outside `publish.yaml`, and
  a public API to keep semver for.
- Bad, because the crate carries scheduling types poly's single-program check does not use.
- Neutral, because poly's check holds none of `node_modules`, so `kept_for_run` only ever keeps the bundled lib files
  there; the rule matters to a host that reads installed packages.

## Alternatives considered

- **Keep the copies in the engine.** Rejected: the layer would exist only as an unmaintained crate in a project that
  no longer uses it, and the engine would re-grow shared parses and observation for the incremental check.
- **Import it unpublished.** Rejected by the maintainer: the layer is meant to be shared, and nothing in it prevents
  publishing.
- **Inherit poly's version.** Rejected: a library's version states its API, and poly's lock-step number changes with
  every release of the binary.
- **Turn `plain-ptrs` on unconditionally.** Rejected: it would remove compressed pointers, and the arena statistics
  that exist only with them, from every other host.
