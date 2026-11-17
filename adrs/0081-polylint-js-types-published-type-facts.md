# 0081 — skif's Type-Aware Analysis Is the Published Crate `polylint-js-types`

- Status: Accepted
- Date: 2026-11-17
- Amends: ADR 0078
- Related: ADR 0003, ADR 0010, ADR 0044, ADR 0051

## Context

skif (ADR 0044) answered from tsrs which members of a project's classes, enums and namespaces are read, so that its
`unused-class-members` rule could credit reads through typed receivers, overrides and implemented interfaces. That
analysis lived in skif's `skif-js-types` crate, built on `skif-tsrs`, which became `polylint-tsrs` (ADR 0078). skif
0.9.0 drops its type-aware analysis, and with it the crate's only user.

The analysis is the larger half of the work skif put into tsrs: program planning per `tsconfig.json`, a schedule that
builds programs before the query is known, member crediting through bases, implementations and subclasses, package
coverage with the reason a package stays untyped, and the measured known false findings of the typed run. Without a
home it would be lost with skif's history.

ADR 0078 made `polylint-tsrs` the one crate poly publishes, on the grounds that it is reusable by any host of tsrs and
has no git dependency.

## Decision

- **The analysis is a crate of its own, `polylint-js-types`**, at `components-tsrs/crates/polylint-js-types`,
  imported from skif's `crates/skif-js-types` at skif commit `75500e05` (MIT, same author; `ATTRIBUTIONS.md`). It
  depends on `polylint-tsrs` instead of `skif-tsrs`. poly's engine does not use it.
- **It is published to crates.io and versioned on its own**, starting at `0.1.0`, under the terms ADR 0078 set for
  `polylint-tsrs`: not part of poly's lock-step release, a `plain-ptrs` feature off by default that forwards to
  `polylint-tsrs` and `tsrs_core`, and tests run in both pointer modes. It is the second exception to the rule of ADR
  0003 and ADR 0010; every other poly crate stays unpublished.
- **Its dependencies are registry crates and `polylint-tsrs` alone.** The four items it took from `skif-core` are its
  own now: the `FileSystem` trait (the subset of skif's trait the analysis reads, with the same signatures, plus
  `DirEntry`, `PathKind`, `MemoryFs` and an `OsFs` for the disk), the hash map and set (`FxHash` from `rustc-hash`
  instead of fixed-seed `ahash`; both are the same in every process), and the timing switch (`enable_timings`,
  process-wide and off by default, instead of `skif_core::timing`). The `tsrs_*` crates are held at the exact version
  `polylint-tsrs` pins.
- **The known false findings travel with the crate.** `KNOWN-DEFECTS.md` lists the three measured cases where the
  typed run reports a live member unused; each needs a failing test before it is fixed.

## Consequences

- Good, because skif's type-aware analysis, its tests and its measured defects stay available to any host, including
  a later poly rule, without depending on skif.
- Good, because no tsrs type appears in its API: a host needs no tsrs pin of its own to use it.
- Bad, because poly now has two crates.io releases to make by hand or by trusted publishing, and the order matters:
  `polylint-tsrs` first when both move.
- Bad, because the crate is published from a repository whose binary does not use it, so only its own tests and the
  CI step that runs them exercise it.

## Alternatives considered

- **Leave it in skif's history.** Rejected by the maintainer: the analysis is the reason the tsrs layer exists, and
  it is reusable as is.
- **Keep depending on `skif-core`.** Rejected: skif-core is skif's own internal crate, and the analysis used four
  small items of it.
- **Merge it into `polylint-tsrs`.** Rejected: `polylint-tsrs` is a program layer for any tsrs host; member facts
  for dead-code analysis are one use of it, with dependencies (`rayon`, `tsrs_checker`) the layer does not need.
