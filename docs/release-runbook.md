# Release runbook

How a release flows, and how to take one back. The workflow is `.github/workflows/publish.yaml`.

## Flow

1. `verify_ci` waits (up to 60 minutes) for the `ci` workflow's required check runs on the tagged commit. A
   `workflow_dispatch` can bypass it with `skip_ci_check`; use that only for an emergency re-publish.
2. The GitHub release is created as a draft. Components and binaries are built with read-only tokens and no secret;
   `sign_components` signs the components in the `release` environment; `checksums` writes `sha256sums.txt`;
   `attest_provenance` and `attest_sboms` attest the files and SBOMs; `publish_assets` uploads everything listed in
   `sha256sums.txt`; and `finalize_release` un-drafts the release once all of them succeeded (ADR 0060).
3. The publish jobs run after `finalize_release`: Homebrew, Scoop, npm, PyPI, the plugins and the major tag.
   Homebrew, Scoop and the major tag only move when the released tag is the newest `vX.Y.Z` tag; re-running an
   older tag leaves them alone.

The release archives, npm platform packages and wheels carry `ATTRIBUTIONS.md` and `THIRD-PARTY-LICENSES.md` next
to the binary. `mise run release:third-party-licenses [out]` generates the latter (crate table from
`cargo metadata --locked` over the five workspaces, then `ATTRIBUTIONS.md` verbatim, which holds the tsrs NOTICE).

## When verify_ci finds no CI run

A tag on a commit whose main-push `ci` run was replaced or cancelled by the concurrency group never gets check
runs. After three consecutive polls with none, `verify_ci` fails fast with "no CI run exists for this commit".
Run `ci.yaml` through `workflow_dispatch` on the tag's commit and re-dispatch the publish, or dispatch publish
with `skip_ci_check=true` once you have confirmed the commit is good.

## Re-publishing old tags

Tags created before the license and `verify_ci` changes (v0.35.0 and earlier) cannot be re-dispatched through the
current workflow: its third-party-licenses job needs `mise-tasks/release/third-party-licenses.sh` at the tag, and
`verify_ci` expects today's check-run names, so older commits fail or never match. Use the workflow file from the
old tag, or publish by hand, and set `skip_ci_check` if a dispatch is still attempted.

## npm dist-tags

`latest` only moves for the newest stable `vX.Y.Z` tag. Re-running an older stable tag publishes the umbrella,
platform and plugin packages under `release-<major>.<minor>` instead; prereleases use `next`.

## Rolling back

- **GitHub release**: `gh release edit vX.Y.Z --draft=true`. The installers and the Action resolve
  `/releases/latest`, so a draft drops out of reach at once. Fix forward with a new patch tag; do not reuse the tag.
- **Major tag (`v0`)**: never delete it, since `Goldziher/polylint@v0` users would break. Re-point it to the previous
  good release: `git tag -f v0 vX.Y.(Z-1)^{commit} && git push -f origin refs/tags/v0` (needs `RELEASE_TAG_TOKEN`
  when workflows changed between the two commits).
- **npm**: prefer `npm deprecate @polylint/polylint@X.Y.Z "reason"` (and the five `@polylint/polylint-<platform>`
  packages) and publish a fixed patch. `npm unpublish` is only allowed within 72 hours and when nothing depends on
  the version; otherwise it is refused.
- **PyPI**: yank the release from the project's Manage page. A yanked version is
  skipped by resolvers unless pinned exactly; files cannot be re-uploaded under the same version.
- **Homebrew**: revert the formula commit in `Goldziher/homebrew-tap` (`Formula/polylint.rb`).
- **Scoop**: revert the manifest commit in `Goldziher/scoop-bucket` (`bucket/polylint.json` and `bucket/poly.json`).

## Release environment

`sign_components`, `publish_homebrew`, `publish_scoop` and `update_major_tag` run in the `release` environment, which
holds `POLY_COMPONENT_SIGNING_KEY`, `HOMEBREW_TOKEN` and `RELEASE_TAG_TOKEN`. Its deployment branch and tag policy
must allow the `v*` tags (and `main`, for a `workflow_dispatch` re-publish). Without the environment or its secrets
the release fails at `sign_components`.

## Component signing key rotation

Component archives are signed with the key in the `release` environment's `POLY_COMPONENT_SIGNING_KEY`; the public
half is listed under `[crates.component_distribution.public_keys]` in `alef.toml`, and `KEY_ID` in
`mise-tasks/components/release.sh` names the signing key. The lock carries two keys, the current one and a next one
held offline; the procedure is in [ADR 0060](../adrs/0060-release-signing-and-attestations.md#key-rotation-two-keys-per-lock).

`component-lock` asserts that the lock's `public_keys` equal `alef.toml`'s table by value, that every entry is signed
by one of them, and that every entry built for the tag is signed with `KEY_ID`; `previous-lock` applies the same
key check, plus the release's `sha256sums.txt` and attestation, before reusing an older lock. Adding or removing a
key changes every component's fingerprint, so that release rebuilds every component.
