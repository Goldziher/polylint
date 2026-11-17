# 0079 — A Project Check May Decline the Project

- Status: Accepted
- Date: 2026-11-17
- Related: ADR 0053, ADR 0072, ADR 0074

## Context

`ProjectAnalysis::project_check` answers findings grouped by file, and an empty answer is a clean pass. An engine that
finds nothing it can check had no other answer. tsrs builds its program from a `tsconfig.json`; on a project without
one it logged a debug line and returned no findings, so a run with `[engines.tsrs] enabled = true` read as a passing
type check of every TypeScript file. skif did the same on a project with no `package.json` or workspace declaration.

The per-file tier already refuses this shape: `Engine::skip_reason` and the `no lint rules for <language>` skip make a
file nothing examined visible and charge it to `--deny-skips` (ADR 0053). The project phase has counted skips too, for
an `--only` that selects nothing, but no engine could produce one.

## Decision

- **`ProjectReport`.** `ProjectAnalysis::project_report` answers `ProjectReport::Checked(findings)` or
  `ProjectReport::Declined(reason)`. It defaults to the findings of `project_check`, so an engine that never declines
  changes nothing; one that can decline overrides `project_report` and answers `project_check` with
  `ProjectReport::into_findings`. The host calls `project_report`.
- **A decline is a counted skip of the project root.** The runner records `<engine>: <reason>` against the root of
  the config the check ran for, in `skipped`, counted by `summary.skipped` and charged to `--deny-skips` /
  `--max-skips`, since it is not a withdrawal. The engine is not counted as having checked anything.
- **A decline is not cached.** The result cache holds findings; an engine decides to decline before it checks
  anything, so asking it again on the next run costs little and keeps the skip in every report. The tsrs and skif
  engine versions move so that an empty result cached before this change no longer answers.
- **The wire carries it as a new payload.** `Payload::ProjectDeclined(reason)` answers `ProjectCheck`. Protocol 3 is
  extended, not bumped: payloads are externally tagged by variant name, so a host that knows the variant reads every
  answer of a component that predates it, and a component that predates it never sends it.
- **tsrs and skif decline.** tsrs on no `tsconfig.json` at or above its `dir` (`no tsconfig.json`), skif on no
  `package.json` or workspace declaration (`no package.json or JavaScript workspace`). ty, pyrefly, rumdl and adrlint
  return no findings only when they are switched off or have no files, which the host never plans, so they do not
  decline.

## Consequences

- Good, because a type check that examined nothing is visible where the verdict is read, and a `--deny-skips` gate
  fails on it.
- Good, because the trait change is additive: every existing `project_check` implementation and caller keeps
  compiling and behaving as before.
- Bad, because a host built before this change cannot decode a decline: it reports the check as failed (exit 2)
  rather than skipped. That is loud rather than silent, and the component index pins components to the host that
  ships them.
- Neutral, because two trait methods now describe one check; `project_check` stays the simple form for engines and
  tests that never decline.

## Alternatives considered

- **Change `project_check` to return `ProjectReport`.** Rejected: it touches every implementation and test in five
  workspaces for an answer only two engines give.
- **A separate `project_skip_reason` asked before the check.** Rejected: a component engine would need a second request
  per check, and tsrs only knows whether a `tsconfig.json` governs the files once it has laid them out.
- **A trailing field on `Response` or a field of `Payload::Project`.** Rejected: a positional message one element
  longer fails to decode in a host that predates it just as an unknown variant does, so it buys no compatibility, and
  `Payload::Project` is a newtype with no room for a field.
- **Report the decline as a finding.** Rejected: a finding on no file is not a skip, `--deny-skips` would not count
  it, and a project with no TypeScript config is not a defect in any file.
