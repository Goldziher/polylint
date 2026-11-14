# 0054 — Secret Scanning with kingfisher, as a Download-Only Component

- Status: Accepted
- Date: 2026-11-14
- Related: ADR 0003, ADR 0028, ADR 0039, ADR 0040, ADR 0042, ADR 0046

## Context

Nothing in poly looks for committed credentials beyond the `detect-private-key` hook. Issue #112 asks for secret
detection on the files a run covers, with findings mapped to lines, an allowlist through poly's own mechanisms, a
known-fake fixture and no network access.

## Decision

A `secrets` engine, lint only, cross-cutting (every text file) and opt-in: `[engines.secrets] enabled = true`. It
wraps the `kingfisher-scanner` library (mongodb/kingfisher, Apache-2.0) with its built-in rule catalog
(`kingfisher-rules`: Betterleaks' rules plus Google's Veles detectors and a few first-party ones), at the library's
default confidence floor (medium, which leaves out the generic low-confidence API-key rule).

- **Findings.** One error diagnostic per visible rule match, code `secrets/<rule id>` (`secrets/betterleaks.github-pat`),
  with the span of the matched value. Rules kingfisher marks hidden (the companion matches other rules depend on) are not
  reported.
- **Redaction.** The scanner runs with `redact_secrets`, which replaces the matched value inside kingfisher after its
  fingerprint is computed. The engine never reads the value: a diagnostic carries the rule name, the length in bytes and the
  confidence. Nothing secret can reach the report, a log line or the result cache, which stores diagnostics.
- **Offline.** kingfisher's `validation` feature, which calls a credential's provider to test it, is not compiled in,
  so nothing here can reach the network at run time.
- **Allowlisting** is the runner's, not the engine's: an inline `poly: allow[secrets/<rule id>] reason`
  (`allow[secrets]` covers the family) and `[per_file_ignores]` entries naming `secrets/...` or `secrets` (ADR 0028).
  `tests/pipeline.rs` pins both through the real `lint` pipeline, each as a pair with a control that reports.
- **Placement.** A new workspace, `components-secrets/`, with the bridge crate `poly-engines-secrets` and the
  alef-generated `poly-engines-secrets-component`, built from the root `alef.toml` like pyrefly's (ADR 0042). The
  component is download-only: no bundled build links it, and it ships for the four component targets (the musl binary is
  bundled and downloads no component). A project that enables the engine on a lockless build gets the standard
  "component not available in this build" failure (ADR 0039), never a silent pass.
- **Pins.** `kingfisher-scanner` `=3.0.3` and `kingfisher-rules` `=3.2.0` from crates.io, both exact: the catalog
  decides what is detected and changes between minor releases, so moving it is deliberate and moves
  `SecretsEngine::version()` with it. `tests/version_audit.rs` reads `components-secrets/Cargo.lock` and fails when the
  engine's version string disagrees. No git pin is needed.

### Why its own workspace

The crates are on crates.io, so ADR 0040's git-pin rule does not force a separate workspace; the reason is the native
build. `kingfisher-vectorscan-sys` links vectorscan (C++). Its build script downloads a prebuilt archive for the target
from a GitHub release, verifies it against a SHA-256 embedded in the published crate, and links it; cmake and Boost are
needed only for a source build. kingfisher's dependency tree is also large (`gix`, `tokei`, `tiktoken-rs`, `aws-lc-rs`;
none of it duplicates poly's). In the root or `components/` workspace that would put a build-time network fetch and a
C++ link into every contributor's and every CI job's `cargo test --workspace`, and every lockfile. In its own workspace
only the `secrets` jobs and the release matrix pay for it, and the rest of the repository builds as before. The cost is a
fourth lockfile, a `deny.toml` and the gate wiring ADR 0042 lists (`mise.toml`, `ci.yaml`, `publish.yaml`, the bump and
lock-check tasks, the hook jobs), all mirrored from pyrefly's.

### Licences

kingfisher-rules is `Apache-2.0 AND MIT AND CC-BY-SA-4.0`. The third term is a word-frequency list (wordfreq by Robyn
Speer, with its own notice) used for entropy and word filtering. Share-alike applies to that data, which the crate
redistributes unmodified, not to the code that reads it or to poly, so the licence needs no change. `components-secrets/deny.toml`
allows `CC-BY-SA-4.0` for the `kingfisher-rules` crate alone (`[[licenses.exceptions]]`), so no other dependency can bring
it in unnoticed, and `ATTRIBUTIONS.md` credits wordfreq, Betterleaks (MIT) and Veles (Apache-2.0).

### Vectorscan build

Prebuilt archives exist for every component target (aarch64 macOS, x86_64 and aarch64 Linux gnu, x86_64 Windows
MSVC) and for Linux musl. The archive is built on Ubuntu 22.04 (glibc 2.35), while the release builds Linux components
in a manylinux_2_28 image (glibc 2.28), so whether it links there is verified in CI (`secrets-manylinux`) rather than
assumed. If it does not, the bridge crate's `vectorscan-source` feature builds vectorscan from source (cmake, Boost
headers, a C++ toolchain); the second manylinux leg exercises that path. The Windows prebuilt uses the dynamic CRT, which
is what a Rust cdylib on MSVC uses.

## Consequences

- Positive: line-accurate secret findings from a maintained rule catalog with no network access and one `poly.toml`
  key; the allowlist is the one every other rule already uses; nothing changes for projects that do not enable it.
- Negative: the first scan in a process compiles the catalog (about 500 rules) into a vectorscan database, once per
  process and component load; its cost and the archive size are not yet measured. Base64-encoded values are scanned
  (a kingfisher default). Rule selection (`select` / `ignore`) is not exposed: allowlist with `poly: allow` or
  `[per_file_ignores]`.
- The component does not run on the bundled musl binary.

## Alternatives considered

- **Betterleaks** (Go, MIT, by the gitleaks author): the fallback as a Go sidecar once its v2 SDK is stable; it is still a
  release candidate and the API moved between candidates. kingfisher's catalog is generated from Betterleaks' rules.
- **gitleaks** (Go, MIT): feature complete with security patches only; its author moved to Betterleaks; 222 rules.
- **ripsecrets** (Rust, MIT): the library prints matches and returns a count rather than findings, the last release
  is from 2025-05, and its rules are thin.
- **trufflehog**: AGPL. **noseyparker**: archived. **secretlint**, **detect-secrets**: need a Node or Python runtime
  (ADR 0049).
- **Put it in `components/` or the root workspace.** Rejected for the build-script network fetch and the lockfile weight
  above.
- **Build vectorscan from source always.** Rejected as the default: cmake and Boost on every release builder for no
  gain where the verified archive links.
