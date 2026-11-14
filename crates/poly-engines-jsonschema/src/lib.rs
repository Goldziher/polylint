//! The JSON Schema validation engine family: lint JSON, JSONC, YAML and TOML against a local schema.

mod doc;
pub mod engine;
mod json;
mod toml_doc;
mod validator;
mod yaml;

pub use engine::JsonSchemaEngine;

use poly_engine_api::engine::Engine;
use poly_engine_api::language::Language;

/// The engines this family registers for `language`: the schema validator on the languages it reads.
pub fn engines_for(language: &Language) -> Vec<Box<dyn Engine>> {
    if JsonSchemaEngine.languages().contains(language) {
        vec![Box::new(JsonSchemaEngine)]
    } else {
        Vec::new()
    }
}
