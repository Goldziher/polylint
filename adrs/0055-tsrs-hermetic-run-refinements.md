# 0055 — tsrs Check: Declared Extends, Scoped Manifests and Project Declaration Files

- Status: Accepted
- Date: 2026-11-15
- Amends: ADR 0051
- Related: ADR 0042

## Context

ADR 0051 made the tsrs check hermetic and dropped "not installed" findings for packages the project declares. In use
that left four gaps. A `tsconfig.json` that extends an npm package (`@tsconfig/node20/tsconfig.json`) reports `TS6053`
and nothing else is checked. Declaration files were never reported, so an error in the project's own `src/global.d.ts`
was invisible. Every `package.json` in the tree declared packages for every file, so in a monorepo one package's
dependency hid another's unresolved import. React projects without `@types/react` in the in-memory tree got `TS7026`
and `TS2875`, which are "types not installed" findings in disguise.

## Decision

- **Declared `extends`.** An `extends` that cannot be resolved (`TS6053`) and names a package declared by a governing
  `package.json` is dropped from the config's errors and replaced by one warning on `tsconfig.json`: "extended config
  `<spec>` is not available without node_modules; compiler options from it were not applied". The program is checked
  without the inherited options. An `extends` of an undeclared package stays an error. Nothing is downloaded.
- **Own declaration files are reported.** Only files under `node_modules` are inputs that are never reported; a checked
  `.d.ts` of the project reports its errors. `skipLibCheck` keeps its `tsc` meaning and skips them.
- **Scoped manifests.** A dependency is declared for a file by the `package.json` files in the file's directory and the
  directories above it, as Node resolves packages. A file below no manifest uses the root-most one. `workspace:`
  dependencies are declared like any other.
- **React.** With `react` or `@types/react` declared and the `jsx` option set, `TS7026` and `TS2875` (for a `react/...`
  runtime) are "not installed" findings and are dropped, like the codes ADR 0051 lists. `report_unresolved_modules = true`
  still keeps them.

The mapping revision of `version()` moves with the filtering.

## Consequences

- A project extending a declared preset is checked instead of failing on its config; options from the preset (such as
  `strict`) are missing, so findings can differ from `tsc`'s, and the warning says so.
- Declaration-file errors a project never saw now appear when it enables the check.
- A dependency declared only in a sibling package no longer hides an import; one declared in a root manifest above still
  counts.
- A `jsx` option inherited from an unavailable extended config is not seen, so React findings stay in that case.

## Alternatives considered

- Re-read the config with the failing `extends` removed from the text: needs a rewrite of JSONC; dropping the error
  from the parsed config has the same effect.
- Scope to the nearest `package.json` only: hides nothing across packages but reports root-only dev dependencies
  (`@types/node`, `vitest`) as errors in every package.
