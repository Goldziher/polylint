# 0051 — TypeScript Type Checking with tsrs as an Opt-In Project Check

- Status: Accepted
- Date: 2026-11-14
- Amends: ADR 0042
- Related: ADR 0039, ADR 0040, ADR 0044

## Context

ADR 0042 made type checking a project check that runs in a downloadable component and is opt-in: ty and pyrefly
for Python. TypeScript has no equivalent in poly: oxlint's type-aware rules and skif (ADR 0044) judge syntax and
the dependency graph, and `tsc` is a Node tool poly does not run (ADR 0049).

[tsrs](https://github.com/oxc-project/tsrs) is a Rust port of the TypeScript 7 compiler, published on crates.io as
`tsrs_*` pre-releases (`7.1.0-beta.tsrs1`). It reads files through a virtual file system, which makes it a fit for
the hermetic project check. The same author's skif already wraps it behind one boundary crate for type-aware
member analysis (skif ADRs 0044 and 0045), which settled the constraints of embedding it: programs live in leaked
arenas, on unix it reserves 32 GiB of address space unless built with `plain-ptrs`, it reads `TSRS_*` and
`RAYON_NUM_THREADS` from the environment, it runs a global worker pool, and its `unicode-id-start = "=1.1.2"` pin
unifies with oxc's `"1"` in a shared lockfile. poly's check differs from skif's in purpose: skif extracts typed
facts, poly reports the checker's diagnostics.

## Decision

poly gets a `tsrs` engine: `tsc --noEmit -p <tsconfig>` as a project check in its own downloadable component.

- One boundary crate. `components-tsrs/crates/poly-engines-tsrs` is the only crate that depends on tsrs, and the
  alef generated `poly-engines-tsrs-component` its producer. It lives in a workspace of its own, like pyrefly and
  secrets (ADR 0040): tsrs pins `icu_normalizer` to `=2.3.0`, and `components/` reaches `sqruff-lib-core`, which
  needs the 2.4 ICU crates, through `poly-bundled`, so the two cannot resolve in one lockfile. A separate workspace
  keeps the root workspace free of tsrs as well. The `tsrs` component is download-only: no bundled build links it.
- Exact pins. Every tsrs crate is pinned with `=` to one version, `default-features = false`, and `tsrs_core` with
  `plain-ptrs`, so the check never reserves address space and cannot abort the host. `version()` embeds the tsrs
  version and a mapping revision; `tests/version_audit.rs` fails when the locked crate version is not in it.
  Upgrading tsrs is its own change. `unicode-id-start` is held at tsrs's pin for the whole lockfile.
- Opt-in and experimental. The engine runs only with `[engines.tsrs] enabled = true`. Enabling it on an existing
  project reports every type error the project has, and tsrs is a beta compiler whose diagnostics can differ from
  TypeScript's, so it is never on by default and the documentation says it is experimental.
- Hermetic. The host hands over every JavaScript, TypeScript and JSON file under `dir` (whole tree, `.gitignore`
  honored, never `node_modules`) and the configs and manifests by name; tsrs reads them through an in-memory file
  system and nothing from the disk. The result is a function of the request, which is what the cache key
  digests. File names are case-sensitive on every platform so the result does not depend on the machine.
- No `node_modules`. Imports of installed packages cannot resolve in a hermetic run. Findings that only say a
  package is not installed (`TS2307`, `TS2792`, `TS7016`, `TS2688`, and the "install type definitions" codes) are
  dropped when a `package.json` of the project declares the package or its `@types` package, and kept when it does
  not, because an undeclared import is a real error. `report_unresolved_modules = true` keeps them all. The check
  never installs, downloads or runs a package manager.
- What is reported. Errors and warnings in sources the host handed over, as `TS<code>` findings (error or warning),
  with the message chain below the head in the description. Declaration files, files under `node_modules` and
  JavaScript files unless the program holds them through `allowJs` are inputs and never reported. Suggestion and
  message categories do not occur in the diagnostic groups the check collects.
- Process constraints. A program is built on a dedicated thread with a large stack, in a scratch arena region, and
  freed by a guard whose `Drop` calls `free_unshared_program`: the one unsafe call of the crate, with its reason
  beside it, in a crate that otherwise denies unsafe code. A panic inside tsrs is caught and becomes an
  error of the run; the program is then left to the process rather than freed. poly never sets or reads `TSRS_*`
  variables, and variables a user sets still reach tsrs. The worker pool of tsrs is global to the process; the
  `threads` option, defaulting to the host's job budget, sets the checkers per program.
- One program per run: the `tsconfig` named by `project`, else the nearest `tsconfig.json` from `dir`. Project
  references contribute their sources. Diagnostics follow `tsc`'s order and stop at the first group that has any:
  config, syntax, then program, global and semantic.

## Consequences

- TypeScript projects can gate on the type checker in the same `poly lint` and pre-commit hook as everything else,
  with one component download.
- The component is large; users who do not enable the engine never download it.
- A monorepo with several `tsconfig.json` files needs one `[engines.tsrs]` table per `poly.toml` scope, or a
  solution config that tsrs follows through `references`; it does not check every config it finds.
- A project that declares a dependency it does not use gets no finding for an import of it that is missing types,
  and global names from an `@types` package that is declared but absent are not reported.
- tsrs's address-space and environment choices apply to the component process, a short-lived one per run. A
  long-lived host would also keep the arenas tsrs's worker threads hold; skif's ADR 0045 records the same limit.
- Licensing: tsrs is Apache-2.0 and derived from Microsoft's TypeScript; its NOTICE ships in `ATTRIBUTIONS.md`.

## Alternatives considered

- Run `tsc` or `tsgo` as a subprocess: needs Node or a native toolchain on the host, which ADR 0049 rules out for
  the default path, and the result would depend on an installed `node_modules`.
- Link tsrs into core: adds the compiler to every user's binary for an opt-in check; ADR 0039 puts such engines in
  components.
- Report only the diagnostics skif's boundary extracts: skif answers member usage, not type errors.
- Install the declared packages hermetically: network and package-manager access break the hermetic, cacheable
  check.
