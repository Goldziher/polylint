# 0064 — Sign Components Without a Toolchain

- Status: Accepted
- Date: 2026-11-16
- Amends: ADR 0060

## Context

ADR 0060 moved the component signing key off the build runner, but alef had no command that signs an existing
archive, so `sign_components` reran `alef component package`. That job installed the Rust toolchain and Go, ran the
manylinux container on the gnu legs and let alef start `cargo metadata`, `rustc` and `go` with the key in their
environment. alef 0.130.0 adds `alef component sign` (alef ADR-0026), which signs the output of
`component package --unsigned` without loading `alef.toml` or starting any build tool, and writes archives
byte-identical to a signed `package`.

## Decision

- `build-components` packages unsigned (`mise run components:release package`, `--unsigned`, inside the same container
  as the build on the gnu legs) and stages the unsigned archives and records, not the raw payloads, as one tar with its
  separate digest manifest.
- `sign_components` checks out the tree, installs only alef (pinned through `mise.lock`), checks the tar and plan
  against the manifest (`unstage`) and runs `mise run components:release sign`: `alef component sign --plan` with
  `--key-env POLY_COMPONENT_SIGNING_KEY --key-id poly-release-2 --public-key <alef.toml's poly-release-2>`, then
  `alef component verify`. `--public-key` refuses a secret that is not the configured key; `--plan` refuses an
  archive with no matching build row.
- Signing does not depend on the host, so every triple signs on `ubuntu-latest`; the job keeps the `release`
  environment, the read-only token and `persist-credentials: false`.

## Consequences

- The key is never in the environment of `cargo`, `rustc`, `go` or a container, and the signing job downloads no
  dependency sources. ADR 0060's negative consequence about the signing job's toolchain no longer holds.
- As before, the signer certifies what the build job packaged: a compromised build can still produce a malicious
  archive, and `input_hash` in a record is not covered by the signature (alef ADR-0026). Attestations and SBOMs remain
  the evidence of what was built.
- The signing job is faster and needs no matrix of runner images.

## Alternatives considered

- Keep re-packaging in the signing job: it keeps the toolchain, and the processes it starts, next to the key, which
  is what alef added `sign` to avoid.
- Detached signatures: rejected upstream, since the runtime reads `component.sig` from inside the archive and the lock
  pins the archive hash.
