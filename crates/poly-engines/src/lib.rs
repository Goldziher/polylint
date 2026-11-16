//! Facade over poly's engine family crates.
//!
//! Each family is behind a feature of the same name and re-exported under it
//! (`poly_engines::sql`, `poly_engines::python`, ...), together with the ready-made component
//! types and the [`Producer`] that adapts engines to the [`PolyComponent`] contract.

// The contract lives in `poly-component-protocol`. The re-export stays because `alef` extracts the
// contract traits through this crate's `lib.rs` and the producer crates it generates name them by
// this path. It precedes `mod producer`: alef 0.130 reads a sibling crate once per run, so when
// `producer`'s `poly_engine_api` re-export reaches `poly-component-protocol` first, a later
// re-export from it here extracts nothing and `alef generate` cannot find the contract.
pub use poly_component_protocol::{ComponentCallError, PolyComponent, PolyFrontend};

mod components;
pub mod producer;

#[cfg(feature = "astgrep")]
pub use components::AstgrepComponent;
#[cfg(feature = "jsonschema")]
pub use components::JsonSchemaComponent;
#[cfg(feature = "mcp")]
pub use components::McpComponent;
#[cfg(feature = "misc")]
pub use components::MiscComponent;
#[cfg(feature = "nix")]
pub use components::NixComponent;
#[cfg(feature = "php")]
pub use components::PhpComponent;
#[cfg(feature = "python")]
pub use components::PythonComponent;
#[cfg(feature = "sql")]
pub use components::SqlComponent;
#[cfg(feature = "typos")]
pub use components::TyposComponent;
#[cfg(feature = "uncomment")]
pub use components::UncommentComponent;
pub use producer::Producer;

#[cfg(feature = "astgrep")]
pub use poly_engines_astgrep as astgrep;
#[cfg(feature = "jsonschema")]
pub use poly_engines_jsonschema as jsonschema;
#[cfg(feature = "misc")]
pub use poly_engines_misc as misc;
#[cfg(feature = "nix")]
pub use poly_engines_nix as nix;
#[cfg(feature = "php")]
pub use poly_engines_php as php;
#[cfg(feature = "python")]
pub use poly_engines_python as python;
#[cfg(feature = "sql")]
pub use poly_engines_sql as sql;
#[cfg(feature = "typos")]
pub use poly_engines_typos as typos;
#[cfg(feature = "uncomment")]
pub use poly_engines_uncomment as uncomment;
