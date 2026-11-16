//! The prose engine family: opt-in grammar and style checks on Markdown and on source-file comments.

mod comments;
pub mod harper;
mod text;

pub use harper::HarperEngine;

use poly_engine_api::engine::Engine;
use poly_engine_api::language::Language;

/// The engines this family registers for `language`: harper is cross-cutting, so every language.
pub fn engines_for(_language: &Language) -> Vec<Box<dyn Engine>> {
    vec![Box::new(HarperEngine)]
}
