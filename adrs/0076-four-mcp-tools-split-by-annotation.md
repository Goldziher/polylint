# 0076 — Four MCP Tools, Split by Annotation Class

- Status: Accepted
- Date: 2026-11-17
- Related: ADR 0020, ADR 0059

## Context

`poly mcp` exposed fourteen tools, one per CLI capability: `lint`, `lint_fix`, `format_check`, `format_write`,
`workspace_lint`, `workspace_lint_fix`, `cache_stats`, `cache_clean`, `rules`, `config_show`, `components`,
`components_install`, `components_uninstall` and `version`. Three problems followed from that shape.

- **Tokens.** `tools/list` measured about 117 KB, 75% of it output schemas: every lint tool carried the full
  `LintDocument` schema and every tool repeated the identity block's. The model-visible part (names, descriptions,
  input schemas, annotations) was about 28 KB, and the server instructions 2.4 KB, repeating a run-result paragraph
  that four descriptions also carried. A check-then-fix flow loaded four tools.
- **Drift.** The server built its run options separately from the CLI, so `--only`, `--skip`,
  `--include-excluded`, `--no-cache`, `--jobs`, `--max-skips`, `--fix-generated` and `--on-missing` never reached it,
  a missing path read as a clean run, and a config error arrived as a JSON-RPC error rather than a result.
- **Honesty.** Annotations are per tool, and clients grant permissions per tool name. Some tools claimed less than they
  did (`rules` was read-only and closed-world but could download the `astgrep` component).

Merging everything into one tool with a `mode` argument would minimize tokens, but a client could then no longer
auto-approve checks while prompting on writes: the tool would have to carry the destructive hint for every call.

## Decision

- **Four tools, one per annotation class.** Read-only and mutating operations stay in separate tools, because
  annotations and client permissions are per tool; everything within one class is one tool with parameters.

  | tool | replaces | annotations |
  |---|---|---|
  | `check` | `lint`, `format_check`, `workspace_lint` | not read-only (runs the repository's configured tools), not destructive, idempotent, open-world |
  | `fix` | `lint_fix`, `format_write`, `workspace_lint_fix` | destructive, open-world |
  | `info` | `version`, `config_show`, `rules`, `components`, `cache_stats`, plus `config validate` | read-only, idempotent, closed-world |
  | `manage` | `cache_clean`, `components_install`, `components_uninstall`, plus `cache gc` and `components prune` | destructive, open-world |

  `check` and `fix` take `kind` (`lint` or `format`) and `scope` (`files`, the per-file tier; `workspace`, the
  whole-project phase; `all`, both, as `poly lint` does). `info` takes a list of `topics` and answers one key per topic.
  `manage` takes an `action`. `components_install` gains `manage`'s destructive hint; it is a rare, explicit call.
- **No aliases.** The old names are removed, not kept as unlisted aliases: nothing outside the maintainers used the
  surface, and the plugin, its skill and the Hermes allowlist ship lock-step with the binary.
- **One run request.** `poly_frontend_api::wire::RunRequest` is the run options both surfaces fill; the CLI maps its
  flags onto it and `poly_host_services::run::run_options` builds the `RunOptions` for both. The MCP input schema of
  `check` and `fix` is derived from it, so a new run option reaches the server by construction, and a test fails
  until a new CLI flag is mapped to a parameter or excluded with a reason. `poly_host_services::run::Verdict` is the
  one classifier of a finished run: the CLI maps it to exit codes 0, 1 and 2, the server maps "not verified" to
  `isError`.
- **Errors are results.** A run that cannot start (a missing path, a path outside the workspace, a config that does
  not load, an unknown engine) answers `isError: true` with `{error: {kind, message}, summary: null}`, never a
  JSON-RPC error, so the model reads the message and can correct its call.
- **Output schemas are resources.** Tools declare no `outputSchema`; the document schemas are served as resources
  under `poly://schema/`. The output contract is unchanged: `check` and `fix` return the CLI's `--format json`
  document plus the `poly` identity block.
- **Result semantics are stated once**, in the server instructions, not in every description.

## Consequences

- `tools/list` drops from about 117 KB to the size recorded by the `tools_list_fits_its_budget` test in
  `crates/poly-mcp/tests/surface.rs`, which fails when the surface grows past its budget.
- Client permission allowlists, saved prompts and scripted calls that name an old tool break. The changelog marks the
  change breaking.
- A caller that read a tool's `outputSchema` reads the resource instead.
- The `rules` topic of `info` never downloads; a core build without the `astgrep` component reports it as missing.
- `commit` message lint, `lock --check` and a `doctor` subset are not exposed: their logic lives in `poly-cli`, which
  the host services cannot depend on.
