# 0074 — Cache Keys Carry Every Input, Relative to the Checkout

- Status: Accepted
- Date: 2026-11-17
- Amends: ADR 0008, ADR 0039
- Related: ADR 0014, ADR 0019, ADR 0035, ADR 0051, ADR 0073

## Context

ADR 0008 makes the result cache's correctness rest on every key capturing every input that changes an output. An
audit of the keys against the engines found inputs that no key carried, so an edit served the previous result:

- ty and pyrefly resolve imports from the disk: an installed package in `site-packages`, and first-party modules the
  check was not handed (gitignored or excluded files, and every other module of a path-scoped run). The key held only
  the served files and the interpreter's paths.
- A component engine that computes a cache input itself (air digests `air.toml`) lost it: ADR 0039 has the host answer
  every cache-path hook from the engine's host hook, which knows nothing of an input the engine computes.
- `rustfmt` read the `--edition` poly passes and the `rustfmt.toml` nearest the file, but its key carried a constant
  edition marker and the config above poly's working directory only; a nested `rust-toolchain.toml` picks another
  `rustfmt` than the probed one. `swift-format` has the same nested-config gap.
- A plain catalog tool reads its own config (`.yamllint`), which nothing named.
- The `cargo` group's declared inputs left out `clippy.toml`, `.clippy.toml` and `.cargo/config.toml`, an untracked or
  out-of-tree copy of them, the toolchain and tool versions and the environment cargo reads; and `cargo deny` judges
  the lockfile against an advisory database it refreshes on every run.
- oxc's external mode depends on the project's `oxlint` / `oxfmt` binary and its `jsPlugins` modules.

The audit also found keys that held a checkout's absolute location (project-check roots and interpreter paths, oxc's
config paths and config-chain digest, ast-grep's rule directories and rule hash, the Go module root, JSON Schema files,
recipe config and dependency digests), so a second worktree, the hook snapshot or an absolute invocation re-ran work
the cache held; and a run narrowed with `--only` replaced each file's record with its own engines' segments only.

## Decision

- **A Python project check keys what it reads from disk.** For a plan that serves Python, the key folds in a
  fingerprint of each `site-packages` directory (every top-level name, the bytes of top-level files and of each
  `*.dist-info/RECORD`) and a digest of every `.py` / `.pyi` under the project root that was not served, ignored files
  included, virtual environments not walked. Keying was chosen over making the checkers read only the served files:
  that would turn every import of an unserved module (a gitignored generated module, any other module of
  `poly lint a.py`) into an error, and pyrefly resolves modules without a filesystem abstraction poly could restrict.
- **A component's own cache-path hooks are asked of it.** The host sends `RunContext`, `FileContext`, `LintInputSalt`
  and `FormatInputSalt` to a component exactly when its index `hooks` bits declare the engine overrides that method;
  every other engine is answered from its host hook as before, so a warm run still loads no component that did not
  declare one. A declared hook the component cannot answer keys a process-unique marker, never the trait default.
  An engine that overrides a format or lint salt also overrides `file_context`, which the stat fast path keys by.
- **Native tools run in the file's directory key per file.** `rustfmt`'s and `swift-format`'s `file_context` and
  format salt carry the fingerprint of their config files (and for `rustfmt` the `rust-toolchain[.toml]` files) from
  the file's directory upward, and `rustfmt`'s the edition poly passes, memoised per directory.
- **A plain catalog tool keys the config-like files** (dotfiles, `*rc`, and config-format extensions) of the file's
  directory and of the tool's working directory, and their ancestors up to the repository root, since the catalog
  names no config files for it.
- **The `cargo` group** declares the clippy and cargo config files as inputs, and each cached tool's key carries a salt
  of `rustc -vV`, `cargo -V` and the tool's own version (run once per repository per process), the `CARGO_*` /
  `RUSTFLAGS` / `CLIPPY_*` environment, and the content of every clippy and cargo config file in the tree (tracked or
  not), above it and in `$CARGO_HOME`. **`cargo deny` is never result-cached**: no local input tells a cached pass from
  an advisory published since.
- **oxc's external mode** keys the binary's npm `package.json` (else its size and modification time) and the
  `jsPlugins` modules of the governing config.
- **An engine's own `<NAME>_*` environment** is part of its project-check key (tsrs reads `TSRS_*`, ADR 0051).
- **Keys are checkout-relative.** A path in a key is relative to the checkout holding it (the nearest `.git`) or to the
  config, rule directory or project root it belongs to; a path outside keeps its full form. A project check stores
  its findings relative to the project root and replays them under the replaying root.
- **A narrowed run keeps the other engines' records.** A file's record keeps the segments of earlier plans for the
  same bytes, and is answered by engine key when another plan wrote it, so `--only` and a full run do not evict each
  other.

## Consequences

Positive:

- Every finding of the audit that serves a stale result is closed, and the next audit has a test to extend per input
  (`stat_index_warm.rs` for locations and narrowed runs, the per-input unit tests beside each salt).
- Worktrees, hook snapshots and absolute invocations share entries.

Negative / risks:

- A Python project check reads and hashes the project's unserved Python files and the `RECORD` files of its
  `site-packages` on every run, hit or miss. A file edited by hand inside an installed package is not seen.
- A component that declares a cache-path hook is loaded on a warm run (air, for R files).
- The `cargo` group pays a few `--version` spawns per run, and `cargo deny` runs on every commit.
- A plain catalog tool re-runs when any config-like file near it changes, related to it or not.
- `extra_paths` and `src` entries outside the project root are still read from disk without being keyed.

## Alternatives considered

- **Hermetic Python checkers** (answer only served files under the project root): rejected above.
- **Reporting each file a checker read and validating them on the next lookup** (a depfile): sound and precise, but it
  needs a protocol change for the components; deferred to the per-file project-check design.
- **An `AirConfig` host hook** instead of forwarding declared hooks: it keeps air's warm runs component-free, but every
  future component computing its own input would need one; the bitmask already says which engines compute their own.
- **Salting `cargo deny` with the advisory database's state**: the run refreshes the database after the key is taken,
  so the key would describe a database the run did not use.
