# 0080 — Engines Keep Their Own Cache in poly's Cache

- Status: Accepted
- Date: 2026-11-17
- Amends: ADR 0044
- Related: ADR 0008, ADR 0039, ADR 0077

## Context

A project check is cached as a whole (ADR 0008): its key digests every file handed over, so one edit anywhere in a
JavaScript workspace re-runs skif over all of it (ADR 0044). Most of that run is parsing modules the edit did not touch.
skif 0.9.0 can keep each module's parse in a store its host gives it (`skif::cache::CacheStore`, `Linter::with_cache_store`,
`Linter::with_cache_salt`); skif computes every key and seals every value, and a run without a store caches nothing.

Where that store lives is poly's choice. In the core build skif runs inside the downloaded `web` component, which only
sees what a request carries, so the host has to tell it where the cache is.

## Decision

- **The store is poly's result cache.** Entries go in a fourth namespace directory, `results/engine/`
  (`Namespace::Engine`), through `poly_cache::EngineDataStore`: atomic writes, a failed read is a miss. `poly cache
  stats`, `clean`, `gc` and the automatic sweep cover it, and it is the per-repository directory `[cache] dir` and
  `POLY_CACHE_HOME` select. Each entry's file name is a result-cache key over the cache format version, the build
  identity of the binary running the engine, the namespace, an id per skif namespace and schema (`skif.modules.<n>`)
  and skif's own 32-byte key.
- **The host hands it to the engine.** `ProjectRequest::cache` is `Some(EngineCache { dir, salt })` when the run caches
  and `None` under `--no-cache` or `[cache] enabled = false`. `salt` is the host's cache format version and build
  identity (`poly_cache::engine_data_salt`). An engine that keeps no cache ignores it; the field is never part of the
  project-check key, since where the cache lives changes no finding.
- **Over the wire it travels as two environment pairs**, `poly.cache.dir` and `poly.cache.salt`, appended to
  `ProjectCheck::environment`; the component rebuilds `EngineCache` from them. Protocol v3 is unchanged.
- **skif's salt** is the host's salt, the engine's `VERSION` and the engine options, so another build, mapping or
  `[engines.skif]` table never reads an entry; skif's keys add its own version, the namespace schema and each module's
  name, text and parse options. A `skif.toml` change reaches skif through the files, as before.
- **A store failure never fails the check.** A store that cannot be opened is logged and the run proceeds without one;
  a lookup that fails or returns bytes that do not open is a miss; a failed write is logged and dropped.

## Consequences

Positive:

- A re-run of the skif check after an edit parses only the changed modules. The run's `Analysis::cache` counters
  (hits, misses, bytes written per namespace) prove it in tests without timing.
- One cache to locate, size, clean and turn off; no second directory under `node_modules/.cache` or the user cache.

Negative / risks:

- A component writes files below the host's cache root, and only in the directory the host named.
- Changing `threads` in `[engines.skif]` invalidates the module entries, although it changes no parse.
- The committed component index still describes skif at its old version until it is regenerated.

## Alternatives considered

- **A trailing `cache` field on `Request::ProjectCheck`.** Requests are positional arrays, and the trailing `targets`
  field is omitted when `None`, so a later field would land in its slot unless the encoding were written by hand; and
  a component that predates the field rejects the longer request, so the host would have to gate it on a description
  flag the committed index does not carry yet. A pair in the existing environment list needs neither.
- **skif's own disk store (`cache-disk`).** It brings a second cache location, size cap and eviction policy, outside
  `poly cache` and `--no-cache`.
- **An in-memory store per run.** It only helps a host that runs skif several times in one process (`poly mcp`); a CLI
  run would gain nothing.
