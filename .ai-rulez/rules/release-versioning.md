---
type: Decision
title: Release Versioning
x-ai-rulez:
  kind: rule
  id: release-versioning
  metadata:
    priority: high
---

# Release Versioning & Distribution

poly ships **a single binary — `poly`** (driven by one config), distributed the way ruff /
oxlint / biome are: **prebuilt, platform-specific binaries attached to a GitHub release, plus an
installer**. The channels are the `curl | sh` / PowerShell installer (`install.sh` /
`install.ps1`), the GitHub Action (`polylint/polylint@v0`, `action.yml` — which shells out to the
same `install.sh`), Homebrew (`brew install polylint/tap/polylint`), Scoop
(`polylint/scoop-bucket`, `scoop install poly`), **npm (`@polylint/polylint`)** and **PyPI
(`polylint`)**. The release artifacts are named `poly-<version>-<triple>.tar.gz` (`.zip` on
Windows), five targets, plus a `sha256sums.txt`; every other channel serves those same archives.
We do **NOT publish to crates.io** — our crates depend on pinned git dependencies (**oxc, biome,
rubyfmt** — see the `allow-git` lists in `deny.toml` and `components/deny.toml`), which crates.io forbids in published crates, and we
have no need for a source-distribution channel. Ruff is **not** a git pin: since 2026-08-29 the
ruff crates come from crates.io at exact `=` versions (ADR 0003's "Amendment — 2026-08-29"); the
`=` is deliberate, since those are unsemvered internals.

**The command is `poly` on every channel.** The npm and PyPI packages are named `polylint`
only because the unscoped `poly` name is taken on both registries; each channel also installs
`polylint` as an *alias* onto the same executable — never a second binary. That alias is part
of the shipped surface on all of them: npm `bin` has two entries, the PyPI distribution has two
console scripts, the Homebrew formula does `bin.install_symlink bin/"poly" => "polylint"`,
`install.sh` writes a relative symlink (`install.ps1` a hard link), and the Scoop manifest's
`bin` array carries both shims.

Versioning is **lock-step**: bump all surfaces to the same `X.Y.Z` in one change. The single
source of truth is the workspace `[workspace.package] version` in the root `Cargo.toml`; member
crates inherit it via `version.workspace = true`. `plugin/.ai-rulez/config.plugins.toml`
`[plugin].version` and generated manifests such as `plugin/.claude-plugin/plugin.json` and
`plugin/.codex-plugin/plugin.json` move with it. The root marketplace resolves that member
bundle. The plugin (ADR 0022) is one more
lock-step surface, not a separately tracked version. So do the wrapper packages:
`npm-package/package.json` **and each of its five exact `optionalDependencies` pins**, the five
`npm-package/platforms/*/package.json` files, `pip-package/pyproject.toml`, and
`pip-package/polylint/__init__.py`.

## Rules

- Tags MUST be `v<version>` (e.g. `v0.1.0`); `.github/workflows/publish.yaml` triggers on
  `v[0-9]+.*` and builds from the tagged commit, failing the run if the tag and the
  `Cargo.toml` version disagree.
- The release workflow cross-compiles the five platform archives (linux gnu/musl x86_64,
  linux gnu aarch64, macOS aarch64, windows-msvc x86_64; Intel macOS is not shipped, ADR 0046), verifies all five are present,
  then uploads `sha256sums.txt` and un-drafts the release. The installers and the GitHub Action
  fetch the archive matching the host.
- **Homebrew installs the prebuilt archives, like every other channel.** The formula is rendered
  from `packaging/polylint.rb.tmpl` by `xberg-io/actions/publish-homebrew-source-formulas`
  (config `packaging/homebrew.json`) — `version` plus a class-level macOS arm64 `url` with
  `on_macos { depends_on arch: :arm64 }` and `on_linux` arm64 / x86_64 archives, then
  `bin.install "poly"` and the `polylint` symlink — and pushed to the shared
  `polylint/homebrew-tap` as `polylint-release-helper[bot]` with a token minted from the
  organization's GitHub App (org secrets `BOT_APP_ID` / `BOT_APP_PRIVATE_KEY`; no PAT). The action
  hashes the archives it downloads; the workflow then checks every rendered hash against the
  release's own `sha256sums.txt` before pushing, so the formula can never disagree with the
  published checksums. `homebrew_bottles` then bottles the formula on macOS arm64 (`arm64_tahoe`)
  and Linux x86_64 / arm64, uploads the bottles to the release, and `publish_homebrew_bottles`
  merges the `bottle do` block. There is no `depends_on "rust"`: the formula never compiles.

  This replaced a source-build formula, and the reasons are worth keeping: a source build meant a
  full cargo compile of ruff + oxc + biome + mago + tree-sitter for every user the tap's bottler
  did not cover, and its runner matrix had no Intel macOS, so those users compiled on every
  install (Intel macOS is no longer shipped at all, ADR 0046). Download and extract measures
  **2.2 s** against that.
- **The wrapper packages ship the release's own binaries, never a rebuild.** pnpm publishes five
  per-platform packages (`os`/`cpu`/`libc`-gated, pulled in through `optionalDependencies` —
  no `postinstall` download) and PyPI publishes per-platform wheels carrying the binary as
  package data, plus a `py3-none-any` fallback whose only job is to explain itself on an
  unsupported platform. Both jobs `gh release download` the archives back off the release and
  verify them against `sha256sums.txt` before packaging; a separate build would produce bytes
  the published checksums do not cover.
- **Both registries authenticate with OIDC trusted publishing** — no `NPM_TOKEN`, no PyPI API
  token. The jobs carry `permissions: id-token: write` and publish using pnpm 12.15.0, which
  supports token-less publishing and provenance. npm can only configure a trusted publisher for
  a package that *already exists*, so every new package needs one manual publish first (under a
  non-`latest` dist-tag).
- **A publish job must never re-draft the release.** `publish_npm`, `publish_pypi`,
  `publish_scoop` and `publish_homebrew` run after `finalize_release` and have nothing depending
  on them, so a failure cannot put the GitHub release back into draft. Only `publish_scoop` is
  `continue-on-error: true` (a red job on a green run); a failed `publish_npm`, `publish_pypi` or
  `publish_homebrew` fails the workflow run, deliberately, so a channel that did not update is
  visible in the run that caused it.
- **Scoop mirrors the Homebrew shape.** `xberg-io/actions/publish-scoop-manifests` renders
  `bucket/polylint.json` and an identical `bucket/poly.json` (so `scoop install poly` keeps working)
  from `packaging/polylint.json.tmpl` into `polylint/scoop-bucket`, through the same GitHub App.
  The workflow checks the rendered hash against `sha256sums.txt` and fails the job if a manifest
  lost its two-entry `bin` array — losing `bin` is silent, since `scoop install` still succeeds
  and produces no command. Only `x86_64-pc-windows-msvc` exists; do not invent an `arm64` block.
- We do not publish to crates.io; the prebuilt binary is the only thing any channel ships.
- Commit `Cargo.lock` and pin git-dependency `rev`s (all crates from one monorepo share a `rev`)
  so a tagged build is reproducible.
- `mise run release:bump <version>` bumps `Cargo.toml`, the npm and PyPI manifests, and
  `.ai-rulez/config.toml [plugin] version` together, refreshes `Cargo.lock`, regenerates the
  plugin outputs, and asserts every substitution actually landed — including that the npm
  `optionalDependencies` pins and the `platforms/` directories describe the same package set —
  before finishing. Never bump one surface without the other: a stale pin resolves to the
  previous release's binary under the new version, and it installs cleanly.
