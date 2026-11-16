//! The tsrs engine family: TypeScript type checking with [tsrs](https://github.com/oxc-project/tsrs), the Rust
//! port of the TypeScript 7 compiler, as a project check, and the `tsrs` component serving it.
//!
//! The engine is the equivalent of `tsc --noEmit -p <tsconfig>`: it builds one program from the project's
//! `tsconfig.json` and reports the compiler's diagnostics as `TS<code>` findings. It is experimental and opt-in
//! (ADR 0051).
//!
//! ```toml
//! [engines.tsrs]
//! enabled = true
//! project = "tsconfig.json"          # default: the nearest tsconfig.json from `dir` up to the project root
//! dir = "."                          # the project directory; its whole tree is handed to the check
//! strict = true                      # overrides the tsconfig's `strict`
//! skip_lib_check = true              # overrides the tsconfig's `skipLibCheck`
//! report_unresolved_modules = false  # report packages the project declares but the check cannot see
//! threads = 4                        # checkers; default: the host's job budget
//! ```
//!
//! # What the check sees
//!
//! The host hands over every JavaScript, TypeScript and JSON file under `dir` (honoring `.gitignore`, never
//! `node_modules`), and tsrs reads them through an in-memory file system: nothing comes from the disk and nothing
//! is written, so the result is a function of the request. File names are case-sensitive on every platform.
//!
//! Because `node_modules` is absent, imports of installed packages cannot resolve. A finding that only says so
//! (`TS2307`, `TS2792`, `TS7016`, and the `Cannot find name ... install type definitions` family) is dropped when
//! the project declares the package in a `package.json`, and reported when it does not. Set
//! `report_unresolved_modules = true` to keep them all.
//!
//! Findings in files under `node_modules` and in JavaScript files (unless the program holds them through
//! `allowJs`) are never reported, though the files are inputs. The project's own declaration files (`.d.ts`)
//! are reported like any source (ADR 0055). Findings the host did
//! not hand over a path for, such as the bundled `lib.*.d.ts`, are dropped.
//!
//! # Process constraints
//!
//! tsrs keeps each program in leaked arenas, which the program owner frees when the check ends; reserves no address
//! space (the `plain-ptrs` feature); and reads its own `TSRS_*` variables and `RAYON_NUM_THREADS` from the
//! environment, which this crate neither sets nor reads (ADR 0051).

mod engine;
mod external;
mod map;
mod owner;
mod program;
mod settings;
mod vfs;

pub use engine::{ENGINE_NAME, TsrsEngine};

use poly_component_protocol::{ComponentCallError, PolyComponent};
use poly_engine_api::engine::Engine;
use poly_engine_api::language::Language;
use poly_engines::Producer;

/// The tsrs family's engines for `language`; empty for a language the check reads nothing of.
///
/// Shaped as an engine provider so a bundled binary could chain it with the other families' functions.
#[must_use]
pub fn engines_for(language: &Language) -> Vec<Box<dyn Engine>> {
    if TsrsEngine.languages().contains(language) {
        vec![Box::new(TsrsEngine)]
    } else {
        Vec::new()
    }
}

/// The `tsrs` component: the tsrs project check behind the component protocol.
pub struct TsrsComponent(Producer);

impl Default for TsrsComponent {
    fn default() -> Self {
        Self(Producer::new(env!("CARGO_PKG_VERSION"), vec![Box::new(TsrsEngine)]))
    }
}

impl PolyComponent for TsrsComponent {
    fn describe(&self) -> Result<Vec<u8>, ComponentCallError> {
        self.0.describe()
    }

    fn call(&self, header: &[u8], body: &[u8]) -> Result<Vec<u8>, ComponentCallError> {
        self.0.call(header, body)
    }
}
