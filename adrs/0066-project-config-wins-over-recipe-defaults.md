# 0066 — A Project's Config Wins Over a Recipe's Default Arguments

- Status: Accepted
- Date: 2026-11-16
- Amends: ADR 0035

## Context

ADR 0035 runs recipe tools from catalog argv templates whose flags poly owns. To stop a cold golangci-lint run from
exceeding a project's `run.timeout` (sized for warm CI caches), exiting 4 and being rerun, the golangci-lint `lint` and
`fix` commands gained a fixed `--timeout=0`, leaving `POLY_TOOL_TIMEOUT_SECS` to bound the run. A fixed flag overrides
every project's `run.timeout`, including one a project set on purpose; the only way back was
`lint_extra_args = ["--timeout=5m"]`, a second place to state what the project's config already says.

## Decision

- A recipe may declare `config_defaults`: `{arg, key}` pairs. poly adds `arg` to the lint and fix commands only when
  the config the tool reads leaves the dotted `key` unset. The arguments go ahead of the user's `extra_args`, so a
  flag of the user's own still wins, and an `args` override, which owns the whole argv, gets none.
- The config consulted is the one the recipe's config fingerprint already covers: a file named by a `config_flags`
  flag, else the first of the recipe's `config_files` found in poly's working directory or an ancestor up to the
  repository root. TOML and JSON are parsed; YAML is read by block indentation plus one-level flow mappings, so the
  core links no YAML parser for this.
- golangci-lint declares `{"arg": "--timeout=0", "key": "run.timeout"}` in place of the fixed flag. A project that
  sets `run.timeout` keeps it; one that does not runs under poly's deadline, as before.

## Consequences

- Good, because a project's chosen timeout is honoured without repeating it in `poly.toml`.
- Good, because the decision is part of the pass fingerprints in the tool's `version()`, so adding or removing
  `run.timeout` invalidates cached verdicts along with the config fingerprint.
- Bad, because a project whose timeout is too short for a cold cache times out again; the remedy is its own config,
  or `lint_extra_args = ["--timeout=0"]`.
- Neutral, because a config golangci-lint finds only from a nested module directory, or in the home directory, is
  outside the consulted scope, the same bound the config fingerprint has.
