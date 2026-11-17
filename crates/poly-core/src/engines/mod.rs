//! Backend implementations. Each backend is a self-contained module implementing
//! [`crate::engine::Engine`]. New backends are added here and wired into the
//! crate-private registry that routes each language to its backend(s). The python, php, sql, nix, typos,
//! astgrep, uncomment and misc (dockerfile, dotenv, hcl, ini, graphql, typst, latex) families live in their own
//! `poly-engines-*` crates and are re-exported here at their old paths. The web (oxc, taplo, markup_fmt,
//! malva, rumdl, adrlint, yaml) and ruby families depend on git-pinned crates, so they live in the
//! separate `components/` workspace and reach the registry through [`crate::register_engine_provider`].

#[cfg(feature = "engine-astgrep")]
pub use poly_engines_astgrep as astgrep;
pub mod catalog_tool;
pub mod config_keys;
pub mod css_hacks;
#[cfg(feature = "engine-dockerfile")]
pub use poly_engines_misc::dockerfile;
#[cfg(feature = "engine-dotenv")]
pub use poly_engines_misc::dotenv;
pub mod format_scope;
pub(crate) use poly_grammar as grammar;

/// Whether the tree-sitter grammar `name` can be loaded right now. Offline this
/// never downloads: a grammar that is not cached reports `false`.
#[must_use]
pub fn grammar_available(name: &str) -> bool {
    grammar::language(name).is_some()
}
#[cfg(feature = "engine-graphql")]
pub use poly_engines_misc::graphql;
#[cfg(feature = "engine-hcl")]
pub use poly_engines_misc::hcl;
#[cfg(feature = "engine-ini")]
pub use poly_engines_misc::ini;
#[cfg(feature = "engine-latex")]
pub use poly_engines_misc::latex;
#[cfg(feature = "engine-typst")]
pub use poly_engines_misc::typst;
pub mod line_index;
#[cfg(feature = "engine-lua")]
pub use poly_engines_lua::{selene, stylua};
#[cfg(feature = "engine-php")]
pub use poly_engines_php::mago;
pub mod native_tool;
#[cfg(feature = "engine-nix")]
pub use poly_engines_nix::nixfmt;
#[cfg(feature = "engine-quality")]
pub mod quality;
#[cfg(feature = "engine-python")]
pub use poly_engines_python::ruff;
#[cfg(feature = "engine-python")]
pub use poly_engines_python::ty;
pub mod rule_config;
#[cfg(feature = "engine-sql")]
pub use poly_engines_sql::sqruff;
pub mod template;
pub mod treesitter;
#[cfg(feature = "engine-jsonschema")]
pub use poly_engines_jsonschema::engine as jsonschema;
#[cfg(feature = "engine-prose")]
pub use poly_engines_prose::harper;
#[cfg(feature = "engine-typos")]
pub use poly_engines_typos::typos;
#[cfg(feature = "engine-uncomment")]
pub use poly_engines_uncomment::uncomment;

/// Forget every memo of a project file's contents before a run reads any config: the host hooks'
/// (oxlint, oxfmt, editorconfig, `go.mod`, …), the tree-sitter tier's `.clang-format` lookups and
/// rustfmt's `Cargo.toml` editions.
///
/// Within a run they are the point: a tree of files costs one lookup per directory. A long-lived
/// process (`poly mcp`) starts many runs, and each must see the configs as they are now. An
/// engine component holds its own copy of the host hooks' memos; starting the run here is what
/// makes its host announce the run to it before the next request.
pub(crate) fn forget_run_memos() {
    poly_engine_api::host_hooks::begin_run();
    treesitter::clang_format::forget();
    native_tool::edition::forget();
}
