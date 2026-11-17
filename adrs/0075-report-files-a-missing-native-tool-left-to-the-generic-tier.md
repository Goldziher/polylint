# 0075 — Report the Files a Missing Native Tool Left to the Generic Tier

- Status: Accepted
- Date: 2026-11-17
- Amends: ADR 0014
- Related: ADR 0027, ADR 0053, ADR 0072

## Context

ADR 0014 lets a default-on native tool (`rustfmt`, `gofmt`, `shfmt`, `shellcheck`), or an opt-in one switched on, be
absent from `PATH`: its language falls through to the tier-2 reindenter, and a missing `shellcheck` lints nothing.
That keeps poly working on a host without the toolchain. The only trace was a `tracing` warning, which hook output and
CI logs bury, so `poly fmt` printed `All formatted.`, exited 0, and the JSON document carried nothing.

The reindenter cannot judge rustfmt's layout. A chained call that `cargo fmt --check` rejected passed `poly fmt` on a
host without rustfmt (issue #230, the miss fixed by 44d5b465).

## Decision

- **The run report names the files.** `LintRun` / `FormatRun`, and the `LintDocument` / `FormatDocument` the CLI and
  the MCP server serialize, carry `degraded: [{tool, language, reason, files}]`, one entry per tool and language,
  omitted when empty. The pretty summary prints one line per tool beneath the file count at every verbosity
  (`rustfmt not found on PATH: 3 Rust files formatted by the generic tier`), and the `json` / `toon` formats echo it
  on stderr.
- **Degraded is not skipped.** The files were formatted by tier 2, or linted by the other engines, so they stay in
  `checked`, stay out of `skipped`, and are not charged to `--deny-skips` or `--max-skips`. Exit codes do not change.
  A file the run did skip or fail on is reported there instead, except that a lint file skipped as
  `no lint rules for <language>` keeps its degraded entry, since the missing linter is why it has none.
- **Derived from the plan.** `PlanPolicy::missing_tool` (ADR 0072's facet) answers once per `(config, language)`
  plan from the memoised presence probe, and the runner groups the plan's files after the run. A cached result
  therefore reports the degradation as a fresh one does, and the per-file loop is untouched.

## Consequences

- Good, because a missing toolchain is visible where the verdict is read, in the summary and in the document a gate
  parses.
- Good, because a host without the toolchain keeps passing: ADR 0014's degradation and every exit code stand.
- Bad, because a gate that wants the full toolchain has to read `degraded` itself; no flag fails the run on it.
- Neutral, because a component engine runs no host tool, so it never reports one; its absence is the components
  report's concern.

## Alternatives considered

- **Count the files as skipped.** Rejected: every `--deny-skips` gate would fail on a host without rustfmt, which is
  the host ADR 0014 promises keeps working, and the files were in fact formatted.
- **Record the fallback from inside `Engine::format`.** Rejected: a cache hit never calls the engine, so a second run
  would report nothing, and collecting from rayon workers needs shared state on the hot path.
- **Fail the run when a default-on tool is absent.** Rejected: it reverses ADR 0014.
