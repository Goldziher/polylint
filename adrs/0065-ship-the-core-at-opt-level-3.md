# 0065 — Ship the Core at opt-level 3

- Status: Accepted
- Date: 2026-11-16
- Amends: ADR 0043

## Context

ADR 0043 ships the core (`--profile core`) at `opt-level = "s"` under a 14 MiB ceiling, trading a measured 1-5%
of cold-run CPU (`docs/size-baseline.md`) for about 5 MB of binary. The organization's release policy is now the
fastest code with symbols stripped, applied to every tool: adrlint, uncomment and skif build with fat LTO, one
codegen unit and stripped symbols, and so does polylint's `release` profile, which the bundled musl build and every
component use.

## Decision

- `[profile.core]` keeps fat LTO, one codegen unit, `strip = true` and `panic = "unwind"` (engine_guard catches an
  engine panic) and moves to `opt-level = 3`.
- The core ceiling moves from 14 MiB to 22 MiB (23,068,672 bytes) in the release workflow's size step and in
  `mise run check:core-size`. A local `aarch64-apple-darwin` build at `opt-level = 3` is 16,514,080 bytes; with the
  macOS-to-Linux ratio of about 1.15 recorded in `docs/size-baseline.md`, the gated `x86_64-unknown-linux-gnu` binary
  is expected near 19 MB, leaving about 20% headroom. CI remains the authority on the gated size.
- The ceiling still guards against unexpected growth; what moves into a component is unchanged (ADR 0039).

## Consequences

- Good, because the core runs the hot path (discovery, cache, runner, the tree-sitter formatters) without the
  `s` penalty.
- Bad, because the core binary and every download of it grow by about 5 MB.
