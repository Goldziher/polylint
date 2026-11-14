# 0049 — External Tools: GPL Tools as Separate Processes, No Bundled Runtimes

- Status: Accepted
- Date: 2026-11-14
- Related: ADR 0013, ADR 0035, ADR 0045, ADR 0047

## Context

Some tools poly wants to run have no usable Rust implementation. shellcheck (Haskell, GPL-3.0),
hadolint (Haskell, GPL-3.0) and golangci-lint (Go, GPL-3.0) are the standard checkers for their
languages. Others need a language runtime: checkstyle, PMD, detekt and ktlint's jar run on a JVM, `mix
format` and credo on the BEAM. ADR 0035 gave a recipe tool a pinned, checksummed download and ADR 0047
let it follow upstream, but neither said what poly may do with a GPL tool, or whether poly may carry a
runtime so that a tool needing one works on a clean machine.

## Decision

**GPL-3.0 tools run as separate processes, downloaded from upstream.**

- A GPL tool is fetched from its upstream release, pinned and checksummed as ADR 0035 and ADR 0047
  describe, and run as a child process over stdin, argv and its report on stdout.
- poly never links a GPL tool and never ships one inside its own artifacts (archives, wheels, npm
  packages, components, the Homebrew formula).
- AGPL tools stay excluded outright: they are neither downloaded nor run.

**poly bundles no runtime.**

- A component ships native code only: a Rust cdylib, a Go sidecar (ADR 0045), or an upstream's own
  native binary.
- poly never bundles a VM or language runtime: no JVM or jlink image, no BEAM or Burrito release, no
  Node, no Python.
- A tool that needs a runtime is a **recipe that requires the host runtime**. The tool itself is still a
  pinned, checksummed download where upstream publishes one (a jar, a distribution zip); the only host
  requirement is the runtime. When the runtime is missing the run reports a visible skip naming it,
  never a pass. The recipe's cache key includes the runtime's resolved version.
- GraalVM native images (palantir-java-format, google-java-format, ktfmt) contain no VM and count as
  native binaries: they are downloaded like any other binary and need no runtime. A platform without an
  upstream native asset falls back to the host tool or a visible skip, never to a jar plus a bundled
  JVM.

**Fallback order.** The pinned download, else the host binary on `PATH`, else a visible skip. Offline,
with nothing cached or locked, is the same visible skip.

## Consequences

- Positive: poly's licence stays clean, its artifacts stay small (ADR 0043), and a repository that
  already installs a runtime gets the tool without installing the tool.
- Negative: a clean machine without a JVM or Elixir cannot run those recipes and gets a skip, which a
  repository must treat as a failure with `--deny-skips` if it relies on the check. Output depends on the
  host runtime's version, which the cache key records but poly does not control.

## Alternatives considered

- Bundle a trimmed JVM or BEAM release so Java, Kotlin and Elixir tools work anywhere: adds tens of MiB
  per platform, and poly would own a runtime's security updates.
- Link or vendor GPL tools: puts poly's own licence in question for no benefit over a child process.
- Re-implement the tools in Rust: the right answer where a crate exists (ruff, oxc); for shellcheck's
  checks and the JVM linters it is a disproportionate maintenance sink.
