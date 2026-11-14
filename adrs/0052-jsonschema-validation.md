# 0052 — JSON Schema Validation Engine

- Status: Accepted
- Date: 2026-11-14
- Related: ADR 0003, ADR 0039, ADR 0040, ADR 0049

## Context

Repositories validate JSON, YAML and TOML files against JSON Schema with `check-jsonschema`-style hooks,
which need a Python runtime and fetch schemas over the network. poly already parses all three formats
for linting and formatting but has no schema validation. A validator reports where a violation is as a
JSON Pointer, not a line and column, and a lint run must be reproducible with networking off.

## Decision

A new engine, `jsonschema`, validates JSON, JSONC, YAML and TOML. It is lint only, opt-in with
`[engines.jsonschema] enabled = true`, and ships as its own downloadable component (`jsonschema`, opt-in,
so projects that never enable it never fetch it). Its crate, `poly-engines-jsonschema`, is in the root
workspace: every dependency is on crates.io.

- **Validator.** The `jsonschema` crate (MIT) with `default-features = false`, which removes the HTTP
  client, TLS and the built-in file resolver. Drafts 4, 6, 7, 2019-09 and 2020-12 are detected from the
  schema's `$schema`, defaulting to 2020-12. `$ref` is served by a retriever that reads only files under
  the project root or the schema cache and fails otherwise.
- **Schema sources.** In order: the file's own reference, then the first `schemas` mapping whose glob
  matches the project-relative path.
  - In-file: `$schema` in JSON and JSONC; a `#:schema <ref>` or `# yaml-language-server: $schema=<ref>`
    comment in the leading comment block of YAML and TOML. A `$schema` naming a JSON Schema meta-schema
    marks the file as a schema itself and names nothing.
  - Mapping: `schemas = [{ glob = "config/*.yaml", schema = "schemas/app.json" }]`, paths relative to
    the project root.
  - A reference is a path (relative to the file for an in-file reference), a `file://` URI, or an
    `http(s)://` URL that already sits in the local schema cache at `<cache_dir>/<host>/<path>`
    (`cache_dir` defaults to `.poly/schemas`). The project root is the nearest ancestor holding a
    `poly.toml`.
- **Never fetch, never pass silently.** poly does not download a schema. A named schema that is remote
  and not cached, missing, or outside the project root is reported as `schema-unavailable` (info) on the
  file. A schema that does not compile is `schema-invalid` (error); a document that does not parse is
  `schema-unparseable` (info).
- **Spans.** Each format is parsed by a span-preserving reader and the failing pointer is looked up in
  it: a small JSON and JSONC parser written for the engine (comments and trailing commas accepted),
  `toml`'s spanned `DeTable`, and the `granit-parser` event stream for YAML, the parser `serde-saphyr`
  already links. A pointer with no span of its own takes the nearest ancestor's, else line 1 column 1.
- **Diagnostics.** Rule code `schema/<keyword>` (`schema/required`, `schema/type`, ...), severity error,
  the message carrying the pointer and the validator's text.
- **Cache.** `version()` embeds the `jsonschema` and `granit-parser` versions. A host hook,
  `HostHook::JsonSchema`, folds a digest of the governing schema file and the local files it `$ref`s into
  the lint cache key, and the references that could not be resolved, so editing a schema, or adding a
  cached remote one, invalidates exactly the results it changes.
- **Coverage.** The engine claims no language coverage: `provides_language_lint` is false, because it
  checks only files that name a schema.

## Consequences

- Positive: schema validation with no runtime or network, reproducible runs, positions at the offending
  node, and no cost for projects that leave it off.
- Negative: one more component and a larger dependency tree (`jsonschema` and its `referencing`,
  `fluent-uri` and numeric crates). A project that wants a SchemaStore schema must put a copy in the
  cache or map a glob to a local file by hand.

## Alternatives considered

- `boon`: compliant and permissive but slow-moving; kept as the fallback if `jsonschema`'s weight or MSRV
  becomes a problem.
- `valico`: stale. `taplo-common`'s schema module: pins `jsonschema` 0.17, async and tied to `reqwest`.
- A pinned, checksummed SchemaStore `catalog.json` snapshot matched by `fileMatch`, and an install step
  that fills the cache: deferred. It is a data and download policy of its own (ADR 0047) and belongs in a
  follow-up that reuses the cache layout above.
- Mapping a pointer to a span lazily, by re-walking the source for the failing pointer: smaller memory on
  large documents, more code than the eager span table needs for config-sized files.
