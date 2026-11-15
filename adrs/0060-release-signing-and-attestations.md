# 0060 — Release Signing Isolation, SBOMs and Attestations

- Status: Accepted
- Date: 2026-11-15
- Related: ADR 0010, ADR 0039, ADR 0054

## Context

A release ships five `poly` archives, a signed component archive per component and target (ADR 0039), the component
lock every core binary embeds, and `sha256sums.txt`; the npm and PyPI packages repackage the same archives. A security
audit of `publish.yaml` found that the jobs compiling the whole dependency tree also held everything worth stealing:

- **H1.** The component build jobs held the Ed25519 signing key, a write token and `persist-credentials`, so any
  build script or proc macro in the tree could sign or upload.
- **H2.** The signing key, `HOMEBREW_TOKEN` and `RELEASE_TAG_TOKEN` were repository secrets, readable by any job.
- **H3.** The tools run beside the key (alef, mise, jq) were checked against the digest the upstream release served on
  the day, and `rustup` came from `curl | sh`; the musl build image was a mutable tag.
- **H4.** A shared `Swatinem/rust-cache` let one run's build output reach the next, and the previous release's lock was
  reused without checking it.

Nothing tied a published archive to the workflow that built it, and nothing described what an archive contains.

## Decision

### Signing in its own job

- `build-components` and `build-binaries` run with `permissions: contents: read`, `persist-credentials: false`, no
  secret, no job-level token and no `rust-cache`. `build-components` stages the unsigned payloads (the libraries and
  executables beside their `.input-hash`) as one tar, with a separate digest manifest artifact
  (`mise run components:release stage`).
- `sign_components` runs on a fresh runner of the same OS and image, in the `release` environment, with a read-only
  token. It checks the tar and plan against the manifest (`unstage`), then `alef component package` re-fingerprints
  the checked-out tree (`cargo metadata`, `rustc -vV`, `go version`: no dependency code runs), signs and verifies. The
  key reaches only that one command. alef has no command that signs an existing unsigned archive, which is why the
  signing job needs the toolchain; `alef component sign` would remove it.
- Uploads happen in `publish_assets`, the one job on the build path with `contents: write`, which uploads exactly the
  files `sha256sums.txt` lists after `sha256sum -c`. `POLY_COMPONENT_SIGNING_KEY`, `HOMEBREW_TOKEN` and
  `RELEASE_TAG_TOKEN` are secrets of the `release` environment.
- Every binary the signing containers run is pinned by sha256 in the repository: alef, mise, jq and rustup-init in
  `mise-tasks/components/release.sh`, alef on the native runners through `mise.lock`. The musl build image is pinned by
  digest.
- `previous-lock` checks the reused lock against that release's `sha256sums.txt` and, when one exists, its build
  provenance attestation, and every key its entries are signed with must equal the key `alef.toml` trusts under that id.
  The merged lock's `public_keys` must equal `alef.toml`'s table.

### SBOMs and attestations

- Release binaries are built with `cargo auditable`, so each carries its dependency list (`cargo audit bin`).
- Each archive built for a tag ships a CycloneDX 1.5 SBOM as `<archive>.cdx.json` (`mise run release:sbom`):
  `cargo-cyclonedx` for the Rust artifacts with the target and features they were built with, `cyclonedx-gomod` for
  the Go sidecars. SBOMs are listed in `sha256sums.txt`; the npm platform packages carry `bin/poly.cdx.json` and the
  wheels `polylint/sbom/poly.cdx.json`. A component reused from an earlier release keeps that release's SBOM.
- `attest_provenance` attests every file `sha256sums.txt` lists and `sha256sums.txt` itself
  (`actions/attest-build-provenance`); `attest_sboms` binds each SBOM to its archive (`actions/attest-sbom`, one per
  archive). Only these two jobs hold `id-token: write` and `attestations: write`. `finalize_release` requires both.

### Key rotation: two keys per lock

The lock trusts two public keys: the **current** key, whose private half is `POLY_COMPONENT_SIGNING_KEY` and whose id is
`KEY_ID` in `release.sh`, and the **next** key, generated ahead and held offline, never in CI. Every lock and every
binary built from it trusts both. A key id and its public key are part of every component's fingerprint (alef), so
adding or removing a key rebuilds every component once; that is expected and needs no `rebuild_components`.

To rotate, in one release:

1. Generate the key after next: `openssl genpkey -algorithm ED25519 -out next.pem`, its public half with
   `openssl pkey -in next.pem -pubout -outform DER | base64`. Store `next.pem` offline.
2. In `alef.toml` `[crates.component_distribution.public_keys]`, remove the retiring key and add the new next key.
3. Put the old next key's private half into the `release` environment's `POLY_COMPONENT_SIGNING_KEY` and set `KEY_ID`
   to its id.
4. Release. `component-lock` asserts the lock carries exactly `alef.toml`'s keys and that every entry built for the tag
   is signed with `KEY_ID`.

Until the first next key exists, `alef.toml` carries only `poly-release-1`; adding `poly-release-2` as the next key
is the first step, a release on its own.

After a suspected key compromise, rotate the same way at once. A binary checks each component against the archive
digest and the signature in the lock it embeds, so a stolen key cannot make an installed binary accept a different
archive; it could sign archives for a future lock, which the rotation stops.

### Verifying a release

```sh
gh release download vX.Y.Z --repo Goldziher/polylint --pattern 'poly-X.Y.Z-x86_64-unknown-linux-gnu.tar.gz*' \
  --pattern sha256sums.txt
sha256sum -c --ignore-missing sha256sums.txt
gh attestation verify poly-X.Y.Z-x86_64-unknown-linux-gnu.tar.gz --repo Goldziher/polylint \
  --signer-workflow Goldziher/polylint/.github/workflows/publish.yaml
gh attestation verify poly-X.Y.Z-x86_64-unknown-linux-gnu.tar.gz --repo Goldziher/polylint \
  --predicate-type https://cyclonedx.org/bom
```

`install.sh`/`install.ps1` take `POLY_VERIFY_ATTESTATION=1` and the GitHub Action `verify-attestation: true` to run
the provenance check before installing; all three fail closed when it does not verify.

## Consequences

- Positive: a compromised build dependency can no longer sign a component, upload an asset or move a tag; it can
  still produce a malicious payload, which the signing job then signs, because the digest manifest comes from the same
  job. Attestations make that visible as a build of this workflow on this commit, and SBOMs say what each archive
  contains.
- Positive: every release asset is attested, and installers and the Action can require it.
- Negative: the signing job installs the toolchain and runs `cargo metadata`, which downloads the dependency sources,
  to recompute the fingerprint; it trusts the same toolchain the build used. The key is in the environment of that
  one `package` command and of the `cargo`, `rustc` and `go` processes alef starts to fingerprint.
- Negative: no cache for the release builds makes them slower. The `release` environment must exist with the three
  secrets before the next release, or the release fails.
- Negative: a release made before this change has no attestation. `previous-lock` accepts such a lock on its checksum
  alone, with a warning; an attacker able to replace both the lock and `sha256sums.txt` of the newest release could
  still get it reused until that release is followed by an attested one.

## Alternatives considered

- **Sign unsigned archives produced by the build job.** The right shape, but alef packs the signature into the archive
  and has no sign-only command; editing archives in this repository would fork alef's format. Belongs upstream.
- **Keep signing in the build job, with the key in a file.** Leaves the key readable by every build script (H1).
- **Sigstore keyless signing for components instead of Ed25519.** The runtime verifies the embedded Ed25519 lock
  offline; keyless verification needs the network and a transparency log. Attestations add Sigstore on top instead.
- **One SBOM attestation for all archives.** `actions/attest-sbom` binds one SBOM per call; a per-archive matrix keeps
  each SBOM bound to the archive it describes.
