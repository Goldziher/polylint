//! The `tsrs` engine: the TypeScript type checker of tsrs as a project check.

use std::path::PathBuf;

use anyhow::Context;
use poly_engine_api::engine::{
    Capabilities, Diagnostic, Engine, OptionKeys, OptionTable, ProjectAnalysis, ProjectDirs, ProjectInputs,
    ProjectRequest,
};
use poly_engine_api::language::Language;

use crate::external::Declared;
use crate::map::findings;
use crate::program::{self, Request};
use crate::settings::{DEFAULT_DIR, OPTION_KEYS, Settings};

/// The engine id, and so the `[engines.tsrs]` table that configures it.
pub const ENGINE_NAME: &str = "tsrs";

/// Cache-key identity: the wrapped crate version plus a marker for this backend's own mapping.
///
/// Bump the suffix whenever the mapping or the filtering of findings changes, and the version whenever the tsrs
/// crates are upgraded.
const VERSION: &str = "tsrs-7.1.0-beta.tsrs2+proj3";

/// The stack of the thread tsrs checks on: checking recurses deeply, and tsrs's own binaries give their program
/// threads the same.
const STACK_SIZE: usize = 512 << 20;

/// The files a TypeScript program reads: sources, and the JSON of `tsconfig.json`, `package.json` and
/// `resolveJsonModule`.
static LANGUAGES: &[Language] = &[
    Language::TypeScript,
    Language::Tsx,
    Language::JavaScript,
    Language::Jsx,
    Language::Json,
    Language::Jsonc,
];

/// Files the host hands over by name, whatever their language: the configs and manifests a check resolves
/// through.
const INPUT_FILE_NAMES: &[&str] = &["tsconfig.json", "jsconfig.json", "package.json"];

/// TypeScript type checking with tsrs. See the crate docs.
#[derive(Debug, Default, Clone, Copy)]
pub struct TsrsEngine;

impl Engine for TsrsEngine {
    /// No per-file hook is overridden, so the host answers all of them from the index and a warm run never loads
    /// the component for a file just to ask an engine that has nothing to say about it.
    fn overridden_hooks(&self) -> u32 {
        0
    }

    fn name(&self) -> &'static str {
        ENGINE_NAME
    }

    fn languages(&self) -> &'static [Language] {
        LANGUAGES
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            lint: false,
            format: false,
            fix: false,
            project: true,
        }
    }

    fn option_keys(&self, table: OptionTable) -> OptionKeys {
        match table {
            OptionTable::Lint => OptionKeys::declared(OPTION_KEYS),
            OptionTable::Format => OptionKeys::declared(&[]),
            OptionTable::CrossCuttingLint => OptionKeys::UNCHECKED,
        }
    }

    fn version(&self) -> &str {
        VERSION
    }

    fn project_analysis(&self) -> Option<&dyn ProjectAnalysis> {
        Some(self)
    }
}

impl ProjectAnalysis for TsrsEngine {
    /// Planned only once `enabled` is set, over the whole tree under `dir`: a program's verdict is about every file
    /// it can reach, so neither `[discovery] exclude` nor nested `poly.toml` scopes cut it.
    fn project_inputs(&self) -> ProjectInputs {
        ProjectInputs {
            gate_key: Some("enabled".to_owned()),
            files: Vec::new(),
            dirs: Some(ProjectDirs {
                dir_key: "dir".to_owned(),
                dir_default: DEFAULT_DIR.to_owned(),
                paths_key: None,
                recursive_key: None,
                recursive_default: true,
                whole_tree: true,
                file_names: INPUT_FILE_NAMES.iter().map(|name| (*name).to_owned()).collect(),
                list_unserved: false,
            }),
        }
    }

    fn project_check(&self, req: &ProjectRequest<'_>) -> anyhow::Result<Vec<(PathBuf, Vec<Diagnostic>)>> {
        if !Settings::is_enabled(&req.config.options) {
            return Ok(Vec::new());
        }
        let settings = Settings::from_options(&req.config.options, req.threads)?;
        let declared = Declared::from_files(req.files);
        let request = Request {
            root: req.root,
            files: req.files,
            settings: &settings,
            declared: &declared,
        };
        let outcome = std::thread::scope(|scope| {
            std::thread::Builder::new()
                .name("poly-tsrs".to_owned())
                .stack_size(STACK_SIZE)
                .spawn_scoped(scope, || program::diagnose(&request))
                .context("cannot start the tsrs worker thread")
                .map(std::thread::ScopedJoinHandle::join)
        })?;
        let Some(outcome) = outcome.unwrap_or_else(|payload| std::panic::resume_unwind(payload))? else {
            tracing::debug!("tsrs: no tsconfig.json among the project files, nothing to check");
            return Ok(Vec::new());
        };
        Ok(findings(
            outcome,
            req.files,
            &declared,
            settings.report_unresolved_modules,
        ))
    }
}
