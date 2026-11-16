# 0068 — Prose and Grammar Checks with harper

- Status: Accepted
- Date: 2026-11-16
- Related: ADR 0003, ADR 0016, ADR 0027, ADR 0039, ADR 0052

## Context

poly spell-checks with `typos` but nothing reads prose: a repeated word, a wrong article or a
run-on sentence in a README or a doc comment passes every check. The usual tool is vale, which is a Go
binary configured with style packs. `harper-core` (Apache-2.0) is a Rust grammar checker with
a bundled dictionary, a Markdown parser and some 300 rules, and runs offline in process.

## Decision

A new engine, `harper`, lints prose. It is opt-in with `[engines.harper] enabled = true`, lint only,
cross-cutting like `typos`, and ships as its own downloadable component, `prose` (opt-in, so projects
that leave it off never fetch it). Its crate, `poly-engines-prose`, is in the root workspace and
depends on `harper-core` from crates.io at an exact `=` version, without the `thesaurus` feature.

- **What it reads.** A Markdown file is read whole through harper's Markdown parser, after a leading
  front-matter block; fenced and inline code and tables are skipped by the parser. In every other
  language, comments are found with the tree-sitter grammar poly already loads, the comment markers are
  dropped, and a run of adjacent whole-line comments is joined into one block so a sentence wrapped over
  several lines is read as one. The block is then parsed as Markdown, since doc comments are written in it.
  Blocks under three words, tool directives (`noqa`, `eslint-disable`, ...), shebangs and lines that end
  like a statement are left out. MDX is not read.
- **Why not `harper-comments`.** It parses comments itself with a private set of some thirty tree-sitter
  grammar crates. poly already ships one grammar pack and shares one parse per file between engines
  (ADR 0057); a second set would add size and give two answers to what a comment is.
- **Mapping.** harper reports spans as character indices into the text it was given. Each block keeps
  the byte offset of every character it was built from, so a finding lands on the original line and column.
- **Rules.** Rule ids are harper's own (`AnA`, `RepeatedWords`, ...), matched without regard to case,
  `-` or `_`, through the shared `select`, `extend_select`, `ignore` and `rules.<id>.level` schema
  (ADR 0016). The default set is harper's curated one minus `SpellCheck`: `typos` owns spelling, and
  harper's dictionary flags the jargon every codebase is full of. `extend_select = ["SpellCheck"]` turns
  it on.
- **Dictionary and dialect.** `dictionary = ["word", ...]` adds words harper must accept; `dialect` is
  `american` (default), `british`, `canadian`, `australian` or `indian`. `comments = false` limits the
  engine to Markdown.
- **Severity and fixes.** Findings are warnings and carry no autofix: a grammar suggestion is a judgement,
  and rewriting prose silently is worse than reporting it. Suggestions are listed in the description.
- **Cache.** `version()` embeds the `harper-core` version and a marker for the extraction and selection
  logic.
- **Coverage.** The engine claims no language coverage: `provides_language_lint` is false, because it
  reads a file's text and not its code.

## Consequences

- Positive: prose checks in Markdown and doc comments with no runtime or network, one config, and no cost
  for projects that leave it off.
- Negative: a component with a sizeable dictionary and rule tables. harper is tuned for general English,
  so technical prose draws false positives; `ignore` and the dictionary are the remedy. Comment
  extraction is heuristic: commented-out code that does not end like a statement can be read as prose.
  The language-agnostic `[lint.harper]` table of the v1 schema passes only `enabled`; options need the
  v2 `[engines.harper]` table.

## Alternatives considered

- `harper-comments`: see above.
- vale: not embeddable, needs style packs and a Go binary.
- Linking the engine into the core: the dependency weight is not justified for an opt-in check (ADR 0043).
- Defaulting `SpellCheck` on: duplicates `typos` and is the noisiest harper rule on code-adjacent text.
