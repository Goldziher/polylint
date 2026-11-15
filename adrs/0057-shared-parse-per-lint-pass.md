# 0057 — One Parse per File per Grammar per Lint Pass

- Status: Accepted
- Date: 2026-11-15
- Related: ADR 0009, ADR 0027, ADR 0029, ADR 0043

## Context

For Rust, Go, Java, Kotlin, Python, Ruby, C# and Swift, three cross-cutting engines parse the same file with
tree-sitter in one lint pass (#187): `quality` through its pooled parser (`engines/quality/mod.rs`), the ast-grep
pack through `StrDoc::try_new` (`poly-engines-astgrep`), and, when enabled, `uncomment` through its `Processor`. Each
`--fix` re-lint repeats all of them. The existing pools save building a parser, not the parse.

What the dependencies allow, read from `Cargo.lock` and the crate sources:

- One `tree-sitter` (0.27.2) serves all three, and `ast-grep-core` 0.51.2, `uncomment` 3.10.4 and
  `tree-sitter-language-pack` 1.25.0 resolve to it. Every engine gets its grammar from the pack's `get_language`,
  which returns the same `tree_sitter::Language` pointer for a name, so the language objects are the same and
  compare equal.
- `ast-grep-core` can be handed a tree: `StrDoc`'s fields `src`, `lang` and `tree` are public and `Root::doc` builds
  a root from any `StrDoc`. Its parser pool (`PARSER_CACHE`) is private, so the parse itself still goes through
  `StrDoc::try_new`.
- `uncomment` cannot: `Processor::plan_removals` and `inspect` look the grammar up and parse inside, with no entry
  point taking a tree.
- In the core build (every release triple except musl) `astgrep` and `uncomment` are downloaded components, loaded
  as a separate library or a sidecar with their own copy of every static; only `quality` is linked. A tree cannot
  cross that boundary. In the bundled build (musl, and `cargo test` with default features) all three are linked.

## Decision

Parses are shared within a link unit, never across the component boundary.

- `poly-grammar` holds a per-thread scope (`SharedParses`, `shared_tree`). The runner opens it around the engines of
  one lint pass over one file (`runner/lint_pass.rs`), where every engine runs in sequence on one thread. Inside it,
  `shared_tree(content, language, parse)` returns the tree already parsed for that content and grammar, or runs
  `parse` and keeps the result until the pass ends.
- An entry is keyed by the content's `Arc<str>` (pointer equality, with the `Arc` held so the allocation cannot be
  reused) and the `tree_sitter::Language`. The tree is stored type-erased and returned only to a caller asking for
  the same type.
- `quality` and `astgrep` get their tree through it. `astgrep` builds its `StrDoc` from a shared tree, or parses with
  `StrDoc::try_new` and shares that tree; it keeps no parser of its own.
- Each `--fix` pass is a new pass over new content and parses again.
- `uncomment` keeps its own parse until the `uncomment` crate takes a tree.

Coverage: in the bundled build a file that both `quality` and the pack examine is parsed once per pass instead of
twice (three times with `uncomment` on becomes two). In the core build nothing changes: the components carry their
own `poly-grammar`, have no scope open, and parse as before.

## Consequences

- Engine output is unchanged: a shared tree is a from-scratch parse of the same bytes with the same language.
  Tests compare each engine's diagnostics with and without the shared tree, in both orders, and count the parses
  of a pass (`poly_grammar::parse_count`).
- An engine's tree outlives its `lint` call by the rest of the pass, at most one tree per grammar per file per
  worker thread.
- The core build still parses twice per file for the pack's languages. Sharing there needs `quality` and the pack
  in one link unit.

## Alternatives considered

- **Link the ast-grep pack into the core.** `engine-astgrep` costs 814,272 bytes at `opt-level = "z"`
  (`docs/size-baseline.md`), the reason it became a component; the core has about 1 MiB of headroom under
  ADR 0043's 14 MiB ceiling, which the reverted hot-path overrides have already exceeded once.
- **Compute quality metrics in the astgrep component.** Moves a default-on engine out of the core, or runs it in
  two places with the core deferring whenever the component is installed. Either changes ADR 0043's core contents
  and the engine identity in the cache key for a gain on two engines.
- **Pass a tree pointer across the component ABI.** Each side links its own tree-sitter runtime and grammars, so the
  pointer would name another runtime's allocation and another copy of the language.
- **An unscoped thread-local slot keyed by content pointer and length.** A freed buffer can be reused at the same
  address with the same length and other bytes; the scope and the held `Arc` rule that out.
- **A cache on `SourceFile`.** `poly-engine-api` has no tree-sitter dependency and `SourceFile` crosses the component
  wire; the scope gives the same sharing without either.
