# Changelog

All notable changes to this project are documented here. The format is based on
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project adheres
to [Semantic Versioning](https://semver.org/spec/v2.0.0.html). The single `poly`
binary drives lint, format, hooks, and commit checks from one `poly.toml`.

## [Unreleased]

### Fixed

- **tsrs type-checks a project whose tsconfig lists a declared `types` package.** `"types": ["node"]` with
  `@types/node` declared but not installed produced a `TS2688` program diagnostic; poly dropped it as a missing
  install, but tsc stops at the first diagnostic group that has any, so no type error in the project was reported.
  Such entries are now left out of the program, and the semantic check runs. `report_unresolved_modules = true` keeps
  them and reports the `TS2688`.

## [0.36.0] - 2026-11-18

### Breaking changes

- **`poly.lock` records the `ref` each Git hook source declares.** A lock written by this version does not parse in
  an older poly, which rejects the unknown key. [Changed]
Each item is described in full under the section named in brackets.
- **Repository trust.** Config keys that run programs (`[tools]` overrides, `[hooks]` whole-project jobs, `[cache] dir` and others) are ignored until `poly trust` records the repository, and `[tools]` `bin`, `root`, `url` and `env` are limited even then. Run `poly trust` once per repository, or set `POLY_TRUST=1` in CI. [Security]
- **Remote `extends` bases and hook sources are fetched over https and ssh only.** `POLY_GIT_ALLOW_PROTOCOLS` allows other transports. [Security]
- **`poly hooks install` refuses a shared `core.hooksPath`** without `--force` or `POLY_HOOKS_ALLOW_SHARED_PATH=1`. [Security]
- **Component cache and lock checks.** A component cache root owned by another user is refused, and a component lock without signing keys is rejected. [Security]
- **`poly mcp` confines paths to its workspace** and refuses to start in `/` or the home directory without `--allow-outside-workspace`. [Security]
- **`poly mcp` exposes four tools (`check`, `fix`, `info`, `manage`) instead of fourteen.** The old tool names are removed with no aliases, and `check` is not annotated read-only. [Changed]
- **Component protocol v3.** Components and sidecars built for protocol 2 are rejected. [Changed]
- **The repository and the Action moved to the `polylint` organization.** The old names redirect. [Changed]
- **`poly commit` exits `2`, not `1`, when it could not verify the message**, and reads the global `--config`. [Changed]
- **The most specific rule level wins a severity remap.** [Changed]
- **YAML that contains `{{` but is valid YAML is now linted and formatted.** [Changed]
- **C-family files under a `.clang-format` are skipped by the generic formatter**, which counts toward `--deny-skips`. [Changed]
- **A multi-ref `git push` runs the pre-push hooks over every pushed ref.** [Changed]
- **`poly config validate` rejects a `poly.toml` that mixes v1 tables with `[engines]`.** [Changed]
- **A project `skif.toml` with a `type-aware` key fails the `skif` check.** skif 0.9.0 removed type-aware analysis;
  delete the key. [Changed]
- **MCP failures are `isError` results, and unknown arguments are `usage` errors.** A missing path, a config that fails to load, an unknown `only` engine and a path outside the workspace answer `{error: {kind, message}, summary: null}` instead of a JSON-RPC error or a clean pass; an unknown or inapplicable argument is refused instead of ignored, and `check` no longer accepts `fix_generated`. [Fixed]
- **A failing or hung native or catalog tool fails its file** (exit `2`) instead of passing it as clean, and a tool is killed past its timeout. [Fixed]
- **A file over `[discovery] max_file_size` (default 32 MiB) is skipped**, which counts toward `--deny-skips`. [Added]
- **A recipe tool with an invalid `files` or `exclude` glob is refused.** [Fixed]

### Added

- **`polylint-tsrs`, a published crate.** The layer poly builds its TypeScript programs on is now a library on
  crates.io, versioned on its own (`0.1.0`): programs of the tsrs checker over a host file system, files and configs
  parsed once per run, an owner that frees a program, an observer of what a program reads, and a cache seam. It was
  imported from skif's `skif-tsrs` and depends on the `tsrs_*` crates alone, at `=7.1.0-beta.tsrs2`; the `plain-ptrs`
  feature is off by default. It and `polylint-js-types` are the crates poly publishes (ADRs 0078 and 0081).
- **`polylint-js-types`, a second published crate.** skif's type-aware analysis, which skif 0.9.0 drops, is a
  library on crates.io, versioned on its own (`0.1.0`): which members of a project's classes, enums and namespaces
  are read, from the tsrs checker over `polylint-tsrs`, with the type coverage of every package. It was imported from
  skif's `skif-js-types` at skif commit `75500e05` with its known false findings (`KNOWN-DEFECTS.md`); the
  `plain-ptrs` feature is off by default. poly's binary does not use it (ADR 0081). `0.1.1` describes the crate in
  its own terms instead of skif's ADRs and commands, marks the precision figure in `KNOWN-DEFECTS.md` as measured in
  skif and not reproduced here, and names its rayon pool threads `js-types-check-*`, apart from its program builders.
- **Prose and grammar checks with harper.** The opt-in `harper` engine (`[engines.harper] enabled = true`, component `prose`) reads Markdown and the comments of every language with a loaded grammar, and reports repeated words, wrong articles and other grammar and style findings as warnings. Rules go through `select`/`extend_select`/`ignore`; `SpellCheck` is off unless selected, and `dictionary`, `dialect` and `comments` are the engine's own keys (ADR 0068).
- **Lua formatting and linting.** `.lua` files format through StyLua and lint through selene, both in-process engines in
  the new `lua` component. `stylua.toml` / `.stylua.toml` and `selene.toml` are read from the nearest directory up to the
  repository root, with `[engines.stylua]` and `[engines.selene]` layered on top. selene runs its built-in `lua51`, `lua52`,
  `lua53` and `luau` standard libraries; a `std` that names a library file reports `unsupported-std`. (ADR 0069, #173)
- **R formatting with air and linting with jarl, no R installation.** The new `r` component runs Posit's `air`
  formatter and the `jarl` linter in process: `poly fmt` and `poly lint` on R files read the project's `air.toml` and
  `jarl.toml` and need neither R nor `Rscript`. `[engines.air]` (`line_width`, `indent_width`, `indent_style`,
  `line_ending`, `persistent_line_breaks`, `assignment_style`, `skip`, `table`, `default_table`) and `[engines.jarl]`
  (`select`, `extend_select`, `ignore`, `rules.<name>.level`, `check_roxygen`, `min_r_version`) override them; jarl's
  safe fixes apply with `--fix`. `styler` stays an opt-in host formatter, and the component is the default for R
  when it is installed (ADR 0070, #130).
- **`poly adr lock`, `poly adr unlock <file> --reason <text>` and `poly adr new <title>`.** Maintain `adrlint.lock`
  and create numbered records with the bundled adrlint and the `[engines.adrlint]` settings, so a repository no
  longer needs the standalone adrlint CLI or `.adrlint.toml`. `lock` refuses while `poly lint` reports an adrlint
  error, honouring `[per_file_ignores]` (a template sharing `0000` with the first record can be exempted from
  `MADR43` for that pair with `"docs/adr/0000-*.md" = ["MADR43"]`); `new` fills the directory's `template.md`/`NNNN-template.md`. A core build asks the `web`
  component through a new `Adr` component request. (ADR 0061, #221)
- **`--timings` for `poly lint` and `poly fmt`** (or `POLY_TIMINGS=1`). Prints per-phase wall time and a per-engine
  table (calls, wall, own CPU, child-process CPU, p95, slowest file) to stderr, and the slow-engine log now counts a
  subprocess engine's child CPU, so a busy `rustfmt` is no longer reported as "waiting, not computing".
- **Twelve more Go rules in the built-in ast-grep pack.** On by default at warning: `go-deprecated-import`
  (`io/ioutil`, `golang.org/x/net/context`), `go-dot-import` and `go-rand-seed`. Opt-in: `go-blank-import-outside-main`,
  `go-fmt-print-outside-main`, `go-time-sleep`, `go-panic-in-library`, `go-goroutine-without-recover`,
  `go-naked-return`, `go-unsafe-import`, `go-http-default-client` and `go-waitgroup-add-in-goroutine`. The pack is now
  42 rules. (#125)
- **Configurable Go architecture rules** in `[engines.astgrep]`: `go_forbidden_calls` (`pkg.Func` selectors with a name
  wildcard, `paths` scope and `allow_in` exemptions), `go_forbidden_imports` and `go_layers` (import layering resolved
  against the governing `go.mod`), reported as `go-forbidden-call`, `go-forbidden-import` and `go-layer-violation`.
  They replace ai-rulez's archlint test package; see the Go architecture rules section of the configuration reference.
  (#125)
- **Typst and LaTeX formatting.** `.typ` files format through `typstyle-core` and `.tex`, `.sty` and `.cls` files through
  `tex-fmt`, both in-process engines in the `misc` component. (#171, #172)
- **BREAKING: `[discovery] max_file_size`.** A file larger than this many bytes (default 32 MiB, `0` for no limit) is not read and
  is reported as skipped (`file too large`), which counts toward `--deny-skips`. (#207)
- **`[engines.shellcheck] install = "auto"`.** With no `shellcheck` on `PATH`, the built-in engine downloads the catalog's pinned, checksummed release and runs it. The default stays `"never"`, and `--offline` never downloads. (#110)
- **YAML schemas for `jsonschema`.** A schema file ending in `.yaml` or `.yml` is read as YAML, `$ref` is followed through YAML schemas for the cache key, and YAML merge keys (`<<`) are resolved before a document is validated.
- **Secret scanning on Linux.** The `secrets` component now ships for x86_64 and aarch64 Linux (glibc 2.28 and newer):
  the release compiles vectorscan from source in its manylinux_2_28 image instead of using the prebuilt archive, which
  needs glibc 2.32. (#112)
- **Third-party licenses ship with every binary.** Each release archive, npm platform package and PyPI wheel carries `THIRD-PARTY-LICENSES.md` and `ATTRIBUTIONS.md` next to the binary (under `polylint/licenses/` in a wheel).
- **Ctrl-C and SIGTERM clean up.** An interrupted run kills the tools it started, removes its scratch directories and temporary files, and exits `130`. (#207)
- **PMD and detekt recipes** for Java and Kotlin, on the host JRE with pinned, checksummed downloads
  (`install = "auto"`). PMD is installed as its whole distribution (a new `dist` release kind, extracted with the same
  path, link and size checks as a single archive member); detekt's SARIF report is written to a per-run file through a
  new `$OUTPUT` argument placeholder, and a new `$FILES_COMMA:<flag>` placeholder passes its inputs as one list. (#133)
- **kubeconform findings carry a line.** The offending resource is located in a multi-document manifest by `kind` and
  `metadata.name`, and a schema error's path is followed to its line. (#128)
- **Scala formatting without a JVM:** `scalafmt` from the upstream native builds as a pinned release recipe, opt-in
  under `[tools.scalafmt]`. (#178)
- **Swift linting:** `swiftlint` as a pinned release recipe, opt-in under `[tools.swiftlint]`, reading its SARIF report.
  (#177)
- **RuboCop-compatible Ruby linting:** `rbcop` as a pinned release recipe, opt-in under `[tools.rbcop]`, reading the
  RuboCop JSON report. (#135)
- **`.editorconfig` conformance:** `editorconfig-checker` as a pinned release recipe, opt-in under
  `[tools.editorconfig-checker]`. (#176)
- **Makefile linting:** `checkmake` as a pinned release recipe, opt-in under `[tools.checkmake]`. (#175)
- **`discovery` and `configs[].withheld` in the JSON document.** `poly lint` and `poly fmt` `--format json` now carry a
  `discovery` object when discovery left something out (excluded files and directories, `excluded_explicit` named paths
  that matched an exclusion, pruned directories, unrecognized files), which was previously stderr only. Each `configs[]`
  entry lists `withheld` when exec-capable keys were withheld from that config by trust (ADR 0059). (ADR 0076)
- **MCP `info` topic `validate`, and `manage` actions `cache_gc` and `components_prune`.** `validate` answers
  `poly config validate` as `{valid, files, partial}`; `cache_gc` takes `max_age_days` and `max_size` like `poly cache
  gc`; `components_prune` removes cached archives nothing references or used recently. (ADR 0076)

### Changed

- **`poly lock` follows a hook source tag that moved.** Every `poly lock` re-resolves each `[[hooks.sources]]`
  `revision` with `git ls-remote` and moves the pin when the tag or branch moved, the declared `revision` changed, or
  the entry predates `ref`; it prints `url ref -> sha` only for entries that changed. `--upgrade` still decides
  remote `extends` refs and `latest` tools. `--check` reports a moved tag or a changed `revision` as drift. Offline,
  recorded entries are kept with a note. `poly hooks install` never moves a pin but re-pins an entry whose declared
  `revision` changed, and `poly hooks run` refuses one locked for another `revision` instead of running the old
  commit (ADR 0082).
- **skif 0.9.2.** An explicit `entry` completes a package whose entries a plugin config computes, `stale-js-config`
  catches a plain pattern that cancels a `!` pattern on a common file but no longer calls a `!` pattern redundant
  when only a development entry also names its file, and the `skif` engine's cache key moves with it.
- **The tsrs type check builds its program through `polylint-tsrs`.** The engine's own copies of the in-memory file
  system, the config loading and the program owner are gone; findings are unchanged. The diagnostics the checkers
  make are now freed with the program instead of staying in the worker threads' arenas (ADR 0078).
- **BREAKING: `poly mcp` exposes four tools instead of fourteen.** `check` (lint or check formatting, writes nothing),
  `fix`, `info` (read-only: `version`, `config`, `validate`, `rules`, `components`, `cache`) and `manage` (cache and
  component maintenance) replace `lint`, `lint_fix`, `format_check`, `format_write`, `workspace_lint`,
  `workspace_lint_fix`, `cache_stats`, `cache_clean`, `rules`, `config_show`, `components`, `components_install`,
  `components_uninstall` and `version`, split along the annotation boundary so clients can still approve checks and
  prompt on writes. The old names are removed with no aliases: update client permission allowlists (`mcp__poly__lint`
  becomes `mcp__poly__check`, and so on) and saved prompts. `check` and `fix` take `kind` (`lint`|`format`) and `scope`
  (`files`|`workspace`|`all`), plus the CLI's run options (`only`, `skip`, `include_excluded`, `no_cache`, `jobs`,
  `max_skips`, `on_missing`, `debug`, and `fix_generated` on `fix`), built from the same request the CLI fills. Tools
  no longer declare an `outputSchema`; the schemas are MCP resources under
  `poly://schema/`. `info`'s `config` topic is now exactly `poly config show --format json`, and its `cache` topic
  matches `poly cache stats --format json`. (ADR 0076)
- **The core binary is built for speed.** `--profile core` moves from `opt-level = "s"` to `opt-level = 3` (fat LTO, stripped), and the size gate moves from 14 MiB to 22 MiB (ADR 0065). The `release` profile used by the bundled build and every component also moves to fat LTO with stripped symbols.
- **Cargo hooks hash their declared inputs once per run.** The clippy, sort, machete and deny hooks share one `git ls-files` listing and one parallel, streamed blake3 pass, and the staged-run mutation probe streams index blobs into hashes instead of holding the tracked tree in memory. (#215)
- **BREAKING: component protocol v3 (ADR 0058); every engine component and sidecar must be rebuilt.** Requests and responses are
  encoded positionally instead of with a field name per struct field, and a diagnostic no longer repeats the engine
  id of the request. Components built for protocol 2 are rejected as incompatible.
- **One oxc copy in the `web` component.** rumdl comes from `polylint-rumdl` 0.3.6, polylint's fork on Oxc 0.159, and
  adrlint 0.5.2 builds on it, so the component no longer links a second, older oxc through rumdl. (#192)
- **oxc 0.159.** The oxc git pin moves to `7053128` on oxc `main`: oxc 0.159.0, `oxc_formatter` 0.79 and `oxc_linter`
  1.94, so oxlint's rules and the JS/TS, JSON, CSS, Markdown, TOML and YAML formatters are the ones of that revision.
- **skif 0.9.0** (was 0.5.0), which changes the `skif` engine's findings:
  - skif 0.9.0 removes type-aware analysis, which poly never turned on, so findings are those of 0.8.0; a `type-aware`
    key in a project `skif.toml` (`[js]`, `[js.workspaces."<dir>"]` or an override's `js` table) is now a
    configuration error.
  - `unused-class-members`, a new rule on by default at warning, reports the methods, fields and accessors of a used
    exported class that no module reads. A read counts by property name. Decorated members and classes, implemented
    interface members, classes an entry file exports and members of a class whose base lies outside the project are
    not judged; the last covers a class extending one from a package, which poly's run never sees installed.
  - `unused-dev-dependencies` also reports a `devDependencies` entry the same manifest lists in `dependencies`.
  - A `.d.ts` file is an entry only when it applies without an import (a script, `declare global`, a module
    augmentation) or is published or listed; once imported, `unused-types` and `unused-exports` judge its exports.
  - An import that `tsconfig.json` `paths` or a bundler alias leads into the project's own sources no longer uses the
    dependency its specifier is named like, so `unused-dependencies` can report that dependency; `paths` targets in a
    workspace package's build output stand for its sources whether or not the output exists.
- **Engine crates refreshed.** mago 1.60.2, markup_fmt 0.28.0, ast-grep 0.51.3 and uncomment 3.12.1, with
  clap_complete 4.6.12, rand 0.10.4 and zip 9.6. Cached results from the engines whose crates moved (oxc, rumdl,
  markup_fmt, malva, taplo, yaml, skif, mago, ast-grep, uncomment) are recomputed once.
- **Markdown lists with mixed item spacing format as loose lists.** The rumdl update carries upstream's MD076 fix: when
  any items of a list are separated by a blank line, `poly fmt` separates all of them, so such lists reformat once.
- **The `file_safety`, `commit`, `lint` and `fmt` hooks run inside `poly hooks`.** `file_safety` no longer starts a `poly hooks check` process per commit: the runner calls the same checks over the staged snapshot and reports the same lines (ADR 0067). `commit`, `lint` and `fmt` run in process too, the default staged pre-commit run included, which is rooted at the staged snapshot (ADR 0073) and stays in process only when no engine it needs is a downloaded component; the output and exit code are the child's. Every in-process hook now runs under the hook timeout and is reported as killed past it. (ADR 0071, #215)
- **BREAKING: the repository moved to the `polylint` organization.** The repository is `polylint/polylint` and the Action is `polylint/polylint@v0`; install with `brew install polylint/tap/polylint` or `scoop bucket add polylint https://github.com/polylint/scoop-bucket`, and point a `#:schema` line at `https://polylint.github.io/polylint/` (`poly migrate` rewrites the old URLs). The old names redirect. Attestation checks (`POLY_VERIFY_ATTESTATION=1`, the Action's `verify-attestation`) accept both the new and the `Goldziher/polylint` identity, so releases up to v0.35.0 still verify. Homebrew and Scoop are rendered from `packaging/` templates by the organization's GitHub App, and `HOMEBREW_TOKEN` and `RELEASE_TAG_TOKEN` are retired. (ADR 0062, #228)
- **BREAKING: `poly commit` exits `2` when it could not verify the message.** A config or I/O failure used to exit `1`, the code for a rejected message. `poly commit` also honours the global `--config`, and reads the `[commit]` table of a `poly.toml` named that way instead of parsing it as a gitfluff file.
- **BREAKING: the most specific rule level wins a severity remap.** With both `rules.F` and `rules.F401` set, the family level used to override the exact code; the longest matching rule now applies, and rules of equal length keep config order.
- **BREAKING: YAML that contains `{{` but is valid YAML is linted and formatted.** Prometheus rules, Taskfiles and Grafana provisioning files were skipped as templates. A file is still skipped when a `{{` sits where a template action would change the document, as in `image: {{ .Values.image }}`. A formatter failure on a document that parses is an error instead of an unchanged file.
- **BREAKING: files a `.clang-format` governs are left to clang-format.** The generic formatter skips C, C++, Objective-C, Java, JavaScript and Protobuf files below a `.clang-format` or `_clang-format` it would otherwise reindent, with the reason `formatted by clang-format (.clang-format present)`, which counts toward `--deny-skips` and `--max-skips`. **Migration:** set `[tools.clang-format] enabled = true`.
- **BREAKING: a multi-ref push is gated on every ref.** `git push origin a b`, `--all` and `--tags` ran the pre-push hooks over the first ref only; they now run over the changed files of every pushed range.
- **BREAKING: `poly config validate` rejects a `poly.toml` that mixes v1 tables with `[engines]`.** It used to accept the file as v1 although the loader ignores the v1 tables; the error points at `poly migrate --to-v2`.
- **Accepted advisories.** `deny.toml` ignores the chrono, chrono-tz and zip advisories published on 2026-11-17: chrono has no fixed release, and the zip fixes need 9.x while harper-core requires 8.x. The only zip reader in the tree is harper's rule-bundle loader, which poly never calls.
- **Internal: the `Engine` trait is split into facets.** Batching, project analysis, planner policy and catalog takeover moved from `Engine` into the `Batching`, `ProjectAnalysis`, `PlanPolicy` and `CatalogTakeover` traits, which a backend implements only when it has the concern. No behaviour, wire-protocol or cache-key change. (ADR 0072)

### Performance

- **skif keeps its module parses in poly's cache** (ADR 0080). An edit anywhere in a JavaScript workspace still re-runs
  the `skif` project check, but skif now parses only the modules that changed: the rest come from the result cache's new
  `engine` namespace, which `poly cache stats`, `clean` and `gc` cover and `[cache] dir` and `POLY_CACHE_HOME` move.
  `--no-cache` and `[cache] enabled = false` give skif no store. A hit refreshes a day-old entry's mtime so the sweep
  evicts unused parses rather than those in steady use; `-j` and `threads` changes reuse the entries, and the
  open-time presence scan skips the namespace. In the core build the host passes the cache to the
  `web` component as two project-check environment pairs (`poly.cache.dir`, `poly.cache.salt`); protocol v3 is
  unchanged, and a component or host that predates them ignores or omits them.
- **Cache keys no longer hold a checkout's location, and a narrowed lint run evicts nothing** (ADR 0077). A second worktree,
  the hook snapshot and an absolute invocation reuse the records of the first (project-check roots, oxc config paths,
  ast-grep rule directories, the Go module root, JSON Schema and recipe digests are keyed checkout-relative), and
  `poly lint --only <engine>` keeps the other engines' records, so the next full run re-runs none of them.
- **A repeated narrowed lint run is answered from the stat index again.** A second `poly lint --only <engine>` re-read
  and re-hashed every file the selection had left without an engine, since only the full run's record was kept. A
  record now also answers for each run scope that found it right for the same bytes, so both the narrowed and the full
  run are stat hits. The stat index format changes (`PSI5`); an older index is discarded and rebuilt on the next run.
- **The tsrs type check checks only the named files on a path-scoped run.** The pre-commit hook names the staged files;
  tsrs now builds the whole program but computes diagnostics for those files only, instead of type-checking the whole
  project and discarding the rest. A change that breaks an unstaged importer is caught by `poly lint .` and CI, not by
  the hook. A run naming most of the program keeps the parallel check of every file. The host passes the reported
  files to a project check that declares `ProjectDirs::narrows_to_targets` (`ProjectRequest::targets`, a trailing
  `ProjectCheck` field on the component wire) and keys its cache on them; other tree checks run once for every run
  (ADR 0074).
- **Fewer component round trips per file.** An engine's decline of a file now arrives in its `Format` reply, so a
  component no longer receives every file twice (a `SkipReason` request, then `Format`), and a component is not loaded
  just to ask. `BatchSupport` is asked once per engine and config instead of once per file. The `uncomment`, `ini`,
  `python-types` and `pyrefly` engines no longer ask any per-file hook. (#190)
- **Shell and Go formatting no longer queue on one process.** The `sh` and `go` sidecars speak alef 0.130's stdio
  protocol 2: payloads travel as raw bytes and concurrent calls are pipelined over the one sidecar, up to one per CPU.
  The per-file limit for a sidecar engine rises from 3 MiB to 12 MiB. (ADR 0063, #189)
- **uncomment shares the lint pass's parse.** With uncomment 3.12, the `uncomment` engine takes the tree `quality` or
  `ast-grep` already parsed for the file instead of parsing it again, where the engines are linked together. (#187)
- **`quality` and ast-grep share one parse.** A lint pass parses each file once for both engines instead of once per engine, and a `--fix` re-lint does the same. (#187)
- **Per-file work is scheduled largest first.** Each file is its own unit of work, taken in descending size order, so a run of large neighbouring files (generated bindings, vendored sources) no longer lands on one worker while the others idle. Output is unchanged.
- **mimalloc on musl.** The bundled `x86_64-unknown-linux-musl` build uses mimalloc, since musl's allocator takes one global lock and serialised the worker threads. Other targets keep the system allocator. (#179)
- **Project checks run in parallel.** Engines that check a whole project load their inputs and run side by side, and `ty` checks a project's files on several threads. Findings keep their order.
- **The whole-project phase overlaps the per-file tier.** A `poly lint` without `--fix` runs the whole-project tools beside the per-file tier, and missing components are installed up to four at a time.
- **Less work per file across the engines.** ruff, oxc, rumdl, ast-grep, typos, mago, sqruff and jsonschema build their per-config state once instead of once per file, discovery shares one `git ls-files` listing per root, and ruff's first-party module walk skips virtualenvs and gitignored directories.

### Fixed

- **The release builds the Windows components again.** The component SBOM step read `jq` output with the CR the
  Windows runner adds, so the publish run failed on `x86_64-pc-windows-msvc` before any channel was updated.
- **Parallel runs no longer exhaust memory on `.properties` files.** The tree-sitter-properties scanner keeps its state
  in a process-global variable, so two workers parsing such files at once corrupted each other and a parse grew without
  bound (over 30 GB on openapi-generator). Parses with that grammar are now serialized.
- **A block comment in an indented `<style>` or `<script>` no longer keeps the HTML formatter from settling.** The
  embedded CSS or JavaScript was formatted with the document's indentation still on it, and the comment's inner lines
  gained a level on every pass. The body is now dedented before it is formatted.
- **A type check that found nothing to check is a skip, not a clean pass.** `tsrs` on a project with no
  `tsconfig.json`, and `skif` on one with no `package.json` or JavaScript workspace, used to report no findings, so the
  run read as a passing check. The project root is now in `skipped` with the reason (`tsrs: no tsconfig.json`),
  counted by `summary.skipped` and charged to `--deny-skips` / `--max-skips`. Component protocol v3 gains the
  `ProjectDeclined` answer, which a host built before it cannot decode: a `tsrs` or `skif` component from this release
  that declines a project, run by an older poly host, fails the check (exit `2`) instead of skipping it. (ADR 0079)
- **`poly lint` and `poly fmt` honour `[cache] dir`.** The per-file runner always used the per-user default cache (or
  `POLY_CACHE_HOME`), so a configured directory was measured and cleaned by `poly cache` but never written by a lint or
  fmt run. Runs and hooks now open the cache through one resolver: `[cache] dir` when set, with its stat index kept
  per checkout, else the default location. A relative `dir` is resolved against the `poly.toml` that declares it, not
  the working directory, so a run from a subdirectory, `poly cache` and the MCP server use the same cache.
- **`[cache]` is root-only.** A nested `poly.toml` that sets it is ignored with a warning naming the file; before, a
  run started in that directory took its cache settings while one from the root did not.
- **A cache directory poly did not create is never wiped.** A format-version bump cleaned `[cache] dir` and rewrote
  its `VERSION` file even when that file was someone else's; such a directory is now refused with an error.
- **Concurrent `--only` runs keep each other's cached verdicts.** The second to finish dropped the first one's.
- **`poly lint` and `poly fmt` honour `[cache] enabled = false`.** Only `--no-cache` turned the result cache off;
  the config key was read by `poly cache` alone. A run with it set now stores and replays nothing.
- **A missing native tool shows in the report.** When `rustfmt`, `gofmt`, `shfmt` or `shellcheck` (or an opt-in
  native tool that is switched on) is not on `PATH`, the summary prints one line per tool (`rustfmt not found on PATH:
  3 Rust files formatted by the generic tier`) and the `json`/`toon` document and MCP results carry
  `degraded: [{tool, language, reason, files}]`, omitted when empty. `poly fmt` used to print `All formatted.` with
  nothing to say rustfmt never ran. The files still count as checked, not skipped, and exit codes do not change.
  (ADR 0075, #230)
- **Result-cache keys carry inputs they missed** (ADR 0077). An edit to any of these now re-runs the work instead of
  replaying the previous result: a package installed into `site-packages` or an unserved first-party module, for the
  `python-types` checks; `air.toml`, in a component build; the crate edition and a nested `rustfmt.toml` /
  `rust-toolchain.toml`, for `rustfmt` (and a nested `.swift-format`); a plain catalog tool's own config file;
  `clippy.toml`, `.clippy.toml`, `.cargo/config.toml`, the toolchain and tool versions and the `RUSTFLAGS` / `CARGO_*`
  environment, for the `cargo` group; the external `oxlint` / `oxfmt` binary and `jsPlugins`; `TSRS_*` variables.
  `cargo deny` is no longer result-cached, since a cached pass cannot see a newly published advisory.
- **The tsrs type check reported nothing when poly ran from outside the project root.** The host hands a component
  paths relative to the project root, and the tsrs component resolved them against its working directory, so
  `cd src && poly lint .`, `cd src && poly lint a.ts` and `poly lint /abs/project` from elsewhere found no
  `tsconfig.json` and printed "No issues found" over real type errors. The paths are now resolved against the root.
  Shipped in 0.35.0.
- **An invalid glob is reported instead of silently dropped.** A malformed pattern in ruff's `exclude`, typos' `extend_exclude`, an `.oxfmtrc` `overrides[].files`/`excludeFiles` entry, a jsonschema `schemas` glob or a recipe's dependency globs now logs a warning naming it, and the valid patterns beside it stay in force. (#217)
- **Hook sources recover from a damaged cached mirror.** A mirror holding a locked commit without its tree is refetched, then re-cloned, instead of failing with `unable to read tree`; the final error names the cache directory to remove.
- **golangci-lint no longer fails beside another golangci-lint, or times out a cold module and reruns it.** poly
  passes `--allow-parallel-runners`, so a run no longer exits 3 (`parallel golangci-lint is running`) on every Go file
  while an editor or another repository holds golangci-lint's per-user lock, and `--timeout=0` only when the project's golangci-lint config (or the file `-c` names) sets no `run.timeout`, so a cold-cache
  run is not killed and a project's own timeout is kept; `POLY_TOOL_TIMEOUT_SECS` bounds the run instead, and a recipe declares such arguments with the new `config_defaults` key (ADR 0066). A whole-run failure (a
  recipe's new `unsplittable_exits`, a signal, poly's deadline, a missing program) fails the invocation's files once
  instead of being retried on halves of its packages or once per package by the per-file loop, and findings printed
  before such an exit are not taken as a verdict. A `serial` tool now runs one invocation at a time across the whole process, not only within one batch.
- **A Markdown file with an empty footnote definition is formatted.** `[^1]:` with no content made
  `oxc_formatter_markdown` panic, so `poly fmt` failed the file (exit 2), also when the definition sat in a
  ```` ```markdown ```` fence. The pinned oxc revision carries the upstream fix (oxc-project/oxc#29735): the
  definition is printed as written and the rest of the file is formatted.
- **Dockerfiles, `.env` and INI files format the same in every build.** The bundled build left them with no formatter,
  while the core build normalised them through the generic tier; both now use the generic tier.
- **Log lines no longer land on the progress spinner.** `tracing` output and still-running notices erase the spinner frame
  first, so `installed component` and similar lines start on their own line. The whole-project phase no longer draws a
  second spinner over its per-tool bars, and a failed whole-project tool with no output (a launch error, a timeout, a
  silent non-zero exit) now prints why it failed.
- **Language-agnostic catalog tools now run when enabled.** A catalog tool that names no language (codespell, typos, proselint, dprint, editorconfig-checker and similar) matched no file, so `[tools.<name>] enabled = true` did nothing. It now applies to every discovered file.
- **A project check no longer depends on how a path is spelled.** `poly lint docs/adr`, `poly lint docs/adr/0003-x.md`
  and a relative path named from a subdirectory skipped the adrlint tree-wide and lock rules (`MADR41`-`MADR44`,
  `ADR50`, `ADR51`) that `poly lint ./docs/adr` ran, and `poly lint README.md docs/adr` with an `adrlint.lock` reported
  every locked record as `ADR51` missing from disk. The config governing a file is now chosen by the directory's
  identity, not by comparing the paths as written, so a project check gets the same project root for every spelling
  and argument order. A project finding under a named directory is reported under the run's own spelling of the file.
- **A lint verdict that depends on other files is re-checked when they change.** The per-file lint cache key now
  folds in an engine's `lint_cache_salt`, so jarl re-lints an R file after an edit to the package's `DESCRIPTION`,
  `jarl.toml` or a file it sources instead of replaying the verdict recorded before. The salt is asked only of an
  engine that declares it, and only in `poly lint`. (#229)
- **BREAKING: a failing native or catalog tool is reported, and a hung one is stopped.** A non-zero exit from `rustfmt`, `gofmt`, `shfmt` or `shellcheck` (a bad `rustfmt.toml`, a shellcheck exit 2) used to pass as an unchanged or clean file; it now fails the file with the tool's first stderr line. A tool is killed after 60 seconds per file, 10 seconds for a version probe and 30 minutes per batch (`POLY_TOOL_TIMEOUT_SECS`, `0` for no limit), and its output is capped.
- **`poly fmt --fix` writes through a symlink and refuses a lost update.** A symlinked file is rewritten at its target instead of being replaced by a regular file, and a file that changed on disk after poly read it is reported instead of overwritten. `poly migrate` keeps a `poly.toml`'s mode and symlink too.
- **Format results are cached per language and file name.** Identical bytes in `x.json` and `package.json`, or in a `.json` and a `.js` file, shared one cache entry although oxc formats them differently.
- **Columns count characters, not bytes.** `quality`, `uncomment`, `typos` and `dockerfile` findings on a line with a non-ASCII character before them reported a shifted column, and a multi-line Dockerfile instruction now spans all of its lines. Columns are 1-based Unicode scalar values. (#180, #183, #201)
- **oxc reads `single_quote`, and JSON `trailing_commas` takes the JavaScript names.** `single_quote = true` was declared for JS/TS but ignored; it now selects single quotes unless `quote_style` is set. JSON accepts `all`, `es5` and `none` beside `always` and `never`; `none` used to mean `always`.
- **Formatters decline what they cannot format stably.** `markup_fmt` formats `<script type="module">` bodies in HTML, which never converged, and declines a file whose multi-line template literals it would re-indent on every pass.
- **shfmt-formatted Bash stays as shfmt wrote it.** The generic shell reindenter flattened a loop that follows assignment lines and dedented a pipeline's continuation line.
- **Deeply nested input fails one file, not the run.** sqruff, mago, ruff, graphql and the Dockerfile parser refuse pathological nesting before parsing, and the `quality` rules and the generic formatter's case handling walk the tree without recursion, so a generated file can no longer overflow the worker stack and abort poly.
- **A glibc-only tool is not downloaded on a musl host.** A recipe whose Linux assets are all `-gnu` builds (zizmor) installed cleanly on Alpine and then failed to start; it is now used from `PATH` or reported missing with the command that installs it. (#114)
- **BREAKING: a recipe tool with an invalid `files` or `exclude` glob is refused** with a message naming the pattern, instead of running over the files the glob was meant to skip. (#217)
- **`mix format` works on a project with dependencies.** The scratch copy links `deps/`, so `import_deps` resolves; files outside the configured root no longer take its `.formatter.exs`, and a project whose dependencies are not fetched is skipped with a reason naming `mix deps.get`.
- **Recipe reports are read more completely.** SARIF artifact URIs are percent-decoded and resolved through `uriBaseId`, Windows drive paths parse, a finding that names no file is counted in one warning instead of dropped, and a tool killed by a signal is an error. A run fails when the project's config cannot be copied for the tool.
- **`proguard-rules.pro` and `consumer-rules.pro` are ProGuard, not Prolog.** They were reindented as Prolog; no engine touches them now.
- **typos corrections follow the word's case.** An `extend_words` correction is capitalised like the word it replaces, as dictionary corrections are, and an entry whose value differs from its key reports that value as the correction.
- **Inline suppressions after code are placed correctly.** A trailing `poly: allow[...]` after code that starts with a comment-opener character (`*p`, `--i`, `#include`) covers its own line, stacked directives all cover the next code line, and `# noqa  # poly: allow[X] reason` on its own line covers the next line.
- **`[per_file_ignores]` and nested configs follow the file, not the path named.** `poly lint docs/adr` matched `docs/adr/*.md` against `x.md`, and a relative path named from a subdirectory stopped looking for a governing `poly.toml` at the working directory.
- **Nested `poly.toml` files get their own PHP rules.** mago's rule set was built from the first file's options and reused for every config; one invalid mago format option no longer drops the valid ones beside it.
- **SQL keeps sqruff's indent width** unless `indent_width` is set; every SQL file whose width differed from poly's default was reindented.
- **An invalid `indent_style`, `line_ending` or `line_width` logs a warning** instead of silently falling back to the default.
- **`[engines.adrlint.rules.MADR31]` enables MADR31.** The rule is off by default, and only `select` or `extend_select` could switch it on.
- **tsrs checks more projects.** A `tsconfig` that extends a package the project declares is no longer stopped at `TS6053`, `types="vite/client"` resolves to its package, a project's own `.d.ts` files report their errors, and declared packages are judged per `package.json` ancestry (ADR 0055).
- **Staged cargo hooks honour the repository's target directory.** `CARGO_TARGET_DIR`, `CARGO_BUILD_TARGET_DIR` and `build.target-dir` were overridden with `<root>/target`, so worktrees could not share a build directory. (#223)
- **Hook commands survive odd inputs.** A tracked file whose name starts with `-` is passed as `./-x.py` instead of being read as a flag, a large environment no longer makes a batched hook fail with `E2BIG`, and inherited `GIT_WORK_TREE`, `GIT_OBJECT_DIRECTORY` and similar variables no longer leak into poly's own git calls.
- **`poly migrate` keeps a source whose settings were not all applied.** A key that conflicts with the existing `poly.toml` leaves the source file in place and is named in the report, fragments merged into a v2 `poly.toml` are converted to `[engines.*]` first, `--write` refuses outside a git work tree, and the `#:schema` line `--to-v2` writes names a URL that is served.
- **`poly cache <subcommand> --cache-dir <dir>` is accepted**, and a large `--max-age` no longer overflows.
- **`poly doctor` no longer warns that a mise shim shadows poly.**
- **The installers find the installed version.** `install.sh` and `install.ps1` read the wrong token of `poly --version`, so an up-to-date install was never recognised. Linux musl aarch64 and Windows arm64, which have no release binary, are rejected with a message, and a download error names the URL.
- **MCP server fixes, carried into the four new tools (ADR 0076).** These fix behaviour of the removed tools;
  each now holds for `check`, `fix`, `info` and `manage`:
  - **BREAKING: a missing path no longer reads as a clean pass over MCP.** A named path that does not exist now answers
    `isError: true` with `{error: {kind: "paths", message}, summary: null}`. A config or `poly.lock` that fails to load
    (`config`), an unknown `only` engine (`pipeline`) and a path outside the server's directory (`workspace`) answer the
    same way instead of a JSON-RPC error.
  - **The MCP `rules` topic claimed to be read-only and closed-world but could download.** `info` never downloads: a core
    build without the `astgrep` component reports it as missing. A failing `*-test.yml` rule test now sets `isError`.
  - **A repository with a remote `extends` base no longer fails every MCP tool.** The server resolves the base from the
    local source cache at the `poly.lock`-pinned revision and never fetches; a base that is not cached is a `config`
    error telling you to run `poly lock` or a CLI `poly lint` once.
  - **A long-lived `poly mcp` kept linting with the project configs of its first call.** The per-directory lookups a
    run memoises (`.oxlintrc.json` and the oxlint services built from it, `.oxfmtrc.json`, `go.mod`, `.clang-format`,
    `Cargo.toml` editions) were never forgotten between calls, so an edit was invisible to the next one, and its stale
    findings were then cached under the new config's key. Every run now starts by forgetting them. On a core build the
    engines live in components that hold their own copy, so the host announces each new run to the components it has
    loaded (a `RunBoundary` request) before the run's first lint or format request.
  - **`poly lock` and `poly trust` take effect on a running `poly mcp`.** The server read `poly.lock` and the trust
    store once, at startup: a moved `extends` pin kept resolving the old revision, a `poly.lock` that did not parse
    ended the process before the handshake, and `poly trust` changed nothing until a restart. Both are now read as they
    are on disk at each call, and a malformed `poly.lock` is that call's `config` error.
  - **MCP `check`/`fix` with `scope: workspace` reported a pass when the whole-project phase never ran.** An untrusted
    repository, or one with no whole-project tool, answered `passed: true` with no tools. It is now an `isError` result
    of kind `untrusted` or `no_workspace_tools`, in the same `{error: {kind, message}, summary: null}` shape as every
    other failure; `scope: all` sets `isError` when the repository is untrusted.
  - **BREAKING: MCP tools ignored arguments they did not understand.** `check {path: [...]}` (a typo for `paths`) linted the whole
    directory, and `manage {action: "cache_clean", dry_run: true}` cleaned the cache. An unknown key, an invalid value
    and a parameter that does not apply to the chosen `scope`, `action` or `topics` are now `usage` errors naming the
    key, on all four tools; `check` no longer accepts the fix-only `fix_generated`.
  - **The read-only MCP `info` tool could fetch and write.** Its `components` topic resolved a remote `extends` base with
    the fetching resolver, and its `cache` topic created the cache directory, wherever `[cache] dir` pointed. Every
    host service now resolves config from the source cache only, and reading the cache footprint creates nothing.
- **A replaced `poly mcp` executable answered with a JSON-RPC error.** Every tool but `info`'s `version` topic now
  returns an `isError` result of kind `stale` naming the restart, and `info {topics: ["version", …]}` answers
  `version` beside the refused topics instead of failing outright. (ADR 0076)
- **The MCP `cache` topic and `cache_clean` / `cache_gc` ignored the config.** They read `[cache] dir` from the
  discovered `poly.toml` whatever `config` or the server's `--config` named, so a run's cache was measured or
  cleaned in the wrong place. They now take `config`, falling back to the server's `--config`.
- **MCP `fix {scope: all}` could write fixes and then fail.** The whole-project phase is now planned, and the
  repository's `poly.lock` pinned, before the per-file tier rewrites anything; a `poly.lock` that does not load is a
  `config` error.
- **`info` topic failures were always `pipeline`.** A refused path or config is now `workspace` and a config that does
  not load is `config`, the kinds `check` reports for the same cause. `info {topics: []}` is a `usage` error rather
  than an empty answer.

### Security

- **BREAKING: remote `extends` bases and hook sources are fetched over https and ssh only.** A config could point git at `git://`, `http://`, `file://` or an `ext::` helper. Other schemes are now rejected before git runs; plain local paths still work. **Migration:** `POLY_GIT_ALLOW_PROTOCOLS` (a `:`-separated list) replaces the allowed set, and a leading `+` adds to it, as in `+http`.
- **BREAKING: `poly hooks install` refuses a shared `core.hooksPath`.** With a global or shared hooks directory, the shims were written where every repository runs them and the stale-shim prune could delete another tool's. **Migration:** unset `core.hooksPath` for the repository, or pass `--force` or set `POLY_HOOKS_ALLOW_SHARED_PATH=1`; poly never prunes outside the repository.
- **BREAKING: the component cache must be private, and a component lock must name its keys.** A component cache root owned by another user is refused; one of your own that is group- or world-writable is tightened to `0700` (and refused if that fails). A lock that lists components without signing keys, or an entry whose key is not among them, is rejected instead of loaded unverified.
- **BREAKING: `poly mcp` confines tool calls to its workspace.** `paths`, `dirs` and `config` arguments are resolved (symlinks and `..` included) and refused outside the directory `poly mcp` started in, and the server refuses to start in `/` or the home directory. The `--config` given on the server's own command line is always accepted. `check`, which replaces `lint` and `format_check`, is not annotated read-only, since it runs the repository's configured tools. **Migration:** start the server in the project directory, or pass `--allow-outside-workspace`.
- **Pinned tool downloads are bound to the catalog.** A URL in `poly.lock` must be https on the origin and release path the tool's catalog entry names, redirects are followed only to GitHub's release hosts or the same origin, and the GitHub token is sent to `api.github.com` only. The install stamp records the binary's size and mtime, and a tool cache that is not private to the current user is tightened or refused.
- **Staged symlinks are resolved before a hook reads them.** A chain of in-tree links that escapes the staged snapshot once resolved is removed from it.
- **The installers and the Action validate the requested version** as `X.Y.Z[-pre]` against the whole string before using it.
- **BREAKING: a repository's config keys that run programs need `poly trust`.** A cloned `poly.toml` could make
  `poly lint`, `poly fmt` and the MCP `check` and `fix` tools run any program with no prompt. Now
  `[tools.<name>] bin` (a path), `url`/`sha256`, `env`, `args`, `extra_args`, `format_extra_args`,
  `lint_extra_args`, `new_from_rev` and `root`; `[hooks]` whole-project jobs and their `skip`/`only` guards;
  `[cache] dir` and `[cache.sccache] dir`/`bin`; oxc `js_plugins = "external"` and `external = true`; and local
  `extends` bases outside the repository are ignored, with one note naming them, until the repository is trusted.
  `poly trust` records a digest of those values in a per-user store (`--show`, `--revoke`, `--list`); changing any
  of them untrusts the repository again. `poly hooks install` records trust; `poly hooks run` and the git hooks refuse
  an untrusted config. `poly mcp` honours only the store. **Migration:** run `poly trust` once in each repository that
  relies on these keys (re-run `poly hooks install` or `poly trust` after pulling a change to them), or set
  `POLY_TRUST=1` (or pass `--trust`) in CI. (ADR 0059)
- **BREAKING: always-on limits on `[tools]`.** Even in a trusted repository, `bin` must be a program name or an
  absolute path, `root` must stay inside the repository, `url` must be https on a host the catalog names for the tool
  (or the `POLY_GITHUB_API_URL` mirror you set), and `env` may not set `LD_*`, `DYLD_*`, `GIT_CONFIG*`,
  `GIT_SSH_COMMAND`, `NODE_OPTIONS`, `JAVA_TOOL_OPTIONS` and similar code-injection variables. **Migration:** install
  a tool from another host yourself and name it with an absolute `bin`.
- **Signed, attested releases with SBOMs.** Component signing moved out of the build jobs into a job on a fresh runner
  in the `release` environment, which checks the build's payloads against a digest manifest before signing; build
  jobs hold no secret and a read-only token, and only the upload job can write to the release. Every release file
  carries a build provenance attestation, each archive ships a CycloneDX SBOM (`<archive>.cdx.json`, also inside the
  npm and PyPI packages), and the binaries embed their dependency list (`cargo auditable`). The tools that run beside
  the signing key are pinned by sha256, and a reused component lock is checked against its release before reuse
  (ADR 0060).
- **Installers verify more.** `install.sh` allows only HTTPS with TLS 1.2 or newer, including redirects, and
  `install.ps1` requires TLS 1.2 or newer. `POLY_VERIFY_ATTESTATION=1` (and the Action's `verify-attestation: true`)
  also verifies the archive's attestation with `gh` and fails closed. The Action caches the release archive and
  re-checks it against `sha256sums.txt` on every run instead of trusting a cached binary.

## [0.35.0] - 2026-11-15

### Added

- **Cross-file Markdown link checks.** `[engines.rumdl] cross_file_links = true` reports a `file.md#anchor` that no
  longer exists in the target file, as a project check over the Markdown tree. `cross_file_links_dir` names the tree.
  (#127)
- **14 more hadolint rules in the `dockerfile` engine** (DL3002, DL3003, DL3004, DL3008, DL3013, DL3014, DL3015,
  DL3016, DL3018, DL3019, DL3020, DL3027, DL3042, DL4006), and `[engines.dockerfile.rules.<id>] level` now overrides a
  rule's severity. (#126)
- **Four opt-in Go architecture rules** in the built-in ast-grep pack: `go-no-init`, `go-no-package-mutable-global`,
  `go-context-background-outside-main` and `go-forbidden-ambient-call`. (#125)
- **Go formatting without a Go toolchain.** A new `go` sidecar component formats with gofumpt and goimports grouping
  (unused imports are not removed), reads the language version and module path from `go.mod`, and is configured under
  `[engines.gofmt]` (ADR 0050). (#108)
- **TypeScript type checking (experimental).** `[engines.tsrs] enabled = true` type-checks a TypeScript project in
  memory with tsrs, the Rust port of the TypeScript 7 compiler, and reports `TS<code>` findings through `poly lint`
  and the builtin pre-commit hook. It ships as a downloadable component in its own workspace (ADR 0051).
- **Secret scanning.** `[engines.secrets] enabled = true` scans files with kingfisher's rule catalog, as a downloadable
  component for Apple silicon macOS and Windows x64 (Linux builds follow once the release container builds vectorscan
  from source). Findings never echo the secret, and `poly: allow[...]` and `[per_file_ignores]` allowlist them
  (ADR 0054). (#112)
- **JSON Schema validation.** `[engines.jsonschema] enabled = true` validates JSON, JSONC, YAML and TOML against a
  local schema, offline, with line and column spans (ADR 0052). (#111)
- **R formatting and linting through pinned release binaries:** `air` formats and `jarl` lints R without an R install
  (opt-in under `[tools.air]` and `[tools.jarl]`); the in-process component for R is not done yet. (#130)
- **Catalog tools with pinned, checksummed downloads:** shellcheck, clang-format, zizmor, kubeconform, google-java-format
  and ktfmt; ktlint as a host-JRE recipe; Elixir `mix format` and `credo` as host-runtime recipes. ADR 0049 records the
  policy: GPL tools run as separate processes and no runtime is bundled. (#110, #124, #114, #128, #133, #134, #109)

### Fixed

- **Vue and Svelte `<script>` blocks are formatted** by `markup_fmt`; template expressions stay as written. (#166)
- **The typos engine no longer flags the `mis-` prefix.** (#167)

### Changed

- **Dependencies refreshed.** Every crates.io dependency moved to its latest release; the oxc git pin moved to
  `crates_v0.158.0`, biome to 2.6.7 and rubyfmt to upstream HEAD. skif 0.5.0 adds the Lerna, `stray-lockfile` and
  Bazel-aware rules to the `skif` engine's findings, gitfluff 0.11.2, ast-grep 0.51.2, tree-sitter-language-pack 1.25
  (the built-in Zig definition query follows its renamed `function_declaration` node). Cached results from the engines
  whose crates moved are recomputed once.
- **npm packages move to the `@polylint` scope.** `@goldziher/polylint` and its five platform packages are now
  `@polylint/polylint` and `@polylint/polylint-<platform>`, and `@goldziher/opencode-poly` is
  `@polylint/opencode-poly` (ADR 0048). Reinstall under the new names; the old packages stop receiving releases.
- **Result cache keys are relative to the checkout** for the ruff context and native-tool configs, so a new git worktree
  reuses cached results. (#165)
- **Quality-metric coverage is no longer lint coverage.** A file whose only coverage is the quality tier (C and
  C++) is skipped as `no lint rules for <language> (quality metrics only)`, so it is not counted as `checked` and
  counts against `--deny-skips` and `--max-skips` (ADR 0053). (#168)
- **Rust 1.100** is the minimum supported version, which skif 0.5.0 requires.

## [0.34.2] - 2026-11-09

### Changed

- **`poly hooks install` locks what it needs.** When `poly.lock` is missing an entry the config declares (a Git hook
  source, a remote `extends` base, an `install = "auto"` tool), install resolves and records it instead of failing with
  "run `poly lock` first", keeping every existing pin and printing one line per entry added. `--locked` (or
  `POLY_LOCKED=1`) keeps the old failure and never writes the lock. Hook runs stay strict. (#158)
- **gitfluff 0.11.1.** Header-shape errors and plain AI banner detection now come from gitfluff itself, replacing
  poly's own hint and exclude rule.
- **Worktrees share the result cache.** The result cache is keyed by the repository's git common dir instead of
  the checkout path, so every `git worktree` of a repository shares one cache and a new worktree's first run reuses
  results for byte-identical files instead of re-running every engine. The common dir is read from `.git` /
  `commondir` without spawning git; a directory outside git keys by its own path as before. The hook staged snapshot
  stays per worktree, and so does the stat index behind the warm-run fast path: a new worktree's index is seeded from
  another's, so runs alternating between worktrees never evict each other. The result cache an older poly kept per
  checkout is removed on first use. Cache format 6: existing caches start cold once. (#159)
- **Dependencies.** alef 0.129.0 for the component build.
- **Damaged components name the failed check.** A cached component that fails verification is reported with the
  check it failed (digest, size, manifest or signature) instead of a generic verification failure.

### Fixed

- **`cargo binstall` resolves the release archives.** The binstall `pkg-url` doubled the archive suffix's dot
  (`poly-0.34.1-<target>..tar.gz`), so every target failed; a publish job now checks each shipped triple.
- **Hook `lint`/`fmt` builtins no longer replay a stale pass.** The hook result cache keyed the builtin `lint` and `fmt`
  jobs on their matched files only, so after a `poly.toml` or tool-config change — or in another worktree sharing the
  cache — a pass recorded under the old configuration was replayed. They are no longer hook-cached; `poly lint` and
  `poly fmt` keep their own per-file cache, which keys every configuration input. (#159)
- **Sidecar start-up.** Starting a just-installed sidecar component no longer fails intermittently with `Text file busy`
  on Linux; alef retries the spawn. (#138)
- **The tree-sitter fallback leaves rustfmt/gofmt output alone.** With `rustfmt`/`gofmt` absent, the generic tier no
  longer re-indents nested `switch` cases, statement labels or the comments introducing a `case` in Go, the bodies of
  Rust macro invocations, or files indented with tabs or two spaces (when `indent_width` is not set). Reindented lines
  now honor `trim_trailing_whitespace = false`. (#142)
- **`poly lock` on a cold cache.** Pinning a Git hook source resolves its ref with `git ls-remote` instead of cloning the
  whole repository, and a hook source or remote `extends` base is fetched as only the pinned commit, at depth 1. A cold
  `poly lock --upgrade` for one hook source drops from about 40 s to 2–3 s and downloads nothing; the first hook run
  that materializes it drops from about 50 s to 8 s, with a 80 MB cache instead of 560 MB. (#157)

## [0.34.1] - 2026-11-09

### Added

- **Tool versions from upstream.** A recipe tool with `install = "auto"` (golangci-lint, hadolint, tflint, …) now
  resolves its download from the tool's own releases instead of a version hardcoded in poly. `version` defaults to the
  latest upstream release and accepts any published version; `poly lock` records the version and per-platform digests
  in `poly.lock` (`--upgrade`, `--check`). Lookups retry transient failures (honoring `Retry-After`), time out, cache
  `latest` for 24 hours with ETag revalidation, lock per tool across processes and work offline from the cache. See
  [ADR 0047](adrs/0047-tool-release-sources.md). (#137)

### Changed

- **polylint.** The GitHub repository is now `Goldziher/polylint` and the Action is `Goldziher/polylint@v0`; the old
  names redirect. The command stays `poly`, the config stays `poly.toml` and the agent plugin id stays `poly`.
- **The release moves the major tag.** Each release force-updates `v0` (and `v1` once releases are 1.x) to the new tag.

### Removed

- **Intel macOS.** The `x86_64-apple-darwin` archive, `@goldziher/polylint-darwin-x64`, the `macosx_10_12_x86_64` wheel, the
  Homebrew Intel block and its component builds are gone; releases carry five platform archives. Intel Mac users build
  from source. See [ADR 0046](adrs/0046-drop-intel-macos.md).

### Fixed

- **Grammar download failures.** A failed tree-sitter grammar download no longer makes the built-in ast-grep pack
  panic as a "release defect" on every file: the failure is retried after 30 seconds, the pack loads every grammar it
  has, and a file whose pack rules could not run reports the real reason. (#122)
- **Mixed-path runs.** `poly lint README.md packages/a/x.ts`, and so a pre-commit run over mixed staged paths, plans
  whole-tree engines such as skif from the outermost config, as `poly lint .` does. (#118)
- **The musl archive runs on stock Alpine.** It no longer needs `libgcc_s.so.1`, and the release smoke-tests it on a
  bare `alpine:3.20`.
- **Components.** `poly components install` and `sync` re-verify a cached copy and repair a damaged one; `list` and
  `status` show it as `corrupt` and show versions. `[components]` names, and engines configured under a disabled
  component, are validated when the config loads. A component skipped by `on_missing = "skip"` warns. On a bundled
  build, `components list` shows the MCP server as bundled and says the `sh` row runs the host `shfmt`, and selecting
  pyrefly says how to get it. `ini` files are linted alike in core and bundled builds. `poly cache` takes `--format`.
- **Config and CLI.** `required_version` is enforced when a config loads. `poly lint` reports unknown engine and
  `[defaults]` keys. A file named on the command line is counted once, `--only` counts a file its engine ran on as
  checked, and the JSON `configs` list is de-duplicated. A closed stdout (`poly config schema | head`) exits quietly,
  an empty file stays empty, and file-checking hook builtins no longer run at the commit-msg stage. `poly commit`
  explains a malformed header and rejects a plain AI banner.
- **Tree-sitter fallback.** With rustfmt or gofmt missing, the fallback no longer rewrites canonical code: on 200 real
  files per language, changed lines dropped from 5319 to 12 for Rust and from 430 to 4 for Go. It now reindents Java.
- **Engines.** MADR31 (the `*` list marker) is opt-in, so `poly fmt` and adrlint agree. HTML, Vue and Svelte
  `<style>` blocks and HTML `<script>` blocks are formatted. GraphQL documents mixing executable and schema
  definitions parse. Diagnostics are ordered deterministically, mago names the real file, and `file-too-long` no
  longer applies to prose and data formats.

### Documentation

- **polylint.** The README, docs site, package READMEs, agent skills, `poly --help` and the MCP server instructions
  name the product polylint (the command stays `poly`) and describe engine components, host tools and recipes
  instead of a "zero-dependency, pure-Rust" binary. New pages: Why polylint, Adopting polylint, Toolchains,
  runtimes and recipes, and a glossary. "Migrating to poly 2" is now "Migrating to the v2 config schema".

## [0.34.0] - 2026-11-08

### Added

- **pyrefly type checking.** `[engines.python-types] backend = "pyrefly"` selects pyrefly instead of ty, from its own
  opt-in `pyrefly` component. The two engines declare which `backend` value picks them (`Engine::selection`), so the
  planner runs one and fetches only its component. pyrefly reads no `pyrefly.toml`, starts no process (the interpreter
  query is forced off and the host passes the `site-packages` paths) and is built without its bundled third-party
  stubs. The component lives in its own cargo workspace, `components-pyrefly/`, because pyrefly's `tikv-jemallocator 0.7`
  cannot share a lockfile with the jemalloc 0.6 that rumdl links; see
  [ADR 0042](adrs/0042-pyrefly-in-its-own-workspace.md). `--only pyrefly` (or `--only python-types`) with nothing
  selected to run is a skip counted by `--deny-skips`, not a clean pass.
- **skif project check.** The `web` component's new `skif` engine lints JS/TS workspaces (npm, pnpm, yarn, bun) for
  dependency, manifest, version-policy and dead-code findings. Opt in with `[engines.skif] enabled = true`; a project
  `skif.toml` is honored. skif runs on the files the host hands over, so the project-check cache is invalidated by
  any member `package.json`, source (Vue, Svelte and Astro included), `tsconfig`, `.gitignore` or lock file changing, and
  it sees the whole tree whatever `[discovery] exclude` says. `unlisted-binaries`, `unused-dev-dependencies` and
  `unused-dependencies` are off until asked for, since they need installed packages. `poly lint --fix` now applies the
  edits a project check attaches to its findings (skif's safe fixes), then re-runs the check once and reports that. Presets `skif` and `js-deps` select the `web` component. oxc stays
  one copy; the `[patch.crates-io]` table in `components/` redirects skif's oxc crates to poly's. See
  [ADR 0044](adrs/0044-skif-project-check.md).
- **adrlint tree-wide rules.** The `adrlint` engine now runs `MADR41a`, `MADR41b`, `MADR42`, `MADR43`,
  `MADR44` and the immutability lock rules `ADR50`/`ADR51` as a project check over the ADR log
  (`dir`, default `docs/adr`, or `paths`; `adrlint.lock` is read by the host). The result is cached
  on every ADR, the lock and a listing of the other files in the log (`MADR42`). Engines declare their
  project-check inputs (`Engine::project_inputs`) instead of the core keeping a table by engine name. Naming a single file reports on it without changing the verdict about the log. Bumps adrlint to 0.5.0; see
  [ADR 0041](adrs/0041-adrlint-tree-wide-rules-as-project-check.md).
- **`sh` component: shfmt as a sidecar.** Shell is formatted by `mvdan.cc/sh/v3`, shipped as a signed Go
  executable that poly spawns once and talks to over alef's framed stdio, so output no longer depends on a
  `shfmt` on `PATH` (byte-identical to shfmt 3.14.1). alef builds, signs and locks it with the other components
  (`producer_language = "executable"`); `[components] disable = ["sh"]` brings the host `shfmt` back, and the
  bundled build keeps it. See [ADR 0045](adrs/0045-sidecar-components.md). Files over 3 MiB are skipped (alef
  carries bytes as JSON number arrays in 16 MiB frames). A crash or timeout of the sidecar stops shell formatting
  for the rest of the run, because alef does not restart a stdio sidecar; an oversize reply and a panic are
  failed calls that leave it running.
- **`poly commit --no-ai-cleanup`.** Disables the built-in removal of AI attribution trailers and banners for one run.
- **`poly.toml` `[commit]` keys.** `[commit.rules] ai_cleanup`, and `enforce_length` and `enforce_case` under
  `[commit.rules.message]`, matching the keys `.gitfluff.toml` already had. The last two keep the Conventional
  Commits length limits and subject case checks when a custom `message` pattern replaces the structural check.

### Changed

- **The released core is smaller.** It is built with a new `core` Cargo profile (fat LTO, `opt-level = "s"`,
  stripped) instead of the thin-LTO release profile, keeping cold-run CPU within about 5% of before, and
  `mise run check:core-size` gates it at 14 MiB in CI and in the release (11.1 MB on `aarch64-apple-darwin`).
  `uncomment` and `ini` are no longer in the core: `ini` joins the `misc` component and `uncomment` is its own
  opt-in component (ADR 0043). The component protocol links `schemars` only behind a feature; the core still
  links it through `poly-config` and `poly-host-services`.
- **`[fmt.shell.shfmt] batch` is gone.** The `sh` component is one long-lived process, so there is no start-up cost
  to amortize. A leftover `batch` key is reported as an `unknown-config-key` warning and otherwise ignored;
  delete it. `[fmt.shell.shfmt] enabled = false` still opts out, but with the component it leaves shell
  unformatted rather than falling back to the tree-sitter reindenter.
- **Dependencies.** ruff 0.17.4 (`ruff_*` 0.0.21), ty 0.0.21, biome v2.6.5 and rubyfmt v0.14.2-38;
  alef 0.128.0 for the component build.
- **`python-types` (ty).** ty renamed the diagnostic `unresolved-import` to `import-not-found`;
  update any `poly: allow[unresolved-import]` suppression or `[lint.python.python-types]` rule
  selection that names the old code.
- **`poly commit`.** Consumes the upstream `gitfluff` 0.11.0 library from crates.io instead of the in-tree fork
  (`crates/gitfluff` is removed); the `[commit]` config loading and `poly.toml` `extends` resolution live in
  `poly-cli`'s `commit/` module.
- **`poly commit`: skipping merge and revert messages.** The merge-in-progress skip applies only when the message
  being linted is the repository's own `COMMIT_EDITMSG`, and now also triggers on `REVERT_HEAD` and
  `CHERRY_PICK_HEAD`, not just `MERGE_HEAD`. Previously any message linted during a merge was skipped.
- **`poly commit`: generated subjects pass.** A subject starting with one of the prefixes `Revert "`, `fixup!`,
  `squash!` or `amend!` is no longer linted.
- **`poly commit`: AI-attribution cleanup is line-anchored.** The built-in cleanup removes whole attribution lines
  (the `Generated with` banners, `Co-Authored-By` trailers naming an AI assistant with an optional bare `<email>`
  line, and standalone Claude bullets) instead of matching across unrelated text.
- **Breaking: `poly commit --exclude` separator.** `--exclude REGEX[->MESSAGE]` splits the custom message off with
  `->`, not `:`. A `:` in the argument now belongs to the regex, so `--exclude 'WIP:Not allowed'` is read as one
  regex; write `--exclude 'WIP->Not allowed'`. `--cleanup FIND->REPLACE` already used `->` and is unchanged.
- **Breaking: `.gitfluff.toml` and `.fluff.toml` reject unknown keys.** A misspelled key such as `no_emoji` is now a
  config error instead of being silently ignored.
- **`poly commit`: `write = true` with `--message`.** A literal message has nowhere to persist a rewrite, so a `write`
  that comes from the config is ignored with a warning; an explicit `--write` still prints the cleaned message.
- **`poly commit`: AI-attribution cleanup covers more assistants.** The built-in cleanup also strips
  `Co-Authored-By` trailers naming ChatGPT, GPT or OpenAI, not only Claude and Anthropic.
- **`poly commit`: colour honors `NO_COLOR`.** With `--color auto`, diagnostics are plain when `NO_COLOR` is set to a
  non-empty value.

### Fixed

- **Project checks served by a component.** The project phase planned only compiled-in engines, so a
  project check hosted by a downloadable component (ty from the `python` component in a core build) never ran.
  It now runs the components' project-check engines too.
- **Markdown code spans.** The Markdown formatter no longer strips a space on only one side of a code span's
  content, which changed the span's meaning (#100).
- **Release workflow.** A draft release left with partial assets by a failed run is rebuilt, and CR is stripped from
  `jq` output read on Windows runners.

## [0.33.1] - 2026-11-05

### Fixed

- **Release.** v0.33.0 was never published: its x86_64-apple-darwin component build failed because
  `mise.lock` had no macos-x64 entries. 0.33.1 ships the 0.33.0 changes below with that fixed.

## [0.33.0] - 2026-11-05

### Breaking changes

See [Migrating to poly 2](https://goldziher.github.io/poly/guides/migrating-to-v2/) and
[ADR 0038](adrs/0038-poly-toml-v2-and-cli-surface.md).

- **CLI flags.** `--no-color` is replaced by `--color auto|always|never`. The `--workspace` /
  `--no-workspace` pair is replaced by `poly lint --workspace auto|always|never`.
  `--force-exclude`, `--include-generated` and `--skip-generated` are replaced by
  `--set KEY=VALUE` (`discovery.force_exclude`, `discovery.generated`). `poly fmt --check` is
  removed because the dry run is the default; it is still parsed, hidden, and fails with a hint.
  `poly fmt --exit-zero-on-fix` is replaced by `--exit-nonzero-on-fix`: `poly fmt --fix` exits `0`
  after rewriting files unless that flag is given.
- **Lock files.** `poly-config.lock` and `poly-hooks.lock` are replaced by a single `poly.lock`,
  written by `poly lock [--upgrade] [--check]`; `poly config update` and `poly hooks update` are
  removed. `poly migrate --to-v2` does not merge the old files: delete them and run `poly lock`.
- **typos is opt-in.** Spell-checking runs only with `[engines.typos] enabled = true`.
  `poly migrate --to-v2` writes that key for a config that relied on the old default.
- **poly.toml v2.** `[engines.<id>]` replaces `[lint.<lang>.<tool>]` and `[fmt.<lang>.<tool>]`;
  `[engines.astgrep]` (`rule_dirs`, `builtin_pack`) replaces `[rules]`; top-level `root` replaces
  `[workspace] root`; `[workspace] run = "auto|always|never"` replaces `[lint] workspace`;
  `per_file_ignores` replaces `per-file-ignores`. The loader reads both schemas and rejects a file
  that mixes them; `poly migrate --to-v2 --write` rewrites a v1 file and keeps its comments.
- **`scripts/` is gone; every repository script is a mise task.** Run `mise run <task>` (`mise tasks`
  lists them, `mise run <task> --help` documents one) instead of `scripts/*.sh`, `scripts/hooks/*.sh`
  and `scripts/*.py`: `components:{dev,generate,parity,release}`, `check:{deps,core-no-oxc}`,
  `release:{bump,homebrew-formula,scoop-manifest}`, `hooks:{rust-max-lines,rustdoc-lint,components-check}`,
  `plugin:{sync-version,sync-skills}`, `verify:{plugin-distribution,plugin-release-results,registry-payload}`,
  `harden`, `measure:core-size`, `crates-io:watch` and `catalog:gen-md`. `mise run set-version` is now
  `mise run release:bump`. The harden corpus data moved to `crates/poly-cli/tests/harden/data/`. No
  compatibility wrappers remain.
- **A needed engine component that is not installed fails the run** with exit `2` (or skips its
  files under `--on-missing skip` / `[components] on_missing = "skip"`); poly never falls back to
  the tree-sitter tier for it.

### Added

- **Downloadable engine components.** Engines ship as signed components, fetched on first use by a
  core build (`cargo build --no-default-features --features core`). The default build
  stays bundled, with every engine and the MCP server linked in. `poly components list|status|install|uninstall|sync|prune|import`
  manages them; `[components]` (`enable`, `disable`, `install`, `on_missing`) selects them;
  `--offline` / `POLY_OFFLINE=1` forbids every download; `POLY_COMPONENT_MIRROR` and
  `POLY_COMPONENT_CACHE` set the download origin and cache root. `poly components prune` keeps
  archives the lock references for any target and archives used within `--keep-days` (default 30),
  and skips a file a loaded library still holds.
- **Core release binaries.** Only `x86_64-unknown-linux-musl` ships the bundled binary (every
  engine linked in, built from the `components/` crate `poly-bundled`, no embedded lock). The other
  five release targets ship a core binary (`--no-default-features --features core`, linking only
  the quality, uncomment and ini engines) with a signed component lock embedded. `python`, `php`,
  `nix`, `typos`, `astgrep` and `misc` are now downloadable components, alongside `web`, `sql`,
  `biome`, `ruby` and `mcp`. `poly rules` and the MCP `rules` tool work in a core binary by
  asking the `astgrep` component (protocol version 2, `Request::Rules`).
- **`poly config validate`**, which checks a config against the compiled component index without
  downloading anything, and a `components` section in `poly doctor`.
- **`required_version`** in `poly.toml`, a semver requirement on the running poly.
- **`--set KEY=VALUE`**, a run-scoped TOML override layered above `poly.local.toml`, and
  `--on-missing fail|skip`.
- **Python type checking with ty**: `[engines.python-types] backend = "ty"`, opt-in.
- **MCP.** Fourteen tools: `components`, `components_install` and `components_uninstall` join the
  set, and `lint`, `format_check`, `lint_fix` and `format_write` take an `install` parameter
  (`config`, `auto`, `never`). A core build serves MCP through the downloadable `mcp` component.
- **GitHub Action.** The `components` input pre-installs components and caches them;
  `version: from-config` reads `required_version`, and `config-file` names the file it reads.

### Changed

- Minimum supported versions raised: Node.js 22.
- Python 3.11 is now the minimum supported version; Python 3.10 reached end of life.
- oxc moves to the `crates_v0.157.0` release-tag commit (`f799f17`), so the git-pinned linter and
  formatters match the oxc crates published as 0.157.0. Cached JS/TS, CSS, Markdown, TOML, YAML and
  HTML results are recomputed once.

### Fixed

- On Windows, a hook source channel `check` or `install` command containing double quotes reaches `cmd.exe` intact instead of with CRT-escaped `\"`, so resolving the hook source no longer reports "has no eligible execution path" for a check such as `python3 -c "..."`.
- `--only` / `--skip` accept engines served by a component, so `poly lint --only sqruff` works on a core build instead of failing with an unknown engine name.
- Git hooks installed by `poly hooks install` re-exec under `mise exec` when mise provides poly in the repository, so a commit from an IDE or GUI without mise activation runs the mise-pinned poly and tools instead of failing with "poly binary is not on your PATH" or running another poly. The repository's mise `[env]` applies to hooks too, and hooks never auto-install tools. When the repository pins poly but mise cannot provide it (an untrusted config, poly not installed), the hook says so in one line and uses poly from PATH; set `POLY_HOOK_MISE=1` to skip mise for a git command. Re-run `poly hooks install` to update existing hooks.
- The generic shell formatter (used when `shfmt` is not installed) keeps the indent of a `\`-continued line that starts with a redirect instead of dedenting it to the command's level.

## [0.32.3] - 2026-11-01

### Security

- Publish SDK-free OpenCode and Hermes plugin packages that the existing-version skips left stale in 0.32.2.

### Fixed

- Verify immutable registry payloads before accepting skips, preserving matching retries and older-tag recovery (#47).

### Changed

- Update uncomment to 3.10.4 and ZIP to 9.2.1.

## [0.32.2] - 2026-11-01

### Added

- **`gau` catalog recipe for GitHub Actions pins.** `[tools.gau] enabled = true` runs
  `gau --lint-pins --format json` (gh-actions-updater 0.4.0 or later) over workflow files and
  `action.yml` manifests and reports every `uses:` ref that is not a bare `@vN` major tag on its
  line, with `sha`, `full_tag` or `branch_or_other` as the rule. Off by default.

### Changed

- **The GitHub Action's `cache: true` caches through `actions/cache@v6`.** That major runs on the
  Node.js 24 runtime, so a self-hosted runner needs Actions Runner 2.327.1 or newer; GitHub-hosted
  runners already qualify. The action's inputs and outputs are unchanged.

### Fixed

- Remove the unused OpenCode SDK dependency from agent plugin packages.
- **Hook commands keep their quoting on Windows.** `cmd /C` received each joined command as a
  CRT-escaped argument, so inner quotes reached the child as literal `\"` and checks such as
  `python3 -c "..."` failed with a syntax error. Poly now passes the command line verbatim, and
  literal `%` in arguments is transported within CMD's limits.

## [0.32.1] - 2026-10-31

### Fixed

- **A sparse staged snapshot no longer hides unchanged directories from tools that walk the tree**
  ([Goldziher/poly#45]). Since 0.30.0 the default `snapshot = "auto"` represented every directory it
  did not write as a symlink back into the worktree, recorded in `.poly-links`. Tools that walk the
  snapshot without following directory symlinks saw none of the files under those directories, so
  `ai-rulez validate` (the `ai-rulez-validate` hook source) reported every rule glob over an
  unchanged directory as `AR101 glob-no-match` and blocked any commit touching `.ai-rulez/`, while
  `--all-files` and a normal checkout were clean. Those directories are now mirrored as real
  directories whose tracked files are hardlinked to the worktree — no byte copy, worktree mtimes
  preserved, with a copy fallback when hardlinking is unavailable — so a non-following walk reaches
  every tracked path; a mirrored path is still never a staged file, and a hook that writes through
  one is still caught by the link guard. The snapshot root also carries a `.git` file pointing at
  the real git dir, since a library-based repository reader does not see the exported `GIT_DIR` and
  otherwise falls back to a walk that skips hidden directories such as `.agents/`. `.poly-links`,
  sparse-checkout handling, submodule and `snapshot_include` links, cleanup and idempotency are
  unchanged.

[Goldziher/poly#45]: https://github.com/Goldziher/poly/issues/45

## [0.32.0] - 2026-10-31

### Added

- **The `adrlint` backend checks MADR architecture decision records** ([ADR 0037](adrs/0037-adrlint-madr-linting.md)).
  Numbered ADR files (`NNNN-*.md`) can now be checked against the MADR template — section presence,
  order and heading level (`MADR01`–`MADR08`), front-matter YAML (`MADR21`), list markers
  (`MADR31`), document-local link anchors (`MADR12b`/`MADR12d`) —
  with `[lint.markdown.adrlint] enabled = true`. It is off by default, because an existing log may
  follow another template, and it is in-process, content-hash cached, configured through poly's
  uniform `select` / `extend_select` / `ignore` vocabulary, and `poly lint --fix` applies the
  structural heading fix. Directory numbering and file names (`MADR41`–`MADR44`), the immutability
  lock (`ADR50`/`ADR51`), link-target existence (`MADR12a`/`MADR12c`) and external links
  (`MADR11`) deliberately stay in the `adrlint` CLI: they need the whole tree, the lock manifest or
  the network, which a per-file cache cannot model honestly.
- The backend reference and configuration guide document the new backend and its rule boundary.

### Changed

- **Dependencies upgraded.** Registry requirements moved to `globset` 0.4.21, `hcl-edit` 0.10.0,
  `json5` 1.3.6, `sqruff-lib`/`sqruff-lib-core` 0.44, `unicode-width` 0.2.3 and `zip` 9.1, with the
  lockfile refreshed across the tree. `rumdl` resolves to 0.3.2 (raised by `adrlint`), so the
  rumdl backend's cache identity now names 0.3.2.

## [0.31.2] - 2026-10-28

### Fixed

- **A hook in a sparse checkout sees tracked files outside the cone again** ([Goldziher/poly#44]). Since
  0.30.0 the default `snapshot = "auto"` links each directory it does not write to the working tree,
  and a directory a sparse checkout omits is not there to link, so a hook reading
  `omitted/b.txt` failed with `No such file or directory` where 0.29.0 passed. Every `skip-worktree`
  entry is now written into the snapshot from the index, so it holds the staged bytes and the live
  checkout is not expanded. This also covers `git update-index --skip-worktree` in a full checkout,
  where a link exposed worktree bytes git no longer compares with the index. The first snapshot after
  the fix writes the omitted entries once, as `snapshot = "full"` and 0.29.0 did, and later runs only
  re-check them; a checkout without skip-worktree entries is unaffected.
- **A `snapshot_include` entry inside a linked directory no longer deletes the worktree file.** In a
  sparse snapshot, `<snapshot>/other/gen.txt` under a linked `other/` is the working tree's own file,
  and linking the entry there removed it and left a symlink pointing at itself; withdrawing an entry
  could likewise unlink a symlink of yours. Such an entry is already reachable through the directory
  link and is now left alone.

[Goldziher/poly#44]: https://github.com/Goldziher/poly/issues/44

## [0.31.1] - 2026-10-28

### Fixed

- **actionlint reports an unknown runner label as a warning again** ([Goldziher/poly#43]). Until 0.29.0
  a failing actionlint run was one file-level warning. Since 0.30.0 poly reads actionlint's report, and
  it made every finding an error, so a workflow on a self-hosted or ARC label (`runs-on: runner-medium`)
  failed `poly lint` without a change to the file. actionlint has no severities of its own; poly now
  assigns one per kind. `runner-label` is a warning, since whether a label exists depends on the runners
  a repository has registered. `shellcheck` takes the level shellcheck printed (`SC2086:info` is info,
  not an error). Every other kind (`syntax-check`, `expression`, `action`, …) stays an error. Declaring
  the labels under `self-hosted-runner.labels` in `.github/actionlint.yaml` clears the warning, and
  `[lint.yaml.actionlint.rules.runner-label] level = "error"` restores the failing exit.

[Goldziher/poly#43]: https://github.com/Goldziher/poly/issues/43

## [0.31.0] - 2026-10-28

### Added

- **`uncollected-test-file` flags tests the runner never collects** ([Goldziher/poly#39]). A green run
  cannot show a test that did not run: pytest's default `python_files` skips `parser.test.py`, so a suite
  of such files passes by running none of them. The `quality` tier now reports a file whose name marks
  it as a test (a dotted `.test`/`.spec` stem anywhere, or a `test-`/`-test`-style name inside a test
  directory) but which the language's runner will not collect, skipping helpers (`conftest.py`, `*_helper.rb`, `fixtures/`, `testdata/`, …). It is
  on at warning severity for Python, Go and Ruby; JavaScript and TypeScript ship off, since Deno and
  `node --test` collect the `*_test.ts` names Jest and Vitest skip. `[lint.<lang>.quality]` takes
  `test_file_patterns` to match a custom runner config and `forbid_test_files` to rule a language's
  tests out entirely (e.g. shell tests must be `.bats`). See ADR 0036.

[Goldziher/poly#39]: https://github.com/Goldziher/poly/issues/39

### Fixed

- **ruff lints for the project's own Python target again** ([Goldziher/poly#42]). poly never read
  `[tool.ruff] target-version` or `requires-python`; it only appeared to while ruff's default target was
  3.10, and the 0.17 ruff line moved that default to 3.11. Since 0.30.0 a 3.10 project was told to make
  3.11-only rewrites (`UP017` `datetime.UTC`, `UP042` `StrEnum`) and `I001` filed `tomllib` as standard
  library. Without a `target_version` in `[lint.python.ruff]`, the target now comes from the nearest
  `.ruff.toml`, `ruff.toml` or `[tool.ruff]` (following its `extend` chain), then from
  `project.requires-python`, and only then from ruff's default. The resolved target is part of the cache
  key, so editing it invalidates cached results.

[Goldziher/poly#42]: https://github.com/Goldziher/poly/issues/42

## [0.30.1] - 2026-10-28

### Fixed

- **Hooks under staged isolation get the original repository's git context** ([Goldziher/poly#37]).
  The snapshot is not a checkout, so a hook enumerating the index (`git ls-files` feeding a
  whole-tree secret scan) failed with "not a git repository". Every hook in a staged run now gets the
  absolute `GIT_DIR` (a linked worktree's own git dir) and the absolute `GIT_INDEX_FILE` of the index
  the commit is building, the alternate index of `git commit <paths>` / `-a` included. No
  `GIT_WORK_TREE` is exported, so a snapshot-local `git init` cannot re-point the checkout, and
  `GIT_OPTIONAL_LOCKS=0` keeps `git status` from rewriting the index's stat cache.

- **`pydocstyle`, `bashate`, `luacheck` and `mh_style` are linters, not formatters.** The mdsf catalog filed
  these report-only checkers as formatters, so an enabled one ran through the format path, where a failing exit
  reads as "unchanged" and every finding was discarded. `overlay.json` reclassifies them, and `poly lint` now
  reports their findings. An enabled catalog tool whose binary is not on `PATH` now logs a warning, since the
  file can still count as checked through another engine and the missing check would otherwise look like a pass
  (Goldziher/poly#36).

- **The npm package is published again.** The release workflow checked for `@goldziher/polylint` with
  `pnpm view` in a job that never installs pnpm, so the check failed and v0.30.0 skipped npm. It uses
  `npm view` now.

[Goldziher/poly#37]: https://github.com/Goldziher/poly/issues/37

## [0.30.0] - 2026-10-28

### Added

- **`golangci-lint` fixes with `run --fix`.** A finding that carries `SuggestedFixes` (or a `Replacement`) is
  fixed on a scratch copy of the whole Go module (nested modules and anything outside it are never copied),
  run over the package directories of the files concerned, once per configured `goos`, and keeping
  `new_from_rev` working by pointing the copy at the repository. Only the files of the run are read back
  and your tree is untouched; a golangci-lint that rejects `--fix` leaves the findings report-only.
- **Recipe tools can be auto-downloaded from archives.** A pinned asset may name a `member`; poly verifies the
  SHA-256 of the `.tar.gz` / `.tgz` / `.zip` as downloaded, then extracts only that file (exact path match, no
  `..` or absolute paths, regular files only, 512 MiB cap) and installs it atomically into the tool cache under
  the member's name. helm, helm-template, golangci-lint, actionlint, tflint and tofu now ship pinned releases for
  darwin and linux on arm64 and x86_64, so `install = "auto"` works for them.
- **Ruby is detected for `Gemfile`, `Rakefile`, `Brewfile`, `Vagrantfile`, `Podfile`, `Capfile`, `Guardfile`,
  `Berksfile`, `*.rake`, `*.gemspec`, `*.ru` and `*.podspec`**, and `Tiltfile` is treated as Starlark.
- **`dockerfile-parser`: `Splicer::splice` returns a `Result` instead of panicking on a bad span**
  (`Error::InvalidSpan` for a reversed, out-of-bounds or mid-character span), and under a backtick
  `# escape=` directive a trailing `\` that cannot continue a line now stays inside the instruction span.
- **Dockerfile detection covers `Dockerfile-*`, `Dockerfile_*`, `Containerfile`, `*.Containerfile` and
  `.tpl` / `.tmpl` wrappers**, so `Dockerfile-local` and `Dockerfile_pyspark_base` are linted. A
  Go/Helm-templated Dockerfile that does not parse is skipped, with the reason reported, instead of
  raising a syntax error. The Dockerfile engine's `version()` gained `+tmplskip`.
- **The built-in directory prune no longer hides tracked source.** `build`, `dist`, `target`, `deps`,
  `vendor` and `coverage` are pruned only when git tracks nothing inside them (one `git ls-files` per
  walk root, loaded lazily), so a tracked `deploy/build/` is linted. Tool caches and environments
  (`node_modules`, `.venv`, `__pycache__`, ...) stay pruned unconditionally, and `[discovery] exclude`
  adds further pruning.
- **Sparse staged snapshot hardening.** A hook that writes through a link into a directory the
  sparse snapshot did not write now fails the run, naming the directory (the working tree under
  each linked directory is compared before and after), instead of changing your working tree
  silently. A ruff config that `extend`s a file outside the staged files' directory chain (a
  `ruff.toml` or `[tool.ruff]` in `pyproject.toml`, followed through further `extend`s) has that file
  written from the index too, not read from the working tree through a link.
- **Nested `poly.toml` `[hooks]` tables now run on commit.** `poly hooks run` and `hook-impl` (git
  starts hooks at the repository root) add the hooks of every nested `poly.toml` with staged files
  under it: they run with that directory as their working directory, get file arguments and
  `files` / `exclude` globs relative to it, and carry the directory in their id (`svc:fmt`).
  `poly hooks install` binds the git hooks a nested table uses. `poly migrate --recurse` writes a
  nested directory's hooks to its `poly.toml` and deletes the nested source when nothing was lost,
  instead of keeping it.
- **`requirements_txt_fixer` file-safety member**, off by default: sorts requirements files like the
  pre-commit-hooks `requirements-txt-fixer` and is re-staged as a fixer. `poly migrate` maps that
  hook to it, so it is no longer reported as unmapped.
- **Ruff plugin tables and linter-wide settings.** `[lint.python.ruff]` now reads every remaining
  `ruff_linter` setting: the `flake8_annotations_*`, `flake8_bandit_*`, `flake8_boolean_trap_*`,
  `flake8_bugbear_*`, `flake8_builtins_*`, `flake8_comprehensions_*`, `flake8_copyright_*`,
  `flake8_errmsg_*`, `flake8_gettext_*`, `flake8_implicit_str_concat_*`, `flake8_self_*`,
  `flake8_tidy_imports_*`, `flake8_unused_arguments_*`, `pydocstyle_*` (decorators and var
  parameters), `pydoclint_*`, `pyflakes_*`, `pyupgrade_*` and `ruff_*` keys, `isort_required_imports`,
  and `builtins`, `namespace_packages`, `allowed_confusables`, `logger_objects`, `task_tags`,
  `typing_modules`, `typing_extensions`, `future_annotations` and `ignore_init_module_imports`.
  `[fmt.python.ruff]` takes `preview` and `exclude` (matched at any depth). `poly migrate` carries
  all of them, so those ruff keys no longer keep the ruff source. The ruff engine's `version()` is
  bumped.
- **Config key diagnostics suggest the fix.** An unknown option in a `[lint.<lang>.<engine>]` /
  `[fmt.<lang>.<engine>]` table names the nearest key the table accepts (`Did you mean
  \`mccabe_max_complexity\`?`), and a tool table one edit from an engine of the language
  (`[fmt.python.rufff]`) is reported with the engine it was probably meant to be. Every registry engine
  declares the keys of both tables; no engine table is left unchecked.
- **SQL formatting audit.** `poly fmt` for SQL applies only the `layout` and `capitalisation` groups,
  with identifier capitalisation (`CP02`) off unless selected; a regression test now pins that mixed-case
  table, column and quoted names come through a format untouched.
- **shfmt follows `.editorconfig`, and `poly migrate` carries `shfmt -i N`.** shfmt reads
  `.editorconfig` only when given a filename, which poly never passes, so a repository indented
  with four spaces was reformatted to poly's two. poly now resolves the file's `indent_style` /
  `indent_size` itself, as shfmt does (`tab` indents with tabs; `space` takes `indent_size`, or 8
  when it is missing; a file that sets no `indent_style` keeps poly's default), unless
  `[fmt.shell.shfmt] indent_width` or `use_tabs` is set, and the formatting cache is keyed on the
  resolved indentation. `poly migrate` maps a lefthook `shfmt -i N -w {staged_files}` job or a
  pre-commit `shfmt` hook with `-i N` to `[fmt.shell.shfmt] indent_width = N` (`-i 0` to
  `use_tabs = true`); other layout flags keep the job a command. The native-tool engine's
  `version()` carries `editorconfig-v1` for shfmt.
- **Real `oxlint` / `oxfmt` as a fallback, and path-scoped oxfmt config.**
  `[lint.<lang>.oxc] js_plugins = "external"` lints the files governed by an `.oxlintrc.json` that
  declares `jsPlugins` with the real `oxlint` (nearest `node_modules/.bin`, else `PATH`), one run per
  config, and parses its JSON report into poly diagnostics; `"skip"` (default) keeps the warning.
  `[fmt.<lang>.oxc] external = true` formats files whose `.oxfmtrc.json[c]` sets `sortTailwindcss`,
  `jsdoc` or `experimentalTernaries` with the real `oxfmt`. `overrides[].files` /
  `excludeFiles` globs are now resolved per file relative to the config, and the nearest nested
  `.oxfmtrc.json[c]` replaces the root one. The formatting cache key carries the digest of the
  governing config and the overrides that match, so editing a nested config re-formats; the oxc
  engine's stat-cache context now also carries the governing oxlint config digest.
- **oxc lint parity with oxlint.** `poly lint --fix` now applies oxlint's safe fixes: the linter
  was built with `FixKind::None`, so no rule produced an edit, and a finding with alternative fixes
  applied all of them instead of the first, as `oxlint --fix` does. `[rules.sort-keys]` options
  (`caseSensitive`, `natural`, `minKeys`, `allowLineSeparatedGroups`) reach the rule: its
  configuration is positional, so the options object follows an `order` argument (`order = "desc"`,
  default `"asc"`), and `args = [...]` spells out the arguments of any other positional rule. The one
  remaining `sort-keys` difference on the armis corpus is oxlint 1.87 skipping computed identifier
  keys (`{ a, [S], b }`), which the pinned oxc revision reports as ESLint does.
- **Markdown fences and front matter are formatted.** `[fmt.markdown.rumdl]` now installs a
  dispatcher on `oxc_formatter_markdown` as oxfmt does: JavaScript, TypeScript, JSON, YAML, CSS,
  SCSS, Less, TOML and nested Markdown fences, and YAML/TOML front matter, are formatted in
  process; other languages and fences that do not parse stay as written.
  `embedded_language_formatting = "off"` (oxfmt `embeddedLanguageFormatting`) turns it off, and the
  `embedded.<js|json|yaml|css|toml>` sub-tables carry the per-family options a `.oxfmtrc.json[c]`
  supplies. The rumdl engine's `version()` carries `+md-embed-1`.
- **`dockerfile-parser` is now vendored as the `crates/dockerfile-parser` workspace crate** (ADR 0003
  amendment). Upstream, HewlettPackard/dockerfile-parser-rs, is archived, so poly maintains it from
  upstream commit `08f3789` (`v0.9.0` plus the image-digest change). It moved to edition 2024 with
  `thiserror` and `LazyLock`, and now accepts input upstream rejected: heredocs (`RUN <<EOF`,
  exposed as `Dockerfile::heredocs`), `ARG A=1 B`, empty `ARG A=` / `ENV A=` values, comments and
  blank lines inside `\` continuations, `COPY ["a", "b"]`, a leading byte order mark and the
  `# escape=` directive. `"\$HOME"` and Windows paths in quoted values no longer fail with an
  unescape error, and a `FROM` digest is validated as 64 hex characters rather than any string
  containing one hex digit. The Dockerfile engine's `version()` is now
  `0.10.0+vendored1+parse-diag-v1+extendsel2`, so cached results from the crates.io parser are
  ignored. `psm` stays at 0.1.24: pest 2.9.5 and `pest_stack_guard` 1.0.0 are the latest releases
  and still pin it exactly, and the stack guard is what turns pathologically deep input into a
  parse error instead of a stack overflow.
- **Hook runner fixes found on a 92k-file monorepo.** A job with `root = "<dir>"` now receives
  file arguments (and `{staged_files}`) relative to `<dir>`, and is skipped when none of its files
  are under it. A job's output is shown
  when it passes with `show_output = true` or `poly hooks run -v`. The staged snapshot is sparse
  (`[hooks] snapshot = "auto"`) unless a `workspace` or cargo job will run, so a one-file commit no
  longer materializes the whole tree. `poly hooks install` keeps a foreign hook as `<hook>.legacy`
  and runs it before poly (a lefthook hook is kept inert; `--replace-lefthook` discards it).
  `[hooks] partial_stage = "restage"` stages only a `stage_fixed` fix over a partially staged file,
  and the withheld-fix failure now names the commands to resolve it. `poly doctor` and `poly hooks
  run` warn when a job needs `node_modules` under an isolated gate. Hook children no longer print
  the shadowed-`poly` warning into their captured output.
- **Recipe tools: helm dependencies and per-chart values, golangci-lint `new_from_rev` / `goos`, pinned buf and
  hadolint.** `[tools.helm]` gains `values_files`, `chart_values`, `skip_charts` and `prepare`
  (`helm dependency build` once per chart, skipped when `charts/` satisfies `Chart.lock`, when
  `POLY_OFFLINE` is set or with `prepare = false`); `.tpl` files and `NOTES.txt` under `templates/` are now
  inputs of helm and of no other engine. `[tools.golangci-lint]` gains `new_from_rev` (`--new-from-rev`)
  and a `goos` matrix; `poly migrate` carries the armis wrapper's `--new-from-rev=HEAD` instead of
  reporting it lost. buf 1.75.0 and hadolint 2.15.1 are pinned with checksums for `install = "auto"`.
- **Recipe tools: `helm dependency build` is retried, and a stale `skip_charts` entry fails.** The prepare step is
  attempted `prepare_attempts` (5) times, waiting `prepare_backoff_secs` (4) doubling after each failure plus up to
  `prepare_jitter_secs` (4) at random, as the armis `check_charts.sh` does. A chart in `skip_charts` is still run
  once, and a tool that succeeds on it fails the run, so the list cannot outlive its reason; `verify_skip_charts =
  false` restores skipping without running.

### Fixed

- **`poly fmt` / `poly lint` on many explicit paths** spent seconds in config resolution and
  discovery per run (a `canonicalize` and a `WalkBuilder` per named file, each re-reading the global
  gitconfig and parent `.gitignore`s). Canonicalization is memoized, named files skip the walker,
  and compiled exclude sets are shared per frame.
- **Recipe tool cache keys and version pins.** A `findings = "all"` tool (helm) was keyed to the process and
  recomputed on every run; it is now keyed to the digest of its project root, so unchanged charts are cache
  hits, and its unit-wide findings reach every file of the run. A `version` pin poly cannot honour (not
  cached with `install` not `auto`, failed download) now makes the tool unavailable with a message instead of
  silently running whatever version is on `PATH`.

### Changed

- **Warm runs walk the tree once.** The nested-config scan now records every file and directory it
  sees, and discovery replays that record in memory, applying the built-in prune set and the
  effective `[discovery] exclude` globs (pruned subtrees are skipped logically) instead of walking
  the same directories a second time. Discovered files now come back in path order. The ruff engine
  remembers package roots per directory for the length of a run, `write_atomic` skips its `chmod`
  when the temp file already has the original mode, and the stat index is sized once on load.
  The doctor's version probe retries an executable the kernel reports as busy, and its bound can be
  lifted with `POLY_PROBE_TIMEOUT_SECS`, so its tests no longer race the clock.
- **Formatter parity with the incumbent tools (measured on a 92k-file monorepo, 300-file samples).**
  - **HCL** now formats with a port of `hclwrite.Format`, so `.tf`/`.tfvars`/`.hcl` output is
    byte-identical to `terraform fmt` / `terragrunt hclfmt` (all 2,682 `.tf`/`.tfvars` files in the
    sample repository are unchanged; before, 233 of 300 were rewritten). `hcl-rs` is gone.
  - **`.oxfmtrc.json[c]` is honored** by the oxfmt-equivalent formatters: `printWidth`, `tabWidth`,
    quotes, commas, `endOfLine`, extension-scoped `overrides`, `sortImports`, `sortPackageJson` and
    `ignorePatterns`. An explicit `[fmt.<lang>.<engine>]` key still wins. Not supported, and warned
    about once: `sortTailwindcss` (its class sorter is JavaScript), `jsdoc`,
    `embeddedLanguageFormatting`, and overrides scoped to part of a language rather than an extension.
    JS/TS went from 300 of 300 files differing from `oxfmt` to 0.
  - **YAML, TOML, Markdown and CSS format with the same crates oxfmt uses** (`oxc_formatter_yaml`,
    `_toml`, `_markdown`, `_css`); YAML is 95 of 95 identical on files oxfmt owns (was 20 of 95).
    The previous formatters remain available: set a taplo layout key, a rumdl rule key or a malva
    key in the engine's table. Fenced code and front matter in Markdown are left as written (oxfmt
    re-formats them with its binary-only embed dispatcher).
  - **`package.json`/`composer.json`/import maps** use the `JSON.stringify` layout npm writes;
    `sort_package_json` (or `sortPackageJson`) sorts `package.json` keys.
  - **Go**: imports are grouped like `goimports` after `gofmt`. **Proto**: default indent is 2 spaces
    (300 of 300 byte-identical with `buf format`).
- **oxlint config is honored.** With a `.oxlintrc.json[c]` poly runs exactly its rules, categories,
  plugins, overrides and `ignorePatterns`, using the nearest config above each file as oxlint does
  (`oxlintrc = false` opts out). On the sample repository poly reported 852 diagnostics against
  oxlint's 2; it now reports the same two plus one `sort-keys` difference caused by the newer rule
  implementation in the pinned oxc revision. `jsPlugins` need a JS runtime and are not run; a
  single warning names them.
- **YAML lint accepts what libyaml/PyYAML accept** (multi-line quoted scalars continued at the
  parent's indent, flow collections closed at the parent key's column): 24 false syntax errors in
  the sample repository are gone.
- **Lock files are never formatted or linted** (`pnpm-lock.yaml`, `Cargo.lock`, `yarn.lock`, ...),
  as `poly migrate` already claimed.
- **SQL**: identifier capitalisation (`CP02`) is off unless selected; it lower-cased keywords the
  dialect did not know (`ADD COLUMNS` became `ADD columns`).
- **Config checks**: a `[lint.<id>]`/`[fmt.<id>]` language id that is a near miss of a known one
  (`pyhton`, `yml`, `js`) is reported with the likely language; `poly doctor` says when a wildcard <!-- poly: allow[typo] deliberate near-miss example of a misspelled language id -->
  exclude (`bazel-*`) only prunes directories below the config root.
- **Dependency refresh.** `cargo upgrade --incompatible --pinned`, plus advancing all three git
  pins to their remotes' HEADs: biome `dd96b11` → `e2e5a36`, oxc `98573d2` → `e2be72d`, rubyfmt
  `f765808` → `884b9fd`. The ruff crates moved `0.0.17`/`0.17.0` → `0.0.20`/`0.17.3`, `rumdl`
  0.2 → 0.3, `sqruff` 0.41 → 0.43, `ast-grep` 0.50 → 0.51, `saphyr` 0.1 → 0.3, `serde-saphyr`
  1 → 2, `rmcp` 3.5 → 3.8, `fancy-regex` 0.19 → 0.20 and `tree-sitter-language-pack` 1.21 → 1.22.
  **The workspace `rust-version` is now 1.99** (was 1.97): `mago` 1.51 → 1.59 requires it. Every
  engine `version()` value fed by a bumped dependency moved with it, so cached results are
  invalidated rather than reused. rumdl 0.3 now reports `MD041` for a file that does not start
  with a level-1 heading; the `rumdl` diagnostics snapshot records it. **The biome pin is back at
  HEAD although ADR 0003 held it at `dd96b11`**: HEAD still carries the fixture whose checkout
  path exceeds Windows' `MAX_PATH`, so the Windows build must be watched. `psm` stays at 0.1.24
  because `pest_stack_guard` pins it exactly.
- **ai-rulez 5.3.0 → 5.7.0.** Every pin (`poly.toml` hook source, the four workflow setups, both
  `ai-rulez.lock` files) moved to the published 5.7.0 release; the reviewed-source build pin is
  gone because 5.7.0 carries the plugin fixes it existed for. The plugin bundles and public
  skills were regenerated: only `Source-Hash` headers and generated hashes changed, plus
  `plugin/package.json`, whose `@opencode/plugin` range the OpenCode generator owns and now emits
  as `^2.0.26` (the earlier hand edit to `^2.1.4` cannot survive generation; the range still
  admits 2.1.4).
- **Hermes adapter ported to `mcp` 2.3.0.** The adapter catches `MCPError` (renamed from
  `McpError`), passes `read_timeout_seconds` as plain seconds, and dumps results with
  `by_alias=True` so the camelCase wire keys (`inputSchema`, `isError`) stay the same. The pin
  moved 1.30.0 → 2.3.0 in `Taskfile.yaml`, `config.plugins.toml` and the generated
  `plugin.yaml`/`pyproject.toml`; the contract test's stdio fixture uses `MCPServer`, the 2.x
  name for `FastMCP`.
- **Final sweep.** The root `@opencode/plugin` 2.1.4 → 2.2.0 (lockfile refreshed with pnpm
  12.15.0) and the `xberg-io/actions` SHA pins (`setup-ai-rulez`, `validate-ai-rulez`,
  `package-ai-rulez-plugin`) advanced to `d922e0d`, whose setup action defaults to ai-rulez 5.7.0.
  Cargo, the other pnpm workspaces and the GitHub Actions tags had nothing newer.
- **Tooling refresh.** pnpm 12.11.2 → 12.15.0, `@opencode/plugin` 2.0.26 → 2.1.4, the OpenCode
  `@opencode-ai/plugin` 1.18.35 → 1.19.4, Astro 7.3.8 → 7.4.2, Starlight 0.42.6 → 0.42.7, the
  Hermes adapter's `mcp` pin 1.28.1 → 1.30.0 (taken further to 2.3.0 below),
  `astral-sh/setup-uv` v10.3.2, `pnpm/action-setup` v6, `actions/deploy-pages` and
  `actions/upload-pages-artifact` v5. `deny.toml` ignores the `zip` 8.x extraction advisories
  (pinned by `ruff_db`; poly never extracts archives with it) and the `serde_yaml` unmaintained
  notice.
- **Builtin `fix` mode, `jobs`, `priority` and the `aws_credentials` check** for `[hooks.builtin]`;
  `{0}` / `{N}` / `{git_args}` placeholders and `$@` / `POLY_GIT_ARG_*` for git hook arguments;
  `poly fmt --exit-zero-on-fix`.
- **Ruff**: every isort key, the remaining pylint limits, `flake8-import-conventions`, format
  `indent-style` / `quote-style` / `skip-magic-trailing-comma` / `line-ending`, `unsafe-fixes`,
  `preview`, and `inherit_mode = "replace"` for nested configs. An explicit `select` that covers a
  rule poly holds back by default (`RUF100`, `B008`, ...) now enables it.
- **`poly migrate`**: lefthook / pre-commit fix-mode jobs, per-job scopes (no exclude is dropped),
  git-argument jobs, `--unsafe-fixes`, `detect-aws-credentials`, ruff `extend`, `requires-python`,
  ruff defaults pinned, and `--recurse` that skips virtualenvs, `.gitignore`d trees and `bazel-*`.

## [0.29.0] - 2026-10-11

### Added

- **A published JSON Schema for `poly.toml`, and `poly config schema`** (ADR 0006 amendment). The
  schema is generated from the config types plus each backend's declared option keys, so editor
  autocomplete and the runtime `unknown-config-key` check share one source of truth. It is served
  at `https://goldziher.github.io/poly/poly.schema.json` and documented on the new
  [configuration reference](https://goldziher.github.io/poly/reference/configuration/) page, with
  a whole-repo test that fails if the published file drifts from the binary.

### Fixed

- **`quality`'s `too-many-parameters` no longer counts a parameter's attribute as a parameter**
  ([Goldziher/poly#28]). tree-sitter-rust attaches `#[cfg(..)]` / `#[string]` to a parameter as a
  sibling of the parameter, so a six-parameter function whose last parameter carried `#[cfg]`
  measured as seven. Attribute and comment nodes are now skipped when counting.

- **Four engine `version()` strings left stale by the previous dependency update** (`sqruff`,
  `astgrep`, `treesitter`, `quality`). The `chore: updated deps` commit bumped `sqruff-lib`
  `0.40` → `0.41`, `ast-grep-core` `0.45.3` → `0.50.0` and `tree-sitter-language-pack` `1.20` →
  `1.21.1` without moving the strings folded into cache keys, so `version_audit` was red on `main`.
  Cached output is now invalidated correctly rather than served under the old keys.

### Changed

- **Dependency refresh.** The oxc (`4dce2c2` → `98573d2`) and rubyfmt (`094cdc8` → `f765808`)
  git pins advanced to their remotes' HEADs; every engine `version()` fed by a moved pin moved with
  it — `oxc`, `markup_fmt` (embedded oxc) and `rubyfmt`. **The biome pin holds at `dd96b11`**:
  biome's HEAD added a test fixture whose relative path is 205 characters, and cargo's 73-character
  checkout prefix pushes it past Windows' 260-char `MAX_PATH`. Cargo extracts git dependencies with
  libgit2, which ignores `core.longpaths` ([rust-lang/cargo#7420],
  [libgit2/libgit2#3053]), so no project-side fix exists; the pin stays at the last biome rev whose
  fixtures fit. The publishing-posture re-check held: all three crates remain unpublishable and stay
  on a git `rev` (ADR 0003 amendment).

- **The Go ast-grep test predicate moved to the shared global utils** (`go-in-test-function`),
  replacing a byte-identical file-local copy in `deep-exit` and `force-type-assert`. Matching is
  unchanged.

### Documentation

- **ADR 0034** decides that test context stays two mechanisms — a per-language syntactic predicate
  and a path glob — with one predicate per language declared in the global utils
  ([Goldziher/poly#26]).
- **`string-concat-in-loop` audited** against the C2 corpus: 93 findings over 2,586 Python files,
  8/20 hand-read false positives, and confirmed not covered by ruff. It stays `off`
  ([Goldziher/poly#27]).

[Goldziher/poly#26]: https://github.com/Goldziher/poly/issues/26
[Goldziher/poly#27]: https://github.com/Goldziher/poly/issues/27
[Goldziher/poly#28]: https://github.com/Goldziher/poly/issues/28
[rust-lang/cargo#7420]: https://github.com/rust-lang/cargo/issues/7420
[libgit2/libgit2#3053]: https://github.com/libgit2/libgit2/issues/3053

## [0.28.2] - 2026-10-03

### Fixed

- **Staged-content snapshots no longer fail in a sparse worktree.** `git checkout-index` is now
  invoked with `--ignore-skip-worktree-bits`, so an omitted tracked entry (mode flag `S`, a
  `skip-worktree` path) is materialized into the staged snapshot instead of failing the whole
  snapshot step. The live checkout is never expanded, unrelated unstaged edits are preserved, and
  the index OIDs and flags are left untouched. Fixes [Goldziher/poly#29], which blocked normal
  (non-bypassed) commit hooks in a sparse checkout.

### Changed

- **Dependency refresh.** biome `f5af193` → `dd96b11`, oxc `7d28a66` → `4dce2c2` (rubyfmt was
  already at its remote's HEAD); the biome crates moved `0.5.7` → `0.7.0`, and `mago` moved
  `1.50` → `1.51`. Every engine `version()` fed by a bumped dependency moved with it —
  `biome_css`, `biome_graphql`, `oxc`, `markup_fmt` (embedded oxc) and `mago` — so cached results
  are invalidated rather than reused across the change.

[Goldziher/poly#29]: https://github.com/Goldziher/poly/issues/29

## [0.28.1] - 2026-09-30

### Changed

- **Dependency refresh.** `cargo upgrade --incompatible`, plus advancing all three git pins to
  their remotes' HEADs: biome `99c7049` → `f5af193`, oxc `827fdbd` → `7d28a66`, rubyfmt `5185d3b`
  → `094cdc8`. The ruff crates moved in lockstep `0.16.5`/`0.0.11` → `0.16.9`/`0.0.15` — biome's
  new HEAD needs `salsa ^0.28.5`, which the old ruff pin could not resolve. `tree-sitter` moved
  0.26 → 0.27, lifting the `uncomment` / `ast-grep` / `tree-sitter-language-pack` hold; those
  advanced to 3.10.2 / 0.45.3 / 1.20, and `mago` to 1.50. Every engine `version()` value that a
  bumped dependency feeds was moved with it, so cached results are invalidated rather than reused
  across the change.

- **`uncomment`'s default ignores now match tag words case-insensitively as substrings**, so a
  commented-out line whose text contains `bug` — including inside `debug` — is preserved rather
  than reported for removal. That is upstream's behavior in 3.10; poly pins it with a test.

## [0.28.0] - 2026-09-16

### Added

- **Native formatters can now run in batches, opt-in per tool** (ADR 0033). A tool whose per-file
  cost is dominated by process startup can be handed many files in one invocation instead of one
  process per file. Switch it on with `[fmt.<lang>.<tool>] batch = true`; implemented for `gofmt`,
  `shfmt`, `zig fmt`, `ktfmt`, `google-java-format`, `dart format` and `gleam format`.

  poly writes each file's in-memory content into a scratch tree it owns, lets the tool rewrite it
  there, and reads the results back. Output is byte-identical to the per-file path — verified over
  400 real shell files — and a batch that cannot prove its answer falls back to per-file rather than
  guessing.

  **It is off by default because there is no default that is right everywhere.** Batching does not
  remove work, it substitutes it: one process spawn for a mirror write, a tool-side read and write,
  and a read back. Measured on one machine, `gofmt` came out 3x faster batched while `shfmt` — which
  starts in almost exactly the same time — came out 4x slower; re-measured under heavy load, `shfmt`
  came out 4-7x faster. The sign depends on file sizes and machine contention, so poly does not
  guess. Where it pays, it pays a lot: `ktfmt` measured 6.4x and `google-java-format` 3.6x.

  `rustfmt`, `swift-format` and `styler` cannot be batched at all. That exclusion is about
  correctness rather than speed: the first two discover their own config by walking up from the
  file, which a scratch mirror does not reproduce, so batching them would quietly change which
  config governs the run.

  The real worktree is never handed to a tool: poly's atomic write remains the only thing that
  touches it, so `poly fmt --check` stays a true dry run. Passing content rather than paths also
  keeps the commit gate honest — a batched tool cannot re-resolve a path back into the worktree or
  follow a `snapshot_include` symlink out of the staged snapshot (ADR 0019).

- **`shfmt` now runs by default when it is installed** (ADR 0014, 2026-09-15 amendment). Shell was
  the one language poly formatted with nothing but whitespace normalization: `shfmt` was opt-in,
  and the tree-sitter tier does not reindent `bash` (it is in neither `BRACE_FAMILY` nor
  `LEAVE_UNTOUCHED`, and no bash indents query ships). Holding a third-party formatter to the
  first-party test did not reserve the slot for a better tool — there is no first-party shell
  formatter coming — it just left shell unformatted.

  Measured before flipping, across the pinned corpus B trees (10 repositories, 97 `.sh`/`.bash`
  files, none unparseable): **19 files changed, 424 diff lines, and a second pass changed
  nothing.** Unlike the shellcheck flip, this cannot redden a build on lint severity — `shfmt` is
  format-only, so what you see is a formatting diff.

  With `shfmt` absent, shell falls through to the generic tier with an info notice, exactly as
  before; the zero-system-dependency guarantee is unchanged. Opt out with
  `[fmt.shell.shfmt] enabled = false`.

- **The tree-sitter tier now structurally reindents shell, with no `shfmt` installed.** Flipping
  `shfmt` on by default only helps hosts that have it; without it a shell file got
  `normalize_whitespace` and nothing else, so **indentation was never touched**. bash now ships a
  poly built-in indents query (`BASH_INDENTS`, `engines/treesitter/indent.rs`) alongside the
  existing Elixir one, so a shell file comes out consistently indented on a machine with no shell
  toolchain at all.

  The model deliberately matches `shfmt`'s default layout, since both can act on the same file:
  `if`/`fi`, `do`/`done`, `{ … }`, `( … )`, `$( … )`, `<( … )` and array literals indent their
  interiors one level; `case` patterns stay at the `case` level with each item's body and its `;;`
  one level in; the closing keywords dedent only when they start their line, so an inline
  `if x; then y; fi` inside a function body keeps the body's indent.

  What it deliberately does **not** touch: heredoc bodies **and terminators** (reindenting a
  `<<EOT` terminator stops it terminating the heredoc), and continuation lines — backslash line
  joins, `&&`/`||` lists and pipelines — whose alignment poly cannot distinguish from an author's
  intent. A file the bash grammar cannot parse falls back to whitespace normalization rather than
  being reindented from a broken tree.

  This is **not** shfmt-equivalent: poly's tier-2 only moves leading whitespace, it never moves a
  token (no `if x\nthen` → `if x; then`, no `b|c)` → `b | c)`). It is the zero-dependency floor,
  and `shfmt` still supersedes it when present.

  One known divergence: for a `cmd ||` continuation followed by a `{ … }` block, `shfmt` indents
  the block relative to the continuation line's own indent, while tier-2 counts levels from column
  0. A file written that way is a fixed point for one formatter and not the other, so it reports as
  unformatted on hosts that disagree. Writing the guard as `if ! cmd; then … fi` avoids it.

  Measured over 488 real third-party shell files (Homebrew, `/usr/share`, the corpus B trees):
  **204 reformatted, a second pass changed nothing, no file went from syntactically valid to
  invalid, no non-whitespace byte changed, and all 2,023 heredoc body/terminator lines — located
  with `shfmt`'s own parser — came through byte-identical.** Agreement with `shfmt -i 2` rose from
  178/480 files already at a fixed point to 204/480.

### Changed

- **Shell now defaults to two-space indentation, not four.** `Language::Shell` was never in
  `default_indent_width`'s two-space list, so it fell through the `_ => 4` catch-all — the fallback
  firing, not a decision about shell. Two spaces is what modern shell is actually written in and
  what the Google Shell Style Guide prescribes, and it is the width poly hands to `shfmt -i`.

  Measured over the pinned corpus B trees (10 repositories, 97 `.sh`/`.bash` files, none
  unparseable): formatting at four spaces rewrites **61 of 97** files and 3,332 diff lines, where
  two spaces rewrites **19** files and 424 lines. **87% of that churn was the width alone**, not
  shfmt's structural opinions — 42 files changed for no reason other than poly's own default.

  This affects anyone who had opted into `[fmt.shell.shfmt]`. To keep the old output, set the width
  on the **engine** table — `[fmt.shell.shfmt] indent_width = 4`. It has to go there and not on the
  language table: `Config::engine_config` reads `indent_width` from
  `tables[<language>][<engine>]`, so `[fmt.shell] indent_width = 4` parses without complaint and
  silently does nothing. No cache invalidation is needed by hand — `__indent_width` is already
  folded into the cache key (`runner/plan.rs:376`), so changed output re-runs on its own.

## [0.27.0] - 2026-09-15

### Changed

- Moved the `biome` and `oxc` git pins to upstream head (both had been pinned at 2026-08-29):
  biome `7d54688` → `99c7049`, oxc `db66f58` → `827fdbd`; `rubyfmt` was already at head. The oxc
  workspace moved `oxc_formatter` 0.65.0 → 0.68.0 and `oxc_parser` 0.147.0 → 0.150.0.

## [0.26.0] - 2026-09-15

### Changed

- Upgraded dependencies and the engine cache keys that track them: mago 1.47 → 1.48.1, rumdl
  0.2.62 → 0.2.73, tree-sitter-language-pack 1.15 → 1.18, rmcp 3.1 → 3.3, serde-saphyr 1.1 → 1.2,
  plus in-range bumps. The affected `Engine::version()` strings move with them — they key the
  result cache, so a wrapped crate that changes while `version()` does not would leave stale output
  cached.

## [0.25.0] - 2026-09-14

### Fixed

- **An opt-in formatter that was never turned on no longer suppresses the catalog tier.** Whether
  poly consults `[tools.<name>]` formatters at all is decided by "does this language already have a
  tier-one formatter?", and that question was answered from declared *capability* rather than from
  whether the backend would actually run. Every native formatter that defaults to off — `shfmt`,
  `zig fmt`, `ktfmt`, `google-java-format`, `swift-format`, `styler`, `dart format`, `gleam format`
  — therefore counted as a working formatter while doing nothing, and withdrew the catalog tier on
  the language's behalf.

  The visible result: `[tools.shfmt] enabled = true` did nothing whatsoever, and `poly fmt --check`
  reported `All formatted.` over shell files `shfmt -l` flags. Nothing was logged; the catalog list
  was discarded before reaching the code that warns about a displaced tool. Measured across a
  nine-repository estate, seven repositories carried that setting and none of them had ever had a
  shell file formatted.

  An earlier fix moved this question after the `enabled` filter so an explicitly disabled formatter
  would stop suppressing the catalog. That was the same bug one step in: the filter drops only an
  explicit `enabled = false`, so a formatter that was never switched on at all sailed past it. Both
  paths now ask `Engine::provides_language_format` — the counterpart of the `provides_language_lint`
  the lint side has always asked.

  **Behaviour change on upgrade:** a config that enables a catalog formatter for a language whose
  built-in formatter is off now formats those files, because that is what it asked for. Enable the
  built-in instead (`[fmt.shell.shfmt] enabled = true`) to keep poly's own wrapper, which still
  takes precedence whenever it is on.

### Added

- **`use_tabs` for shfmt, so tab-indented shell trees can adopt the formatter.** poly injects
  `-i <indent_width>` for shfmt, and `Config::engine_config` filters `0` out as "unset" for every
  engine — so the one value shfmt uses to mean "indent with tabs" was the one value poly could not
  send. A repo whose shell is tab-indented therefore had to accept a whole-tree reindent as the
  price of turning shfmt on, which in practice meant leaving it off. `[fmt.shell.shfmt] use_tabs =
  true` emits `-i 0` instead. Measured on a 65-file tab-indented repo: 61 files reformatted before,
  zero after, with the formatter genuinely running in both cases.

  Off by default, and `indent_width` still governs the width when it is off.

### Changed

- **ast-grep rule directories support a shared `utils.yml`.** A file by that name in a rule
  directory declares *global* utility rules — predicates other rules reference by `matches:` —
  instead of every rule carrying its own copy in a file-local `utils:` block. Every `utils.yml`
  under the configured directories merges into one namespace, so a util may reference one declared
  in another file; ast-grep sorts them topologically. Editing one invalidates the rule cache exactly
  as editing a rule does.

  poly's built-in pack now uses the same mechanism for its own helpers: the four Rust test-context
  predicates that were copied verbatim into five rule files live in `builtin/utils.yml` and are
  referenced as `rust-in-test-context`. The refactor preserves every rule's matching — the pack's
  `*-test.yml` corpora assert it — but `ENGINE_VERSION` moves to `builtin-pack-3` regardless, since
  the rules compile through a different registration than any cached payload did.

  This was previously reported as blocked on the unmaintained `serde_yaml`. It was not:
  `ast_grep_config` re-exports its own `from_str`, so the pack deserializes `SerializableGlobalRule`
  with the parser it already used for rules, and no dependency was added or swapped.

### Fixed

- **The config fingerprint no longer depends on the host path separator.** `ConfigFingerprint`
  built both the reported `root` and the relativized `[rules] dirs` with `Path::display`, so the
  same commit described itself as `packages/api` on Linux and `packages\api` on Windows — and, for
  any config with a nested rules directory, hashed differently on the two. The field is new in
  0.24.0, so no released fingerprint was affected.

### Testing

- The built-in pack's `*-test.yml` corpora now run under `cargo test`. They previously executed only
  when someone ran `poly rules test` against that directory by hand, so nothing in CI verified the
  semantics of the 26 rules a default `poly lint` runs.

## [0.24.0] - 2026-08-31

This release is mostly one theme: **a report that reads as clean now has to have earned it.**
Coverage that a run withdrew is named and accounted for, every suppressed finding is reported,
a formatter that cannot converge says so instead of claiming success, and the machine-readable
document answers "what did you not look at?" without scraping a human summary.

**Breaking:** `poly lint --format json` / `--format toon` and their `poly fmt` counterparts emit
an object rather than a bare array. See the entry under Changed.

### Fixed

- **ast-grep reaches five languages it could never see.** A file's language was handed to the rule
  lookup as poly's own id, but a rule's `language:` deserializes to a validated
  tree-sitter-language-pack grammar and is therefore always stored under a *grammar* name. For the
  five poly ids that are not grammar names — `jsx`, `jsonc`, `mdx`, `jinja`, `mustache` — the two
  never met, and no configuration could make them: `language: jsx` fails to deserialize, so no rule
  could be keyed to it, and a `language: javascript` rule was never looked up for a `.jsx` file.
  A `.jsx` file therefore had **no** ast-grep coverage at all, from the built-in pack or from a
  user's own rules, and nothing said so. The id is now resolved to the grammar that parses it, and
  a hardcoded table is guarded by a test asserting every poly language id resolves to a real
  grammar. `ENGINE_VERSION` moves to `engine-3`, since a cache written by the old binary holds the
  empty result those files used to get.

- **A file the formatter cannot settle is now reported instead of called formatted.** `poly fmt`
  re-runs the engine chain until the content stops changing, capped at five passes so an
  oscillating backend still terminates — but exhausting that cap was silent, so poly wrote the file,
  reported success, and a following `poly fmt --check` reported drift forever with nothing to
  explain why. Real HTML has been observed where `poly fmt --fix` produced different bytes on seven
  successive runs. Such a file is now a per-file error naming the pass cap, so the run exits 2 and
  says it verified less than it claims.

- **`undocumented-unsafe-block` no longer flags `unsafe` in test code.** Under the 2024 edition
  `unsafe { std::env::set_var(..) }` inside a test module is the *only* legal way to write that
  statement, so demanding a `SAFETY` comment for it was the rule being wrong about the code. The
  rule's own note had recorded this as a precondition for ever enabling it; it now carries the same
  `in-test-context` carve-out the other Rust rules use, covering both `#[cfg(test)] mod` and
  attribute-marked test functions. It stays `off` pending a corpus audit of what remains. Part of
  [#22](https://github.com/Goldziher/poly/issues/22).

- **Switching an engine off no longer removes a language from the run.** Two shapes of that, both
  found by review before release. `[fmt.rust.rustfmt] enabled = false` dropped the engine from the
  plan — but Go, Rust, Zig, Java, Kotlin, R, Swift, Dart and Gleam have no separately registered
  tree-sitter entry, and their one engine is the thing that *hands the file to* the tier-2
  reindenter when the native tool is off. Removing it left those languages entirely unformatted.
  The `Engine` trait grew a `self_manages_enabled` predicate so a backend that degrades on its own
  terms stays in the plan; `enabled = false` means "do not shell out to the native tool", never
  "stop formatting this language".

  The second shape was quieter: an emptied format plan was treated as "no backend covers this
  language", which had been unreachable and is not any more, so `poly fmt --only ruff .` printed
  `All formatted` over files nothing had touched. Walked files now get the same accounting named
  files already had.

- **A disabled tier-one formatter no longer suppresses the catalog tier.** The check that decides
  whether catalog tools are consulted ran *before* the `enabled` filter, so switching off `gofmt`
  to reach for a catalog formatter produced neither.

### Changed

- **A built-in rule declares its default path exclusions in its own YAML, and a user can now
  override them.** The pack's four noisy rules were filtered by a hardcoded `NOISY_PATH_EXCLUSIONS`
  table in Rust, which its own docs called a stand-in: a reader could not see the exclusion next to
  the rule, and could not opt back in. Each rule now declares its globs with the standard ast-grep
  `ignores:` key, beside the `note:` that justifies them — the same key a user's own rule file can
  use, through the identical parse path, with no poly-specific schema. The globs are unchanged, so
  the four rules exclude exactly the paths they did.

  They are now **defaults**: a `[per-file-ignores]` entry naming a rule **replaces** its declared
  globs rather than unioning with them, matching how a user rule with a pack rule's `id` replaces
  it outright. This does not change any rule's default severity — it makes the exclusions auditable,
  which is the prerequisite for that decision, not the decision.

- **BREAKING — `poly lint --format json` / `--format toon` now emit an object, not an array.**
  The payload gains three things a consumer previously had to reconstruct by walking every record:
  a top-level `skipped` list, a top-level `errors` list, and a `summary` of
  `checked` / `skipped` / `errored` counts.

  ```json
  // before
  [{ "path": "src/main.py", "diagnostics": [] }]

  // after
  {
    "results": [{ "path": "src/main.py", "diagnostics": [] }],
    "errors": [],
    "skipped": [],
    "summary": { "checked": 1, "skipped": 0, "errored": 0 }
  }
  ```

  `errors` was already promoted to the top level of the MCP payload precisely so a caller could
  gate on coverage without scanning; `skipped` was not, and there was no count of what had been
  checked. A run where every file was skipped produced an empty diagnostic list, set no error flag,
  and read exactly like a clean pass — which an agent told to "lint the changed files and fix what
  it finds" is right to treat as done. Closes
  [#19](https://github.com/Goldziher/poly/issues/19).

  **Gate on `summary.checked`, not on an empty `results`.** The three counts are not a partition of
  `results` and cannot be derived from it: a file checked and found clean produces no record at all,
  and a file whose language poly has no rules for can be *both* a skip and a result, since the
  cross-cutting backends still run over it.

  The CLI and the MCP server now serialize the same `poly-core` type, so the two surfaces cannot
  answer "what did you actually check" differently. A skip still does not set `isError` — it is
  coverage information, not a failure.

- **A coverage gate no longer fails on your own configuration.** poly now draws one line across
  every mechanism that withdraws coverage: **a limit of poly is charged to `--deny-skips` and names
  the limit; an instruction you wrote is never charged, and names itself.** So a file left
  unchecked by `--only`/`--skip` or by `enabled = false` is reported with the flag or the exact
  config table that did it — `every engine disabled by config: [lint.toml.taplo]` — instead of
  `no lint rules for TOML`, which read as a poly limitation and sent readers looking for a backend
  that was sitting right there, switched off in their own file. Files poly genuinely cannot check
  are still counted and still fail the gate.

### Added

- **Every suppressed diagnostic is now reported, in a `suppressed` array.** Three mechanisms
  removed findings from a report with nothing anywhere saying so: a rule's default path exclusions,
  a `[per-file-ignores]` glob match, and an in-source `poly: allow[…]` directive. Each dropped
  finding is now one entry in a top-level `suppressed` list carrying the file, the rule code, and
  which mechanism dropped it (`default-path-exclusion` / `per-file-ignore` / `inline-suppression`),
  so **`results` plus `suppressed` is the unfiltered finding set from a single run** — no second
  run with the filters off. None of these counts against `--deny-skips`: each is something the
  caller asked for, and the rule the reporting surface follows is that a caller instruction is
  never charged and always names itself. The array is omitted when empty, so a run that suppressed
  nothing keeps exactly the JSON shape consumers already parse. The MCP `lint` / `lint_fix` tools
  return the same document and inherit it. Closes
  [#21](https://github.com/Goldziher/poly/issues/21).

  ```json
  {
    "suppressed": [
      {
        "path": "crates/app/src/frb_generated.rs",
        "code": "placeholder-implementation",
        "reason": "default-path-exclusion"
      }
    ]
  }
  ```

- **`[lint.<lang>.oxc] plugins` enables an oxlint plugin.** oxlint compiles in whole plugins its
  default set (`unicorn | typescript | oxc`) leaves switched off, and rule selection cannot reach
  them: a filter resolves only against the rules of already-enabled plugins, so
  `extend_select = ["vitest/expect-expect"]` was a no-op that read like a configuration. The key
  turns the plugin on, and the rules then behave like any other. An unknown plugin name fails the
  run rather than being dropped — silently ignoring it would leave a user believing a rule set was
  live over a run that checked nothing of the kind.

  This is how poly answers "the pack has no JavaScript rules": for the case that prompted it,
  oxlint's own `expect-expect` is framework-aware and takes an `assertFunctionNames` option, which
  is exactly the escape hatch a hand-written pack rule for the same job measured 12/20 false
  positives for wanting. See `docs/pack-rule-audit.md`.

- **A hardening harness that runs poly over real third-party code.** poly's own fixtures are small,
  hand-written and chosen to exercise a known path, so they cannot answer the questions that decide
  whether a release is safe: does poly error on anything in a large tree nobody wrote for us, does
  formatting converge, does the cache ever serve a different answer than a cold run, and how many
  findings does a rule actually produce on code we did not write.

  `task harden` runs it, one process per repository so a panic in one cannot take the rest of the
  run with it. Each root appends an NDJSON record **before** its assertions run, so a failing root
  still leaves its measurement behind, and every threshold it was judged against is written into its
  own record so a result read months later is self-describing. It gates on invariants — any errored
  file, a second format pass that still changes something, a warm or `--no-cache` run that disagrees
  with the cold one, and any `no lint rules for <language>` skip for a language poly claims to lint
  — and reports per-rule finding counts per repository, which is how a pack rule earns a default
  severity.

  Three corpora, allowed to assert different things, because **an invariant holds regardless of what
  a tree contains and a count does not**. The sibling working trees next door are never written to
  (formatting runs against a disposable copy, and the cache is redirected out of the tree) and their
  counts are trend data only. Pinned third-party clones are the only corpus whose counts may gate.
  Generated-code repositories are audit input for the rule severities. Native toolchains are
  disabled in the gated run, since `rustfmt`/`gofmt`/`shellcheck` are default-on when present and
  would otherwise make the result depend on the runner image.

  Not a CI job: it clones large third-party trees, and the output that justifies the cost is the
  part that cannot fail a build by itself. `cargo test --workspace` still compiles it on every PR,
  so it cannot rot between runs. See `docs/harden-corpus.md`, which also records the procedure for
  extending the generated-code corpus — publishing the query is what makes a corpus auditable, and
  a list of repositories is not.

- **`[hooks] snapshot_include` lets a build read a file git does not track.** The staged snapshot
  holds exactly the index content, which is what makes a commit gate check the bytes a commit would
  capture — but a `workspace` hook whose build reads a gitignored input then fails under the gate
  while passing in the worktree, and the tool's error names the missing file rather than the
  isolation that removed it. Closes [#20](https://github.com/Goldziher/poly/issues/20).

  ```toml
  [hooks]
  snapshot_include = ["config/local.json", ".env.build", "vendor/fixtures"]
  ```

  Opt-in and named one path at a time, so the default stays honest. Entries must be
  repository-relative — an absolute path or a `..` segment is refused when the config loads — and an
  entry that does not exist is skipped with a warning rather than failing the gate. Every run prints
  a note naming the entries, green runs included, because an allowlist is a deliberate exception to
  what the gate claims to have checked.

  Entries are **linked**, not copied, which has one consequence worth knowing: a hook that writes to
  one writes the real file in your worktree, outside the usual write-back rules. Per-file hooks never
  receive these paths (the matched list comes from the staged set), so reaching it takes a
  `workspace` hook writing to an allowlisted path on purpose. Linking is also what keeps the entry
  correct — the snapshot manifest is pruned against the index, which never holds these paths, so a
  recorded copy would be deleted on the next refresh and an unrecorded one would go stale forever.

- **Every lint/format result now says which engines and which configuration produced it.** A poly
  version does not move when a wrapped crate does, and a `poly.toml`, `poly.local.toml`, nested
  config or `extends` base can change underneath a byte-identical binary. Two runs then both report
  clean with nothing to say they are not comparable — so a consumer memoizing results on the
  binary's identity alone is wrong the moment either moves. Closes
  [#18](https://github.com/Goldziher/poly/issues/18).

  The MCP identity block gains an `engines` digest over the compiled-in backends and the versions
  they wrap; the `version` tool returns the full map. Host-toolchain backends (`rustfmt`, `gofmt`,
  `shellcheck`, …) are deliberately excluded, because their version depends on the machine and the
  checkout rather than on the binary — folding them in would make a binary identity that changes
  when the binary did not.

  The document gains a `configs` list, one entry per configuration that governed the run, each with
  the directory it resolved from and a hash of the fully merged config. A monorepo carries several,
  and each result names the one that governed it, so a difference between sibling packages is
  attributable rather than anomalous. The hash ignores comments and formatting, and is independent
  of where the repository is checked out — the resolver rewrites `[rules] dirs` to absolute paths,
  which would otherwise have meant two identical commits never agreeing.

- **Every engine table now accepts `enabled`, and `poly lint --only` / `--skip` restrict a run to
  named engines.** Only three backends read an `enabled` key before this, each out of its own
  options table, so `[lint.python.ruff] enabled = false` was reported as an unknown key *and* ruff
  ran anyway — a setting that reads as honoured and does nothing. It is now a key every engine
  table accepts, read by the runner's plan rather than by any backend, in both the per-language
  `[lint.<lang>.<engine>]` and the language-agnostic `[lint.<engine>]` form. That makes
  "spell-check off for this one language" expressible: `[lint.typos] enabled = false` with
  `[lint.python.typos] enabled = true` re-enables it for Python alone.

  An absent key still means *the engine's own default*, which is not the same as `false` — most
  backends are on, `uncomment` is opt-in, and each native-toolchain tool carries its own — so only
  an explicit `false` withdraws an engine.

  `--only ruff` / `--skip typos` apply the same overlay for one invocation, which is what makes it
  possible to prove what a single backend did: with every engine running at once, a comment
  surviving an `uncomment` pass cannot be told apart from `uncomment` never having examined it.
  Closes [#17](https://github.com/Goldziher/poly/issues/17).

  Three properties keep the flag from becoming a way to under-check without saying so:

  - It **narrows and never widens**, so an engine the config leaves off stays off when named.
  - An **unrecognized name fails the run** (exit 2, listing what is recognized) rather than
    emptying every plan, checking nothing, and exiting 0.
  - A file left with no selected engine reports its own skip reason instead of borrowing
    `no matching engine for this file type`, which would be false — and that reason is **not**
    charged to `--deny-skips`, since the budget exists to catch coverage a run lost without saying
    so, and a restriction named in the invocation is the opposite.

  Naming engines also skips the whole-project phase, which is built from tools (`cargo clippy`,
  `cargo-deny`) rather than engines — running one cheap engine without paying for a `cargo` build
  is half of what the flag is for. The bypass reaches the phase's coverage prediction by
  construction, so the per-file tier is never told Rust was linted elsewhere by a phase that did
  not run.

## [0.23.1] - 2026-08-30

### Fixed

- **The npm packages publish again.** `npm-package/stage-binaries.sh` changed into the package
  directory before it resolved its `--artifacts` argument, so the workflow's relative
  `--artifacts artifacts` — relative to the repository root — was reinterpreted as
  `npm-package/artifacts` and the script reported `error: not a directory: artifacts`. Every step
  before it had passed: the six archives were downloaded off the release and verified against
  `sha256sums.txt`. v0.23.0 therefore shipped its GitHub release, Homebrew formula, Scoop
  manifest and PyPI wheels, and no npm package at all — `@goldziher/polylint` stayed on the
  `0.0.0` bootstrap placeholder, which is what `npm install` would have fetched.

  The argument is now resolved against the invocation directory before the `cd`, so a relative
  and an absolute path both mean what the caller meant. `pip-package/build_wheels.py` takes the
  same argument and never changes directory, which is why PyPI was unaffected.

## [0.23.0] - 2026-08-30

### Added

- **A `poly.toml` that is wrong now says so, instead of running at its defaults.** Two shapes of
  mistake were accepted in silence. A wrongly-typed value — `mccabe_max_complexity = "oops"` —
  reports `invalid-config-value` naming the key, the type it wants, the type it got, and what
  poly did instead. An unknown *top-level* section reports as `unknown-config-key`, which is the
  most complete way for a config to do nothing: a misspelled `[discovry]` drops every exclusion
  it holds, and the only symptom is poly checking more files than you expected — which reads as
  poly being wrong rather than the config being wrong. This completes the work
  `unknown-config-key` started for keys inside known tables in 0.22.0, and closes
  [#16](https://github.com/Goldziher/poly/issues/16).

  Keys whose set comes from an upstream serde type are deliberately not type-checked, since poly
  has no type of its own to quote back; where a key is both hand-declared and recognised
  upstream, upstream wins. `extends` and `exclude_mode` are exempt because both are consumed
  before the typed schema sees the table, and reporting them would fire on correct configs.

- **`poly config show` prints the effective merged config**, with `extends` bases, the
  hierarchical cascade and `poly.local.toml` all applied. It used to print a summary that named
  the section and stopped, so a discarded value was invisible and there was no way to ask poly
  what it actually parsed. The body is valid TOML, so `diff <(poly config show) poly.toml` is the
  whole workflow; resolution facts that are not config ride as leading comments, and
  `--format json` / `--format toon` carry the same data for a machine reader.

- **poly is on npm and PyPI again, as `@goldziher/polylint` and `polylint`.** They were dropped
  in v0.9.0 on the reasoning that the installer, the Action and Homebrew covered everyone; that
  turned out to be wrong for the two ecosystems where a linter is a project dependency rather
  than a machine-level tool. A JavaScript repo pins its formatter in `package.json` and a Python
  repo pins it in its lockfile, and neither can express "run this `curl | sh` on every developer
  machine and in CI". The names differ from the command because unscoped `poly` is taken on both
  registries — but the executable is `poly` on every channel, with `polylint` installed
  alongside as an alias for the same binary so that whichever name you installed under, both
  work.

  Neither package downloads anything when you install it. npm ships six per-platform packages
  gated on `os`/`cpu`/`libc` and pulled in through `optionalDependencies`; PyPI ships
  per-platform wheels carrying the binary as package data. That is a deliberate move away from
  the `postinstall`-downloader the old npm package used: installs now work offline and behind a
  proxy, the binary is covered by the registry's own integrity hashes, and nothing runs a script
  at install time. The glibc/musl split matters and is handled on both sides — the musl build is
  dynamically linked against musl and will not start on glibc, so npm gates it with `libc` (and
  the launcher re-checks at runtime, for package managers that ignore the field) and PyPI tags it
  `musllinux_1_2` against `manylinux` for the others. The manylinux minor version is read out of
  the ELF's own `GLIBC_2.x` references rather than hardcoded from the builder image, because a
  tag claiming an older glibc than the binary needs installs happily and then dies on `exec`.

  On a platform with no published binary, both wrappers say so and point at the source build
  instead of failing with a stack trace: npm's launcher names the platform packages it looked
  for, and PyPI publishes a `py3-none-any` fallback wheel that pip only ever resolves when no
  platform wheel matches.

- **Scoop is a distribution channel on Windows** — `scoop bucket add goldziher
  https://github.com/Goldziher/scoop-bucket && scoop install poly`. The manifest points at the
  same `x86_64-pc-windows-msvc` archive the installer uses and carries its hash straight from the
  release's `sha256sums.txt`, so the bucket and the published archive cannot drift.

- **`poly lint` and `poly fmt` take `-q` / `--quiet`.** Verbosity only ever went up: a run over
  Kubernetes printed its headline and then twenty-nine lines of discovery and skip detail on top
  of it, so the last thing on screen was what poly declined to check rather than the 21,657 files
  it checked in twelve seconds. `--quiet` drops the itemised notes and nothing else — every count
  and every reason stays in the summary, because a skip poly stops reporting is the failure the
  notes exist to prevent. Findings still print (it is not `--silent`), exit codes are unchanged,
  and the `--format json` / `toon` document is untouched. It conflicts with `--verbose` and
  `--debug` at parse time: asking for less and more at once is a mistake worth catching.

- **A progress indicator on runs long enough to look like a hang.** `poly lint` over a large
  repository produced no output at all until it finished. A spinner now names the phase and the
  elapsed time, appearing only after 400ms so short runs stay silent. It draws on **stderr and
  only when stderr is a terminal**, so a pipe, a file and a CI log see nothing; it never touches
  stdout, so `--format json` is byte-identical with and without a terminal; it never hides the
  cursor, which makes a Ctrl-C that leaves a terminal without one unreachable rather than merely
  unlikely; and the worker threads are not told it exists, so the per-file hot path is unchanged.

### Changed

- **The human report ends with its verdict.** Findings, then the files that failed, then the
  qualification notes, then the summary — the earlier order printed the headline and buried it
  under the notes. A reader's eye lands on the bottom of the output, which is now where the
  answer is.

- **Counts are grouped and nouns agree with them.** `41576 issue(s) found` reads as machine
  output and makes the reader do both the digit-grouping and the grammar; it is now `41,576
  issues found`, `1 file linted`, `6 directories skipped`. The summary's qualification moved from
  a 380-character parenthesis onto its own indented lines, wrapped to the terminal at item
  boundaries — an 80-column window used to shred it mid-path. The `pass --verbose to list them`
  hint is said once at the end of the skip block instead of once per reason, where it was
  printed twelve times in a single run.

- **One palette, named by meaning.** Every surface reached for `.red()` / `.yellow()` inline, so
  "what colour is a skip?" had as many answers as call sites. `poly_core::report::theme` now
  defines the semantic set — severity, success, failure, skipped, secondary, path, code, heading
  — and the report, `doctor`, `cache` and the hook summaries draw from it. Still the basic 16
  ANSI colours (they respect the user's own terminal theme rather than assuming a background),
  still through `if_supports_color`, and colour is never the only carrier of meaning: a
  `--no-color` run loses decoration and nothing else. The severity mappings themselves are
  unchanged.

- **`--debug` now implies `--verbose`.** `Verbosity` became an ordered level —
  `Quiet < Normal < Verbose < Debug` — rather than two independent booleans, so each step is a
  superset of the one below and "less" is expressible at all. `Verbosity::new(verbose, debug)`
  still exists and still means what it did.

- **CI runs poly against poly, on Linux and Windows.** The repository dogfooded only through
  local git hooks, so anything landing via `--no-verify` or from a fork was never checked by the
  tool it ships. The new job is the only one that exercises discovery, the engine registry and
  the report path end to end on a real polyglot tree rather than on fixtures. Windows is in the
  matrix deliberately: path handling, line endings and the ignore walk are where a
  cross-platform linter actually breaks, and the existing Windows job proved only that poly
  compiles and its tests pass. The test job also gained `--no-fail-fast`, which this project's
  own rules call not optional — cargo otherwise stops at the first failing test binary and the
  rest are rediscovered one CI round at a time.

- **The publish workflow packages the binaries it already released, rather than building its
  own.** The npm, PyPI and Scoop jobs download the archives back off the GitHub release and
  verify them against `sha256sums.txt` before packaging. A second build would almost always
  produce equivalent output and occasionally not, and the failure would be a package whose
  contents no published checksum describes.

  All three jobs run after the release is live and are `continue-on-error`, with nothing
  depending on them: a registry whose trusted publisher is missing or misconfigured shows up as
  a red job on a green release run, and cannot put the release back into draft. Both registries
  authenticate by OIDC — there is no npm or PyPI token in the workflow.

- **`scripts/release-bump.sh` moves the npm and PyPI manifests with everything else**, and
  asserts each substitution landed rather than trusting `sed` and `jq` to have matched. The npm
  umbrella package carries the version in seven places — its own, plus one exact pin per platform
  package — and a bump that moved six of them would publish a package that installs cleanly and
  resolves the previous release's binary. It also checks that the pinned package names and the
  `platforms/` directories describe the same set, so a target added to one and not the other
  cannot silently drop a platform.

### Fixed

- **`.pypirc` is detected as INI.** It was mapped as an *extension*, but a leading-dot name with
  no second dot has no extension, so the entry only ever matched a file literally named
  `something.pypirc` — a shape nobody writes — while the `.pypirc` that exists in the wild
  reached no INI detection at all. It now sits in the filename table beside `.npmrc` and
  `.editorconfig`, where the other dotfile INI names already were.

- **`lazy-ignore` no longer fires on prose that documents a suppression marker.** The rule scans
  for `# noqa`, `// eslint-disable` and friends written without a justification, which is right
  in source and wrong in Markdown: there, every occurrence is prose explaining the syntax, a
  table cell naming it, or a fenced example showing it — never a directive, because nothing lints
  the prose it sits in. poly's own documentation exposed it. `docs/CONFIGURATION.md` describes
  what this rule does, and `poly lint` reported that description as a lazy ignore; a rule that
  reports the sentence explaining it has outrun its evidence.

  Deliberately a language check rather than a Markdown-aware parse of code spans and fences: a
  suppression inside a fenced block is also an example, so parsing to find it would only make the
  rule fire on the cases it most clearly should not. Source files are untouched — an unjustified
  `# noqa` in Python is still reported and a justified one still is not, both pinned by tests.

## [0.22.0] - 2026-08-30

### Changed

- **Dependencies upgraded across the board.** `oxc` and `biome` move to current upstream revisions,
  `uncomment` to 3.6, and `rumdl` (0.2.62), `tree-sitter-language-pack` (1.15.12), `mago` (1.47.4)
  and `typos-dict` (0.14.1) move with the lockfile. Every affected engine's `version()` moved with
  its dependency, so no warm cache can serve output computed by the previous parser — the audit
  test that enforces this caught five engines that would otherwise have gone stale. The `oxc`,
  `biome` and `rubyfmt` git pins were re-checked against crates.io and all three remain necessary:
  `oxc_linter`, `oxc_formatter_core`, `oxc_formatter_json` and `biome_graphql_parser` are still
  unpublished, crates.io `oxc_formatter` is a `0.0.0` name reservation from 2023, `biome_analyze`
  there is 2024 code under the version number of our 2026 pin, and `rubyfmt` is `0.0.0-wip`.
  GitHub Actions were updated too (`checkout` v7, `upload-artifact` v7, `download-artifact` v8).

### Fixed

- **A stylesheet of IE `*property` hacks no longer hangs the run.** `biome_css`'s parser recovers
  from the hack in time exponential in how many the file contains — ten cost 0.46 s and twelve
  cost 8.88 s, roughly fourfold per additional hack. YUI's `reset-fonts-grids.css`, vendored into
  thousands of projects and into Django's own documentation theme, carries 37: `poly lint` on
  Django grew to **27 GB** and was OOM-killed after five minutes having reported nothing at all.
  The whole run died, not just the file. Both CSS backends now decline a stylesheet carrying the
  hack — malva rejected it with a syntax error anyway — and Django lints in 2.4 s using 0.2 GB.

  The check requires the `*` to sit in declaration position, so the universal selector (`* {…}`,
  `.a > *`) is untouched; across 487 stylesheets in the test corpus it matches two files, both of
  which genuinely carry the hack.

- **Binary files are skipped instead of reported as broken text.** poly discovers files by path,
  and a path is a weak claim about content, so a compiled artifact reached the runner like any
  other file and the first thing done with it was to decode it as UTF-8. That decode fails, and
  the failure was reported as *malformed text* — an error-severity `invalid-utf8` finding from
  `poly lint`, a per-file error from `poly fmt`. Django's 1,263 compiled `.mo` catalogs therefore
  made `poly fmt --check` exit 2 on a repository with nothing wrong in it, and no exclude could
  fix it without also excluding real files. Content carrying a NUL byte is now skipped before the
  decode is attempted.

  Malformed *text* still errors, and deliberately so: a source file that lost a byte is a defect
  a reader must see. The carve-out requires a NUL byte rather than merely invalid UTF-8, so
  Latin-1 source and truncated sequences keep reporting. Skipped rather than dropped, so the file
  stays out of the `checked` count, appears in the JSON `skipped` payload, and remains visible to
  `--deny-skips`.

- **A backend panic no longer takes the whole run down.** poly compiles roughly twenty
  third-party parsers in-process; their APIs return `Result` but their internals assert, and
  engines run inside a rayon `par_iter`, which propagates a worker panic to the process. One
  file could therefore destroy an entire run: `biome`'s GraphQL parser asserts on a *fragment
  description* — a real language feature, not malformed input — and a single three-line
  `.graphql` file made `poly lint` over a 63 MB repository produce a **zero-byte report** and
  exit 101. Not a partial report; nothing at all, including every finding from the other 20,000
  files. A panic is now contained per file and reported as the ordinary "poly failed on this
  file" error that already existed, so the run continues and exits 2. The same repository now
  reports 19,399 findings.

- **Each native tool's own config file is part of the cache key.** `rustfmt` reads
  `rustfmt.toml`, `shellcheck` reads `.shellcheckrc` and `swift-format` reads `.swift-format`,
  none of which were hashed. Editing one and re-running served the previous run's output —
  formatting at `max_width = 120`, changing it to `40`, and running again reported "All
  formatted" and left the file unchanged. Silent wrong output, not a missed invalidation.

- **The MCP surface honours `[discovery] force_exclude`.** It was hardcoded, so one `poly.toml`
  answered differently depending on whether the CLI or `poly mcp` asked.

- **`quality/lazy-ignore` no longer scans Rust `#[allow(..)]`.** It was poly's loudest rule at
  10,486 corpus findings, roughly 95% of them allow-attributes, and it was wrong on both counts.
  A line-oriented scan can only read text after the closing `)]`, so it flagged
  `#[allow(dead_code, reason = "...")]` — the form the language itself sanctions since Rust 1.81
  — and an explaining `//` comment on the line above. It also duplicated the built-in
  `allow-attribute-without-reason` pack rule, which parses the construct properly and ships
  `off` after a hand read of its 13,254 findings found FFI bindings and macro-generated glue
  rather than drive-by lint muting. Opt in with
  `extend_select = ["allow-attribute-without-reason"]`.

- **Every quality threshold rule names its ceiling.** `nesting-too-deep` said "(max exceeded)"
  and `cyclomatic-complexity` said nothing at all, leaving a reader who disagreed with a finding
  unable to tell what number to change in `poly.toml`.

- **`extend_select` now extends instead of replacing** in the rumdl (Markdown), INI and
  Dockerfile backends. These three treated it as an allow-list, so
  `extend_select = ["MD013"]` silently disabled every other rule — the opposite of ADR 0016,
  of the documented vocabulary, and of what ruff, mago and dotenv already did.

  **Breaking for anyone relying on the old behaviour:** a config using `extend_select` under
  `[lint.markdown.rumdl]`, `[lint.ini]` or `[lint.dockerfile]` will now also report findings
  from the other default rules, which can fail a previously passing run. Rename the key to
  `select` to keep the replace semantics. Cached results are invalidated automatically.

- **Per-rule tool parameters under `[rules.<id>]` are honoured.** ADR 0016 promised that any key
  other than `level` is passed to the backend as a tool parameter; no backend read them. Now
  wired for oxlint (`[lint.javascript.oxc.rules.max-params] max = 6`), rumdl and sqruff. This
  also fixes a data-loss path: `poly migrate` emits per-rule parameters when importing a
  markdownlint config, and every one of them was being discarded.

- **Documented formatter keys that did nothing now work.** `[fmt.python.ruff]`
  `docstring_code_format`, `docstring_code_line_length`, `line_length` and `indent_width` were
  documented but never read. The malva, markup_fmt, GraphQL and YAML backends clobbered a
  user-set `print_width` / `line_break` with the global default; user config now wins.

- **Six more per-language `[lint.<lang>.typos]` keys are honoured** — `extend_exclude`,
  `extend_words`, `extend_identifiers` and the three `extend_ignore_*_re` patterns. Only
  `extend_ignore_words` was read before.

- **PHP maintainability metrics no longer fail CI.** mago reported six threshold rules
  (`cyclomatic-complexity`, `excessive-parameter-list`, `too-many-methods`,
  `too-many-properties`, `too-many-enum-cases`, `kan-defect`) at error severity; they are now
  warnings, while mago's correctness, safety and security rules keep error severity.

- **`eslint/max-classes-per-file` is off by default.** Widening the oxlint categories enabled it
  as a member of `pedantic`; its default of one class per file is a position poly holds nowhere
  else.

- **A malformed sqruff rule name can no longer panic the run.** Per-rule config section names and
  keys are interpolated into an INI document that `FluffConfig::from_source` panics on if
  malformed; unusable names are now dropped with a warning.

### Added

- **Code-quality metrics for twelve languages that had no lint rules at all.** A new
  cross-cutting `quality` tier measures function, type and file length, parameter count,
  nesting depth and cyclomatic complexity straight off the tree-sitter parse, and scans for
  ignore markers written without a reason. Findings are warnings, so nothing new fails CI.
  Covers Python, Rust, Go, JavaScript, TypeScript, TSX, Java, Kotlin, C, C++, C# and Ruby;
  `magic-number` and `law-of-demeter` ship off and are opt-in. Each rule defers to a tier-1
  backend that already reports the same thing, so Python keeps ruff's `C901` rather than
  gaining a second complexity finding. See ADR 0027.

  A language the tier can measure now leaves the `no lint rules for <language>` skip and
  counts toward `N file(s) linted`. Languages it cannot model structurally — Zig, Swift,
  Dart, Gleam, Elixir, PHP, Nix, Scala, Lua, R — keep the skip: a line count is not
  knowledge of the language.

- **`poly rules list` now shows the built-in rule pack.** It previously enumerated only rules
  loaded from `[rules] dirs`, so the 13 rules poly ships enabled were invisible to the one command
  that lists rules — a user who hit a `force-cast` warning had no way to find where it came from.
  The listing now covers built-in and user rules together, showing each rule's source, its
  effective severity, and the default its author shipped. It is computed from the same resolution
  path `poly lint` uses, so it cannot drift from what actually runs, and it reflects
  `[rules] builtin`, `select` / `extend_select` / `ignore` and per-rule `level`. `--format json`
  and `--format toon` carry the same fields, as does the MCP `rules` tool.

- **`--force-exclude` and `[discovery] force_exclude` now work.** Both were parsed and never
  read. Explicitly naming a path that matches an exclusion now honours the exclusion by default,
  `[discovery] force_exclude = false` genuinely turns that off, and `--force-exclude` overrides
  the config key. `--include-excluded` still wins over both. No behaviour changes for a repository
  that sets neither.

- **Unknown configuration keys are now reported.** `[lint.*]` and `[fmt.*]` accepted any key at
  all, so a typo — or a key a backend never supported — parsed silently and did nothing. This was
  the root cause of a family of defects fixed earlier in this release, where documented keys had
  no reader. Each backend now declares the keys it reads, and `poly lint` warns on anything else
  in that backend's table, naming the key and listing what the table does accept. Where possible
  the declaration is derived from the upstream option type rather than hand-maintained, so it
  cannot drift; the rest is held in place by tests asserting that every key read is declared and
  every key declared is read.

  Deliberately not reported, to avoid false positives: the contents of `[rules.<id>]` sub-tables
  (arbitrary tool parameters by design), unknown language ids, and unknown tool names, which may
  come from an `extends` base. `[hooks]` and `[discovery]` are not covered yet.

- **`shellcheck` now runs by default when it is installed.** Shell was poly's largest remaining
  coverage gap — 528 files across the test corpus reported `no lint rules for Shell` while
  shellcheck sat on the same machine and poly held a working backend for it. Opt out with
  `[lint.shell.shellcheck] enabled = false`; with shellcheck absent, Shell falls through to the
  generic tier exactly as before.

  **This is the first default that can fail a build**, because shellcheck's `error` level maps to
  error severity and `poly lint` exits non-zero on it. Measured across 48 repositories: it adds
  1,215 findings, of which three are errors. 1,118 of those findings are a single true-positive
  pattern — `SC2188`, a redirection with no command — concentrated in one repository's generated
  test stubs, whose function bodies are a bare `>/dev/null`, so the tests do nothing. Excluding
  that repository, the other 47 contribute 97 findings between them. The three errors are a
  malformed `# shellcheck` directive, a second diagnostic on the same line, and a missing shebang
  in a husky-generated hook. `shfmt` stays opt-in. See ADR 0014.

- **A built-in ast-grep rule pack, on by default.** poly's ast-grep backend shipped with no
  rules at all — it matched a language only if a repository pointed `[rules] dirs` at YAML of its
  own, which nearly none do. It now embeds 26 curated rules across nine languages (C#, Elixir, Go,
  Java, Kotlin, Python, Ruby, Rust, Swift), parsed through the same path as user rules and layered
  beneath them, so a user rule sharing an `id` still wins. Thirteen ship enabled; the rest ship
  `off` and are opt-in via `extend_select`. Disable the pack wholesale with `[rules] builtin =
  false`. See ADR 0029.

  Swift, Kotlin, Elixir and C# gain real lint coverage for the first time and leave the
  `no lint rules for <language>` skip. Every rule reports at warning severity, so no CI turns red.

- **Python analysis now builds from the published ruff crates** rather than a pinned git commit
  (`ruff_linter = "=0.16.5"`, the supporting crates `"=0.0.11"`). Astral began publishing the ruff
  workspace to crates.io in June 2026. Exact `=` pins are deliberate — these are private-by-intent
  internals that are not semvered, so a caret range would let `cargo update` walk into unannounced
  breakage. No Python diagnostic or formatting output changed; the cache invalidates once because
  the engine version string moved with the dependency. `oxc`, `biome` and `rubyfmt` remain git
  pins for the reasons recorded in ADR 0003's amendment.

- **Inline suppression directives.** `// poly: allow[RULE] reason` and
  `// poly: allow-file[RULE] reason`, written in the host language's comment syntax and applied
  centrally so every backend honours them. A reason is mandatory: a directive without one does
  not suppress and reports `lazy-ignore` instead. See ADR 0028.

- **`eslint/complexity` is enabled by default** for JavaScript and TypeScript. Its own default
  threshold is 20 — the same cyclomatic-complexity budget poly applies elsewhere.

- **`[discovery] generated` aligns how machine-generated files are treated.** poly handled them
  three different ways: `lint` reported on them, `fmt` skipped only files carrying a content-hash
  stamp, and `lint --fix` withheld on any "DO NOT EDIT" banner. `lint --fix` now matches `fmt`, so
  a banner-only generated file is fixed like any other, and one key opts out of all three at once.
  An opted-out file is reported as *skipped*, not silently dropped, so it stays out of the
  `checked` count and remains visible to `--deny-skips`.

  The hash-stamp guard is unchanged and deliberately so: reformatting a hash-stamped body
  invalidates the hash, the generator's own verify step then reports drift on a file nobody
  touched, and the remedy is a regeneration that discards the formatting. `--fix-generated` keeps
  working and now governs exactly that case; `--skip-generated` / `--include-generated` are new.

- **Markdown fixes that rewrite document structure are applied.** `MD001` (heading increment) was
  withheld from both fix paths on the grounds that a formatter must not change a document's
  outline. The exemption did not hold: `MD025` rewrites the outline far more aggressively and was
  always applied — on a real changelog `poly fmt --fix` demoted 21 of 22 top-level headings — so
  poly declined a one-line repair while performing a wholesale one. Markdown autofix is now
  uniform. The `MD020` guard stays, because it prevents *corruption* rather than restructuring:
  `### C#` is a valid open ATX heading under CommonMark and rumdl's fix silently drops the `#`.

## [0.21.12] - 2026-08-29

### Added

- **The cache home now carries a `CACHEDIR.TAG`.** poly's cache — result entries, hook staged
  snapshots and fetched remote sources — is entirely derived and regenerated on demand, but
  nothing on disk said so, and on one machine it had grown to 21 GB that every backup dutifully
  copied. The [cache directory tag](https://bford.info/cachedir/) is the cross-tool way to
  declare that: `tar --exclude-caching`, `rsync --exclude-tag`, Borg, restic and cleanup tools
  such as voom all skip a tree that carries one, without needing to know anything about poly.
  Written once, when the cache directory is created, and never rewritten — once the file is on
  your disk it is yours to edit.

  **Only when poly owns the directory.** A `POLY_CACHE_HOME` you pointed somewhere of your own
  may hold more than poly's cache, and a tag there would tell every backup tool on the machine
  to skip whatever else is in it. That is the same distinction poly already draws before
  tightening permissions.

## [0.21.11] - 2026-08-27

### Fixed

- **Directories skipped by the built-in prune set are now reported instead of vanishing.** Discovery
  prunes any directory named `build`, `dist`, `deps`, `vendor`, `target` and the like at any depth,
  and said nothing about it — so a tracked `src/.../commands/build` holding first-party source was
  dropped from every walk while the run reported a clean pass, and naming the same file explicitly
  still reported findings. The summary now names what the built-in set removed, alongside the
  exclusions it already reported. `.git` is pruned but not reported; the count stays out of the
  "nothing was checked" headline, so a repository with a `node_modules` reads exactly as before.
- **`[discovery] no_prune` keeps a directory the built-in set would have pruned**, for a repository
  where `build` or `dist` is ordinary source rather than build output. Bare directory names, not
  globs; read from the run's root config. Built-in defaults are unchanged.
- **A hook-source provisioning failure no longer hard-blocks `git commit`.** `prepare-commit-msg`
  exits non-zero on such a failure and git offers no `--no-verify` for that hook, so a commit could
  not be made at all. It now warns and continues; every other stage still treats the failure as
  fatal, so a `pre-commit` gate is never silently weakened.
- **`poly.local.toml` is found from the main worktree.** It is gitignored by design, so it never
  exists in a freshly created linked worktree — while poly's hooks fire there, because they live in
  the shared `.git/hooks`. Provisioning searched only the worktree root and failed, taking
  `poly hooks install` with it, so the shims could not even be reinstalled to escape.
- **`POLY_SKIP_HOOKS=1` skips every poly hook for one invocation**, a poly-level escape that does
  not require disabling every hook in the repository via `core.hooksPath`.

### Changed

- Refreshed the pinned Biome, OXC, and Ruff revisions to their current upstream heads (OXC brings
  formatter 0.65.0 and parser 0.147.0). Engine cache identities track the new versions. The Ruff
  identity had been recording a revision that never existed upstream — each bump replaced only its
  leading characters — and now carries the short revision the other engines use.

## [0.21.10] - 2026-08-25

### Performance

- **Whole-repository cache misses no longer issue one failed filesystem read per file and engine.**
  The result cache inventories persisted keys once at open and resolves absent keys in memory, while
  concurrent session writes remain visible without serializing rayon readers.
- Results from engines that finish in under 5 ms are recomputed instead of paying serialization and
  atomic-write costs. Disabled caches now skip content digests and cache-key construction entirely.
- Slow single-file backend runs now emit structured timing events to identify pathological inputs,
  and Criterion coverage tracks hit, miss, post-write contention, and index-open costs.

## [0.21.9] - 2026-08-25

### Fixed

- **Kotlin Elvis-expression method chains keep their continuation indentation.** The generic
  formatter now preserves intentional Kotlin alignment that cannot be inferred from brace depth,
  preventing generated Gradle scripts from being de-indented on each format run.

### Changed

- Updated compatible and incompatible Rust dependencies and refreshed the pinned Biome, OXC, and
  Ruff revisions to their current upstream heads. Engine cache identities track the new versions.

## [0.21.8] - 2026-08-23

### Fixed

- **Malformed embedded JavaScript no longer aborts an Astro format run.** OXC parse failures now
  leave the source unchanged, matching the existing handling for malformed markup, while unexpected
  embedded-printer failures still fail the run instead of being reported as clean.
- **Invalid UTF-8 now produces contextual lint and format results.** Lint emits a structured
  diagnostic and explicit skip; format names the invalid byte offset and records that formatting was
  skipped. Neither path mutates undecodable input or collapses it into an unexplained I/O failure.

### Changed

- Updated the pinned Biome and OXC revisions to their current upstream heads and upgraded
  tree-sitter-language-pack to 1.15.7. Engine cache identities track each new parser/tool version.

## [0.21.7] - 2026-08-22

### Fixed

- **Astro formatting is idempotent for nested expressions.** Embedded JavaScript and TypeScript
  now pass through OXC instead of markup_fmt's unstable no-op path. Non-JavaScript blocks such as
  `<style>` remain untouched by that callback.
- **Malformed markup and HCL no longer abort repository-wide format checks.** Syntax-invalid files
  are left unchanged, and HCL output is reparsed before it can replace valid source. This also
  prevents hcl-rs from emitting invalid shell-command string escapes.

### Changed

- Updated compatible and incompatible Rust dependencies, including the pinned Biome, OXC, and Ruff
  revisions. Engine cache identities track the new versions and OXC's direct Astro dependency.

## [0.21.6] - 2026-08-16

**No functional change.** Every line changed since 0.21.5 sits inside a `#[cfg(test)]` module, and
no dependency moved — the lockfile diff is ten lines, all of them this workspace's own version. This
release exists so that a tagged commit has a green CI run behind it: the fixes below landed after
`v0.21.5` was published, and a published tag is not worth force-moving.

### Fixed

- **The test suite no longer asserts the developer's platform.** Five tests had been failing on the
  Linux and Windows runners, some since before 0.21.4, and each was hiding the next: CI runs without
  `--no-fail-fast`, so the first failure in a test binary stopped everything behind it. None was a
  product defect — in every case poly behaved correctly on the platform and the test encoded an
  assumption that only held on macOS.

  - A probe test used `/usr/bin/true` as "a binary that prints nothing", but GNU coreutils'
    `true --version` prints a version; it now uses a temp script, as its two sibling tests already
    did.
  - A `PATH`-shadowing test asserted a mode-644 file is not an install. Windows has no execute bit,
    and `is_executable_file` documents that explicitly for non-unix, so the assertion contradicted
    correct behaviour. Split: the directory and missing-file cases stay cross-platform.
  - A remote-checkout test compared bytes against an LF spelling; git's `core.autocrlf` is on by
    default on Windows, and the test is about which revision was checked out, not how git spells a
    newline.
  - A catalog-linter test required `shellcheck` to be installed. The half that asserts *silence*
    stays unconditional and keeps its force without the binary, because the refusal is decided from
    the resolved argv before the executable is probed; the half that asserts the engine ran now runs
    where the tool exists and prints an explicit skip otherwise.
  - A hook-shell test asserted the Unix `"$@"` append unconditionally. `cmd.exe` has no positional
    parameters, so poly appends the quoted file paths there instead — the decision to append is
    shared between platforms, only its spelling differs, and Windows now has its own assertion for
    that spelling rather than losing the coverage.

## [0.21.5] - 2026-08-15

### Added

- **A path-scoped `poly lint --workspace` now says the whole-project phase is repository-wide.**
  `--workspace` opts a path-scoped run back into the phase, and the phase then ran the configured
  whole-project tools over the entire repository — it passes no file list, and `[discovery]
  exclude` filters poly's own discovery, not what such a tool reads. Nothing said so, so findings
  from outside the named paths read as a dropped filter. The run now prints
  `note: whole-project phase covers the entire repository, not just the named paths (and not
  filtered by [discovery] exclude)` on stderr, mirroring the note the default path-scoped skip
  already emits. Behaviour is unchanged; only the silence is.

### Fixed

- **An enabled `[tools.<name>]` linter that poly refuses to run now says so.** A catalog tool whose
  lint command rewrites files is not wired as a linter — correctly, since a fix command would
  overwrite the source and still exit 0 — but the refusal was silent. The user asked for that
  linter, poly ran nothing for it, and the run reported success: a green result standing in for work
  never done, which is the same false pass the refusal was added to close.

  `poly lint` now logs a warning, once per tool and language, naming the tool, quoting the mutating
  command it declined to run, and pointing at `poly fmt`, which still runs the tool as a formatter.
  Both refusal reasons are covered: a mutating flag or subcommand in the resolved argv (`sqruff fix`,
  `rubocop --autocorrect`, and the eleven other shipped tools), and a tool with no read-only mode at
  all (`pyupgrade`), which is refused by name. Which tools are refused, and how an argv is
  classified, are unchanged.

- **Documented that plain `poly lint` is not read-only.** poly applies no fixes without `--fix` —
  the per-file tier writes only under `--fix`, and the whole-project phase is asked for check
  mode — but that phase *executes* the configured whole-project tools against the live worktree,
  and their own side effects are outside poly's control: `cargo clippy` populates `target/` and
  can refresh `Cargo.lock`, a `go` invocation can append to `go.work.sum`, a type checker writes
  its cache. A consumer measured exactly that (a plain `poly lint` left `go.work.sum` a line
  longer) against docs, `--help`, and an MCP tool description that all said `poly lint` never
  writes. The README, `poly lint --help`, the `workspace_lint` MCP tool description, ADR 0019, and
  the bundled skills/commands now state it, and name `--no-workspace` / `[lint] workspace = false`
  as the way to get a run that cannot touch the tree.

  The one machine-readable claim is corrected too: the `workspace_lint` MCP tool was annotated
  `readOnlyHint: true`, and clients gate auto-approval on that hint rather than on the description
  beside it — so the single case the hint exists to catch, a tree that changes without a prompt, was
  the case it waved through. It is now annotated as side-effecting: not read-only, still not
  destructive, since poly deletes nothing and writes no output of its own there. **MCP clients that
  auto-approve read-only tools will now prompt for `workspace_lint`.** That is the only behaviour
  change in this entry; the CLI is documentation-only.

- **Exclusion notes name the glob you wrote, not the one poly matched with.** Discovery re-anchors
  an exclude glob to the walk root — a repo-root `packages/dart/**` becomes `**` for a run rooted
  inside `packages/dart`, and a nested config's `gen/**` becomes `packages/gen/**` for a run rooted
  above it — and the report was quoting that rewritten form. `excluded by [discovery] exclude /
  --exclude: **` reads as though everything is excluded, and sends the reader looking for a rule
  that appears in no config file.

  Each rule now carries its authored text alongside the matcher glob, and the note (and `poly
  doctor`'s over-broad-exclude finding, with its `/`-anchoring remedy) quotes the authored text.
  Nothing about which files are excluded changed: matching still uses the re-anchored glob.

- **A nested `poly.toml` now governs its files however the run was invoked.** Hierarchical
  resolution (ADR 0018) discovered nested configs by walking *down* from the run root, so a run
  rooted at or below a sub-project never found that sub-project's own config: `poly fmt .` honored
  `packages/app/poly.toml`, while `poly fmt packages/app`, `poly fmt packages/app/src`, and `poly
  fmt packages/app/src/x.py` all silently fell back to the repo-root config.

  The explicit-path case is the damaging one, because the pre-commit hook path is *always* handed
  explicit staged paths (`poly lint --no-workspace --force-exclude <files>`). A repo whose nested
  config excluded generated output watched `poly fmt --check .` honor the exclusion and then had
  those same files rewritten at commit time. The same gap made a sub-project's `[defaults]` and
  `[per-file-ignores]` inert on that path, so the hook and the whole-repo run disagreed about both
  formatting and findings.

  The config set now also registers the chain of configs *above* each run root, so the governing
  config is a property of the file rather than of the invocation.

  Two consequences worth stating. A run rooted inside a sub-project now applies that sub-project's
  rules — its `line_length`, its `[per-file-ignores]`, its engine config — where it previously
  applied the repo root's; this is the ADR 0018 behavior, and it means such a run agrees with `poly
  lint .` instead of contradicting it, but formatting and findings for those paths can change. And
  a nested `[discovery] exclude` now prunes files named explicitly on the command line, so a path
  poly used to check may now be reported as excluded rather than checked
  (`--include-excluded` overrides this).

- **A Homebrew-installed `poly` now reports its real build identity, and shares the result cache
  again.** Homebrew builds from the GitHub source tarball, which carries no `.git`, so `build.rs`
  could derive no id from `git describe` and the binary reported `unknown build unknown`. The
  channel is folded into the cache identity, and only a `release` channel gets the
  machine-independent `release/<version>` key — so every Homebrew install fell back to a
  per-binary identity, shared cached results with nothing, and re-checked every file after each
  upgrade. The formula now sets `POLY_BUILD_ID`, which is exactly what `build.rs` documents the
  variable for.

  The formula's own smoke test asserted only that the version *number* appeared somewhere in
  `poly --version`; that substring matched the broken output too, which is why this shipped
  unnoticed. It now asserts the whole version line.

  The release workflow stamps the same variable rather than leaving `git describe` to infer it.
  The release force-moves the `v0` major tag onto the release commit, so a `workflow_dispatch`
  re-run — the one path that rebuilds — could have `git describe` resolve `v0`, whose stripped
  form is `0` rather than the version, silently stamping a release artifact as a `dev` build.

- **Content-hash stamps are now found below a leading licence or build preamble.** The stamp scan
  was capped at the same five lines as the generated banner, but generators place the stamp
  directly beneath their header and accept that header anywhere in the first ten lines. Any
  generated file carrying an SPDX block, copyright notice, or build-tag stanza ahead of the banner
  was therefore stamped by its generator and invisible to poly, and `fmt --fix` reformatted it —
  the generator's verify step then reported drift on a file no human had touched.

  The scan now reaches eleven lines instead of five — the depth at which a generator can both write
  a stamp and read it back, so poly protects exactly the set the generator can verify and no more.
  As before, both windows are counted from the end of a leading YAML frontmatter block rather than
  from the start of the file, so a frontmattered document gets the window on top of its
  frontmatter. Only the structured `<project>:hash:<digest>` shape gets the wider window; the
  generated banners and the bare-word `sourcehash` / `@checksum` markers keep the five-line one,
  because those are ordinary English and a false match there drops a hand-written file out of the
  format gate entirely.

  One consequence worth stating: a line that *begins* with `<project>:hash:<digest>` is treated as
  a stamp, and that now holds through the tenth line rather than the fifth. A hand-written document
  whose sixth-to-tenth line starts with such a token will be skipped where it previously was not.
  Prose that mentions a stamp mid-sentence is unaffected — any space before `:hash:` disqualifies
  the line — as is a marker with no hex digest after it.

- **A content-hash stamp now requires a real digest length and a terminated line.** The structured
  `<project>:hash:<digest>` marker accepted any run of eight or more hex characters, so tokens of 8,
  63 or 65 characters — lengths no hash function produces — counted as stamps, as did a token
  wrapped in markdown backticks. Each silently dropped a hand-written file out of the format gate,
  and the wider header window above doubled the depth over which that applied. The digest must now
  be a canonical hash length (32, 40, 56, 64, 96 or 128 hex characters) and be followed only by
  whitespace or a comment terminator.

  The check is deliberately not pinned to 64 characters: the marker is generator-agnostic, and
  requiring one generator's digest size would drop every sha1- or md5-stamped generator out of the
  gate instead.

  For the same reason the whole stamp — marker keyword, project name and digest alike — is matched
  **case-insensitively**, so an uppercase digest is a valid stamp. A survey of a 1265-stamp corpus
  found every digest lowercase, so no generator in use today depends on this; it is accepted because
  hex is case-insensitive by definition and rejecting it would restrict producers for no benefit.
  Case folding happens before the length and terminator checks, so an uppercase token gets no
  latitude a lowercase one would not.

- **A hash token inside a code block is documentation, not a stamp.** A file that *documents* the
  stamp format — writing `<project>:hash:<digest>` inside a fenced or indented code block — was
  classified as generated and skipped in its entirety, so nothing in it was ever formatted. Inside a
  block the token is genuinely bare and line-terminated, so unlike the inline-backtick case nothing
  about the line itself gave it away.

  Fenced code blocks are now excluded in every shape a consumer could measure: backtick and tilde
  fences, either delimiter carrying an info string, fences longer than three characters, fences
  indented or nested inside a list item or a blockquote, and a fence opening on the first line. Fence
  state is tracked across the whole scanned header, so suppression is scoped to the block — a stamp
  above a fence, or after one closes, still counts. Inline code spans were already handled by the
  terminator rule and are unchanged, as are the canonical digest widths, the header window, and the
  terminator set.

  **Indented code blocks are deliberately not covered.** Treating a four-space indent as code is a
  CommonMark rule, and it is the only change in this area that would *widen* the skip rather than
  narrow it. Indentation is structural — not a code-block marker — in the languages that carry the
  overwhelming majority of real stamps, and stamps written as block-comment continuations already sit
  indented. Suppressing there would strip protection from real stamps in order to catch documented
  ones, trading this false skip for a worse-shaped one, so indented blocks remain known-bad.

- **`[discovery] exclude` is now honored for paths named explicitly on the command line.** A root
  config's exclude globs were re-anchored onto the walk root, and that re-anchoring silently dropped
  any glob that pruned an *ancestor* of the named path — so `exclude = ["packages/dart/**"]` pruned
  correctly on `poly fmt .` and not at all on `poly fmt packages/dart/lib/two.py`. The excluded file
  was formatted in place and the report did not mention it. A second hole applied when the named path
  was a file: the file's own path was used as the anchoring subtree, so every glob written against its
  parent directory was classified as a sibling and discarded.

  This mattered most where it was least visible. The pre-commit hook never names a directory — it
  passes explicit staged file paths — so a repository could watch `poly fmt --check .` honor its
  exclusions and still have those same files rewritten at commit time. Unanchored globs
  (`**/*.generated.ts`) survived both holes, which is why existing coverage never caught it.

  Exclusions now match component-wise, so `packages/dart/**` prunes `packages/dart/lib` without
  swallowing `packages/dartlang`. One cosmetic consequence: an exclusion firing through the new
  ancestor rule is currently attributed to the re-anchored pattern rather than the authored glob.

- **A `json` or `toon` report that cannot be serialized is no longer emitted as an empty document.**
  Both renderers fell back to `[]`, which is byte-identical to a clean run over zero files — and the
  exit code did not distinguish them either, since it is computed from the run rather than the render.
  A total loss of output was indistinguishable from success on both channels at once, with nothing on
  stderr. The `toon` path inherited the same fallback by degrading to JSON.

  A serialization failure is now reported: nothing is written to stdout, the failure and its format
  are named on stderr, and the process exits non-zero. The existing TOON-to-JSON degradation is kept —
  that one preserves the data — but the empty-document floor beneath it is gone.

- **`poly lint` no longer rewrites source files through a catalog tool.** A catalog tool whose lint
  command rewrites files in place was wired up as a linter, so `poly lint` mutated the file and then
  reported it clean. With `[tools.sqruff] enabled = true` — one line, no overrides — a `.sql` file was
  reformatted on disk while the run printed `No issues found.` and exited 0.

  The mutating-command guard only recognized a fixed set of flags in a flat token list, so it missed
  both mutating *subcommands* (`sqruff fix`, `ruff format`, `php-cs-fixer fix`) and the many rewrite
  flags outside that set (`--autocorrect`, `--in-place`, `--write-changes`, `--reformat`, …). Thirteen
  shipped tools wired a file-rewriting lint command; 85 more were reachable through a `command`
  override. Arguments are now classified by shape — flags against the flag list, positionals against
  a mutating-subcommand list — and a tool with no read-only mode at all is refused by name.

  Such a tool now declines the lint capability rather than running. Linting a temporary copy was
  considered and rejected: a fix command's exit code reports whether *fixing* succeeded, so it would
  still have reported a findings-laden file clean — hiding the data loss while keeping the false
  pass. Formatting is unaffected; `poly fmt` still runs every one of these tools.

- **A catalog tool and a built-in backend no longer both run under the same name on one language.**
  The engine-name uniqueness guard only walked the built-in registry, but catalog engines are
  appended to the same plan — same capability filter, same `[<kind>.<lang>.<tool>]` config table, same
  cache-key path. `[tools.shellcheck] enabled = true` alongside `[lint.shell.shellcheck] enabled =
  true` produced four diagnostics for three real problems, all labelled `shellcheck`, with nothing in
  the JSON naming the producer.

  A name collision is now resolved as what it is — the same tool wrapped twice — by keeping whichever
  half actually does the work: the built-in when it is enabled and present, the catalog tool
  otherwise. Yielding to the built-in unconditionally would have removed lint coverage for anyone who
  had enabled only `[tools.<name>]`, trading duplicate diagnostics for none. A displaced entry is
  reported, since `command` / `args` / `env` set on that table stop applying.

- **Hook-source git commands no longer inherit the ambient `GIT_*` state a git hook exports.**
  Running `poly hooks` from a real git hook leaked that state into the internal git commands that
  operate on cached hook-source repositories. `GIT_INDEX_FILE` — which git sets to an absolute path
  for `git commit -a`, `--include`, and pathspec commits — made the source checkout run against the
  *consuming* repository's index, so provisioning aborted and printed an unrelated repository's file
  list. `GIT_DIR` made the mirror's origin-URL verification read from the consuming repository as
  well; that check exists to block a cache-poisoning source substitution, so it was answering from
  the wrong repository. It failed closed, but a check redirectable by an inherited environment
  variable is not a check.

  Only the variables needed to reach a remote are now passed through. Provisioning failures name the
  hook source, its revision, and the cache path instead of dumping foreign filenames.

### Changed

- **Minimum supported Rust version is now 1.97**, required by the upgraded PHP backend crates.

- **The engine version audit now enumerates the registry** rather than a hand-written list, so a
  backend cannot be omitted from it silently — which had already happened once, leaving a backend
  advertising a stale grammar-pack version while the audit stayed green. Backends that wrap no
  pinned Rust crate are declared exempt in one visible place instead of being skipped.

- **The generated-file skip's precedence over the result cache is now pinned by a test.** The skip
  short-circuits before the cache is consulted, which is why upgrading to a fixed poly takes effect
  on the first run rather than after a cache eviction; nothing previously stopped a refactor from
  reversing that.

- **All dependencies upgraded**, including the pinned oxc, ruff, and biome revisions. The PHP
  backend crates were held at an exact version that excluded every later release; they now track
  minor versions so upgrades are picked up. Every affected backend's version marker was bumped
  alongside its crate, so cached results from the previous release are invalidated rather than
  reused — the first run after upgrading re-checks each file.

## [0.21.4] - 2026-08-14

### Fixed

- **Generated-file detection now scans below leading YAML frontmatter.** Hash stamps and generated
  banners in the five lines after a valid frontmatter block are recognized, while unterminated
  frontmatter and markers beyond that bounded window remain ordinary source.

## [0.21.3] - 2026-08-14

### Fixed

- **Generated-file detection now requires a structured content-hash stamp.** Ordinary source such
  as `use a::hash::b;` is formatted and lint-fixed normally instead of being mistaken for generated
  code. Markers such as `alef:hash:<digest>` remain protected.

## [0.21.2] - 2026-08-14

### Fixed

- **Hook exclusions now share discovery's repo-root anchoring syntax.** A leading slash is treated
  as a repo-root anchor when hook globs are compiled, so `/e2e/**` excludes the root `e2e` tree
  without excluding nested package directories. Recursive patterns such as `**/target/**` retain
  their existing behavior.

## [0.21.1] - 2026-08-14

### Fixed

- **Bare Jinja, Vento, and Mustache templates now fail closed instead of being guessed from their
  contents.** XML documentation in a C#-generating `.jinja` file looked like rendered markup, so
  markup_fmt reflowed the template and produced invalid generated code. Generic templates are now
  formatted only when a second extension names a markup target, such as `.html.jinja` or
  `.xml.jinja`; `.cs.jinja` remains code, and ambiguous bare templates are preserved byte for byte
  with an actionable skip reason.

- **Explicit roots now honor configured exclusions consistently.** Naming a file no longer silently
  bypasses `[discovery] exclude`, and a relative file preceding an excluded directory no longer
  loses the directory's config anchor and exposes its files to formatting. Root count and argument
  order now have the same result. `--include-excluded` is the deliberate one-off override for a
  named file or directory; the existing `--force-exclude` flag remains accepted for compatibility.

## [0.21.0] - 2026-08-13

### Fixed

- **A hook that fails under staged isolation now says so.** Since 0.20.0 every hook in a
  commit-gating run validates the staged snapshot, not the worktree — correct, and nobody wants it
  back — but a failure gave no hint the content checked was not what is in the working tree. Two
  consumers lost a debugging round to the same shape independently: a clippy error naming a field
  that exists in their worktree, because the matching change sat in a file they had not staged, and
  `cargo check` had passed locally seconds earlier. The stage banner already named the tree, but it
  has scrolled off by the time a wall of compiler output ends. The failure now carries the reason
  and the two ways out — stage the rest, or `[hooks] isolate = false`. Printed once per run at the
  first qualifying failure, only under staged validation, and never for a timeout or a withheld
  fix, which carry their own remedies.

- **A language poly has no lint rules for is no longer counted as linted.** `poly lint .` over a
  tree holding `a.kt`, `c.xyz` and `d.py` reported `No issues found. (2 file(s) linted)` and
  exited 0. The Kotlin file was in that count: it routes to the cross-cutting backends
  (spell-check, ast-grep, comment removal) and to nothing carrying a single Kotlin rule, so a
  consumer with Kotlin, Swift and Zig as first-class languages read a green run as full coverage
  of three languages nothing had examined. `--deny-skips` could not catch it either, because
  none of it was reported as a skip.

  Such a file now leaves the linted count and joins the skipped set with the language named —
  `skipped a.kt: no lint rules for Kotlin` — which puts it in front of `--deny-skips` and
  `--max-skips`, and in the `skipped` field of the `--format json` / `toon` document. The reason
  is deliberately distinct from the existing `no matching engine for this file type`: an engine
  *did* match, poly simply has nothing to say about the language, and the fix for that is a
  linter for it (`[tools.<name>]`) rather than a rename or an exclude.

  The cross-cutting backends still run over these files and their findings are still reported —
  a Kotlin file can carry a typo finding and still be uncovered by any Kotlin rule. The same
  accounting applies wherever a language's linter is opt-in or absent, so a shell script with no
  `shellcheck` installed is reported rather than counted. **Default behaviour is unchanged: this
  is coverage information, not a failure, and a run whose only skips are no-rules languages
  still exits 0.** Only `--deny-skips` / `--max-skips` turn it into a gate.

  **The whole-project phase counts as coverage.** `poly lint` is two halves, and the per-file
  tier has no Rust rules — but the whole-project phase runs `cargo clippy`, so a plain
  `poly lint .` over a Rust repository was printing hundreds of `no lint rules for Rust` lines a
  few lines above the `✓ cargo-clippy` that contradicted them. `poly lint` now resolves which
  languages its whole-project tool set covers *before* the per-file run and those files are not
  recorded as skipped at all — so the linted count, the note, the JSON payload and
  `--deny-skips` describe the same run rather than being reconciled at display time. The mapping
  is keyed on the whole-project tool id: `cargo-clippy` establishes Rust coverage, while
  `cargo-sort` / `cargo-machete` / `cargo-deny` deliberately do not (they read manifests and the
  dependency graph, never `.rs` source). With the phase off — `--no-workspace`, `[lint]
  workspace = false`, a path-scoped run, or a repo with no `[hooks]` section — nothing lints
  Rust in that run, so the skip is accurate and still appears.

- **The skipped-file note is grouped by reason instead of capped as a list.** A run could emit
  229 consecutive identical lines, which both drowned the findings and pushed every *other*
  reason past the old flat cap of 20 — so the rare skip worth reading was exactly the one
  dropped. Each reason is now bounded on its own: a reason covering more than three files
  collapses to `skipped N file(s): <reason>` plus three example paths, and a reason covering
  three or fewer still names every file. A bulk reason therefore costs two lines however many
  files it covers and can never crowd out a one-off. `--verbose` lists every file individually,
  and `--format json` / `toon` carry the complete set as before.

- **A walked file poly cannot identify is no longer invisible.** `c.xyz` above appeared nowhere
  at all: it is neither a language poly knows nor a path the caller named. The run summary now
  reports `N file(s) of unrecognized type not checked` and names the first few. They are
  deliberately *not* itemised as skips — poly's own tree holds 83 unidentifiable files out of
  446 (snapshots, lock files, images, `LICENSE`), and one skip each would fire `--deny-skips` in
  every repository and bury the skips that mean something. A path named explicitly on the
  command line is unaffected and remains a skip, since naming a path is a request to check it.

- **The cache-permission warning no longer fires forever after an upgrade.** 0.20.0 began
  creating cache directories `0700` but never changed one that already existed, so every user
  upgrading from an older poly — whose slot was created under the usual `022` umask — saw
  `cache directory is readable by other local users` on every single run, with no way to make
  it stop short of a manual `chmod`. A warning that never goes away trains people to ignore
  warnings, which is worse than the low-severity issue it reported.

  The rule now splits on who chose the location. A directory resolved from the **default**
  per-user OS cache home (`~/.cache/poly/…`, `~/Library/Caches/poly/…`) with no
  `POLY_CACHE_HOME` and no `[cache] dir` override is poly's own: poly picked the path, created
  it, and invited nobody to share it, so it is tightened to `0700` in place and silently — there
  is nothing for the user to decide. A location named **explicitly** by `POLY_CACHE_HOME`,
  `[cache] dir`, or `--cache-dir` keeps 0.20.0's behaviour exactly: its mode may encode a
  deliberate choice (a shared CI cache, a team directory), so poly never changes it and warns
  once with the `chmod 700` command.

  Tightening is best-effort and Unix-only: poly checks that it owns the directory (`st_uid`
  matches the effective uid) before changing the mode, and a chmod that fails anyway — read-only
  mount, another account's directory — degrades to the same warning rather than failing the run.
  The warning is now de-duplicated per directory rather than per process, so a run that meets two
  different loose directories reports both.

## [0.20.1] - 2026-08-13

### Fixed

- **A cargo hook is no longer charged for a package-cache lock held outside the run.** Before
  spawning a hook in the `cargo` exclusion set, poly probes `$CARGO_HOME/.package-cache` and,
  if a process outside the run holds it, waits for it to clear **before** starting the hook's
  time budget — so a `cargo deny check` that takes seconds on its own can no longer be killed
  at the whole-project budget for a wait it did not cause. The wait prints
  `⏸ waiting to start: <hook> … the hook has not been spawned and its time budget has not
  started`, is bounded by half the hook's own budget (no new configuration — a hook with
  timeouts disabled never waits), and on expiry the hook is started anyway rather than
  withheld: a check that did not run must never be reported as a pass.

  This mitigates the common case, and says so rather than claiming a fix. The lock can still be
  taken between the probe and the spawn, cargo re-acquires it mid-run, and the artifact-
  directory lock (`<target-dir>/<profile>/.cargo-lock`) is not probed at all. For those, the
  post-spawn notice above remains the report — and it says outright that the budget is still
  counting, because poly cannot pause a queue it does not own.

- **A hook's `run` line is no longer broken, or a source file truncated, by poly's appended
  arguments.** Unix hook lines were built as `{line} "$@"`, so a line whose tail cannot take a
  trailing word failed to parse — a compound command ending in `done`, `fi`, `esac` or `}` — and
  the shell diagnostic named poly's appended text rather than the hook, reading as a poly bug.
  Worse, a line ending in a dangling redirect operator (`cargo build >`) made the appended `"$@"`
  the redirect **target**, so poly truncated one of the matched files. That is now refused.

  A line that already forwards positionals with `"$@"` or `"$*"` no longer gets a second copy
  appended, which is what makes script-shaped lines expressible: they read the arguments
  themselves. The check deliberately does not key on `$1` — `grep -q TODO "$1" && shellcheck`
  guards on the first file while relying on the append for the rest, and that works today.
  `argv[0]` is now the hook id, so a shell diagnostic names the hook that produced it.

- **`cmd_quote` no longer lets a trailing backslash escape its own closing quote** (Windows). It
  doubled embedded quotes and escaped `%` but left backslashes alone, so a value ending in an odd
  run of them stopped the quoting terminating where intended. Only the runs immediately before a
  quote and before the closing quote are doubled; interior backslashes in ordinary paths are
  untouched. Not reachable through tracked filenames, which cannot end in a backslash.

- **A hook's output is shown while it runs, instead of all at once after it exits.** The progress
  preview fed its sink only after the child exited, so a long-running hook showed nothing for its
  whole run — a hook that was working and a hook that was wedged looked identical, which is what
  the preview exists to tell apart. Output is now handed over as it arrives, from the
  supervisor's poll loop rather than the drain threads: feeding from a drain thread would hold a
  lock across a terminal draw, and a stalled drain is a pipe nobody empties, which blocks the
  child forever. The combined stream is now interleaved by arrival rather than all stdout then
  all stderr; captured stdout and stderr are byte-identical to before.

- **`cargo sort` no longer reports a pass for a check it failed.** `cargo sort --check` exits 0
  when it considers a manifest badly *formatted*, printing only a warning, and the cargo group
  gates on the exit code — so the hook rendered a green check while the tool had complained. Both
  the check and fix commands now pass `--no-format`, so the exit code is a faithful signal for
  sort order, the one thing the hook exists to check. Suppressing rather than honouring that
  opinion is deliberate: cargo-sort indents with four spaces and poly formats TOML with taplo at
  two, so gating on its formatting verdict would deadlock poly's own hooks against each other.
  The same flag on the fix command stops `poly lint --fix` churning manifests that `poly fmt`
  then reverts.

## [0.20.0] - 2026-08-13

### Added

- **`poly doctor`** — the command to run before filing a bug. Reports the running executable and its
  build identity, **every** `poly` on `PATH` with each one's version, the config files in effect, and
  the cache directory; `--format json` for pasting into a report. It exits non-zero on a real defect,
  such as one install shadowing another.

  A shadow warning also fires from ordinary commands when a different `poly` earlier on `PATH` would
  answer a bare `poly`. It is emitted before argument parsing, because the incident that motivated it
  was `poly --version` — which prints and exits without reaching any command handler.

- **`--deny-skips` and `--max-skips <N>` on `poly lint` and `poly fmt`.** A skipped file — no
  matching engine, a generated file, an unreadable path — is now always counted and named with its
  reason, so a run that checked nothing cannot look like a clean pass. A run emptied entirely by
  skips reports `Nothing was linted.` rather than `No issues found.` The new flags turn a skip into
  a hard failure for CI, exiting **2** — the class callers treat as an error, rather than the 1 that
  is often treated as success. `--verbose` lists every skipped file instead of the first 20.

- **`poly doctor` warns when an exclude rule prunes more than the directory it names.** `exclude`
  patterns are gitignore rules: a glob with no `/` in it matches a directory of that name **at any
  depth**, so `exclude = ["e2e/**"]` silently skips every nested `e2e` in the tree, not just the one
  at the root. A consumer was checking two files where they expected five.

  The semantics are unchanged and deliberately so — they are gitignore's, and anchoring is already
  expressible with a leading slash (`exclude = ["/e2e/**"]`). What was missing was any way to notice.
  `poly doctor` now reports each rule that pruned directories at two or more distinct depths, names
  the directories, and suggests the anchored form. The behaviour is documented in the `--exclude`
  help, the `[discovery] exclude` schema, the MCP parameter, and ADR 0017.

- **Build identity in `--version`, and in every MCP response.**

  ```text
  poly 0.19.7 (dev build v0.19.7-10-g361844a, debug)
  ```

  Two builds of the same version were previously indistinguishable. One consumer had to diff file
  mtimes to tell a fixed poly from one that was corrupting their templates, and another reported a
  status against the wrong binary. The id comes from `git describe` at compile time — no `--dirty`,
  no timestamp — so it stays a pure function of the committed tree and does not disturb reproducible
  builds. MCP callers get the same identity on every response, since they have no `--version` to
  fall back on.

- **`poly mcp` fails loudly once its own binary is replaced.** It fingerprints its executable at
  startup and re-checks per request. Eight long-lived servers were found still answering from a
  deleted inode with known-destructive semantics; upgrading poly left every running server stale
  forever, with no expiry and no signal. The `version` tool stays exempt so it can explain why the
  others stopped.

- **Discovery now reports what it excluded, per rule**, so a summary can distinguish "everything is
  clean" from "everything I chose to look at is clean". One repo carried formatting drift for weeks
  because a top-level check reported an unqualified pass while an excluded directory held
  unformatted files.

  ```text
  All formatted. (260 file(s) checked, 2 skipped (Go/Helm template syntax), 2 file(s) and 8 director(ies) excluded by config)
    excluded by [discovery] exclude / --exclude: **/identify/tags.rs (1 file(s)), .ai-rulez/** (1 dir(s)), and 5 more rule(s)
    excluded directories were not walked, so the files inside them are not counted
  ```

  Files and directories are counted separately on purpose: an excluded directory is pruned at its
  boundary and never walked, so the files inside it were never measured. Reporting a single total
  would claim a precision nobody paid for.

- **`--force-exclude` / `[discovery] force_exclude`, applied automatically in the hook path.**
  `[discovery] exclude` pruned the directory walk but never applied to a file named explicitly on
  the command line. That is right when a human names a file and wrong in a hook, which is *always*
  handed explicit staged paths — so a repo excluding `**/*.tf` found the exclude silently inert in
  its own pre-commit hook, poly reformatted 519 lines of Terraform, and `terraform fmt` then
  rejected the result, leaving the repo unable to be green under both tools.

  The generated `lint` and `fmt` builtin hooks now pass `--force-exclude`, so a repo's excludes hold
  where they matter without any config change. A direct CLI invocation is unchanged: naming a file
  still checks it.

### Changed

- **Whole-project hooks now actually run in parallel.** A priority group ran under a single
  boolean — if *any* hook in it could not run beside a peer, the *entire* group ran one hook at
  a time — and the stage-level `parallel` key defaults to `false`, so in practice a repo with
  even one inline job (poly's own `poly.toml` among them) ran every builtin hook serially on an
  otherwise idle machine.

  Replaced with named exclusion sets scheduled as chains (ADR 0024): hooks naming the same set
  run sequentially in config order, everything else runs concurrently. New job key
  `serial = true | "<name>" | false`. The cargo builtins (`clippy`/`sort`/`machete`/`deny`) join
  a shared `cargo` set by default, because every cargo subcommand takes the package-cache lock
  and a build takes the build-directory lock; a job whose `run` line invokes `cargo` anywhere
  joins that set automatically. See `adrs/0024-hook-concurrency-exclusion-sets.md` for the model
  and its trade-offs.

- Refreshed the whole dependency tree. All pinned git backends move to their latest upstream
  **release** commits, keeping the existing practice of pinning releases rather than arbitrary
  `HEAD`:

  | backend | from | to |
  | --- | --- | --- |
  | oxc (`oxc_formatter`, `oxc_linter`, …) | `65fe65d` — oxlint 1.76.0 / oxfmt 0.61.0 | `c42d639` — oxlint 1.78.0 / oxfmt 0.63.0 |
  | ruff (`ruff_linter`, `ruff_python_formatter`, …) | `80790b3` — 0.16.1 | `5b48a04` — 0.16.2 |
  | biome (`biome_css_analyze`, `biome_graphql_analyze`, …) | `1139f1c` | `6b8f09c` |
  | rubyfmt | `b63fbaa` | `5185d3b` |

  Registry dependencies were upgraded in the same pass, including the exact-pinned `mago` PHP family
  (`=1.42.0` → `=1.43.0`, bumped as a set so the monorepo crates stay consistent), `rumdl` 0.2.42 →
  0.2.54, `tree-sitter-language-pack` 1.14.0 → 1.14.3, `typos` 0.10.43 → 0.10.44, `uncomment` 3.5.1 →
  3.5.2, `ast-grep-core` 0.44.1 → 0.45.1, and `hcl-rs` / `hcl-edit` 0.19.7 / 0.9.6 → 0.19.8 / 0.9.7.

- Every affected engine's `version()` was bumped alongside its wrapped crate, so cached results
  produced by the previous backend versions are invalidated rather than reused. The `version_audit`
  test enforces this contract and now passes against the refreshed lockfile.

- **A file the formatter cannot process now fails the run (exit 2) instead of reporting success.**
  `poly fmt` logged an engine error at `warn` and dropped the file from the results entirely, so an
  unparseable file was erased from the accounting and reported as `All formatted. (0 file(s)
  checked)` with exit 0 — a gate passing on a file it never read. Each affected path is now named
  with the engine's own message.

  Exit **2** specifically. Consumers automate against these codes and at least one treats `1` as
  success for `--fix`, since `1` means "files were rewritten"; folding an unreadable file into
  either `0` or `1` would be wrong in opposite ways. The full contract is now: `0` clean, `1` files
  changed (or would change), `2` the run failed.

- **`extends`: `exclude` lists accumulate instead of replacing.** Under `extends`, arrays previously
  replaced wholesale, so a repo adding a single exclude had to restate the entire inherited array —
  freezing a copy that later changes to the shared base never reached. The repos that most needed a
  shared base were exactly the ones that silently stopped tracking it.

  `hooks.builtin.{lint,fmt,file_safety}.exclude` now inherit `[discovery] exclude` by the same rule,
  which is why poly's own `poly.toml` drops four duplicated exclude stanzas. Opt out per table with
  `exclude_mode = "replace"`.

  **This changes behaviour for anyone who restated `exclude` in order to *shrink* an inherited
  list** — they now get the union. Non-`exclude` arrays (`select`, `stages`, `clippy_args`) still
  replace, deliberately: those are values, not floors, and unioning them would make shrinking a rule
  set impossible.

- **The result cache now folds in the identity of the poly build that produced an entry**, and
  **existing caches are discarded on first run** as a result of a cache-format bump.

  Invalidation previously depended on an author remembering to bump a hand-written
  `Engine::version()` string, and the audit test only enforced that it embedded the *wrapped crate's*
  version — so a change to poly's own logic invalidated nothing. Worse, two builds reporting the same
  version produced identical keys despite behaving differently, so one build was served the other's
  results. That was reproduced: a binary whose formatter would rewrite a file read a cached entry
  from a different build and reported `All formatted`, exit 0.

  A **release** build's identity is `release/<version>` with nothing machine-local, so two release
  builds of one version still share entries and CI caching is unaffected. Development builds include
  an executable fingerprint, because `git describe` alone cannot see uncommitted work.

  Cache eviction also now runs automatically (at most once per 24h per repo: entries older than 14
  days, then oldest-first to a 1 GiB ceiling). It previously never ran unless invoked by hand.

- **`poly lint --fix` no longer rewrites machine-generated files.** Files whose opening lines carry
  a `DO NOT EDIT` / `@generated` / `auto-generated` / `:hash:` marker are still **reported** on —
  that is how a generator bug gets noticed — but the fix is withheld, because rewriting generated
  output is churn the next generation run reverts, and it can destroy the evidence.

  The reported case: ruff's `F841` fired on an unused binding in a generated test, and that binding
  was the only signal that 39 generated tests across 8 files called an API and asserted nothing.
  Running the obvious `poly lint --fix` rewrote it and turned a correct diagnostic about a real
  upstream defect into a clean lint pass.

  The withheld fix is reported rather than silent:

  ```text
  1 generated file(s) not fixed (pass `--fix-generated` to include them).
  ```

  `poly fmt` skips a generated file **only when its header stamps a content hash** (`alef:hash:`,
  `sourcehash`, `@checksum`), because reformatting invalidates the hash and the remedy is a regen
  that discards the formatting — a loop. A bare `DO NOT EDIT` banner does *not* trigger it: skipping
  in `fmt` removes the file from the format gate entirely, and a generator that stamps a
  hand-written file would otherwise drop it out of enforcement with nothing reporting it.

- **`poly lint <paths>` no longer runs the whole-project phase.** Explicit path arguments now scope
  the run to the per-file tier; `poly lint` with no paths is unchanged and still runs
  `cargo clippy` and the other whole-project tools. Previously a path-scoped lint silently escalated
  to an unbounded whole-workspace `cargo` build — nothing in the argument list distinguished
  `poly lint some/file.py` from a full-repository run, and when another process held the cargo
  package lock it blocked indefinitely with no output. Two agents in one reporting repo concluded
  poly was broken; one was killed at 13 minutes.

  A skipped phase is announced:

  ```text
  note: whole-project phase skipped for path-scoped run (pass --workspace to include it)
  ```

  **Naming the root is not path scoping.** `poly lint .` (or `./`, or an absolute path to the same
  directory) means "lint everything" and still runs the whole-project phase, exactly as `poly lint`
  with no arguments does. Only a genuine narrowing — a file or subdirectory — scopes the run.

  **Action required for commit gates that pass staged paths and rely on clippy running:** add the
  new `--workspace` flag. `poly lint --workspace <paths>` restores the previous behaviour.

- Minor: `cargo deny` no longer carries a stale, unused OpenSSL license allowance; every
  workspace crate now sets `publish = false`; `scripts/release-bump.sh` now asserts
  `.codex-plugin/plugin.json`'s version alongside the Claude plugin manifests it already checked.

### Fixed

- **Security: a hook's fix could be written to a file outside the repository.** A tracked
  symlink (git mode `120000`) whose blob names an absolute path outside the repo was faithfully
  recreated as a real symlink inside the staged snapshot `poly hooks` isolation builds, and
  write-back then copied a hook's rewrite straight through it — the destination open follows
  symlinks. Hook isolation is on by default and every routine `git commit` reaches the
  write-back path, so cloning a hostile repository and committing to it was enough. Reproduced
  end to end, not theorised.

  Closed at both layers. The snapshot now removes, on every refresh, any materialized symlink
  whose target does not lexically resolve inside it (checked right after `git checkout-index`).
  Write-back separately checks the destination with `symlink_metadata` before ever opening it —
  a symlink is refused, never followed — and rejects any index path carrying a `..`, an
  absolute, or a drive/UNC component.

- **Write-back no longer trusts `git diff-files` to decide a worktree file is safe to
  overwrite.** That was its only gate before copying a staged-content fix over live worktree
  bytes, and it is the same stat-based dirtiness probe this repo had already stopped trusting
  elsewhere. Under a stale index stat cache — reproducible with
  `git update-index --assume-unchanged` — it called a genuinely modified file clean, and poly
  then destroyed the unstaged work. Write-back now compares blake3 of the worktree bytes against
  the actual index blob (`git cat-file blob :0:<path>`) before writing. The write is also atomic
  now — a sibling temp file plus rename, preserving the destination's permissions — so an
  interrupt mid-copy can no longer truncate the author's source file.

- **A withheld fix now names the actual reason it was withheld**, instead of always printing
  "these files have unstaged changes" — which was false for the symlink and unreadable-path
  cases above, and actively misleading in the security case: it told the author to stage their
  work when the real cause was a symlink poly refused to follow. Five distinct reasons are now
  reported per path (unstaged changes, worktree is a symlink, worktree is not a regular file,
  the snapshot copy is unreadable, the path escapes the repository); the two security refusals
  are marked as such and never offer the `git add` remedy.

- **A hook queued behind a cargo lock is no longer reported as hung.** A cargo hook blocked on
  the package-cache or build-directory lock held by a process outside the run — a developer's
  own build, `rust-analyzer` — prints nothing while it waits, and was indistinguishable from a
  genuinely wedged hook: both said "still running". The liveness notice now says it is waiting
  on a lock and names the resource.

- **A hook that finished on its own is no longer reported as killed.** At the timeout boundary
  poly entered the kill path, reaped the child's real exit status, and then reported `TimedOut`
  regardless — a hook that passed blocked the commit and told the author it had been killed.
  Measured at 52–117 false reports per 8192 spawns under contention, so this was not theoretical.
  A child's own exit status now wins: poly re-probes before signalling, and on Unix treats the
  absence of a termination signal as proof it was not killed. One consequence worth knowing —
  a hook that traps `SIGTERM`, overruns its budget, and then exits non-zero by itself is now
  reported `Failed` with its exit code rather than `TimedOut`, because that is what happened.

- **Killing a hook on Windows no longer leaves its grandchildren running.** The kill path
  signalled only the direct child, so a hook that spawned its own processes — a shell running a
  build, a test runner with workers — left them alive holding locks after poly reported the hook
  killed. The child is now assigned to a Job Object at spawn and the whole tree is terminated,
  matching the process-group behaviour Unix already had. The job carries no kill-on-close limit,
  so a daemon a hook legitimately started still survives a normal run.

- **Cache directories are created owner-only (`0700`) on Unix.** The per-repo cache slot holds
  the hook staged snapshot — a full mirror of the source being committed — and was previously
  created under the process umask, commonly world-readable. An existing directory is
  deliberately never `chmod`'d, since a shared CI cache mode may be intentional; poly warns once
  with the fix command instead.

- **`poly fmt --format json` omitted a file it failed to process, instead of reporting the
  error.** An errored file was simply absent from the output, indistinguishable from a file
  checked and found clean; the only evidence was the exit code and a `WARN` log.
  `poly lint --format json` never had this bug, so the two commands' JSON output was asymmetric.
  `FormatResult` now carries an `error` field and the format reporter emits a record for every
  errored file — a machine consumer must still check the exit code, not just the payload. Format
  failures now also echo to stderr under `json`/`toon`, matching what lint already did.

- **MCP consumers lost per-file errors.** The poly MCP server now carries a run-level `errors`
  array, a per-file `error` distinct from `skipped`, and sets `isError` on any run with errors.
  An empty findings list was previously not proof of a clean run — a file that failed to lint or
  format could vanish from the result with no signal at all.

- **`poly lint` no longer exits 0 for a file it could not lint.** A per-file engine failure was
  logged as a warning and the file was dropped from the results, so a run that failed to check
  something reported success. `poly fmt` already handled this through `FormatError`; `lint` now
  matches it.

  An engine **error** is deliberately a third category, kept distinct from a **skip**: a skip is poly
  correctly declining to act (no matching engine, a generated file), an error is poly failing.
  Conflating them would have rebuilt the same false pass in a new shape, and would have made
  `--deny-skips` fire on the wrong thing. Errored files do not count toward the skip budget, are
  never counted as checked, and are named individually:

  ```text
  error bad.py: stream did not contain valid UTF-8
  1 file(s) could not be linted and were NOT checked.
  ```

  Exit code is **2**, not 1: an errored file produced no findings, so it is an internal failure
  rather than a lint result. When any file errored, the headline reads `Lint did not complete.` —
  `Fixed N issue(s)` still reports what the run did, but no longer stands in as the verdict.

- **Markdown that merely documents template syntax is no longer skipped.** The Go/Helm template guard
  scanned raw file content, so any markdown containing `{{ … }}` — including inside a fenced code
  block or an inline code span — was excluded from formatting entirely. poly could not format its own
  CHANGELOG.

  Template syntax inside fenced blocks, indented code blocks, and inline code spans is now treated as
  documentation rather than as a template. Syntax outside them still triggers the skip, and YAML
  detection is unchanged, so a genuine Go-templated `Taskfile.yaml` is still skipped. An unterminated
  fence resolves toward skipping — leaving a template unformatted costs nothing, reflowing one
  destroys it.

- **`poly fmt` no longer changes which rows a SQL query returns.** It rewrote `where id = NULL` into
  `where id is NULL`. Those are not equivalent: under three-valued logic `= NULL` evaluates to
  UNKNOWN and matches nothing, while `IS NULL` matches every null — so a command whose job is
  whitespace and casing silently changed a query's result set. Worse, `= NULL` is almost always a
  real bug, and the "fix" destroyed the evidence of it. `AL05` deleting unused table aliases under
  `fmt` is the same shape.

  Same root cause as the Markdown heading fix below: `lint` and `format` did not own disjoint rule
  sets, so `format` applied every rule that carried an autofix. The line is now **presentation
  versus meaning**: `poly fmt` applies only the Layout and Capitalisation rule groups, which cannot
  change what a query does. Aliasing, Ambiguous, Convention, Jinja, References and Structure are
  reported by `poly lint` and never auto-applied. The format-side suppression is derived from
  sqruff's own registry, so a rule group added upstream is excluded from `fmt` until it is
  explicitly vetted in — an unreviewed rule can only make `fmt` do less.

  `CV05` is still **reported**; poly just no longer applies it for you.

- **Per-file hooks validated the worktree while workspace hooks validated staged content**, and
  nothing in the report said which. Both failure directions were reproduced against the shipped
  0.19.7 binary: a violation present only in the index was committed with exit 0, and a violation
  present only in the worktree blocked a commit whose staged content was clean. Every hook now runs
  from one execution root resolved once per run, and the stage banner names the tree it validated.

  Because a hook's rewrite lands in the snapshot, it is carried back to the worktree **only** where
  the worktree copy is byte-identical to the index. Where they differ the author is holding unstaged
  work, so the fix is withheld and a `stage_fixed` hook fails the run naming the files — rather than
  overwriting work the fix never saw, or `git add`ing hunks the author deliberately left out.

  Three accepted trade-offs ship with the wider scope (ADR 0019). A partially staged file (`git
  add -p`) is judged on its staged hunks alone — an unstaged hunk in the same file can no longer
  fail or pass the gate. A gitignored `poly.local.toml` is invisible to the snapshot, so local
  overrides that loosen hooks stop applying under `git commit`; `poly hooks run --all-files` is
  unaffected, and `[hooks] isolate = false` in the committed `poly.toml` restores worktree scoping.
  And a hook's own untracked side effects — logs, generated files outside its matched set — land in
  the snapshot and are discarded on the next refresh, so anything a hook is meant to deliver must be
  a file its own `files`/glob actually matches.

- **A hook that never returns no longer hangs the commit silently.** One wedged a consumer's
  pre-commit for 25 minutes with zero output, blocking four commits, with `--no-verify` the only way
  through. Hooks now run under a budget — 10 minutes per-file, 30 minutes whole-project — chosen as
  hang detectors rather than performance budgets, since killing a cold `cargo clippy` would be a
  worse defect than the hang. On overrun the process group is signalled and the hook is reported as
  **killed by poly**, which is a distinct fact from failing on its own merits. A liveness line names
  the running hook after 15s, and skipped, killed and setup-failed hooks now render with distinct
  markers — previously `-` meant both "skipped" and "still running".

  Set a per-job budget with `timeout` in `poly.toml` — whole seconds (`90`), a suffixed duration
  (`500ms`, `30s`, `10m`, `1h`), or `0` / `off` / `none` to disable. Stage and hook `before` / `after`
  steps and `precondition` probes are bounded too, at 10 minutes and 60 seconds respectively; an
  unbounded stage `before` reproduced the same hang one level up. A malformed value names the hook
  it came from — "invalid timeout" in a repo with thirty hooks is not a diagnosis.

  Resolution is **environment override → `timeout` in config → shape default**. The environment wins
  deliberately: it is the escape hatch of whoever is actually running the hooks, and they are the only
  party who knows how fast that machine is — a config author cannot, and an operator often cannot edit
  the config at all (a CI checkout, a fork). An unparseable environment value is ignored with a
  warning rather than read as "disabled", since failing open on a typo would silently reinstate the
  hang the mechanism exists to bound.

- **`poly lint --fix` no longer deletes single lines out of the middle of a prose comment block.**
  The opt-in `[lint.uncomment]` backend judged each comment *line* independently, and tree-sitter
  reports one comment node per `//` or `#` line — so one line of a paragraph could be classified as
  dead code and removed while its neighbours survived, welding the remainder into a sentence the
  author never wrote. `::` is a code operator, and the prose guard required six word-shaped tokens;
  backticked and parenthesised words did not count, so ordinary technical prose scored just under the
  bar.

  The unit of judgement is now the contiguous comment **block**, never the line. A block is removable
  only when every line in it is covered by a removal — a `TODO` or `~keep` line inside vetoes the
  whole block — **and** every line reads as code. A partial-block edit is no longer merely unlikely,
  it is unrepresentable. A separate guard covers the standalone case, where a one-line English comment
  naming a `::` path has no block to belong to.

  The deliberate trade: a block mixing prose and code is now kept whole, so poly will miss some real
  commented-out code. A missed removal is cosmetic; a wrong removal destroys what the author wrote and
  looks like nothing happened.

- **`poly lint --fix` reported `No issues found.` while rewriting files.** The runner applied fixes,
  re-linted, and summarised only what *remained* — discarding the fact that it had changed the file at
  all. Check mode was always correct; only `--fix` was silent. This is why the comment deletion above
  went unnoticed in a consumer repository: the tool reported doing nothing while doing something. The
  summary now names what was fixed.

- **Remote hook sources no longer duplicate the consumer's cargo builtins.** Each `[[hooks.sources]]`
  entry was lowered through a synthetic config carrying `present = true`, and the cargo builtin group
  is default-on whenever a `[hooks]` section exists — so every source re-emitted
  `cargo-clippy`/`cargo-sort`/`cargo-machete`/`cargo-deny` under its own `{source}:` prefix. Each
  configured source added a full redundant pass over the workspace. In this repository the pre-commit
  stage went from 14 hooks to 10, and from 1m29s to 44s.

  The duplication was `PATH`-probed, so a machine missing `cargo-sort` or `cargo-deny` saw fewer
  duplicates than one with the full toolchain installed.

- **`poly fmt` no longer rewrites a document's heading structure.** `lint` correctly filtered out the
  formatting-category rules that `fmt` owns, but `format` ran the **entire** rule set through the fix
  coordinator — so `poly fmt --fix` applied any rule that happened to carry an autofix, whatever it
  changed. The two paths were never a clean partition.

  Concretely, MD001 (heading increment) demoted `### Go` to `## Go` in a document beginning with an
  `h1`, silently changing which sections nest under which. The violation is real, but the repair is
  ambiguous — demote the `h3`, or insert the missing `h2` above — and those produce different
  documents. Structural rules are now excluded from the format path, and their autofix is dropped from
  `lint` as well, since `poly lint --fix` applies diagnostic edits and would otherwise restructure the
  outline by the other route.

  MD001 is still **reported** by `poly lint`; poly just no longer guesses the repair. Anyone relying on
  `--fix` to normalise heading levels now gets an error to resolve by hand instead.

- **A heading whose text ends in `#` no longer loses that `#`.** rumdl's MD020 treated a trailing run
  of `#` as a closing sequence even with nothing separating it from the heading text, so `### C#`
  became `### C #`; MD003 then normalised that to `### C` in any document whose dominant style is open
  ATX. CommonMark §4.2 requires a closing sequence to be preceded by whitespace, so `### C#` is an
  ordinary heading whose text is `C#` and there was never anything for MD020 to report.

  Wider than the reported C# case: it fired on any ATX heading at any level ending in `#`, through
  both `poly lint --fix` and `poly fmt --fix`. It also **self-reverted** — a docs pass would repair
  the heading and the next `--fix` would undo the repair — which is why it survived unnoticed. Only
  the `### C\#` escape avoided it, because rumdl's pattern excludes a preceding backslash.

  The wrapped rule is now guarded for lines the author wrote verbatim, which keeps the legitimate
  `##Foo##` → `## Foo ##` fix alive; the diagnostic is suppressed too, since the construct is valid
  CommonMark and a permanent unfixable warning on every C# heading would just push people back to the
  escape. An upstream patch is prepared — this guard is what makes consumers safe before it lands.

- **Templates that do not render markup are no longer reformatted.** `poly fmt --fix` on a `.jinja`
  template rendering Go rewrote

  ```go
  data, err := json.Marshal(r)
  if err != nil {
  ```

  into `json.Marshal(r) if err != nil`, concatenated the closing braces, and emitted source that does
  not compile — 30 sites in one consumer's Go binding, which broke a downstream build. markup_fmt
  reflows on the assumption that whitespace is insignificant, which holds for HTML and not for most
  other languages, and the extension `.jinja` names the *template syntax*, never the *rendered*
  language.

  Jinja, Vento and Mustache are now formatted only when the target is actually markup: a double
  extension names it outright (`marshal.go.jinja` is not markup, `page.html.jinja` is), and otherwise
  the body must contain a literal tag once `{{ … }}`, `{% … %}` and `{# … #}` constructs are
  stripped — so a comparison inside a template expression is not mistaken for a tag. Declining is the
  safe default: leaving a template unformatted costs nothing, reflowing one destroys it. Dedicated
  markup languages (`.html`, `.vue`, `.svelte`, `.astro`, `.xml`) are unambiguous and unaffected.

- **A staged-snapshot file rewritten out of band is repaired instead of served forever.** The
  snapshot treated a path as current when its index OID was unchanged and the file existed, so
  anything that rewrote a snapshot file in place without changing an OID — a `workspace = true` hook
  running in fix mode, or a truncating crash — was served on every later run, permanently. That is a
  false pass: the fix never reaches the index, and every subsequent check-only run passes on bytes
  that are not staged. The manifest now records size and mtime alongside the OID.

- **Upgrading poly no longer leaves a window where the binary is absent.** `install.sh` used
  `install -m 0755` with a `cp` fallback; all of those keep the destination inode and rewrite in
  place, so the binary was partial or non-executable for the duration. Because poly's git hooks are
  installed globally and fail closed, that blocked **every commit in every repo on the machine**
  while an upgrade ran. Both installers now stage to a temporary path and rename over the
  destination, which is atomic within a filesystem.

  The git-hook shim also no longer assumes a missing install when poly is being installed *right
  now*: it looks for a staged or partially-written poly beside the destination and says so, and
  separately detects an installed poly whose directory is not on the `PATH` the hook sees. It still
  fails closed — a hook that silently skips when its binary is missing would be the exact failure
  this release exists to remove.

- **`uncomment --fix` no longer deletes prose that continues across lines.** A comment ending
  mid-clause — typically on `;` — was classified as commented-out code, so `poly lint --fix`
  removed it. In a reported Helm values file this deleted

  ```yaml
  # Chromiumoxide engine still gets built because browserEndpoint is set;
  ```

  from the middle of a four-line paragraph, leaving text that still reads as valid English with its
  explanation silently gone — data loss that passes CI and survives review. `code_only` judged prose
  by its closing character alone; a long, overwhelmingly word-shaped line is now treated as prose
  regardless of how it ends. Genuinely commented-out code ending in `;` is still reported.

- **`poly fmt` no longer reports a file it declined to inspect as one it verified.** The summary
  counted every discovered file as "scanned", including files a backend skipped on purpose — YAML
  carrying Go/Helm template actions, and (see below) templates that do not render markup. A skipped
  file and a checked-and-clean file produced byte-identical output:

  ```text
  $ poly fmt --check Taskfile.yaml     # skipped: contains {{.CLI_ARGS}}
  All formatted. (1 file(s) scanned)
  $ poly fmt --check plain.yaml        # actually checked
  All formatted. (1 file(s) scanned)
  ```

  The only signal was an `INFO` log that is invisible at default verbosity *and* absent on a cache
  hit, so a warm re-run had no signal at all. One consumer concluded from this that their templated
  YAML was covered when it was being skipped. The summary now separates the two and names the cause:

  ```text
  All formatted. (249 file(s) checked, 2 skipped (Go/Helm template syntax))
  ```

  Backends declare this through a new defaulted `Engine::skip_reason` hook, so the runner can count
  skips rather than each backend silently returning `Unchanged`.

- **A path argument that does not exist now fails the run instead of reporting success.**
  `poly fmt --check typo.py` printed `All formatted. (0 file(s) scanned)` and exited 0 — a green
  result that verified nothing. A mix of real and missing paths was worse: only the real ones were
  checked, the run still exited 0, and the file count looked plausible, so a hook or CI step feeding
  poly a stale path list was indistinguishable from a passing gate. Every unresolvable path is now
  named on stderr and the run exits 2. Reported independently by `html-to-markdown` (13 paths,
  0 scanned, exit 0) and arbitrated against `crawlberg`'s non-reproduction — the variable was path
  existence, not how many paths were passed.

- **A Rust inner attribute is no longer read as a shebang.** A `.rs` file starting with
  `#![deny(...)]`, `#![allow(...)]` or `#![no_std]` was treated as a script, which flagged every
  generated binding crate in `file_safety`'s shebang checks (~133 files in one reporting repo) and
  mistagged files during hook file-type identification. Both call sites now look one byte past
  `#!`; `[` is not a valid interpreter path in any language, so the check is applied generally
  rather than special-cased to `.rs`.

- **Documentation corrections that were themselves defects.** Every example in `ACTION_USAGE.md`
  referenced `Goldziher/poly@v1`, which does not exist — only `v0` does — so anyone copying them got
  a "ref not found" failure. The README also presented verify-after-the-fact as the general way to
  pin poly, when in fact the installer (`POLY_VERSION`), the GitHub Action (`version:`) and
  `cargo binstall` all support a true, checksum-verified pin; Homebrew is the only channel that
  cannot, and the README now says so plainly. An unpinned-install one-liner had already been copied
  verbatim into eight sibling repositories, so published examples are treated here as a defect
  surface rather than prose.

- **The Homebrew publish step no longer exits 0 when `HOMEBREW_TOKEN` is unset.** It skipped the
  tap update and reported success, so an unset secret would have left `brew install` serving the
  previous release indefinitely with nothing in the run to say so. The tap has in fact tracked
  every release, so no published version was affected — this closes a latent hazard rather than a
  live one. The step now fails the job with `HOMEBREW_TOKEN is not set`, so a missing secret
  blocks the release instead of quietly leaving Homebrew behind.

- Adapted the OXC backend to two upstream API changes in oxc `c42d639`:
  - `oxc_formatter::format` lost its trailing session argument; poly now calls the four-argument
    service-less form, which is the compatibility wrapper for exactly this use.
  - `oxc_linter::Message::rule` was removed. The rule identity now travels on the diagnostic's
    `OxcCode` (`scope` = plugin display name, `number` = rule name), so poly maps that instead.
    Reported codes are unchanged in shape — bare `no-debugger` for `eslint` rules, `oxc/const-comparisons`
    for plugin-scoped ones. Note that plugin-scoped codes now use oxlint's *normalized* plugin display
    name, so the three plugins oxlint renames (`jsx_a11y` → `jsx-a11y`, `react_perf` → `react-perf`,
    `nextjs` → `next`) report under the hyphenated form; adjust `[per-file-ignores]` entries keyed on
    the old spelling.

## [0.19.7] - 2026-08-12

### Fixed

- Tier-one formatters now retain exclusive ownership of their languages when an overlapping catalog
  formatter is enabled. This prevents tools such as `clang-format`, enabled for C-family files, from
  running after OXC and rewriting JavaScript, CJS, MJS, or TypeScript with C/C++ spacing rules.

## [0.19.6] - 2026-08-06

### Fixed

- A configured catalog formatter now displaces poly's generic tree-sitter reindenter instead of
  chaining with it. Formatters run in sequence and the whole chain repeats to a fixed point, so a
  language served by both poly's fallback reindenter *and* an external formatter had the two fighting
  over indentation on every pass. This is what kept Elixir from converging: even with `[tools.mix]`
  declared, poly reindented the file and `mix format` reindented it back. The fallback is dropped only
  when the catalog tool's binary is actually on `PATH`, so a configured-but-missing tool leaves the
  fallback in place rather than silently dropping all formatting for the language.
  (`crates/poly-core/src/runner.rs`, `crates/poly-core/src/engine.rs`,
  `crates/poly-core/src/engines/catalog_tool/mod.rs`)

  Elixir projects should now declare the formatter that owns the language:

  ```toml
  [tools.mix]
  enabled = true
  root = "packages/elixir"
  ```

## [0.19.5] - 2026-08-06

### Fixed

- Elixir formatting no longer strips indentation, and no longer fights `mix format`. Elixir has no
  native backend and `tree-sitter-elixir` ships no `indents.scm`, so it runs on poly's built-in
  `ELIXIR_INDENTS` query — which modelled only `do…end` and `fn…end`. A file containing neither, such
  as a top-level `%{}` map, produced zero captures, and `emit_reindented` still trimmed every line and
  re-emitted it at level 0: a `mix format`-formatted checksum map came back flattened to column 0, and
  the two formatters then oscillated forever. Every Elixir package in the xberg-io polyrepo had drifted
  this way and failed `mix format --check-formatted`. Two changes: `try_reindent_builtin` now returns
  `None` when the query captures nothing at all, so a file poly has no structural model of falls
  through to whitespace normalization instead of being flattened; and `(map)`, `(list)`, `(tuple)` and
  `(bitstring)` are tagged `@indent.auto` so their interiors are emitted verbatim. They are
  deliberately not `@indent` — `mix format` aligns a wrapped `=>` continuation at +4 under a +2 entry,
  which a level-counting model cannot express, so `@indent` would only shrink the oscillation rather
  than end it. Existing Elixir fixtures were all `do…end`/`fn…end`, the only constructs the query
  modelled, so the idempotency tests passed vacuously and never caught this.
  (`crates/poly-core/src/engines/treesitter/indent.rs`)

## [0.19.4] - 2026-08-06

### Fixed

- `poly fmt --fix` and `poly lint --fix` no longer strip a file's permissions. Both write through
  the same `write_atomic` helper, which creates a sibling temp file and renames it over the target.
  The temp file is born with `0666 & !umask` and has no relationship to the file being rewritten, so
  the rename replaced a `0755` script with a `0644` one — formatting an executable script silently
  cleared its exec bit, which is how `publish-npm/scripts/publish.py` in `xberg-io/actions` started
  failing ruff's `EXE001`. The original mode is now applied to the temp file before the rename.

## [0.19.3] - 2026-08-05

### Fixed

- `extends` path validation means the same thing on every host. The guards that keep an
  `extends` source's `file` inside the base checkout were written against `std::path`,
  which parses per-platform: `/etc/passwd` is rooted but *not absolute* on Windows, so
  `is_absolute()` accepted it there, while `C:\Windows\system.ini`, `\\server\share\x`
  and `a\..\b` are an ordinary filename and a single component on Unix and were accepted
  here. `file` is portable config resolved inside a checkout, so it is now inspected as a
  string: POSIX roots, UNC roots and drive qualifiers (including the drive-relative `C:x`)
  are rejected everywhere, as is a `..` segment separated by either slash. This was
  failing `ci` on the windows runner.

## [0.19.2] - 2026-08-05

### Fixed

- `typos` no longer flags lint rule codes. Linter configuration lists them by the hundred —
  ruff `lint.select`/`lint.ignore` and per-file-ignores, `# noqa:` suppressions, rumdl
  `disable` — and the dictionary read the short uppercase ones as misspelled acronyms
  (`CPY` → `COPY`/`CPU`), so every repo carrying such a config had to allowlist rule codes
  by hand. An identifier of at most five uppercase letters followed by at most five digits
  (`CPY`, `CPY001`, `S310`, `PLR0917`, `ASYNC230`, `MD012`) is now treated as valid. The
  shape stays narrow: a longer uppercase run, or any token containing a separator, is an
  ordinary identifier and is still spell-checked.
- The `typos` and `treesitter` engines report the dependency versions they actually build
  against. `typos-dict` 0.13.31 → 0.14.0 and `tree-sitter-language-pack` 1.13.3 → 1.14.0
  were bumped without updating either engine's `version()`, so cached results produced by
  the old dictionary and the old grammars were served as fresh. `version_audit` was failing
  on `main` for both.

## [0.19.1] - 2026-08-04

### Fixed

- `java` and `csharp` are no longer tree-sitter bracket-reindented. Their grammars/external
  scanners produce platform-dependent CSTs, which caused nondeterministic formatting output
  across macOS and Linux. Both languages now use deterministic whitespace normalization instead.

## [0.19.0] - 2026-08-01

### Added

- New `crates/poly-workspace` crate: the whole-project lint orchestration (`cargo clippy` /
  `cargo-sort` / `cargo-machete` / `cargo-deny`) is extracted out of `poly-cli` into a shared
  library (`run_workspace_lint`, `render_workspace_outcome`) with a narrow public API, consumed
  by both `poly-cli` and the new MCP workspace tools with no dependency cycle. `poly lint`'s CLI
  output is byte-identical to before.
- The MCP server (`poly mcp`) gained a broader tool surface: `rules` (list/test the custom
  ast-grep rule packs, read-only) and `config_show` (effective merged config, read-only) join the
  existing read-only `lint` / `format_check` / `cache_stats`, alongside the mutating `lint_fix` /
  `format_write` / `cache_clean`. Two new tools, `workspace_lint` and `workspace_lint_fix`, expose
  the whole-project phase as async Tasks (rmcp's `TaskManager`; poll `tasks/get`, cancel with
  `tasks/cancel`) so a multi-minute `cargo clippy` run doesn't block the call; a client that
  doesn't declare the tasks capability gets a synchronous (blocking) result instead. Every tool
  now returns typed structured content (`CallToolResult.structured_content`, with a derived JSON
  schema) in addition to a JSON or compact TOON text block, selectable per request via a `format`
  parameter. Transport stays stdio-only.
- poly now publishes its own Claude/Codex plugin and ai-rulez marketplace (`Goldziher/poly`),
  registering `poly mcp` as a stdio server plus 5 skills and 2 slash commands that teach an agent
  to use poly as its lint/format orchestrator. Install with `/plugin marketplace add
  Goldziher/poly` then `/plugin install poly@poly` (Claude); the Codex manifest lives at
  `.codex-plugin/plugin.json`. The plugin version is lock-step with the workspace version, bumped
  via `scripts/release-bump.sh`.

### Changed

- Bumped the pinned `ruff` (0.16.1, `80790b3`), `oxc` (`65fe65d`, `oxc_formatter` 0.61 /
  `oxc_parser` 0.142), and `biome` (2.5.6, `1139f1c`) git dependencies to their latest revisions;
  `rubyfmt` was already current. Adjusted for one upstream API rename
  (`Diagnostic::primary_message` → `concise_message`). The affected engine cache keys
  (`version()`) were bumped so upgraded output is re-run.

## [0.18.3] - 2026-07-29

### Fixed

- `poly fmt` is a pure formatter again: it no longer runs the whole-project lint phase. Under `--fix` it had been
  invoking `cargo clippy`/`cargo-sort`/`cargo-machete`/`cargo-deny` — linting, not formatting. That phase now runs
  only under `poly lint --fix` (and the commit gate). The `--no-workspace` flag is removed from `poly fmt`, since it
  only gated the phase that no longer runs.

## [0.18.2] - 2026-07-29

### Fixed

- `poly commit` now resolves remote git `extends` bases, matching `poly lint`/`poly fmt`. Previously the
  commit-message linter loaded `poly.toml` through the network-free resolver and hard-failed with "remote git
  extends source requires the poly CLI resolver" whenever a repo inherited its shared base from a pinned remote
  (rather than a local sibling path), breaking the commit-msg hook. The CLI now passes its remote resolver into
  gitfluff so the repo-local `[commit]` rules load regardless of where the base lives.

## [0.18.1] - 2026-07-29

### Fixed

- The result cache no longer serves stale output after a `poly` upgrade. poly's own version is now folded into
  every cache key, so a new binary can never reuse a predecessor's cached lint/format result — closing a
  non-idempotency where `poly fmt --fix` left a file that a fresh (`--no-cache`) format would still change, because
  an engine's hand-maintained `version()` had not moved even though the binary's output had. The trade-off is one
  re-run of otherwise-cached work after each upgrade.
- A `poly lint`/`fmt`/`hooks` run now self-heals an incompatible on-disk cache layout (wiping the entry tree when
  the `VERSION` sentinel is stale) rather than only under `poly cache gc`. The read-only `poly cache stats`/`size`
  maintenance commands stay non-destructive.

### Changed

- `poly lint --fix` and `poly fmt --fix` now run the whole-project / interop tools in **fix mode** instead of
  check-only: `cargo sort` sorts in place (drops `--check`), `cargo-machete` gains `--fix`, and `cargo clippy` runs
  `--fix --allow-dirty --allow-staged` (preserving `-- -D warnings` and any `clippy_args` override); `cargo deny`
  has no autofix and stays check-only. Previously these always ran in check mode, so `--fix` reported the findings
  but never applied them and they reappeared on every run. `poly fmt` gains a `--no-workspace` flag and only runs
  the whole-project phase under `--fix`, so `poly fmt --check` stays a fast, pure formatter. The git-hook /
  commit-gate path is unchanged and remains check-only.

## [0.18.0] - 2026-07-26

### Added

- `poly.toml` can now declare a top-level `extends` list to inherit any config section (`[discovery]`,
  `[lint.*]`, `[fmt.*]`, `[tools.*]`, `[per-file-ignores]`, `[hooks.*]`, `[defaults]`, …) from local and pinned
  remote base configs, reusing the `path`/`git`/`revision` vocabulary of `[[hooks.sources]]`. Bases are
  deep-merged beneath this `poly.toml`, and `poly.local.toml` still wins on top. A symbolic git ref (branch or
  tag) requires the new `poly config update` subcommand to resolve and pin it into `poly-config.lock`; `poly
  config resolve`/`show` prints the effective merged config. See [ADR 0020](adrs/0020-shared-remote-configuration.md).

### Fixed

- `uncomment` no longer false-positives on comments that are not commented-out code. A new `code_only` option
  (default `true`) keeps a removal only when the comment lexes as code in the file's language, so machine-generated
  headers (`# alef:hash:…`, `Re-generate with:`, `DO NOT EDIT`), multi-line English NOTE blocks, and `key = value`
  directive comments are left alone while genuine commented-out code (`// let x = foo();`, `# print("debug")`) is
  still reported. Set `code_only = false` to restore the previous strip-every-comment behaviour. The engine cache
  key was bumped so upgraded output is re-run.
- `poly fmt --fix` now converges to a fixed point in a single invocation. Each file's format-engine chain is re-run
  (bounded, up to 5 passes) until the content stops changing, so a following `poly fmt --check` is clean even when
  an underlying formatter (clang-format, csharpier, google-java-format) is not idempotent on the first pass.

## [0.17.1] - 2026-07-24

### Changed

- Bumped the pinned `oxc`, `ruff`, and `biome` git dependencies to their latest revisions and refreshed the
  `uncomment` backend to 3.5.1. The affected engine cache keys (`version()`) were bumped so upgraded output is
  re-run, and stale `rumdl`/`tree-sitter-language-pack` version markers left by the previous dependency refresh
  were corrected.
- Homebrew now ships bottles. The `poly` formula is generated as a source build, so the tap's centralized
  bottle pipeline compiles and attaches prebuilt bottles for macOS and Linux; `brew install Goldziher/tap/poly`
  pours a bottle once built. The `curl | sh` and PowerShell installers continue to use the prebuilt release
  binaries.

## [0.17.0] - 2026-07-21

### Added

- The whole-project lint phase now preserves tool colours. When `poly lint`'s own output is a colour-capable terminal,
  captured `cargo clippy` / `cargo-deny` / type-checker diagnostics keep their ANSI colouring instead of being stripped;
  redirected or `--no-color`/`NO_COLOR` output stays plain.

### Changed

- Opinionated default-policy audit across the wrapped tools:
  - MDX (`.mdx`) files no longer report `MD033` (inline HTML), `MD036` (emphasis-as-heading), `MD041` (first-line
    heading), or `MD051` (link fragments) — all noise against JSX/ESM content and toolchain-generated anchors. Plain
    `.md` is unchanged, and any rule is re-enableable via `enable`.
  - `typos` findings are now **warnings** rather than errors, so a single dictionary false positive no longer fails CI
    (`poly lint` exits non-zero only on errors).
  - `ruff`'s flake8-bugbear `B008` is disabled by default — it false-positives on the FastAPI / typer `Depends()` /
    `Query()` argument-default idiom — while the rest of bugbear (`B006`, …) stays on. Re-enable with
    `extend_select = ["B008"]`.
  - TOML now formats with a 2-space indent (matching YAML/JSON and taplo's own default) instead of 4, and the `taplo`
    formatter honours the global `line_length` instead of a hardcoded 120.

### Fixed

- `poly lint` no longer reports a false `parse-error` on valid JSONC (`.jsonc`) files that use trailing commas —
  including the trailing commas poly's own JSONC formatter emits. Strict `.json` still rejects them, and genuine JSONC
  syntax errors are still reported at their correct position.

## [0.16.0] - 2026-07-21

### Added

- MDX (`.mdx`) formatting. `.mdx` files are now discovered and routed through the `rumdl` backend with its MDX flavor
  enabled, preserving imports and JSX while normalizing Markdown.

### Changed

- Bumped the pinned `biome`, `oxc`, `ruff`, and `rubyfmt` git dependencies to their latest revisions and ran
  `cargo upgrade --incompatible`. Affected engine cache keys were bumped so upgraded output is re-run.
- The YAML and Markdown/MDX backends now skip files that contain Go/Helm template syntax — detected by scanning file
  content for template actions (`{{ … }}`, `{{- … }}`, `{{/* … */}}`), not by filename. GitHub Actions `${{ … }}`
  expressions and MDX/JSX object literals are not treated as templates. Each skip emits an info-level message.
- Config resolution now respects a repo-root `poly.toml` when poly is invoked from a subdirectory: it cascades and
  deep-merges every `poly.toml` (and sibling `poly.local.toml`) from the git repo root down to the working directory,
  and re-anchors exclude globs so they still match relative to the walk root.

### Fixed

- `poly hooks install` now installs shims only for configured hook types and prunes stale shims, and no longer prints an
  empty `[stage]` banner for a stage that runs no jobs — so `git commit -n` stays quiet.

## [0.15.5] - 2026-07-20

### Changed

- Bumped the `uncomment` backend to 3.5.0, which extends a `~keep` marker across a whole contiguous block of
  standalone single-line comments instead of preserving only the marked line. A multi-line rationale comment now
  survives with a single `~keep` rather than one per line. The engine cache key was bumped to re-run `uncomment` on
  upgrade.

## [0.15.4] - 2026-07-12

### Fixed

- Prevent `poly hooks` (and `poly fmt`/`poly lint`) from intermittently hanging: the native-tool and catalog-tool
  stdin backends now always feed a child's stdin from a dedicated thread while draining its stdout/stderr, instead of
  writing small inputs inline before the drain. A wrapped tool that emitted output while still reading stdin could fill
  its output pipe and deadlock against the blocked writer.

## [0.15.3] - 2026-07-12

### Changed

- **Dependency refresh.** Bumped `rmcp` (2.1 → 2.2), `saphyr` (0.0.9 → 0.0.11), `uncomment` (3.2 → 3.4), and
  `memchr` (2.8.2 → 2.8.3), along with transitive crates.io updates.

## [0.15.2] - 2026-07-12

### Fixed

- Allow the intentional Windows read-only attribute reset used to rebuild invalid cached hook checkouts.

## [0.15.1] - 2026-07-12

### Fixed

- Resolve simple `command -v <binary>` hook path checks portably on Windows.

## [0.15.0] - 2026-07-12

### Changed

- Share URL-keyed Git hook mirrors and immutable commit checkouts globally under the XDG cache, with per-source interprocess locking.

## [0.14.0] - 2026-07-12

### Changed

- Declare explicitly selected local-path and Git hook catalogs under `[[hooks.sources]]` in `poly.toml`; `poly-hooks.toml` is now producer-only.
- Select guarded producer execution paths using machine-local channel preferences from `poly.local.toml`.

## [0.13.0] - 2026-07-12

### Added

- Load executable local-path and Git hook sources from `poly-hooks.toml`, lock remote revisions, and refresh them with `poly hooks update`.
- Install missing hook toolchains on install or first run using machine-local channel preferences from `poly.local.toml`.

## [0.12.0] - 2026-07-09

### Added

- **Opt-in `uncomment` comment-removal lint backend.** A new cross-cutting lint
  engine (like `typos`) that strips comments across every language it recognizes,
  wrapping the pure-Rust [`uncomment`](https://crates.io/crates/uncomment) crate
  (tree-sitter based). Each removable comment is reported as a **warning** (which
  never fails CI) carrying a delete-edit, so `poly lint` surfaces them and
  `poly lint --fix` removes them. **Off by default**; enable and tune it with a
  language-agnostic `[lint.uncomment]` block plus optional per-language
  `[lint.<lang>.uncomment]` overrides (`enabled`, `remove_todos`, `remove_fixme`,
  `remove_docs`, `use_default_ignores`, `preserve_patterns`). Preservation rules
  keep shebangs, `~keep`, TODO/FIXME, documentation, and user patterns; a language
  `uncomment` does not recognize is left untouched.

## [0.11.0] - 2026-07-08

### Added

- **Per-group `[hooks.builtin.cargo] lint = false`.** Keep the cargo group
  (`clippy`/`sort`/`machete`/`deny`) as a `pre-commit` gate while excluding it
  from the whole-project phase of `poly lint`. Where `[lint] workspace = false`
  disables that phase wholesale, this opts out a single builtin — useful when a
  lightweight `poly lint` (e.g. a CI `validate` job with a plain checkout) cannot
  compile the workspace, but a properly provisioned job still runs clippy. The
  underlying `Hook::skip_in_lint` flag drops a hook from `poly lint`'s workspace
  phase without affecting git-hook runs.

### Fixed

- **Windows-correct staged snapshots.** The staged-content snapshot that isolates
  whole-workspace hooks now normalises CRLF and directory symlinks, so hooks that
  compile or analyse the tree behave the same on Windows as on Unix.

## [0.10.0] - 2026-07-07

### Added

- **`poly lint` runs whole-project tools.** After its per-file tier, `poly lint`
  now runs the same whole-workspace analysis tools a `pre-commit` hook would —
  `cargo clippy` / `cargo-sort` / `cargo-machete` / `cargo-deny` and any
  configured whole-project jobs (e.g. type checkers) — on the live worktree,
  folding their pass/fail into the report and the exit code. It reuses the
  existing `[hooks.builtin.cargo]` + inline `workspace = true` config as the
  single source of truth, so `poly lint` surfaces the same findings a commit
  would. On by default; opt out with `--no-workspace` or `[lint] workspace =
  false`. A repo with no `[hooks]` section runs only the per-file tier. With
  `--format json`/`toon` the whole-project section is written to stderr (stdout
  stays a single valid document), so machine consumers must check the exit code.
- **Animated `poly hooks` progress.** An interactive `poly hooks` run now shows a
  live spinner per concurrently-running hook with a rolling output preview,
  collapsing to a `✓/× id (duration)` line as each finishes. Non-interactive
  runs (CI, pipes) keep the deterministic, quiet report unchanged.

### Fixed

- **Security: bump `crossbeam-epoch` to 0.9.20** (RUSTSEC-2026-0204 — invalid
  pointer dereference in the `fmt::Pointer` impl for `Atomic`/`Shared`). Dropped
  the now-obsolete `quick-xml` advisory ignores (RUSTSEC-2026-0194/0195); the
  pinned ruff rev now resolves `quick-xml 0.41.0`, which is unaffected.

## [0.9.0] - 2026-07-06

Alignment release: `poly` is now the single brand for everything you type or run.
The GitHub repository moved to [`Goldziher/poly`](https://github.com/Goldziher/poly)
(old URLs redirect).

### Changed — breaking

- **Built-in hook keys renamed.** `[hooks.builtin] polylint` / `polyfmt` are now
  `lint` / `fmt`. The old keys are rejected — update `poly.toml` (e.g.
  `[hooks.builtin] lint = true`).
- **`polylint.toml` is no longer read.** Only `poly.toml` (plus the
  `poly.local.toml` override) is discovered. Rename any remaining `polylint.toml`.
- **Cache moved out of the repo.** The result cache and hook staged-snapshot now
  live in the per-user cache directory (`~/.cache/poly/<repo-key>` on Linux,
  `~/Library/Caches/poly/…` on macOS, `%LOCALAPPDATA%\poly\…` on Windows) instead
  of the in-repo `.polylint/` folder — so nothing poly-generated lands in the
  working tree. A legacy `.polylint/` directory is auto-removed on the next run.
  `POLY_CACHE_HOME` overrides the base; `[cache] dir` still pins an explicit root.

### Changed

- **Internal crate `polylint-core` renamed to `poly-core`** (every workspace
  crate now uses the `poly-` prefix). Visible only via `RUST_LOG` targets.
- **Homebrew formula renamed** `polylint` → `poly`: install with
  `brew install Goldziher/tap/poly`.
- Logo and README refreshed to the `poly` wordmark and branding.

### Removed

- **npm and PyPI wrapper packages are discontinued.** poly is now distributed via
  the `curl … | sh` / PowerShell installer, the GitHub Action, and the Homebrew
  tap only. Prebuilt release binaries are unchanged; if you installed the `poly`
  command through `@nhirschfeld/polylint` (npm) or `polylint` (PyPI), switch to
  the installer or `brew install Goldziher/tap/poly`.

## [0.8.0] - 2026-07-06

### Added

- **Custom-rule tier.** Write your own lint rules — and codemods — as
  [ast-grep](https://ast-grep.github.io) YAML, in any of the 300+ languages poly
  can parse. Custom rules run in-process alongside the native backends on every
  `poly lint`, and `poly lint --fix` applies any `fix:` rewrites they declare. No
  plugin, no fork, no extra toolchain: rules run on the same tree-sitter grammars
  poly already bundles. Point `[rules] dirs` at one or more rule directories
  (default `[".poly/rules"]`); each rule is a standard ast-grep document whose
  `language:` field names a grammar.
- **`poly rules test` / `poly rules list`.** Verify rules against companion
  `<name>-test.yml` snippets (`valid` must not match, `invalid` must) and list the
  discovered rules. `poly rules test` exits non-zero on any failed snippet.
- **`fixed:` rule-test assertion.** An `invalid` test case may be a
  `{ code, fixed }` table that asserts the rule's applied autofix output, not just
  that the rule fires.

### Fixed

- **`[rules] dirs` resolve relative to the config file**, not the process working
  directory, so a rule set is found from any subdirectory.

### Changed

- **Dependency refresh.** Bumped `saphyr` (0.0.6 → 0.0.9).

## [0.7.0] - 2026-07-05

### Changed

- **Misspellings are now reported as errors and are never autofixed.** The
  `typos` backend previously emitted `warning`-severity findings with a
  single-correction autofix. Auto-correcting a typo silently rewrites
  identifiers, string keys, and API names that only *look* misspelled — a
  frequent source of regressions — so a typo is now surfaced at `error` severity
  (it fails `poly lint`) with the dictionary suggestion in the message, and
  carries no autofix. Resolve typos by hand (or allow-list the word).
- **Formatting rules no longer leak into `poly lint`.** rumdl's `Whitespace`
  category (line length `MD013`, trailing spaces, hard tabs, blank-line runs,
  final newline) is a `polyfmt` concern, yet every such rule also surfaced as a
  `poly lint` finding — flooding lint with formatting noise the linter cannot
  act on. `poly lint` now suppresses the `Whitespace` category and reports only
  structural / content findings (broken links, heading structure, unused
  references); `poly fmt` still owns and fixes the formatting rules.
- **Whole-project type-checkers are no longer wired into the per-file catalog
  lint tier.** `pyrefly`, `mypy`, `ty`, and the like resolve imports across the
  whole project and infer an import root from the project layout, which the
  per-file, exit-code catalog tier cannot provide — every cross-module import
  became a spurious `missing-import`. They are now refused as catalog linters
  (with a one-time warning); run them as a dedicated whole-project step instead.
- **Dependency refresh.** Bumped the `oxc` / `ruff` / `biome` git dependencies to
  their latest upstream commits and freshened crates.io dependencies (`rumdl`
  0.2.28, `typos-dict` 0.13.31, `tree-sitter` 0.26.10, `tree-sitter-language-pack`
  1.12.4, and others).

### Fixed

- **Catalog linters run against the real file on disk, not a temp copy.** A
  catalog-tier linter (e.g. `shellcheck`, `actionlint`) was fed a temp copy of
  the source, which destroyed project context: a Python type-checker could not
  resolve sibling modules or the project virtualenv, and `actionlint` no longer
  saw a `.github/workflows/` path. Read-only linting now runs against the real
  file whenever its on-disk content matches what is being linted, falling back to
  a temp copy only when they diverge (e.g. a re-lint after an in-memory fix).
- **`poly hooks` whole-workspace snapshot now materializes git submodules.** The
  staged snapshot is built with `git checkout-index`, which writes only blob
  entries — a submodule gitlink left *no* content, so a compile hook that reached
  into a submodule (e.g. a test that `include_bytes!`es a fixture from one) failed
  to build in the sandbox even though the real tree compiles. Each populated
  submodule is now exposed in the snapshot as a symlink into the live worktree, so
  compile-time references resolve.
- **Built-in `typos` allow-list for ubiquitous technical terms.** Common,
  always-correct tokens the dictionary otherwise flags — established
  abbreviations (`ser`, `flate`, `fpr`, `arange`, `unparseable`) and well-known
  OSS names (`certifi`, `onnx`, `wasm`, `tesseract`, `pdfium`, `pymupdf`,
  `surrealdb`, `mkdocs`, `mkdocstrings`, `rumdl`) — are now valid out of the box,
  so every repo no longer re-lists them in `extend_words`.

## [0.6.0] - 2026-07-05

### Added

- **Whole-workspace hook isolation for `poly hooks`.** Hooks that analyze the
  whole project rather than a file list — `cargo clippy`/`sort`/`machete`/`deny`
  and type checkers like `pyrefly` — can now be marked `workspace = true` (the
  `cargo` builtin group sets it automatically). A whole-workspace hook takes no
  appended filenames (`{staged_files}` opts back in) and runs against a
  **non-destructive snapshot of the git index** at `.polylint/staged`, so a
  pre-commit check sees exactly what the commit would capture: unstaged edits and
  untracked files never affect it, and — unlike `git stash`-based approaches — the
  working tree is never touched. The snapshot is a persistent, git-ignored cache
  sourced from the index blob and refreshed incrementally (only files whose staged
  object id changed are re-materialized), so cargo/pyrefly/`tsc` incremental caches
  stay warm; cargo is pointed at the real `target/` and coexists with dev builds.
  On by default for the commit-gating stages (`pre-commit`, `pre-merge-commit`) and
  skipped for `--all-files`; opt out with `[hooks] isolate = false`. See ADR 0019.
- **Default-on result caching for the `cargo` builtin group**, keyed on the Rust
  source/manifest set (`**/*.rs`, `Cargo.toml`, `Cargo.lock`, `deny.toml`,
  toolchain files). A commit touching no Rust skips `clippy`/`sort`/`machete`/`deny`
  entirely; a whole-workspace hook's cache key digests the **staged** snapshot
  content, so reverting an unstaged edit is never a false hit. Opt out with
  `cargo = { cache = false }`.

## [0.5.1] - 2026-07-04

### Fixed

- **Trailing whitespace no longer leaks into `poly lint`.** The tree-sitter
  generic tier (and the format-only native-tool backends that fall back to it —
  `gofmt`, `rustfmt`, `swift-format`, …) previously reported a
  `trailing-whitespace` **lint** diagnostic that `poly lint --fix` could not act
  on: the diagnostic carried no autofix, and the fix lives on the format path.
  Worse, `lint` flagged it even in files that `fmt` deliberately leaves alone
  (e.g. a Swift file marked `// swift-format-ignore-file`), so the warning could
  never be cleared. Trailing whitespace is now purely a **`polyfmt`** concern:
  the generic tier and the format-only native backends declare `lint: false` and
  emit no lint diagnostics; run `poly fmt --fix` to strip trailing whitespace.

## [0.5.0] - 2026-07-04

### Added

- **Live per-hook progress for `poly hooks`** — when stderr is a terminal, each
  hook now prints a `▶ <id> …` line as it starts and a `✓/× <id> (<duration>)`
  line as it finishes, so a long-running hook (`cargo clippy`, `cargo test`, …) is
  visibly running instead of leaving the terminal blank until the whole stage
  completes — which read as a hung commit. Progress goes to stderr and is
  suppressed when stderr is not a terminal (piped / CI), so captured output is
  unchanged.
- **Autofixable count in the lint summary** — `poly lint` now reports how many of
  the findings can be resolved automatically (`N fixable with the `--fix`
  option.`), making the value of a follow-up `--fix` run obvious from a dry run.
  The line is omitted when nothing is fixable.

## [0.4.0] - 2026-07-04

### Added

- **Colored `poly hooks install` / `uninstall` output** — a green ✓ header with
  the hook count and the (relative) hooks directory, then one line per hook name,
  replacing the flat list of absolute paths.

### Changed

- **Installed git-hook shims resolve `poly` from `PATH`** rather than baking in an
  absolute path to the binary, so a hook always runs whatever `poly` is current
  (a recorded absolute path could pin a stale or moved build). When `poly` is not
  on `PATH` the shim now fails with a clear, actionable message and a non-zero exit
  instead of proceeding as though the hook had passed. Re-run `poly hooks install`
  to migrate existing shims.

### Fixed

- Native-toolchain formatter output is normalized to LF line endings; some
  first-party CLIs emit CRLF on Windows, which made output platform-dependent.

## [0.3.0] - 2026-07-04

### Added

- **Native-toolchain formatter backends** — opt-in backends that invoke a
  language's canonical first-party CLI when it is present on the host: Java,
  Kotlin, R, Swift, Dart, and Gleam. Off by default (enabled per-tool in config);
  when the tool is absent the language falls through to the tier-2 tree-sitter
  formatter, so the zero-dependency guarantee is intact for anyone who has not
  opted in.
- **C# tier-2 support** — a `Language::CSharp` variant so `.cs` files route to the
  tree-sitter generic formatter (deterministic, zero system dependency) instead of
  being skipped. Maps the `c#` / `csharp` catalog names and the `.cs` extension.
- **Elixir `do…end` reindent** in the tier-2 formatter. Elixir's blocks are
  keyword-delimited (`do…end`), so they matched neither the brace-counting path nor
  a language-pack indents query (tree-sitter-elixir ships none) and were left at
  column 0. A new built-in-indents-query dispatch slot plus a minimal Elixir query
  produces `mix format`'s 2-space nesting; idempotent, with heredocs/strings
  preserved.

### Changed

- **`poly fmt` honors `// swift-format-ignore-file`** — a Swift file carrying the
  directive is left byte-for-byte untouched (the same whole-file skip marker
  `swift-format` respects), mirroring the generated-lock-file skip. Protects files a
  project opted out of formatting and machine-generated swift-bridge glue.
- Bumped `fs-err`, `serde-saphyr`, and `sqruff` to their latest releases.

### Fixed

- **`poly hooks` now enforces the `commit-msg` stage.** Lowered hooks kept
  `Stage::default()` (pre-commit), so the runner dispatched the `poly commit`
  (Conventional Commits) builtin in file-input mode, matched no files, and silently
  skipped it — the git `commit-msg` hook never enforced anything. Every lowered hook
  is now stamped with the stage it was lowered for; latent for any non-pre-commit
  builtin, only `poly-commit` surfaced it.
- **Rust files named like `dockerfile.rs` are no longer misdetected as
  Dockerfiles.** Language detection now lets a known file extension (`.rs` → Rust)
  win over the Dockerfile filename match, so `engines/dockerfile.rs` and similar no
  longer produce spurious Dockerfile parse errors.

## [0.2.0] - 2026-07-03

### Added

- **Biome CSS + GraphQL linters** — two in-process tier-1 lint backends built on
  the official `biomejs/biome` analyzer crates, filling gaps polylint had no
  native linter for. Both are lint-only and coexist with the existing malva/
  graphql formatters. Configured via `[lint.css.biome]` / `[lint.graphql.biome]`
  with the shared `select`/`extend_select`/`ignore` surface; default rule groups
  are `correctness` + `suspicious`.
- **`poly migrate`** — new subcommand that absorbs a repo's `ruff` / `typos` /
  `taplo` / markdownlint config (including `pyproject.toml` `[tool.ruff]` /
  `[tool.typos]` / `[tool.codespell]`) into `poly.toml`, comment-preserving, then
  deletes or strips only the sources poly can fully honor. Dry-run report by
  default; `--write`, `--recurse`, `--verify`, `--strip-superseded`.
- **Native typos config** — `_typos.toml` / `.typos.toml` / `pyproject
  [tool.typos]` / `[tool.codespell]` are honored, including `extend-ignore-re`
  (region masking), `extend-ignore-words-re` / `-identifiers-re`, and full
  ancestor-chain merging.
- **Dockerfile rule selection** — the Dockerfile backend now honors
  `[lint.dockerfile]` `select` / `extend_select` / `ignore`.

### Changed

- Dry-run `poly fmt` (no `--fix`) now reports "N file(s) will change" instead of
  the past-tense "N changed", which implied files were rewritten.
- Bumped the pinned oxc (`c0c69dc`) and ruff (`1cb2012`) revisions and ran
  `cargo upgrade --incompatible` (clap_complete, rand, rmcp, rustc-hash).

### Removed

- **The R (air/jarl) tier-1 backend.** Migrating air+jarl onto official
  `biomejs/biome` was disproportionately costly (a large fork rebase across biome
  API drift plus a non-upstream patch), and air/jarl were the sole consumers of
  the `lionel-/biome` fork. Dropping them removes that fork from the dependency
  graph and unblocks the official biome CSS/GraphQL analyzers with no crate
  collision. R now falls through to the tier-2 tree-sitter formatter (best-effort
  format, no lint).

## [0.1.15] - 2026-07-02

### Added

- **Hierarchical (monorepo-aware) config resolution** (ADR 0018). Running `poly`
  from a monorepo root now discovers nested `poly.toml` files and cascades them
  the way ruff/eslint resolve config: a file is governed by the deep-merge of its
  ancestor config chain (workspace root as base, nearest config wins), so a
  sub-project's `poly.toml` declares only its diff and inherits `[defaults]`, the
  `[lint.*]`/`[fmt.*]` rule tables, and `[per-file-ignores]` from above.
  - New `[workspace] root = true` marker bounds the upward cascade; a `.git`
    directory is an implicit boundary, so single repos need no annotation.
  - `[discovery] exclude` globs are unioned tree-wide, each rooted at its own
    config directory (a nested config prunes only its subtree); `[per-file-ignores]`
    globs resolve relative to their owning config's directory.
  - `--config <path>` pins a single config and bypasses nested resolution.
  - Fully back-compatible: a repo with one root `poly.toml` and no nested configs
    resolves every file to the root config, identical to before.

## [0.1.12] - 2026-07-02

### Fixed

- **ruff cache-key** now folds the E501/`line_length` engine change (0.1.11).
  The `line_length`-honoring fix altered lint output for the same input without
  bumping the ruff engine `version()` suffix, so warm `.polylint` caches kept
  serving stale 88-column E501 diagnostics. Bumped the suffix (`+e501`) to
  invalidate them. (CI is unaffected — fresh runners have no cache.)

## [0.1.11] - 2026-07-02

### Fixed

- **ruff E501 now honors `line_length`.** The line-too-long rule read ruff's
  `pycodestyle.max_line_length`, which poly never set — so it stayed pinned at
  ruff's hardcoded 88 regardless of the configured `line_length` (while the
  formatter correctly used 120). poly now mirrors the resolved `line_length`
  onto `pycodestyle.max_line_length` in both the default and per-config settings,
  so `select = ["ALL"]` projects with a 120 limit no longer see false-positive
  E501 on 89–120 char lines.

## [0.1.10] - 2026-07-02

### Fixed

- **actionlint**: restrict linting to GitHub Actions workflow files
  (`.github/workflows/**/*.yml|yaml`). Previously `poly lint .` ran `actionlint`
  on every YAML file (including `Taskfile.yml`, `docker-compose.yaml`, etc.),
  emitting spurious "jobs section is missing" errors. The tool now silently skips
  non-workflow YAML. A new `path_globs` field in the catalog model provides a
  general mechanism for future path-scoped tools.

### Added

- **ruff / isort**: `known_first_party` and `known_third_party` options for the
  ruff engine, settable in `poly.toml` under `[lint.python.ruff]`. Resolves
  false `I001` (import-block un-sorted) errors when a first-party package lives
  in a `src/`-layout that the package-root walk cannot reach from a sibling
  `tests/` directory.

  ```toml
  [lint.python.ruff]
  known_first_party = ["kreuzberg_cloud"]
  ```

## [0.1.9] - 2026-07-02

### Fixed

- **rustfmt**: `poly fmt` now honours the project's `rustfmt.toml` /
  `.rustfmt.toml` when formatting Rust source. Previously poly always injected
  `--config max_width=120`, which silently overrode every option in the
  project's rustfmt config (not just `max_width`). Now poly walks up from each
  source file to find the nearest `rustfmt.toml` and passes its directory via
  `--config-path`, letting rustfmt load the full project configuration. When no
  config file is found the existing 120-column default is preserved.

## [0.1.8] - 2026-07-01

### Fixed

- **cli**: `poly lint` exits non-zero only when a diagnostic is error-severity.
  Warning/info/hint findings are still reported but no longer fail the run, git
  hooks, or CI — matching the ruff/eslint/clippy convention. Previously any finding
  (including warnings) exited non-zero.

### Changed

- **deps**: upgrade dependencies to their latest versions (`cargo upgrade --incompatible`):
  quick-xml 0.40 → 0.41, plus clap_complete and indicatif.

## [0.1.7] - 2026-07-01

### Added

- **Uniform rule selection across `ruff`, `sqruff`, and `rumdl`** — all three now
  accept the canonical `select` / `extend_select` / `ignore` vocabulary through the
  shared parser, with each tool's native keys (`rules`/`exclude_rules`,
  `enable`/`disable`) kept as back-compat aliases and unioned. Unknown or blank rule
  codes are surfaced with a warning and skipped instead of dropped silently.
- **Uniform per-rule severity remap** — a configured `[lint.<lang>.<tool>.rules.<code>]
  level` is now honored for every engine as a post-lint remap on the normalized
  diagnostic code, including engines with no native severity configuration.
- ADR 0016 (uniform rule-selection model) and ADR 0017 (path exclusions and
  per-file rule ignores) documenting the configuration design.

### Fixed

- **Tier-2 generic formatter** no longer rewrites the interior of multi-line
  strings, heredocs, raw strings, or block comments on the query-driven reindent
  path (the brace-counting path was already guarded); their significant leading
  whitespace is preserved byte-for-byte.
- **`rubyfmt` cache key** now folds the pinned git rev instead of a stale version
  string, so a rev bump invalidates cached Ruby output.

### Changed

- **Homebrew distribution now ships bottles.** The tap formula builds `poly` from
  source and the release dispatches the tap's bottle workflow, so `brew install`
  pours a prebuilt bottle on supported platforms (macOS ARM64, Linux x86_64/ARM64)
  and builds from source elsewhere.

### Internal

- Extracted a shared `deserialize_options` helper for the format-only backends
  (malva, markup_fmt).
- Added a `Cargo.lock` drift-guard test asserting every backend's `version()`
  embeds the resolved crate version or pinned git rev — enforcing cache-key
  discipline across all 17 backends.

## [0.1.6] - 2026-06-30

### Added

- **Per-tool rule configuration** — a uniform `select` / `extend_select` / `ignore`
  surface plus per-rule `[rules.<id>]` overrides (`level` + tool-specific params)
  for `mago`, `ruff`, `oxc`, `sqruff`, `rumdl`, and R/`jarl`. `select` by category
  replaces the default set; unknown rule/category names error loudly.
- **Formatter options** via `[fmt.<lang>.<tool>]` for `yaml`, CSS/SCSS/Less
  (`malva`), HTML/Vue/… (`markup_fmt`), GraphQL, and TOML (`taplo`).
- **ruff per-plugin parameters**: `pydocstyle_convention`, `mccabe_max_complexity`,
  `pylint_max_args`, `pylint_max_branches`, `pylint_max_returns`.
- **Path exclusions** across config, CLI, and MCP: `[discovery] exclude`, a
  repeatable `--exclude <glob>` flag, and an MCP `exclude` parameter.
- **`[per-file-ignores]`** — gitignore-style glob → rule-code suppression, applied
  as a cross-engine post-lint filter.
- **`[tools.*]` `env` and `root`** — environment variables and a working directory
  for catalog tools (e.g. running `golangci-lint` per Go module).
- **`[hooks.builtin.cargo] clippy_args`** to override the clippy invocation.
- **oxc** per-rule `Deny` severity and JS/JSON formatter options.
- Per-engine `indent_width` override (honored uniformly by every formatter).
- A `Taskfile.yaml` with the standard dev tasks.

### Fixed

- **ruff INP001 false positives** and **isort first-party misclassification** —
  the package root is now resolved from the file's directory, so per-file linting
  matches ruff's whole-tree behavior. (isort `I001`/`I002` are in the default set,
  so this affected every run.)
- **Single-file invocations** now apply `[per-file-ignores]` and report the
  correct path (a file passed as its own root collapsed to an empty match path).
- **Lint cache correctness** — the cache key now folds the file path (byte-identical
  files such as empty `__init__.py` no longer collide and serve each other's
  path-dependent diagnostics) and the effective `[defaults]` globals.
- **HCL** inline trailing comments are no longer lost on format (files with
  comments route to the structural tier instead of the comment-stripping path).
- **Dockerfile** parse failures now surface as `Error` diagnostics instead of
  being silently swallowed.
- **sqruff** parse/lex errors are reported as `Error` (not `Warning`).
- **R** `--fix` applies only fixes whose status is `Safe`.
- **rustfmt** (native-toolchain backend) honors the 120-column line width.
- **`php_version`** rejects a non-numeric component (e.g. `"8.x"`) instead of
  silently defaulting it to `0`.

### Performance

- **mago** caches its rule registry per run instead of rebuilding it for every file.

### Changed

- `oxc` and `sqruff` no longer advertise a `fix` capability they did not implement.

### Documentation

- Corrected ADR drift (configuration, backend selections, distribution, catalog,
  caching) and README inaccuracies (version-pin examples, MCP tool names and
  parameters, version badges).

[0.1.6]: https://github.com/Goldziher/poly/releases/tag/v0.1.6
