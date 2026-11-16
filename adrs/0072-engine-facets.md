# 0072 — Optional Engine Concerns as Facets of the Engine Trait

- Status: Accepted
- Date: 2026-11-16
- Related: ADR 0013, ADR 0033, ADR 0039, ADR 0041

## Context

`Engine` (`crates/poly-engine-api/src/engine.rs`) had grown to 31 methods: identity and declarations, the per-file
work, the cache inputs, the coverage predicates, batching (ADR 0033), whole-project analysis (ADR 0041), the selection
between engines that share a table, and the catalog tier's takeover of the tier-one formatter (ADR 0013). All but four
had defaults, and most implementors override none of the later groups: of 68 (backends and test stubs), 10 batch, 20
analyze a project, 12 answer a planner question and 5 take over a formatter. A reader had to find one concern's contract among 31
methods, and a backend author could not tell which methods belonged together.

Constraints on any split:

- `dyn Engine` stays the unit the registry and runner hold, and the per-file path (`skip_reason`, the cache inputs,
  `lint`, `format`) gains no allocation and no extra dynamic call.
- The component wire protocol and the component index do not change shape, and no `version()` or cache key changes:
  a refactor must not invalidate a cache.
- Defaults stay where they are and answer as before.

## Decision

`Engine` keeps what the runner asks of every engine, for every file: identity and declarations (`name`, `languages`,
`capabilities`, `version`, `option_keys`, `overridden_hooks`), the cache inputs (`host_hook`, `run_context`,
`file_context`, `lint_input_salt`, `format_input_salt`, `format_config`, `lint_cache_salt`, `lint_cache_version`),
`skip_reason`, `lint` and `format`.

Everything else moves into four facet traits in `crates/poly-engine-api/src/engine/facets.rs`, each `: Engine`, with its
methods, documentation and defaults unchanged:

| Facet             | Methods                                                                                          | Accessor           |
| ----------------- | ------------------------------------------------------------------------------------------------ | ------------------ |
| `Batching`        | `batch_support`, `format_batch`, `lint_batch`, `batch_shard_floor`                               | `batching()`       |
| `ProjectAnalysis` | `project_check`, `project_inputs`, `selection`                                                   | `project_analysis()` |
| `PlanPolicy`      | `provides_language_lint`, `provides_language_format`, `self_manages_enabled`, `supersedes_generic_formatter` | `plan_policy()` |
| `CatalogTakeover` | `takes_over_tier_one`, `claimed_paths`, `supplements_tier_one`                                   | `catalog_takeover()` |

- A backend implements a facet only when it has the concern, and returns `Some(self)` from the facet's accessor on
  `Engine`. Every accessor defaults to `None`.
- Callers do not use the accessors. An inherent `impl dyn Engine + '_` in `facets.rs` carries one method per facet
  method, of the same name, that answers with the backend's facet or with the facet's default. Every call site on a
  `&dyn Engine`, `Box<dyn Engine>` or `Arc<dyn Engine>` is therefore unchanged, and each default is written once: the
  facet's default method and the `None` branch share a helper.
- A wrapper offers a facet exactly when the engine it wraps does: `FormatScope` answers `inner.batching().map(|_| self)`
  and forwards. `ComponentEngine` offers every facet and answers each method from the index or the component, as before.
- The method names are those of the old trait methods, so `poly_component_protocol::ENGINE_METHODS` keeps every entry
  and gains the four accessors as host-side entries; `tests/component_protocol_coverage.rs` scans `Engine` and the four
  facet traits.

## Consequences

- An engine's impl shows which concerns it has: a project-check engine has an `impl ProjectAnalysis`, a batching tool an
  `impl Batching`, and each facet's contract reads in one place.
- The per-file path is untouched. A facet question costs, for an engine without the facet, the one call the default
  method cost before (the accessor answering `None`); for an engine with it, one more indirect call. The questions are
  asked while planning, once per project, and per file only while grouping files for a batch, where the engines that
  answer are the process-spawning ones batching exists for.
- Calling a facet method on a concrete type needs the facet trait in scope, and a concrete type without the facet must
  be called through `&dyn Engine` to get the default. Tests that did so were changed.
- The wire protocol, the component index (`index.json`), the alef-generated component crates and every `version()` are
  unchanged; no cache entry is invalidated.

## Alternatives considered

- Supertraits (`trait Engine: Identity + Coverage + CacheInputs + Batching + …`): no extra call anywhere, but every
  implementor would write an empty `impl` for each concern it lacks (nearly 300 across 68 implementors), which is the
  opposite of implementing only what a backend has. Concrete-type calls of the identity methods would also need a
  second trait import in about 45 files.
- Moving the cache inputs into a facet as well: they are asked per file, and a facet would add an indirect call to
  every cache lookup of the engines that have them (the tree-sitter tier among them).
- Keeping one trait and grouping its methods under doc headings: no compile-time boundary, so the next concern would
  land in the same trait again.
