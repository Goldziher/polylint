---
description: The poly MCP server — the four tools (check, fix, info, manage) split by read/write class, their kind/scope/topics/action params, component downloads, async Tasks, and the isError contract
name: poly-mcp
license: MIT
metadata:
    author: Goldziher
    repository: https://github.com/polylint/polylint
---

<!--
AI-RULEZ :: GENERATED FILE — DO NOT EDIT
Content-Hash: blake3:d648ec45380f56c14ddbb16137e6d1db7319baf7a24494cb81b609fa4dd31ae5
Source-Hash: blake3:da70d54d8c2126bf3777e5c8e7e79f8b53fb3e4255a3df3861975e837f0f9417
Schema-Version: v1
-->

# poly MCP

`poly mcp` is a stdio MCP server exposing poly's lint/format/cache/rules/config surface as four
tools. Prefer it over shelling out to the CLI when working through MCP — every tool returns
typed `structured_content` plus one text block that mirrors the CLI's `--format json`, or the
compact TOON encoding when asked. Tools declare no `outputSchema`: the output schemas are MCP
resources under `poly://schema/` (`lint`, `format`, `failure`, `workspace`, `info/version`,
`info/rules`, `info/components`, `info/cache`, `info/validate`, `manage/cache`,
`manage/components`, `identity`, `poly.toml`).

`poly mcp --config <PATH>` pins a fallback config file for requests that do not name one.

Requests are confined to the directory `poly mcp` was started in: every `paths`, `dirs` and
`config` argument is canonicalized (symlinks and `..` resolved) and one outside that directory is
rejected with an error. The `--config` file itself is always accepted. `poly mcp
--allow-outside-workspace` turns the confinement off.

## Tool surface

Four tools, split along the annotation boundary (ADR 0076) so a client can grant permission per
tool name. The old fourteen names (`lint`, `lint_fix`, `format_check`, `format_write`,
`workspace_lint`, `workspace_lint_fix`, `cache_stats`, `cache_clean`, `rules`, `config_show`,
`components`, `components_install`, `components_uninstall`, `version`) are gone, with no aliases.

| tool     | does                                         | annotations                                              |
| -------- | -------------------------------------------- | -------------------------------------------------------- |
| `check`  | lint or check formatting; writes nothing     | not read-only, not destructive, idempotent, open-world   |
| `fix`    | apply lint autofixes or formatting in place  | destructive, open-world                                  |
| `info`   | read-only introspection                      | read-only, idempotent, closed-world                      |
| `manage` | cache and component maintenance              | destructive, open-world                                  |

`check` is not read-only because it runs the tools and hooks the repository's own `poly.toml`
configures (and may download a missing engine component): only run it against repositories you
trust. Gate `fix` and `manage` behind explicit intent.

### `check` and `fix`

`check` mirrors `poly lint` / `poly fmt`; `fix` mirrors `poly lint --fix` / `poly fmt --fix`
(and fix mode for the whole-project phase). Same parameters:

- `kind` — `lint` (default) or `format`.
- `scope` — `files` (default): the per-file tier over `paths`, like `poly lint --workspace never`
  or `poly fmt`. `workspace`: the whole-project phase only (`cargo clippy` / `cargo-sort` /
  `cargo-machete` / `cargo-deny` and inline whole-project jobs) over the whole repository; takes no
  `paths`, lint only. `all`: both, as `poly lint` does; lint only.
- Run options shared with the CLI: `paths` (empty means the current directory), `exclude`
  (gitignore-style globs merged with `[discovery] exclude`), `config` (path to a `poly.toml`),
  `include_excluded`, `only` / `skip` (mutually exclusive engine selection), `max_skips` (0 is
  `--deny-skips`), `install` (`config` default, `auto`, `never`), `on_missing` (`fail` / `skip`),
  `no_cache`, `jobs`, `fix_generated`, `debug`.
- `format` — `json` (default) or `toon`; selects only the paired text block, `structured_content` is
  always JSON.

The result is the CLI's `--format json` document — `{results, errors, skipped, summary, configs}`,
plus optional `suppressed`, `components` and `discovery` — with a `poly` identity block.
`scope: workspace` returns the whole-project report (`passed`, `fixed`, `tools[]`, optional
`untrusted`); `scope: all` returns the lint document with that report under a `workspace` key.
Arguments that do not go together (for example `kind: format` with `scope: workspace`) fail with
a `usage` error.

A whole-project scope executes tools against the live worktree, and their own side effects — a
refreshed lock file, a populated build or type-checker cache — are not poly's to control, even in
`check`.

### `info`

Read-only, never downloads, never runs repository tools. `topics` is an array of `version`,
`config`, `validate`, `rules`, `components`, `cache` (default `["version"]`); the result has one key
per topic.

| topic        | answers                                                                         |
| ------------ | ------------------------------------------------------------------------------- |
| `version`    | binary health: `executable_current`, `executable_status`, `uptime_seconds`, the compiled-in `engines` map, `components` |
| `config`     | exactly `poly config show --format json`: `{config, resolution}`               |
| `validate`   | `poly config validate`: `{valid, files: [{path, problems: [{path, key?, message}]}], partial: [{component, engines}]}` |
| `rules`      | `poly rules list --format json`; with `test: true`, also runs `*-test.yml` snippets and a failing test sets `isError` |
| `components` | `poly components status`: `bundled`, `installed`, `not installed`, `unavailable`; with `paths`, what they need, what is `missing`, what would be `would_download` |
| `cache`      | `poly cache stats --format json`: `{format_version, on_disk_version, namespaces[], total_entries, total_bytes}` |

Parameters besides `topics`: `config`, `paths` (`components`: paths to judge; `validate`: config
files or directories), `install` (`components`: the policy `would_download` assumes), `dirs` and
`test` (`rules`), `format`.

`info {topics: ["version"]}` still answers when the server's executable was replaced or deleted,
which is what makes it the call that explains why the others stopped. The `rules` topic never
downloads: a core build without the `astgrep` component reports it missing. A topic the host cannot
answer carries `{error: {message}}` under its key and sets `isError`.

Remote git `extends` bases are resolved from the local source cache at the `poly.lock`-pinned
revision and never fetched; a base that is not cached is a `config` error telling you to run
`poly lock` or a CLI `poly lint` once.

### `manage`

`action` selects the operation:

- `cache_clean` — clear the result cache.
- `cache_gc` — evict by `max_age_days` and/or `max_size` (for example `500M`, `2G`).
- `components_install` — download and verify components by `names` (components or presets),
  `languages` and/or `paths`; `config`, `dry_run` reports the plan. Writes only the component cache.
- `components_uninstall` — remove components by `names`. Refuses a component loaded in this
  process (`isError`).
- `components_prune` — remove cached archives nothing references or used recently.

The cache actions return `{command, freed_bytes}`, like `poly cache clean|gc --format json`.

## Components

In a core build (every release binary except Linux musl) the engines are downloadable components;
the bundled Linux musl build links them in and nothing is ever missing. The result's `components`
block (`used`, `installed_now`, `missing`) is present only when a run needed one. A missing
component that fails the run sets `isError`; one skipped under `install: "never"` shows in `skipped`
and `components.missing` without failing it. Check `components.missing` before reading an empty
`results` as clean.

`check` and `fix` are open-world: with `install: "config"` (default, follows `[components] install`)
or `"auto"` they download what the run needs before running. `"never"` downloads nothing and skips
the files a missing component would have handled. A server started `--offline` never downloads
whatever `install` says. `install` governs engine components only; an opt-in recipe tool with
`[tools.<name>] install = "auto"` follows its own setting.

There is no `hooks` tool: `poly hooks` is CLI-only.

## Tasks

Whole-project scopes, and runs or installs that must download a component, return an async Task
(SEP-2663) when the client declared the tasks capability: the call returns a task handle and the
client polls `tasks/get`, optionally `tasks/cancel`. The status message names the components; there
is no byte-level progress and no `notifications/progress`. A client without the tasks capability
gets a blocking, synchronous result from the same call.

## Every result identifies the binary that answered

Each result carries a `poly` block (version, build id, channel, executable, pid, `flavor` —
`bundled`, `core` or `partial` — `components_lock` — a digest of the embedded component lock,
or null — and an `engines` digest — a blake3 hash over the sorted compiled-in engine name + version pairs,
excluding native-toolchain and catalog engines since those depend on the host and on config)
in `structured_content` and in `_meta`, because an MCP caller has no `poly --version` to fall
back on. The digest is what distinguishes two builds sharing one version number (a `dev`
build, or two builds of one tag) when their wrapped crates differ; the full engine map is on
the `info` `version` topic. The server fingerprints its own executable at startup and re-checks it
per request: if the binary is replaced or deleted underneath a long-lived server, every call but
`info {topics: ["version"]}` fails rather than answering with superseded behaviour.

## Per-file outcomes: checked, skipped, error

`summary` is `{checked, skipped, errored}` — the run's own count of what it did, and the field
to gate on. It cannot be derived from `results`: `results` holds only files with something to
report, so a file that was checked and found clean produces **no** `results` entry at all — a
present entry does not mean checked, either, since `results` also carries a synthetic entry
(empty `diagnostics`, `skipped` or `error` set) for every file the run declined or failed on.
Gate on `summary.checked`, not on an empty `results`. The skipped/error sets and `results` overlap
on purpose:

- **Checked** — counted in `summary.checked`. Not derivable from `results`.
- **Skipped** (`skipped` field, and the top-level `skipped` array) — poly correctly declined the
  file (e.g. a template dialect no backend handles). Not a failure.
- **Errored** (`error` field, and the top-level `errors` array) — poly failed to process the
  file (unreadable file, backend crash, bad engine config). Distinct from `skipped` on purpose:
  a skip is a deliberate decision, an error is poly failing on a file it accepted.

## isError

**An MCP-driving agent must check `isError` (or `summary.errored`) before trusting any other
part of the result.** `isError: true` means the run did not verify what was asked:

- a file poly failed on (non-empty `errors`),
- a blocking missing component,
- skips over `max_skips`,
- a run that could not start.

A run that could not start returns `{error: {kind, message}, summary: null}`, where `kind` is one of:

- `paths` — a named path does not exist (this used to read as a clean pass),
- `workspace` — a path outside the server's directory,
- `config` — the config or `poly.lock` failed to load,
- `pipeline` — for example an unknown `only` engine,
- `render` — the result could not be serialized,
- `usage` — arguments that do not go together.

Error-severity findings and formatting drift are **not** errors: they are the CLI's exit 1 and
appear in `results`. Check `isError` first; then read `summary` for coverage and `results` for
diagnostics.
