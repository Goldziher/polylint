# 0048 — Publish npm packages under the `@polylint` scope

- Status: Accepted
- Date: 2026-11-14
- Related: ADR 0010, ADR 0022, ADR 0046

## Context

The npm packages were published under the maintainer's personal scope: the `@goldziher/polylint` wrapper, its five
per-platform packages and the OpenCode/Pi plugin `@goldziher/opencode-poly`. The product is called polylint and the
project now has its own `@polylint` organization on npm, so the personal scope no longer describes who publishes them.

## Decision

Every npm package of this repository is published under `@polylint`: `@polylint/polylint`,
`@polylint/polylint-<platform>` for each platform and `@polylint/opencode-poly`. Only the scope changes; the package
names, the `poly` and `polylint` executables and the lock-step version stay as ADR 0010 and ADR 0022 describe them.
The unscoped `poly` name stays taken, so the scope is still what makes the wrapper's name available.

## Consequences

- Positive: the packages sit under the organization that owns the product, and ownership can be shared without moving
  a personal scope.
- Negative: users of `@goldziher/polylint` and `@goldziher/opencode-poly` must reinstall under the new names; the old
  packages stop receiving releases. npm can configure trusted publishing only for a package that already exists, so each
  of the seven new packages needs one manual first publish before the release workflow can publish by OIDC.
