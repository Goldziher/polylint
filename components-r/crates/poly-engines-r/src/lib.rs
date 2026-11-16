//! The R engine family: formatting with [air](https://github.com/posit-dev/air), linting with
//! [jarl](https://github.com/etiennebacher/jarl), and the `r` component serving both (ADR 0070).
//!
//! Both tools are Rust libraries built on one parser and one biome formatter, so a run needs no R installation.
//! They are configured the way their own CLIs are, by the project's `air.toml` and `jarl.toml`, with poly's
//! `[engines.air]` and `[engines.jarl]` tables laid over them (see [`AirEngine`] and [`JarlEngine`]).
//!
//! When the `r` component is installed it formats and lints R. The opt-in host tool `styler` (`Rscript`) stays
//! available with `[engines.styler] enabled = true`; its formatter then runs after air, which is rarely wanted, so
//! `[engines.air] enabled = false` is the way to prefer it.

mod air;
mod air_config;
mod jarl;
mod jarl_config;
mod jarl_context;

pub use air::{AIR_NAME, AirEngine};
pub use jarl::{JARL_NAME, JarlEngine};

use poly_component_protocol::{ComponentCallError, PolyComponent};
use poly_engine_api::engine::Engine;
use poly_engine_api::language::Language;
use poly_engines::Producer;

/// The R family's engines for `language`: air then jarl for R, none for any other language.
///
/// Shaped as an engine provider so a bundled binary could chain it with the other families' functions.
#[must_use]
pub fn engines_for(language: &Language) -> Vec<Box<dyn Engine>> {
    match language {
        Language::R => vec![Box::new(AirEngine), Box::new(JarlEngine)],
        _ => Vec::new(),
    }
}

/// The `r` component: air and jarl behind the component protocol.
pub struct RComponent(Producer);

impl Default for RComponent {
    fn default() -> Self {
        Self(Producer::new(
            env!("CARGO_PKG_VERSION"),
            vec![Box::new(AirEngine), Box::new(JarlEngine)],
        ))
    }
}

impl PolyComponent for RComponent {
    fn describe(&self) -> Result<Vec<u8>, ComponentCallError> {
        self.0.describe()
    }

    fn call(&self, header: &[u8], body: &[u8]) -> Result<Vec<u8>, ComponentCallError> {
        self.0.call(header, body)
    }
}
