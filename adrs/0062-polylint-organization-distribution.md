# 0062 — Distribute Through the `polylint` Organization

- Status: Accepted
- Date: 2026-11-16
- Related: ADR 0010, ADR 0046, ADR 0048, ADR 0060

## Context

The repository moved from `Goldziher/polylint` to the `polylint` GitHub organization (`polylint/polylint`), next to
the sibling tools skif, adrlint and uncomment. The organization runs one shared Homebrew tap (`polylint/homebrew-tap`,
tap `polylint/tap`) and one Scoop bucket (`polylint/scoop-bucket`) for all of its tools, and its own GitHub App writes
to them. Until now the release workflow pushed to the personal `Goldziher/homebrew-tap` and `Goldziher/scoop-bucket`
with a personal access token (`HOMEBREW_TOKEN`), and moved the major tag with a second one (`RELEASE_TAG_TOKEN`). The
formula and manifest were produced by repository scripts (`mise run release:homebrew-formula`,
`release:scoop-manifest`), different from the templates the other organization tools use.

Build provenance attestations (ADR 0060) name the repository and workflow that signed them, and a certificate keeps the
identity it was issued under. The attestations of v0.35.0 and earlier were signed as `Goldziher/polylint`. GitHub Pages
for the old owner does not redirect, so the documentation is now served only at `polylint.github.io/polylint`.

## Decision

- **Every owner reference names the organization.** Repository, docs site (`https://polylint.github.io/polylint/`),
  installer URLs, the GitHub Action (`polylint/polylint@v0`), the plugin marketplace, the Go sidecar module paths and
  the component `url_template` all use `polylint/polylint`. The npm scope is `@polylint` (ADR 0048). Personal author
  metadata and repositories that remain under `Goldziher` are unchanged.
- **Homebrew and Scoop follow the organization pattern.** The formula and manifests are rendered from templates in
  `packaging/` by `xberg-io/actions/publish-homebrew-source-formulas` and `xberg-io/actions/publish-scoop-manifests`,
  and pushed as `polylint-release-helper[bot]` with tokens minted by `actions/create-github-app-token` from the org
  secrets `BOT_APP_ID` and `BOT_APP_PRIVATE_KEY`. The release workflow checks every rendered hash against the
  release's `sha256sums.txt` before pushing. The formula stays a prebuilt-binary formula for macOS arm64 and Linux
  arm64 and x86_64, with the `polylint` symlink, the `Aliases/poly` alias and the `formula_renames.json` entry.
  Scoop gets `polylint.json` and an identical `poly.json`.
- **Homebrew is bottled.** After the formula lands, the release builds bottles on `macos-26` (`arm64_tahoe`),
  `ubuntu-latest` (`x86_64_linux`) and `ubuntu-24.04-arm` (`arm64_linux`) with `xberg-io/actions/homebrew-build-bottles`,
  uploads them to the release, and merges the `bottle do` block with `xberg-io/actions/homebrew-merge-bottles`, as
  every other formula in the tap does.
- **No personal access tokens.** `HOMEBREW_TOKEN` and `RELEASE_TAG_TOKEN` are retired. The major-tag move uses an App
  token with contents and workflows write, falling back to `GITHUB_TOKEN` when the App cannot mint one.
- **Attestation checks accept both identities.** `install.sh`, `install.ps1` (and through them the GitHub Action) and
  the component release's check of the previous lock verify against `polylint/polylint` first and fall back to
  `Goldziher/polylint`, so releases up to v0.35.0 still verify.
- **`poly migrate` rewrites old schema URLs.** A `#:schema` line naming `goldziher.github.io/poly/` or
  `goldziher.github.io/polylint/` is pointed at `polylint.github.io/polylint/`.

## Consequences

- Positive: all organization tools share one tap, one bucket, one bot identity and one credential; there is no
  personal token to rotate, and the formula gets bottles.
- Positive: the `mise run release:homebrew-formula` and `release:scoop-manifest` scripts are gone; the templates are
  the whole per-repository surface.
- Negative: the npm and PyPI trusted publishers must name owner `polylint`, repository `polylint`; a publisher still
  naming the old owner rejects the OIDC token. The GitHub App must be installed on the tap, the bucket and this
  repository.
- Negative: the legacy `Goldziher/polylint` identity stays in the attestation checks for as long as releases up to
  v0.35.0 are installable.
- Negative: the render action hashes the archives itself rather than reading `sha256sums.txt`, so the cross-check is
  a separate workflow step that has to be kept.

## Alternatives considered

- **Keep the repository scripts and only change the owner.** Smaller diff, but the only tool in the tap rendered
  differently, and it still needed a personal token.
- **Drop the legacy identity from the attestation checks.** Simpler, but `--verify-attestation` would refuse every
  release published before the move, and the first post-move release would refuse to reuse the v0.35.0 component lock.
