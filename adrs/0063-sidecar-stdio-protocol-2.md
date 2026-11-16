# 0063 — Sidecars Speak alef's Stdio Protocol 2

- Status: Accepted
- Date: 2026-11-16
- Amends: ADR 0045, ADR 0056

## Context

ADR 0045 runs a sidecar as one child process behind alef's stdio transport, which serialized every call to it and
carried payloads as JSON arrays of numbers, up to four characters per byte, in frames bounded at 16 MiB. So the host
declined files over 3 MiB for a sidecar engine, split sidecar batches into 2 MiB requests (ADR 0056), and rayon
workers queued on the one process (#189).

alef 0.130.0 adds stdio protocol 2 (alef ADR-0025): after the unchanged protocol-1 `describe` handshake, a sidecar
that finds `2` in `ALEF_STDIO_PROTOCOLS` may select binary frames whose payloads are raw bytes of at most 16 MiB and
which carry request ids, so the host pipelines up to the `max_in_flight` calls the sidecar declares over the same
process. Protocol-1 sidecars keep working.

## Decision

- **The `go` and `sh` sidecars select protocol 2** and declare `max_in_flight` equal to `GOMAXPROCS`. Each call runs on
  its own goroutine; responses are written whole under one lock, in any order. Session state (registered configs) is
  behind a read-write lock, and the Go sidecar holds goimports' process-wide local prefix for the duration of each
  `imports.Process`, so concurrent calls under different configs cannot see each other's prefix.
- **No protocol-1 fallback for calls.** poly pins alef exactly and every host it ships advertises protocol 2, and an
  older poly never resolves these artifacts from its own lock. A host that does not advertise it gets a failed
  `describe` naming the requirement, rather than a sidecar that keeps a second, JSON code path alive for no user.
- **Limits follow the raw frame.** `poly_component_protocol::sidecar::MAX_FILE_BYTES` rises from 3 MiB to 12 MiB, which
  leaves a third of the 16 MiB frame for output that grows, the request header and the reply's logs. The host's batch
  chunk (`SIDECAR_BATCH_BYTES`) rises from 2 MiB to 8 MiB, so a batch's output may double before its reply no longer
  fits. A file over the limit is still declined with a counted skip, and a reply over the frame limit is still that
  call's error rather than a dropped connection.

## Consequences

- Shell and Go formatting scale across the worker pool instead of queueing on one process; within a batch the
  goroutine pool of ADR 0056 still applies.
- A protocol-2 timeout abandons only its own call unless the sidecar answered nothing for the whole deadline (alef
  ADR-0025), so a slow file no longer stops the connection for every other call.
- The JSON number-array encoding of ADR 0056 now only frames the `describe` reply.
- Sidecar artifacts change, so the `go` and `sh` components are rebuilt and relocked for the next release.

## Alternatives considered

- Keep protocol 1 alongside protocol 2: dead code in poly's own pairing, since its host and sidecars ship together.
- Several sidecar processes per component: still rejected for the reasons in ADR 0045 and alef ADR-0025 (per-process
  sessions).
- A higher file limit (up to the 16 MiB frame): a formatted reply larger than the frame would turn into a per-file
  error after the work was done; declining up front is cheaper and counted.
