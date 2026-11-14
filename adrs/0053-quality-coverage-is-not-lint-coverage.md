# 0053 — Quality Metrics Are Not Lint Coverage

- Status: Accepted
- Date: 2026-11-14
- Amends: ADR 0027
- Related: ADR 0031

## Context

ADR 0027 let the `quality` tier claim lint coverage (`Engine::provides_language_lint`) for the twelve
languages it structurally models, so a file only `quality` examined counted in `summary.checked`. Its
rules measure size and complexity: function length, parameter count, nesting, cyclomatic complexity. None
says whether the code is correct. A clean run over a C tree therefore printed `N files linted` with no
correctness rule having run, the same overclaim ADR 0027 refused for languages with no model, one step
removed (issue 168).

## Decision

- **`quality` never claims lint coverage.** `provides_language_lint` is `false` for it in every language.
  It still runs and its warnings still report.
- **A file whose only lint-side engine is `quality` is skipped**, with the reason
  `no lint rules for <language> (quality metrics only)`. It keeps the `no lint rules for` prefix, so
  existing matching holds. The file leaves `summary.checked` and counts against `--deny-skips` and
  `--max-skips`.
- **The reason is chosen by `coverage::models_language`** (the former coverage predicate, unchanged), so a
  language `quality` cannot model keeps the plain `no lint rules for <language>`.
- **Languages with a real linter are unaffected.** Python (ruff), JavaScript and TypeScript (oxc), and Go,
  Rust, Java, Kotlin, C# and Ruby (built-in ast-grep pack, ADR 0029) still count. Today only C and C++
  become quality-only. A new pack rule or backend for them moves them back to checked.

## Consequences

- `poly lint` over a C or C++ tree reports those files as skipped, and a gate on `summary.checked` or
  `--max-skips 0` sees them. Exit code 0 is unchanged without a strictness flag.
- The skip is a limit of poly, not of config, so it is charged to the skip budget like any other
  `no lint rules for` reason (ADR 0031).
