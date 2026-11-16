# The hardening corpora

poly's own fixtures are small, hand-written, and chosen to exercise a known path.
They cannot answer the questions that decide whether a release is safe to ship,
or whether a lint rule can be on by default:

- does poly **error** on anything in a large tree nobody wrote for us?
- does formatting **converge**, or do two backends fight over the same file?
- does the cache ever serve a **wrong** answer?
- how many findings does a rule actually produce on code we did not write?

`mise run harden` answers those against real code. This document is about
_which_ real code, and why the three corpora are allowed to assert different
things.

It is a tool you run, not a job that runs itself. There is no CI schedule and no
push trigger: the harness clones several large third-party trees, and the output
that justifies that cost — the per-rule counts — is the part that cannot fail a
build on its own. A nightly job whose realistic failures are "GitHub was slow"
and "a pinned tree moved shape" gets muted, and the invariants that _can_ gate
would be muted with it. `cargo test --workspace` compiles the `#[ignore]`d harness
on every PR so it cannot rot, and the `dogfood` CI job already answers "does poly
survive a real tree" per commit. Run this before a release, or when a rule's
default severity is in question — on a laptop, or on demand through the
`harden` workflow (`workflow_dispatch` only, see "Running it on CI").

## The rule that splits them

**An invariant can be asserted on any input. A count needs a pinned one.**

"No file errored" and "formatting converges in two passes" are true or false
regardless of what a tree contains, so any corpus can gate them. "`todo-marker`
fires 412 times" is a property of the tree — gate that against a moving input and
the check fires on somebody else's commit, gets disabled within a month, and
takes the invariants with it.

| | A — local siblings | B — pinned OSS | C — generated code |
| --- | --- | --- | --- |
| Where | `../*`, in place | `/tmp/poly-harden/b` | `/tmp/poly-harden/c` |
| Acquisition | none | `git fetch --depth=1 <sha>` | same |
| Stability | unstable, may be dirty | pinned | pinned, but the population is a judgement |
| Written to | **never** | disposable | disposable |
| Invariants | gate | gate | gate |
| Per-rule counts | trend only | **gate** | audit input |

## A — the local siblings

Enumerated by `mise run harden:enumerate-local`, which emits the manifest the
records quote verbatim: "what did you actually measure" is the first question
anyone asks of a corpus number, and a list is the only honest answer.

Four traps, each of which has already produced a wrong measurement:

- **A plain `ls` misses nested roots.** `xberg-io` is a polyrepo whose own
  ignore rules hide the projects inside it, so scanning it as one root sees
  almost nothing. The enumerator descends one level.
- **`find -type d -name .git` misses repositories.** A worktree's `.git` is a
  _file_, and at least one sibling is not a git repository at all.
- **`git ls-files` must never enumerate files.** Outside a repository it exits
  128 and, with stderr swallowed, returns an empty list — which reads as "no
  files" rather than "not a repository". poly enumerates the files; the script
  only finds roots.
- **Worktree containers duplicate a project.** Roots sharing a git common
  directory collapse to one.

These are live working trees, so the harness never writes to them: linting runs
in place through `poly_core` (never the CLI, whose whole-project phase executes
`cargo` against the worktree), formatting runs against a disposable copy, and the
cache is redirected under `POLY_HARDEN_ROOT`.

## B — pinned third-party trees

`crates/poly-cli/tests/harden/data/repos.b.tsv`. Fetched at depth 1 at an exact ref — poly reads no
git history, so depth is pure cost — and **pinned to a commit**, because this is
the only corpus whose counts may gate. Bumping a sha is a deliberate
re-baselining commit, which is the review moment where a change in the numbers
gets looked at.

A `ref` that is not a 40-character sha is a branch. The orchestrator says so and
that root's counts are not reproducible.

Licences are checked against an allow-list before anything is cloned. Nothing
copyleft: not a linking question, since nothing is linked, but an artifact
derived from a tree is a redistribution one, and the cheap answer is to not have
the content. Relatedly, **the record types have no field that can hold source
text** — findings are carried as `path:line` and as counts. Keep it that way.

### What B cannot tell you

It is idiomatic, reviewed, hand-written open source, and it systematically
under-reports exactly the patterns the AI-quality rules exist for. A maintained
project does not leave `todo!()` in `main`. A rule audited only against B is
audited against the population least likely to trip it.

## C — generated code

`crates/poly-cli/tests/harden/data/repos.c.tsv` currently holds **codegen** output: SDKs and clients
emitted by a generator. That is directly relevant to the path-exclusion question
(issue #21) — it is the `frb_generated.rs` failure mode at scale — and it says
close to nothing about LLM-authored _application_ code, which is what issues #23
and #24 are about. A green result on the codegen half must not be read as
covering the other.

The LLM-authored half is deliberately empty rather than filled with plausible
guesses. Adding to it means following this procedure, and recording the query and
date in the manifest:

1. **Identify by the artifacts agents leave behind**: `CLAUDE.md`, `AGENTS.md`,
   `.claude/`, `.cursor/rules/`, `.github/copilot-instructions.md`,
   `.aider.conf.yml`, `.windsurfrules`.
2. **Corroborate from history**, which is harder to fake than a config file:
   commit trailers (`Co-Authored-By: Claude`, `Co-authored-by: Copilot`,
   `aider:`) on **at least 30% of the last 200 commits**, so a repository that
   used an agent twice does not qualify.
3. **Filter**: OSI-permissive licence; at least 200 source files in the target
   language; at least six months of history; not a fork; not a
   `*-template`/`*-starter`/tutorial repository.
4. **Hand-verify** three randomly sampled files per candidate are application
   code rather than scaffolding, and record the verdict.
5. **At least ten repositories per target language**, and **report per-repository
   counts, never only the aggregate**. One file in one repository once supplied
   320 of 324 findings for a rule; an aggregate hides that and a per-repository
   table does not, which is why the records carry `top_paths`.

Publishing the query is what makes the corpus auditable. Publishing a list of
repositories is not.

### Our own agent-written repositories are not corpus C

`spikard`, `scythe`, `basemind`, `poly` and `xberg-io` are heavily agent-written
and cover a dozen languages, but they are corpus A. Treating them as the AI
corpus is the trap issue #23 names: they are our own repositories, Rust-heavy and
Rust-idiomatic, so a JavaScript rule audited only against them is audited against
almost no JavaScript.

## Native toolchains

`rustfmt`, `gofmt` and `shellcheck` are default-on **when present on `PATH`**, so
identical poly binaries produce different output on two machines. The gated run
leaves them off. `POLY_HARDEN_NATIVE_TOOLS=1` runs a separate, explicitly
non-gating leg with them enabled — that is where those backends get real-world
exercise — and the record marks it, so the two can never be compared by accident.

## Running it

```sh
mise run harden                      # corpus B, core tier
mise run harden --corpus a           # the local siblings, read-only (mise run harden:local)
mise run harden --tier extended
mise run harden django ripgrep       # named roots only
```

One process per root, so a panic in one repository cannot take the rest of the
run with it. Each root appends an NDJSON record to
`/tmp/poly-harden/results.ndjson` **before** its assertions run, so a failing
root still leaves its measurement behind. Every threshold a root was judged
against is written into its own record, so a result read months later is
self-describing.

## The local-repository pass (release gate)

Corpus A above measures the siblings in place through `poly_core`. The release
gate is a stricter, CLI-level pass over every real local repository — each git
repository directly under the polyrepo root and under `../xberg-io`, except
poly itself:

```sh
mise run harden:repos -- --poly target/release/poly
mise run harden:repos -- --poly target/release/poly --only xberg,alef --timeout 600
```

Flags: `--root DIR` (repeatable, replaces the defaults), `--out DIR` (default
`reports/polylint-harden/<timestamp>/`), `--only`/`--skip` (comma-separated
names), `--timeout SECONDS` (per invocation, default 1200), `--parallel N`
(default 1; timings are only comparable when sequential), `--keep-copies`.

The source trees are never touched. Each repository is copied from **`HEAD`
only** (`git archive`, so uncommitted work and `export-ignore` paths are not
measured), re-initialised as a git repository and committed, then every run
happens in the copy. Per repository: cold lint (`--no-cache`, with `--timings`
when the binary has it), a cache-priming and a warm lint, a `-j 1` lint, a
format dry run, `fmt --fix` twice and a final dry run. Lint runs use
`--workspace never`. Output goes to `<out>/<repo>/` (`lint-cold.json`,
`lint-warm.json`, `fmt.json`, `fmt-fix-{1,2}.log`, `timings.txt`, `stderr.log`,
`meta.json`) plus `summary.md` and `failures.md` at the top.

Triage `failures.md`, which groups problems by kind. Hard kinds fail the run
(exit 1): `poly-error-exit` (exit 2), `poly-errors` (non-empty `errors`),
`panic`, `stack-overflow`, `signal`, `timeout`, `non-convergence` (the second
`--fix` rewrote files, or a dry run still reports drift), `cache-mismatch`
(primed or warm lint differs from cold), `jobs-mismatch` (`-j 1` differs from
default jobs), `component-lock`, `bad-json`. Advisory kinds: `skip-outlier`
(more than 20% of files skipped as generated) and `slow-file` (a backend run
over 2 s, from the `backend run` log lines). Lint findings are not failures;
only poly failing on a file is. `meta.json` lists the files the first
`fmt --fix` changed, which is the place to look when a formatter rewrites code
it should not.

## Running it on CI

`.github/workflows/harden.yaml` is dispatch-only (`gh workflow run harden.yaml`).
Inputs: `ref` (branch, tag or commit; default the dispatched ref), `only`
(comma-separated root names, applied to corpora B and C and to `harden:repos
--only`; a name that matches nothing in a corpus skips that corpus, and a name
that matches nothing anywhere fails the release-gate pass) and `tier` (`core` or
`extended` for B and C when no root is named).

It builds the **bundled** binary (`components/` crate `poly-bundled`, the same
build as the `dogfood` job and the musl release, with every engine linked
in-process), then runs, in order: corpus B, corpus C, and the release-gate pass
(`harden:repos`) over the B and C clones plus shallow clones of the public
polylint org repositories (`adrlint`, `skif`, `uncomment`, `rumdl`).

Corpus A is not run: it is the maintainer's local sibling trees, and
`mise run harden --corpus a` refuses to run under CI. The org clones in the
release-gate pass are its stand-in.

The job fails only on the invariants the scripts already assert (errors,
non-convergence, cache mismatch, `-j` mismatch, panic, timeout); per-rule counts
never fail it. The job summary lists each stage's outcome, per-root checked and
finding counts, the top rules by findings and `harden:repos`' `failures.md`. The
`harden-reports` artifact (30 days) holds the corpus NDJSON files and the
`harden:repos` output directory.
