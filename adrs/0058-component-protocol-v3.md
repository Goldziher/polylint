# 0058 — Component Protocol v3: Declines in the Reply, Positional Encoding

- Status: Accepted
- Date: 2026-11-15
- Amends: ADR 0039
- Related: ADR 0045, ADR 0056

## Context

ADR 0039 fixed the component wire format as MessagePack with named fields and a request per `Engine` hook. A
profile of component runs found three per-file costs that the format, not the engines, imposed:

- `Engine::skip_reason` was a `SkipReason` request carrying the whole file, sent before the `Format` that carried it
  again, so every file of an engine that declares the hook crossed the transport twice. A failed `SkipReason` was
  logged and read as "not declined", so the file counted as checked.
- `Engine::batch_support` was a `BatchSupport` request per file the runner considered batching, although its answer
  depends on the config alone.
- Named-field encoding wrote every field name of every struct on every message: a response of diagnostics repeated
  `severity`, `title`, `span` and the rest per finding, plus the engine id the request had already named.

Four engines that override no per-file hook (`uncomment`, `ini`, `python-types`, `pyrefly`) never narrowed
`Engine::overridden_hooks`, so their index entries carried every hook bit (255) and the host asked each hook per file.

## Decision

`PROTOCOL_VERSION` is 3. Host and component still compare it for equality in `Init`; a v2 component is rejected as
incompatible and every component is rebuilt.

- **Declines travel in the format reply.** `Request::SkipReason` is removed. `WireFormatOutput` gains
  `Declined(reason)`: the producer asks the engine's `skip_reason` inside `Format` and `FormatBatch` and, for a
  declined file or member, answers `Declined` without calling `format`. The host turns it into the
  `poly_engine_api::declined::Declined` error, and the format runner treats that engine as having declined: the
  content is left as it is, the decline is not cached, and when every engine routed to the file declined it the file
  is a counted skip with the reason, as an up-front `skip_reason` answer would make it. `ComponentEngine::skip_reason`
  answers only what the host knows locally (an unavailable component, the sidecar frame limit) and never makes a call.
  The `SKIP_REASON` hook bit stays in the index as a record that the engine overrides the method.
- **Lint replies carry no skip.** The lint runner never consults `skip_reason` for any engine, linked or component, so
  a lint skip in the reply would either be dead or make component builds count files differently from the bundled
  build.
- **`BatchSupport` is asked once per engine and config** for the life of a connection; `Connection` keeps the answers
  by engine name and config fingerprint. A failed ask is not remembered.
- **Positional encoding for requests and responses.** `Request` and `Response` are encoded with `rmp_serde::to_vec`:
  a struct or struct variant is an array of its fields in declaration order, every field present (`nil` for `None`).
  Enums stay externally tagged by variant name. `Describe` keeps named fields: it is read once per load and is the
  one message that grows `#[serde(default)]` fields compatibly. `ProjectCheck.threads` loses its
  `skip_serializing_if`, since an omitted field would shift every position after it.
- **The engine id leaves the diagnostic.** `WireDiagnostic.engine` is `Option<String>`: `None` means the engine the
  request named and is the common case; a component reporting for another engine id still says so. Conversions take
  the requested engine (`wire_diagnostic`, `diagnostic_from_wire`).
- **`Connection::call` takes the built request** and writes the registered config id into it (`Request::config_mut`),
  re-pointing it on the one retry, instead of rebuilding (and cloning the file descriptors of) the request per attempt.
- The four engines above declare `overridden_hooks() = 0`, so their index entries stop asking any hook.

The Go sidecars (`go`, `sh`) speak v3: they decode positional headers and encode positional responses. Their stdio
framing is alef's and is unchanged here.

## Consequences

- A component answers a declined file with one request instead of two, and a decline can no longer be lost to a
  failed `SkipReason`. An engine without declines, and a warm run that hits the cache, no longer loads the component
  to ask `skip_reason` at all.
- A declined file is asked again on every run: the decline is not stored in the format cache.
- A member a component declines inside a format batch is left to the per-file path, which asks once more.
- A non-Rust component must emit arrays in exactly the Rust declaration order; `schemas/component-protocol.json`
  documents the encoding in `ProtocolSchema`. Reordering or inserting a field in a request or response type is a
  protocol change.
- The sidecar stdio envelope still carries a payload as a JSON number array with one call in flight per process; that
  is an alef transport change (ADR 0056).

## Alternatives considered

- Keeping `SkipReason` and caching its answer by content digest: still one extra round trip per first sight of a
  file, and the cache would have to repeat the format cache's key.
- Returning the decline as a new `Engine` trait method (`format_or_decline`): a trait-wide change for one transport;
  the typed error keeps the trait as it is and every non-component engine untouched.
- Carrying `BatchSupport` in the `RegisterConfig` reply: answers it for every engine of the component whether asked
  or not, and changes `RegisterConfig`'s payload for a question only some engines have.
- Positional encoding for `Describe` too: it is read once per load, and its defaulted fields are how older
  descriptions keep decoding.
