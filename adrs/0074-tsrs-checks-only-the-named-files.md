# 0074 — tsrs Checks Only the Files a Path-Scoped Run Names

- Status: Accepted
- Date: 2026-11-17
- Amends: ADR 0051
- Related: ADR 0019, ADR 0042, ADR 0058, ADR 0073

## Context

ADR 0051 made the tsrs check a whole-tree project check: the host hands it every JavaScript, TypeScript and JSON file
under `dir`, the check builds one program and reports every diagnostic, and the host keeps the findings on the files the
run named. The pre-commit `lint` hook runs `poly lint --workspace never <staged files>` in the staged snapshot (ADR 0019;
tsrs is a downloaded component, so ADR 0073's in-process path declines and the hook starts the child). Committing one
`.ts` file therefore type-checked the whole project, and threw away all but that file's findings.

`tsc` itself has the narrower question: `getDiagnosticsOfAnyProgram` takes an optional list of source files and
computes the syntactic, binding, semantic and declaration diagnostics of those files only, over the full program.
tsrs ports it unchanged.

The component protocol is positional since v3 (ADR 0058). `ProjectCheck.threads` was added as an optional trailing
field without a protocol version change (ADR 0042), and every component is built and released with the host it
serves.

## Decision

- **`ProjectRequest::targets`.** The project-check request carries the files whose findings the run reports, spelled as
  in `files`; `None` means every file. An engine may restrict its work to them, but its findings for each target must
  equal those of the same request with `None`. Other files stay inputs. Engines that ignore the field are unaffected.
- **The host's rule.** Only a check that reads a tree of its own (`ProjectDirs`) is narrowed, as its findings already
  were. The targets are the loaded files the run's `RunScope` admits: the named files, and the files under a named
  directory. They are `None` when every directory of the check lies under a named directory (`poly lint .`, or a
  directory that contains the project) and when the run named every loaded file, so such a run is the full check it
  was. A file-only run (the hook) resolves only loaded files sharing a name with a named file.
- **Cache.** The targets join the project check's cache key; a full run's key is unchanged. A narrowed result never
  answers a full run or a run over other targets, while a cached full result answers a narrowed run, as it did.
- **Wire.** `ProjectCheck` gains `targets: Option<Vec<String>>` as its last field, paths relative to `root` like
  `files`, with `#[serde(default)]` so a request one element shorter decodes as `None`. `PROTOCOL_VERSION` stays 3, as
  it did for `threads`: host and component ship together, and the committed schema is regenerated. A component built
  before this field rejects the longer request, which a release never pairs with this host.
- **tsrs.** With targets, the program is built from every file and config as before, and
  `get_diagnostics_of_any_program` gets the program's source files among the targets. A target the program does not
  hold (a JSON file it does not import, an excluded file, `package.json`) is not checked; no target at all builds no
  program. Every file is checked when the checked `tsconfig` is a target, since file-less diagnostics are reported on
  it, and when any file of the program has a syntax error, since `tsc` then reports the syntax errors alone and the
  targets' findings must match. The mapping revision of `version()` moves.
- **When narrowing pays.** tsrs computes the diagnostics of named files one after another on one checker, and those
  of every file on all its checkers at once. The check narrows while `targets x checkers < the program's own source
  files` (the bundled libraries not counted) and checks every file otherwise, so `poly lint src` over most of a
  program keeps the parallel pass. The host drops the findings it did not name either way.

tsrs moves to `7.1.0-beta.tsrs2` in the same series; its API and its `icu_normalizer` and `unicode-id-start` pins are
unchanged.

## Consequences

- A pre-commit hook over a few staged files pays for parsing and binding the program and for checking those files,
  not for checking every file.
- A change that breaks a file the run does not name is not reported by that run: a staged `util.ts` that changes a
  signature passes the hook while the unstaged `main.ts` calling it fails. `poly lint .` and CI check every file. This
  was already true of what the hook reported; it is now also true of what it computes.
- A global diagnostic that only checking another file produces is reported on the `tsconfig`, which a narrowed run does
  not report anyway.
- The checker answers per-file diagnostics as `tsc`'s watch mode and editors ask them; a diagnostic whose position
  depends on the order files are checked in could differ, as it can between `tsc` runs with different checker counts.

## Alternatives considered

- **Keep checking the whole program and filter.** Correct and simple, but the hook paid for every file.
- **Hand the check only the named files.** The program would not resolve imports into the other files.
- **Bump the protocol to v4.** Every component, the Go sidecars included, would move for one optional field that
  host and component already ship together; the `threads` precedent kept the version.
