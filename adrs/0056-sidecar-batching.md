# 0056 — Sidecar Batching and Frame Encoding

- Status: Accepted
- Date: 2026-11-15
- Amends: ADR 0045
- Related: ADR 0050

## Context

ADR 0045 runs a sidecar as one child process that every call goes through, so rayon workers queue on it and Go and
shell formatting were effectively serial. Its stdio envelope carries a payload as a JSON array of numbers, up to four
characters per byte, and a frame is bounded at 16 MiB in both directions. The Go sidecars built that array as a `[]int`
and decoded it back through one, an allocation of eight bytes per payload byte on each side.

## Decision

- **Batches.** The `go` and `sh` sidecars answer `BatchSupport` (format only) and `FormatBatch`, and advertise the
  `BATCH_SUPPORT` hook bit in `describe`, so the host sends `WireFileSlice` batches (`batch_shard_floor` 2) and one
  request formats many files. A member that cannot be formatted (invalid UTF-8, a panic) is its own error item and
  does not fail its siblings; a slice outside the body fails the request. Members are formatted on a goroutine pool
  sized to `GOMAXPROCS`, which is the parallelism a single-process sidecar has: a batch of one request uses every
  core instead of one file at a time. Results equal per-file formatting; a test compares them.
- **Host chunking.** The host splits a batch sent to a sidecar into requests of at most 2 MiB of content, because a
  request or reply of more than about 4 MiB of content does not fit a frame. A file over the 3 MiB file limit is
  declined like the single-file path declines it.
- **Frame encoding.** The Go sidecars write the number array straight into the output buffer with
  `strconv.AppendUint` and parse it with a hand-written scanner into the byte slice, with no intermediate `[]int`. The
  wire format is unchanged.

## Consequences

- The component index entries for `go` and `sh` carry `hooks = 128`, regenerated from `describe`.
- The wire cost of four characters per byte and the 3 MiB file limit remain. Removing them needs a binary payload in
  alef's stdio envelope (an `invoke_bytes` carrying raw bytes in the frame instead of a JSON number array), which is
  an alef change.
- goimports keeps its local prefix in a package variable, so the Go sidecar sets it once per batch before the pool
  starts and never writes it from a worker.

## Alternatives considered

- Several sidecar processes: contradicts the one-child model and the restart budget of ADR 0045.
- Sending a whole shard in one frame: overflows the frame for any tree of ordinary size.
