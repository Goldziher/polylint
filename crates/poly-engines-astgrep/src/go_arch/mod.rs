//! Configurable Go architecture rules: forbidden calls, forbidden imports and import layering.
//!
//! The built-in pack ships fixed Go rules (`go-no-init`, `go-forbidden-ambient-call`, ...); what a
//! project forbids, where, and which layer may import which is policy, so it is configuration. The
//! checks run over the same parse the ast-grep rules use, in the same `lint` call, and report through
//! the same `[engines.astgrep]` `select` / `extend_select` / `ignore` / `rules.<id>.level` vocabulary.
//! They need no Go toolchain: calls and imports are resolved syntactically from each file's own import
//! block, and module-internal imports are resolved against the `go.mod` that governs the file.
//!
//! | Key | Rule id | Meaning |
//! |-----|---------|---------|
//! | `go_forbidden_calls` | `go-forbidden-call` | `<import path>.<Name>` selectors nobody may use |
//! | `go_forbidden_imports` | `go-forbidden-import` | import paths nobody may import |
//! | `go_layers` | `go-layer-violation` | layers of the module and which may import which |
//!
//! Every path glob (`paths`, `allow_in`, a layer's `paths`) is matched against the file's path
//! relative to its module root (the directory holding the nearest `go.mod`), with `/` separators.

mod check;
mod config;
#[cfg(test)]
mod tests;

pub(crate) use config::GoArch;

/// Rule id of a forbidden `<import path>.<Name>` selector.
pub(crate) const FORBIDDEN_CALL: &str = "go-forbidden-call";
/// Rule id of a forbidden import path.
pub(crate) const FORBIDDEN_IMPORT: &str = "go-forbidden-import";
/// Rule id of an import that crosses a layer boundary the configuration does not allow.
pub(crate) const LAYER_VIOLATION: &str = "go-layer-violation";
