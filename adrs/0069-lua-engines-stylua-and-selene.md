# 0069 — Lua Engines: StyLua and selene

- Status: Accepted
- Date: 2026-11-16

## Context

Lua had no tier-1 engine: `.lua` files went to the tree-sitter reindenter and carried no lint coverage, and the
only lint was the opt-in `luacheck` catalog tool, which needs a host install. StyLua (formatter) and selene (linter)
are the de facto Lua pair and both are published as libraries on crates.io under MPL-2.0 (#173, #129).

Checked against the published crates rather than the issue text:

- `stylua` 2.6.0 builds on `full_moon` 3.0.0; `selene-lib` 0.31.0 builds on `full_moon` 2.2.0. They do not share a
  parser, so cargo keeps both.
- `stylua`'s library shares a package with its CLI, so its `clap` 3 (`atty`, `proc-macro-error`) is compiled and
  trips `cargo deny` as two unmaintained-crate notices.
- MPL-2.0 is already on the license allow-list; it is file-level copyleft and poly links the crates unmodified.

## Decision

- **A new `poly-engines-lua` family in the root workspace, shipped as the `lua` component.** The `components/`
  workspace exists for git-pinned crates and `links` conflicts; neither applies (both crates are registry
  dependencies, no native library), so the family follows `nix`, `sql` and `jsonschema`: a `poly-engines-lua` crate,
  an alef-generated `poly-engines-lua-component`, an `engine-lua` feature on `poly-core` and a `lua` feature on
  `poly-engines`. It is in the `all` preset and not opt-in, like every tier-1 language.
- **Two engines.** `stylua` (format only) and `selene` (lint only), registered for the new `Language::Lua`
  (`.lua`). Tier-1 replaces the tree-sitter reindenter for Lua.
- **Exact pins.** `stylua = "=2.6.0"` and `selene-lib = "=0.31.0"`: formatter output and the lint set change
  between releases, so a caret range would change results without a `version()` bump. selene's parser is reached
  through a `full_moon_2` alias pinned to the version `selene-lib` resolves.
- **Native configs through host hooks.** `stylua.toml`/`.stylua.toml` and `selene.toml` are resolved by the host
  (`HostHook::StyluaConfig`, `HostHook::SeleneConfig`): the nearest file at or above the linted file, up to the
  repository root. Their digests salt the cache keys, so a component and an in-process engine key identically. The
  precedence is StyLua/selene defaults, poly's layout globals (`column_width` = `line_length` 120, `line_endings`),
  the native file, then `poly.toml`. Indentation stays StyLua's default (tabs), because that is what Lua projects'
  `.stylua.toml` files assume.
- **StyLua verifies its output** (`OutputVerification::Full`): a formatter bug is an error on that file, never
  changed code.
- **selene supports its built-in standard libraries only** (`lua51`, `lua52`, `lua53`, `luau`, joined with `+`). A
  `std` naming a library file (`roblox`, a project's own `.yml`) reports an `unsupported-std` warning rather than
  linting against the wrong globals; supporting library files needs a YAML reader and a roblox std fetch, and is
  left for demand.
- **The `luacheck` catalog entry stays.** It is opt-in, additive and unrelated to the engine name; deleting
  vendored mdsf data would only break users who enabled it. The catalog `stylua` formatter is yielded to the
  tier-1 formatter by the existing "already built in" rule. selene is the built-in lint; docs point there.
- **`cargo deny` ignores RUSTSEC-2024-0370 and RUSTSEC-2024-0375** in both lockfiles, recorded beside the other
  maintenance-notice ignores.

## Consequences

- Good, because Lua gets a formatter and a real linter with no host install, configured by the files projects
  already have.
- Good, because the hooks keep cache keys right for component-hosted engines without the engine reading disk in
  the host.
- Bad, because two `full_moon` majors and `clap` 3 compile into the component and the bundled build.
- Bad, because roblox and custom std libraries are not supported yet.
- `Language::Lua` replaces `Other("lua")`: fixtures that used Lua as the "tree-sitter only" language moved to Scala.
