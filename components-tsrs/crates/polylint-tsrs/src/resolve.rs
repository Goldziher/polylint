//! Module resolution without a program: what a specifier in a file resolves to under a config's compiler options,
//! answered by tsrs's own resolver over the host's file system.

use tsrs_core::{CompilerOptions, P, ResolutionMode};
use tsrs_module::{DefaultResolver, ResolutionHost, ResolverOptions};
use tsrs_vfs::FS;

use crate::shared_configs::ConfigHost;

impl ResolutionHost for ConfigHost {
    fn fs(&self) -> &dyn FS {
        tsrs_tsoptions::ParseConfigHost::fs(self)
    }

    fn get_current_directory(&self) -> &str {
        tsrs_tsoptions::ParseConfigHost::get_current_directory(self)
    }
}

/// What a module specifier resolved to.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct ResolvedModule {
    /// The file, as tsrs names files.
    pub file_name: String,
    /// The extension the file was resolved with (`.ts`, `.d.ts`, `.js`, ...).
    pub extension: String,
    /// Whether the file belongs to a package below `node_modules`.
    pub external_library: bool,
}

/// A resolver of module specifiers under one set of compiler options, as a program of those options resolves them,
/// but with no program built: a host uses it to learn whether the modules a file imports still resolve to the files
/// they did. It keeps tsrs's resolution caches, so one resolver answers many specifiers.
///
/// tsrs allocates the resolver's data and its answers in the region current on the calling thread; a host that
/// resolves repeatedly enters a region of its own around the resolver's life and drops it afterwards.
pub struct ModuleResolver<'h> {
    resolver: DefaultResolver<'h>,
}

impl<'h> ModuleResolver<'h> {
    /// A resolver reading through `host` under `options`, those of a parsed config
    /// (`ParsedCommandLine::compiler_options`).
    #[must_use]
    pub fn new(host: &'h ConfigHost, options: P<CompilerOptions>) -> Self {
        Self {
            resolver: tsrs_module::new_resolver(ResolverOptions::new(host, options)),
        }
    }

    /// What `specifier` resolves to from `containing_file` (as tsrs names files) in `mode`, the module format of the
    /// import (`tsrs_core::RESOLUTION_MODE_NONE` to leave it to the options); `None` when it does not resolve.
    #[must_use]
    pub fn resolve(&self, specifier: &str, containing_file: &str, mode: ResolutionMode) -> Option<ResolvedModule> {
        let (resolved, _trace) = self
            .resolver
            .resolve_module_name(specifier, containing_file, mode, None)
            .ok()?;
        resolved.is_resolved().then(|| ResolvedModule {
            file_name: resolved.resolved_file_name.to_owned(),
            extension: resolved.extension.to_owned(),
            external_library: resolved.is_external_library_import,
        })
    }
}
