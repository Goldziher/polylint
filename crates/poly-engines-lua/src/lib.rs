//! The Lua engine family: the StyLua formatter and the selene linter.

pub mod selene;
pub mod stylua;

pub use selene::SeleneEngine;
pub use stylua::StyluaEngine;

use poly_engine_api::engine::Engine;
use poly_engine_api::language::Language;

/// The languages this family serves.
pub static LANGUAGES: &[Language] = &[Language::Lua];

/// The engines this family registers for `language`: the formatter first, then the linter. Empty when the
/// family does not handle it.
pub fn engines_for(language: &Language) -> Vec<Box<dyn Engine>> {
    if LANGUAGES.contains(language) {
        vec![Box::new(StyluaEngine), Box::new(SeleneEngine)]
    } else {
        Vec::new()
    }
}
