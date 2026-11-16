# 0067 — Run Builtin Hooks In Process

- Status: Accepted
- Date: 2026-11-16
- Amends: ADR 0012

## Context

`poly hooks` lowers every `[hooks.builtin]` member to a shell line that starts the running `poly` binary again:
`poly lint …`, `poly fmt …`, `poly commit` and `poly hooks check …` for `file_safety`. Each child loads the config,
discovers files, opens the result cache and builds its own rayon pool. #215 asked for an in-process hook kind so that
work is not repeated per hook.

A child is not only a cost. It owns its working directory (the staged snapshot under ADR 0019), its stdout and stderr
(captured as the hook's output), its process-global state (the `--set` layer, the colour override, the tracing
subscriber) and its lifetime (a hook over its timeout is killed). `poly lint` and `poly fmt` read all four: they find
`poly.toml` from the working directory, install `--set discovery.force_exclude=true` as a process-wide layer, render
paths as given and colour by the stream they print to, and can run for as long as the engines take. Run in the
parent, the same calls would print absolute snapshot paths, colour by the parent's terminal, leak the override into
the runner, and could not be stopped by the hook timeout. The `-j` share each child gets (`job_budget`) already keeps
concurrent children from oversubscribing the cores.

The `file_safety` checks have none of these dependencies: `collect_violations(root, args)` takes the directory and
files explicitly, reads only those files and the AWS credential sources, and prints a fixed text.

## Decision

- `poly-hooks` gains `HookCommand::InProcess { name, args, line }` and a process-wide handler registered with
  `poly_hooks::in_process::register`. The runner calls the handler with the directory the child would have run from
  (the execution root plus the hook's `cwd`, so the staged snapshot under isolation) and the matched files relative
  to it, and turns the returned exit code and output into the hook's status and output.
- `line` is the equivalent re-exec command. It runs when no handler is registered, when the handler declines `name`,
  or when the hook declares `args` or `env` or does not pass filenames. It is also the hook's identity for the cache
  key, the argv budget and the snapshot scope, so both forms are interchangeable.
- `file_safety` lowers to `InProcess { name: "file-safety", .. }`; `poly hooks` registers a handler that parses the
  arguments with `poly hooks check`'s own grammar and renders the same text.
- `lint`, `fmt` and `commit` stay re-exec. Moving them needs, first, config loading and report rendering that take
  a root and an output sink instead of the working directory and the process streams, and a way to bound an
  in-process run by the hook timeout.

## Consequences

- Good, because a commit with `file_safety` enabled no longer starts a `poly` process for it, and a library consumer
  that registers no handler keeps the previous behaviour exactly.
- Good, because the extension point exists for `lint` and `fmt` once their prerequisites are met.
- Bad, because an in-process check cannot be killed at its timeout. The file-safety checks are bounded (content scans
  stop at 10 MiB per file), so this is accepted for them and is a prerequisite for any heavier check.
- Bad, because an in-process check sees all matched files in one call where a file list too long for one command
  line was split into batches, each a separate child; `case_conflict` then also compares across what used to be batch
  boundaries, and the output is grouped by check over all files rather than by batch.

## Alternatives considered

- **Run `lint` and `fmt` in process now.** Rejected for the reasons above: their output, colour, `--set` layer and
  timeout would all change, and ADR 0019's guarantee that a hook judges exactly the staged snapshot rests on the
  child's working directory.
- **A persistent `poly` server the hooks connect to.** Rejected: a long-lived process per repository adds lifecycle,
  staleness and security questions for a saving of tens of milliseconds per commit.
- **Keep every builtin re-exec and rely on the `-j` share alone.** That bounds oversubscription but still starts a
  process for checks that need nothing a child provides.
