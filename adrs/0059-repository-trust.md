# 0059 — Repository Trust for Config Keys That Run Programs

- Status: Accepted
- Date: 2026-11-15
- Related: ADR 0012, ADR 0013, ADR 0018, ADR 0020, ADR 0035

## Context

`poly.toml` is a file every clone of a repository brings with it, and several of its keys make poly run a program:
`[tools.<name>] bin`, `url`/`sha256` (a download poly then executes), `env` (`LD_PRELOAD`, `DYLD_INSERT_LIBRARIES`,
`GIT_CONFIG_*`), `args`, `extra_args`, `format_extra_args`, `lint_extra_args`, `new_from_rev` and `root`; `[hooks]`,
whose `workspace = true` jobs plain `poly lint` runs in its whole-project phase and whose `skip`/`only` guards run
through `sh -c` while the hooks are lowered; `[cache] dir` and `[cache.sccache] dir`/`bin`; and the oxc options
`js_plugins = "external"` and `external = true`, which hand files to an `oxlint`/`oxfmt` found in the repository's own
`node_modules/.bin`. A local `extends` path could also read any file the user can read, including through the MCP
server, whose workspace guard checked only the top-level config. So `poly lint .` in a freshly cloned repository, or
an MCP `lint` call steered by text a model read, executed whatever the repository named, with no prompt.

## Decision

- **A per-user trust store.** `poly trust [PATH]` records the repository holding PATH in `trust.json` in poly's
  per-user config directory (`POLY_CONFIG_HOME`, else the platform config directory via `etcetera`, as the cache
  resolves its own: `$XDG_CONFIG_HOME/poly` or `~/.config/poly` on Linux, `~/Library/Preferences/poly` on macOS,
  `%APPDATA%\poly` on Windows). A repository is keyed by its canonical git root, or
  the config's directory outside git. `--revoke` forgets it, `--list` lists every record and `--show` prints the
  exec-capable keys each config sets and whether they are trusted, changing nothing.
- **Bound to the values, not the path.** A record holds a blake3 digest per `poly.toml` in the repository, computed
  over the config's directory (relative to the root) and every exec-capable key of its fully merged table: `extends`
  bases, the hierarchical cascade (ADR 0018) and `poly.local.toml`. Table keys are hashed in sorted order, so
  reordering a file does not untrust it; any change to an exec-capable value does. Both the cascaded table and the
  file loaded alone (`--config`, nested hook configs) are recorded. Local `extends` bases outside the repository are
  listed by canonical path.
- **Untrusted keys are ignored, not fatal.** When a config's digest is not in the store, its exec-capable keys are
  removed before deserialization and one note per repository on stderr names them and says to run `poly trust` (also
  under `--format json`/`toon`, so stdout stays one document). A bare `bin` (a name looked up on `PATH`) and
  `install = "auto"` downloads of a catalog release stay allowed: they run the user's own programs or a release whose
  origin and checksum poly's catalog fixes. `[hooks]` stays in the typed config but is withheld: `poly lint` neither
  lowers it (so no guard runs) nor runs any whole-project tool, and reports `skipped (untrusted)` in the
  whole-project section and in the MCP `workspace_lint` result. A local `extends` base outside the repository is not
  read until a record names it.
- **Hooks.** `poly hooks install` records trust, since installing the shims is the user's explicit decision to run
  the repository's `[hooks]` on every commit. `poly hooks run` and the installed `hook-impl` refuse an untrusted
  config, including a nested one, with a message pointing at `poly trust --show`; they do not silently run a subset.
- **One-run overrides.** `--trust` and `POLY_TRUST=1` honour every key for one process, for CI and throwaway
  checkouts; neither writes a record. `--trust` is a flag of this process only: the `poly lint`/`poly fmt` children a
  hook spawns see it only through `POLY_TRUST`.
- **MCP.** `poly mcp` always enforces the store: neither `--trust` nor `POLY_TRUST` applies, since the client side
  must not be able to widen what a model-driven server executes. The server reads the store once; a repository
  trusted after it started needs a restart.
- **Store hardening.** The store directory is refused, not tightened, when another user owns it or group or others
  can write it, because a writer could already have planted a record; an unreadable store trusts nothing and says so.
  The file is written to a `0600` temporary in the same directory, synced and renamed over the old one.
- **Always-on restrictions**, trusted or not: `bin` is a bare program name or an absolute path, never a relative
  path resolved against wherever poly runs; `root` is relative and free of `..`; a `[tools]` `url` must be https on
  an origin the catalog names for the tool, the binding `poly.lock` URLs already had (a `POLY_GITHUB_API_URL` the user
  set is accepted as written, plain http included, since it names their own mirror); and `env` may not set `LD_*`,
  `DYLD_*`, `GIT_CONFIG*`, `GIT_EXEC_PATH`, `GIT_SSH`, `GIT_SSH_COMMAND`, `GIT_ASKPASS`, `GIT_PROXY_COMMAND`,
  `GIT_EXTERNAL_DIFF`, `BASH_ENV`, `ENV`, `NODE_OPTIONS`, `PERL5OPT`, `RUBYOPT`, `PYTHONSTARTUP`,
  `JAVA_TOOL_OPTIONS`, `JDK_JAVA_OPTIONS` or `_JAVA_OPTIONS` (case-insensitively). `PATH`, `JAVA_HOME` and the
  like stay allowed in a trusted repository: they are how a tool is pointed at its toolchain.
- **Enforcement is process-wide** and set by the front end. `poly-config` gates only when told to, so a library
  caller or unit test keeps the old behaviour; every `poly` subcommand enforces.

## Consequences

- **Breaking** for a repository relying on these keys: until someone runs `poly trust` (or sets `POLY_TRUST=1` in
  CI), its tool overrides are ignored and its whole-project tools, `cargo clippy` included, do not run under
  `poly lint`. `poly hooks install` users are trusted at install time; a later pull that changes `[hooks]` or any
  other gated key makes the next commit refuse until `poly trust`, as `direnv` does for a changed `.envrc`.
- Every change to `[hooks]`, including a non-executing one such as a stage's `files` glob, needs re-trust: the whole
  table is hashed, because separating the executing keys from the rest of the hook model would be a second schema to
  keep in step with it.
- A `[tools]` url on a host the catalog does not name is now refused even when trusted; such a tool is installed by
  hand and named with `bin`.
- What remains outside the model: an enabled catalog tool still runs from `PATH` and reads its own repository config
  (some tools load plugins from it); `rustup` honours a repository's `rust-toolchain.toml`; native toolchains run as
  the user configured them. poly does not sandbox the tools it runs.

## Alternatives considered

- **Prompt on first use.** Interactive prompts break CI and every non-terminal caller (hooks, MCP), and a prompt that
  appears on each change trains users to accept it.
- **Fail the run on any exec-capable key.** Safer to explain but makes every repository that uses them unusable until
  trusted, including for the non-executing parts of `poly lint`.
- **Trust by path only.** A record that does not bind the values lets a `git pull` swap in a different command
  without the user seeing it.
- **Hash the whole config.** Any edit to `[defaults]` or a rule selection would untrust the repository, which teaches
  users to re-trust without reading.
- **An allowlist of programs instead of a trust store.** It cannot cover `[hooks]` jobs, which are arbitrary shell.
