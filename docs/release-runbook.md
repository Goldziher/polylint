# Release runbook

How a release flows, and how to take one back. The workflow is `.github/workflows/publish.yaml`.

## Flow

1. `verify_ci` waits (up to 30 minutes) for the `ci` workflow's required check runs on the tagged commit. A
   `workflow_dispatch` can bypass it with `skip_ci_check`; use that only for an emergency re-publish.
2. The GitHub release is created as a draft, binaries and components are built, `checksums` uploads
   `sha256sums.txt`, and `finalize_release` un-drafts it.
3. The publish jobs run after `finalize_release`: Homebrew, Scoop, npm, PyPI, the plugins and the major tag.
   Homebrew, Scoop and the major tag only move when the released tag is the newest `vX.Y.Z` tag; re-running an
   older tag leaves them alone.

The release archives, npm platform packages and wheels carry `ATTRIBUTIONS.md` and `THIRD-PARTY-LICENSES.md` next
to the binary. `mise run release:third-party-licenses [out]` generates the latter (crate table from
`cargo metadata --locked` over the five workspaces, then `ATTRIBUTIONS.md` verbatim, which holds the tsrs NOTICE).

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

## Component signing key rotation

Component archives are signed with the key in the `POLY_COMPONENT_SIGNING_KEY` secret; the public half is
`poly-release-1` under `[crates.component_distribution.public_keys]` in `alef.toml`, and `KEY_ID` in
`mise-tasks/components/release.sh` names the signing key.

To rotate: add the new key id and public key to `alef.toml`, switch `KEY_ID` and the secret, and release. The
`component-lock` job asserts the lock's key set and each artifact's key id in `publish.yaml`:

```text
and ((.public_keys | keys) == ["poly-release-1"])
and all(.artifacts[]; .key_id == "poly-release-1" and (.url | startswith($prefix)))
```

Edit both lines for the new id. During a dual-key period the key set is both ids (for example
`["poly-release-1", "poly-release-2"]`) and the artifact check accepts either; narrow them back once binaries signed
with the old key are no longer in use. Older binaries embed the lock they were built with, so keep the old public key
until they have aged out.
