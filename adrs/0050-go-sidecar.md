# 0050 — The Go Sidecar: gofumpt and goimports Grouping Without a Go Toolchain

- Status: Accepted
- Date: 2026-11-14
- Amends: ADR 0045
- Related: ADR 0014

## Context

Go formatting was the native-toolchain `gofmt` engine (ADR 0014) plus a Rust regrouping pass that imitates
`goimports`. Both need a host `go` installation, so the output of `poly fmt` depended on the installed Go, and
the regrouping was an approximation. Every library involved is Go: `mvdan.cc/gofumpt/format` and
`golang.org/x/tools/imports`. ADR 0045 made a Go executable a first-class component.

## Decision

- **A `go` sidecar** at `components/sidecars/go`, shaped like `sh`: pure Go, `CGO_ENABLED=0`, one component
  hosting the engine `gofmt` for language `go`, so `[engines.gofmt]` keeps working and the component engine
  replaces the built-in one of the same name. It is the home for Go-library tools that need no type
  information; each future one gets its own config switch rather than its own runtime.
- **What it runs.** `format.Source` from gofumpt, then `imports.Process` with `FormatOnly: true`, which sorts
  and groups imports and never adds or removes one, since that needs the `go` command. An unused import stays.
  `golines` is a `package main` module and `golangci-lint` is GPL-3.0; neither is linked.
- **Options** (`[engines.gofmt]`): `gofumpt` (default `true`; `false` is plain `gofmt`), `extra`
  (`gofumpt -extra`, default `false`), `group_imports` (default `true`), `local_prefixes` (goimports `-local`),
  and `lang_version` and `module_path`.
- **`go.mod` is a host hook.** `lang_version` and `module_path` come from the nearest `go.mod`'s `go` and
  `module` directives (`HostHook::GoMod`, `poly_engine_api::host_hooks::go_mod`) unless the options set them.
  The sidecar sees only the bytes, and the format cache is keyed on the two values, so editing `go.mod`
  reformats. gofumpt's rules, such as `0o777`, depend on both.
- **Bundled builds keep the host tool**, as for `sh`: the `engine-gofmt` feature, part of `all-engines`,
  marks the component linked. `[components] disable = ["go"]` selects the host `gofmt` in any build.
- **The engine version** is `gofumpt-<version>+x-tools-<version>`, read from the binary's build info, so a
  bump of either module invalidates the cache.

## Consequences

- Go formatting is the same on every machine and needs no toolchain.
- The default is gofumpt, which is stricter than `gofmt`: a repository that was `gofmt`-clean can see
  changes on first run. `gofumpt = false` restores `gofmt` output; the grouping pass applies either way.
- One more download of about 6 MB for a repository with Go files.

## Alternatives considered

- **Keep the host `gofmt` and the Rust regrouping.** Rejected: output depends on the installed Go, and the
  regrouping only approximates `goimports`.
- **A separate sidecar per Go tool.** Rejected: each would carry its own Go runtime (about 5 MB).
- **Type-aware analyzers in the sidecar.** Rejected: `go/packages` needs the `go` command and module
  downloads. They stay recipe-tier.
