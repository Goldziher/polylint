# 0071 — Run the lint, fmt and commit Hooks In Process Where the Child Can Be Reproduced

- Status: Accepted
- Date: 2026-11-16
- Amends: ADR 0067
- Related: ADR 0012, ADR 0019

## Context

ADR 0067 moved `file_safety` into the `poly hooks` process and left `lint`, `fmt` and `commit` as `poly` children,
naming what moving them needed: config loading that takes a root instead of the working directory, a `--set` layer
that is not process-wide, report paths and colour that match the child's, output into a buffer, and a timeout.

Working through them showed one constraint that the CLI cannot remove. Below the CLI, the per-file tier still reads
the process working directory in several places: discovery's exclude base, the per-file-ignore match bases, the
project root of a run, and inside engines the shellcheck batch's config anchor, the native tools' config
fingerprint, the recipe tools' run directory and the subprocesses those tools start. A child started in the staged
snapshot (ADR 0019) sees the snapshot through all of them. The `poly hooks` process sits in the repository, so an
in-process run over the snapshot would judge the snapshot's files with the worktree's tool configs.

`commit` has no such dependency: gitfluff's runner takes the working directory, the colour choice and its streams
as arguments, and the `[commit]` config loader takes a start directory.

## Decision

- **`commit`** lowers to `InProcess { name: "commit" }`. The handler parses `poly commit`'s arguments, runs
  gitfluff from the hook's directory into buffers with colour off (the child's stderr is a pipe, where gitfluff's
  `auto` resolves to off), and maps errors to the child's `poly commit: <error>` line and exit 2. It declines
  `--stdin`.
- **`lint` and `fmt`** lower to `InProcess { name: "lint" | "fmt" }` with the same line. The handler runs the
  per-file tier only when the hook's directory is the process's working directory: `poly hooks run --all-files`,
  a stage without isolation (`pre-push`, `isolate = false`) and other runs without a snapshot or hook `cwd`. There
  it loads the config from that directory exactly as the child does, so every working-directory read agrees. A
  staged snapshot or a hook `cwd` keeps the re-exec until the per-file tier takes its working directory as an
  argument.
- The `--set discovery.force_exclude=true` the line carries is applied to that run's `RunOptions` and config, not
  installed with `poly_config::set_run_overrides`. Any other `--set` declines.
- Report paths are the files as the runner passes them, relative to the hook's directory, as on the child's
  command line. The pretty report is rendered with `render_*_pretty_run` and its ANSI codes stripped, because the
  child's stdout is a pipe. Component notes and errors go after it, in the order the child writes them, and the
  exit code follows `run_lint` / `run_fmt` (0, 1, 2).
- The handler declines whatever it would not reproduce byte for byte: a non-pretty `--format`, `--deny-skips`,
  `--max-skips`, timings, the parent's `--offline`, `--trust` or `--config` flags (a child gets none of them), and
  `FORCE_COLOR` / `CLICOLOR_FORCE` in the environment (the child could colour its pipe).
- **Timeouts.** Every in-process call, `file_safety` included, runs on a thread of its own with a 16 MiB stack,
  never on the hook runner's worker, so a `lint`/`fmt` call fans out over its own rayon pool, as the child did, and
  not over the runner's. The runner waits for the call under the hook's budget (`timeout::budget_for`), announces
  it on the same cadence as a child and reports `TimedOut` past the limit. A thread cannot be killed, so a timed-out
  call is abandoned: it keeps running until `poly hooks` exits, and its result is discarded.

## Consequences

- Good, because a commit-msg gate and an unisolated `lint`/`fmt` run no longer start a `poly` process per hook,
  and the process's one-time work (the rayon pool, engine version probes, loaded grammars and components) is
  shared by every hook in the run. Config loading, discovery and the cache open still happen per hook.
- Good, because an in-process check is now bounded by the same budget as any other hook.
- Bad, because the default pre-commit run, which validates a staged snapshot, still re-execs `lint` and `fmt`.
- Bad, because log lines the child would have written to its stderr (poly's own `info`/`warn` notices) go to the
  parent's log stream once, not into each hook's captured output. The untrusted-keys note is not printed either;
  `poly hooks` refuses an untrusted config before any hook runs, so there is none to print.
- Bad, because a timed-out `lint --fix`/`fmt --fix` keeps writing until the process exits, where a killed child
  stopped at once. The default budget is ten minutes, so this applies to a wedged run.
- Bad, because concurrent in-process `lint` and `fmt` hooks share the global pool sized by the first one's `-j`
  instead of each getting its own share.

## Alternatives considered

- **Change the working directory around the call.** Rejected: it is process-wide, and hooks run concurrently.
- **Pass absolute snapshot paths and relativize the report.** Rejected: the paths would come out right, but the
  engines would still read configs from the worktree, which is the false pass ADR 0019 exists to prevent.
- **Thread an explicit working directory through the per-file tier and its engines now.** Deferred, not rejected:
  it touches the engine implementations, which are being restructured, and it is what lets a staged run go in
  process next.
- **Leave timeouts to the hang detector of the commit itself.** Rejected: a check that cannot be bounded is the
  hang ADR 0067 accepted only for the bounded file-safety checks.
