# 0070 — R Formatting and Linting with air and jarl in a Workspace of Their Own

- Status: Accepted
- Date: 2026-11-16
- Related: ADR 0003, ADR 0039, ADR 0040, ADR 0051

## Context

R formatting was the opt-in host tool `styler`, run through `Rscript` (`native_tool/spec.rs`), which needs R and the
styler package on the host; R had no linter outside the opt-in `air` and `jarl` recipes (ADR 0035), which download a
release binary. Both tools are Rust libraries: [air](https://github.com/posit-dev/air) (Posit's formatter) and
[jarl](https://github.com/etiennebacher/jarl) (a linter modeled on lintr) are built on air's own parser and on biome's
formatter and rowan crates. air's crates are `publish = false`, jarl's are unpublished, and the crates.io `jarl` and
`workspace` are unrelated crates.

air and jarl pin a biome revision of their own (`biomejs/biome` 8c43bc0), and `components/` pins another
(ec3b4b2) for its biome engine, which reads the same crates (`biome_formatter`, `biome_rowan`, `biome_parser`). A
lockfile holds one source per git rev, so the two tools could join `components/` only by moving one of the two
consumers to the other's biome, and a biome bump moves its API.

jarl's 0.7.1 tag builds against personal forks of air (`etiennebacher/air_pratt`, with no revision in its manifest),
ark (`etiennebacher/ark`, a branch) and biome (`lionel-/biome`). Its main branch (dc3f249, the day this ADR was
written) pins the upstream repositories by revision: `posit-dev/air` 9b7627a, `posit-dev/ark` d2d011b and
`biomejs/biome` 8c43bc0. air's 0.13.0 tag is a release branch that builds against yet another biome fork.

## Decision

poly gets an `r` component, built from a cargo workspace of its own, `components-r/`, like pyrefly, secrets and tsrs
(ADR 0040).

- One boundary crate. `components-r/crates/poly-engines-r` is the only crate that depends on air and jarl, and the
  alef generated `poly-engines-r-component` is its producer. It holds two engines, `air` (format) and `jarl` (lint and
  safe fixes), registered for R. The component is on by default and download-only: no bundled build links it.
- Git pins, by revision, all from the tools' own upstream repositories: `posit-dev/air` at 9b7627a (the revision
  jarl pins, so formatter and linter share one parser and one biome), `etiennebacher/jarl` at dc3f249 (jarl-core, and
  jarl-semantic with it), and transitively `posit-dev/ark` and `biomejs/biome` at the revisions those two pin. The
  workspace's `deny.toml` allows exactly those four sources. The pins are one revision per repository.
- Why not the release tags. jarl 0.7.1 depends on forks and a branch, which a lockfile can pin but a reviewer cannot
  audit as upstream, and air 0.13.0 uses a biome fork that jarl's tag does not; mixing them would put three biome
  sources in one lockfile. The chosen revisions are post-release commits of the upstream repositories. The formatter
  therefore differs from the `air` 0.13.0 binary where air's main branch changed behavior since the tag (suppression
  directives were reworked); `tests/air.rs` pins the output of the chosen revision. Moving to the next air and jarl
  releases is a pin bump with a `version()` change and a refreshed lockfile (`tests/version_audit.rs`).
- Configuration is the tools' own. air reads the nearest `air.toml` (or `.air.toml`) above the file; a project that
  has one is formatted as `air format` formats it, so poly's 120-column default is not laid under it. A project without
  one gets poly's defaults (line width 120, the language's indent width, `lf` endings). `[engines.air]` keys
  (`line_width`, `indent_width`, `indent_style`, `line_ending`, `persistent_line_breaks`, `assignment_style`, `skip`,
  `table`, `default_table`) override either. air's `exclude` and `default-exclude` patterns are reported as skips. jarl
  reads the nearest `jarl.toml`; `[engines.jarl]` takes the shared rule selection (`select`, `extend_select`,
  `ignore`, `rules.<name>.level`), `check_roxygen` and `min_r_version`, and the other keys of `rules.<name>` are that
  rule's `[lint.<name>]` options. A `CORR` rule is an error and every other rule a warning unless `level` says
  otherwise. Only jarl's safe fixes are attached.
- No R installation. jarl is given no library of installed packages, so rules that resolve a bare call to its package
  see only what the file and the project's `NAMESPACE` say.
- Precedence with `styler`. `styler` stays an opt-in host tool. When the component serves R, the format plan drops
  every native tool that is off or absent, because such a tool formats through the tree-sitter reindenter, which would
  run after air and fight it. With `[engines.styler] enabled = true` and the tool present, styler runs after air;
  `[engines.air] enabled = false` makes it the only formatter. The `air` and `jarl` recipes (ADR 0035) are unchanged:
  an enabled `air` recipe still takes R formatting over from the component.
- Cache keys. `air`'s format key folds in a digest of the governing `air.toml`; `jarl`'s lint key folds in `jarl.toml`
  and, for a file in an R package (or a script that calls `source(`), a digest of the R and native sources and the
  package metadata that jarl's cross-file rules read.

## Consequences

- R files are formatted and linted with no R installation, in process. A core build downloads the `r` component on
  the first run over R files, as it does for every other family, and a missing component is never replaced by the
  tree-sitter reindenter.
- poly carries two biome sources, one per workspace, until air and jarl follow poly's biome (or the reverse).
- The lockfile pins unreleased commits of two small projects, and `cargo deny` allows four git sources for them. The
  crates.io watch lists the pinned crates so a real release is noticed.
- jarl's cross-file rules (`unused_function`, `duplicated_function_definition`) read the package from disk, so the
  `jarl` engine is cached on a package digest and re-reads the package once per run.
- jarl's `exclude` and `include` file filters, and air's `exclude` patterns of a user-level config, are not applied;
  poly's `[discovery]` and `per_file_ignores` decide which files run.

## Alternatives considered

- Recipe-tier entries for the release binaries only (the existing `air` and `jarl` recipes): they download and run a
  binary per run, have no per-file API, and need the platform's glibc; they stay available as opt-ins.
- Move air and jarl into `components/`: needs one biome revision for three consumers, or a second biome source in
  that lockfile, which is what a workspace of its own avoids.
- Pin jarl 0.7.1 and air 0.13.0: three biome sources and forks without a revision, as above.
- Vendor the crates: ADR 0003 forbids it.
