//! Builds the program of one `tsconfig.json` over the request's files and collects the compiler's diagnostics,
//! in the order `tsc --noEmit` reports them: the config's, the syntax errors', then the program's, global and
//! semantic ones, stopping at the first group that has any. With targets, the diagnostics of the targets alone.
//!
//! The program is built from the request alone: tsrs reads through an in-memory file system, so nothing comes from
//! the disk and nothing is written. Everything that leaves this module is owned data; the program is freed first.
//!
//! The process environment still applies to tsrs, which reads its own `TSRS_*` variables and `RAYON_NUM_THREADS`
//! (ADR 0051). This module reads none of them and passes the checker count explicitly.
//!
//! The file system, the config, the program and its owner are `polylint-tsrs`'s (ADR 0078). What stays here is what
//! a check adds: which `tsconfig` it is, the `extends` it forgives, the files it narrows to, and the copy of every
//! diagnostic into owned data.

use std::collections::HashMap;
use std::panic::{self, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{anyhow, ensure};
use poly_engine_api::engine::ProjectFile;
use poly_engine_api::line_index::LineIndex;
use polylint_tsrs::{BuildRequest, BuiltProgram, MemoryFs, ProgramFiles, ProgramHost, SharedConfigs, SharedFiles};
use tsrs_ast::Diagnostic;
use tsrs_compiler::{Context, Program};
use tsrs_core::arena::Region;
use tsrs_core::{CompilerOptions, P, Tristate, tspath};
use tsrs_diagnostics::Category;
use tsrs_tsoptions::ParsedCommandLine;

use crate::external::{Declared, TYPE_FILE_CODE, package_name};
use crate::settings::Settings;
use crate::vfs::{tsrs_name, tsrs_path};

/// The first chunk of the scratch region configs and program bookkeeping are allocated in.
const SCRATCH_CHUNK: usize = 1 << 20;

/// `Cannot read file '{0}'.`
const CANNOT_READ_FILE: i32 = 5083;

/// `File '{0}' not found.`: tsrs reports an `extends` it cannot resolve with it, the argument being the specifier.
const FILE_NOT_FOUND: i32 = 6053;

/// What a check needs.
pub(crate) struct Request<'a> {
    pub root: &'a Path,
    pub files: &'a [ProjectFile],
    /// The files whose findings are wanted; `None` for every file.
    pub targets: Option<&'a [PathBuf]>,
    pub settings: &'a Settings,
    pub declared: &'a Declared,
}

/// A diagnostic, copied out of tsrs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Raw {
    /// The file as tsrs names it; `None` for a diagnostic about the program as a whole.
    pub file: Option<String>,
    /// 1-based `(start line, start column, end line, end column)`.
    pub span: Option<(u32, u32, u32, u32)>,
    pub code: i32,
    /// An error, or else a warning.
    pub error: bool,
    pub message: String,
    /// The message chain below the head, one line each, indented by depth.
    pub chain: Vec<String>,
    /// The first argument the message was formatted with: the module of `Cannot find module '{0}'`.
    pub argument: Option<String>,
}

pub(crate) struct Outcome {
    /// The `tsconfig` that was checked, as tsrs names it.
    pub config: String,
    /// Whether the config's program holds JavaScript files.
    pub allow_js: bool,
    /// Whether the config sets the `jsx` option.
    pub jsx: bool,
    pub diagnostics: Vec<Raw>,
}

/// Checks the project, or returns `None` when no `tsconfig.json` is among the files.
///
/// # Errors
/// A `project` that is not among the files, or a panic inside tsrs.
pub(crate) fn diagnose(request: &Request<'_>) -> anyhow::Result<Option<Outcome>> {
    let mut memory = MemoryFs::new();
    for file in request.files {
        memory.insert(&tsrs_name(request.root, &file.path), Arc::clone(&file.content));
    }
    let Some(config) = locate(request, &memory)? else {
        return Ok(None);
    };
    let cwd = tsrs_path(&request.root.join(&request.settings.dir));
    let host_config = host_path_of(request.root, request.files, &config);
    let targets: Option<Vec<String>> = request
        .targets
        .map(|targets| targets.iter().map(|path| tsrs_name(request.root, path)).collect());
    // The files and configs of the check, which its one program reads through: dropped last, after everything that
    // points into them.
    let (files, configs) = (Arc::<SharedFiles>::default(), Arc::<SharedConfigs>::default());
    let (fs, boms) = memory.into_fs();
    let host = ProgramHost::new(fs, boms, cwd, Arc::clone(&files), Arc::clone(&configs));
    let program_files = Arc::new(ProgramFiles::default());
    // tsrs allocates the config and the compiler host in the region current on this thread.
    let region = Region::new_scratch(SCRATCH_CHUNK);
    let entered = region.enter();
    let result = panic::catch_unwind(AssertUnwindSafe(|| {
        check(
            &Subject {
                config: &config,
                host_config: &host_config,
                targets: targets.as_deref(),
            },
            &host,
            &program_files,
            request.settings,
            request.declared,
        )
    }));
    // The program and its loader regions went with `check`; then the scratch region, the program's hold on its
    // files, and the check's files and configs.
    drop(entered);
    drop(region);
    drop(program_files);
    drop(host);
    drop((files, configs));
    match result {
        Ok(outcome) => outcome.map(Some),
        Err(payload) => Err(anyhow!("tsrs panicked: {}", panic_text(payload.as_ref()))),
    }
}

/// The host's path of the file tsrs names `name`, which is how [`Declared`] scopes manifests: tsrs normalises
/// separators and drive prefixes, so its names do not share a prefix with the host's manifest directories on Windows.
fn host_path_of(root: &Path, files: &[ProjectFile], name: &str) -> PathBuf {
    files
        .iter()
        .find(|file| tsrs_name(root, &file.path) == name)
        .map_or_else(|| PathBuf::from(name), |file| file.path.clone())
}

/// The `tsconfig` to check: `project`, else the nearest `tsconfig.json` from `dir` up to the project root.
fn locate(request: &Request<'_>, memory: &MemoryFs) -> anyhow::Result<Option<String>> {
    if let Some(project) = &request.settings.project {
        let path = tsrs_path(&request.root.join(project));
        ensure!(
            memory.contains_file(&path),
            "tsconfig `{}` is not among the project files",
            project.display()
        );
        return Ok(Some(path));
    }
    let root = tsrs_path(request.root);
    let mut directory = tsrs_path(&request.root.join(&request.settings.dir));
    loop {
        let candidate = tspath::combine_paths(&directory, &["tsconfig.json"]);
        if memory.contains_file(&candidate) {
            return Ok(Some(candidate));
        }
        let parent = tspath::get_directory_path(&directory);
        if directory == root || parent == directory {
            return Ok(None);
        }
        directory = parent;
    }
}

fn tristate(value: Option<bool>) -> Option<Tristate> {
    value.map(|on| if on { Tristate::True } else { Tristate::False })
}

/// The diagnostics that say why a `tsconfig` gave no program; `Cannot read file` when tsrs does not say.
fn config_failure(config_path: &str, diagnostics: &[P<Diagnostic>], lines: &mut Lines) -> Vec<Raw> {
    let mut raw: Vec<Raw> = diagnostics
        .iter()
        .filter_map(|diagnostic| lines.raw(diagnostic))
        .collect();
    if raw.is_empty() {
        raw.push(Raw::global(
            CANNOT_READ_FILE,
            format!("Cannot read file '{config_path}'."),
        ));
    }
    raw
}

/// Sets the options the check owns beside the checker count, which the build sets: `tsc --noEmit` and the overrides
/// of `settings`.
fn apply_settings(config: &mut ParsedCommandLine, settings: &Settings) {
    let Some(base) = config.compiler_options() else {
        return;
    };
    config.parsed_config.compiler_options = Some(P::new(CompilerOptions {
        no_emit: Tristate::True,
        strict: tristate(settings.strict).unwrap_or(base.strict),
        skip_lib_check: tristate(settings.skip_lib_check).unwrap_or(base.skip_lib_check),
        ..(*base).clone()
    }));
}

/// Drops the `extends` of a package the project declares but the hermetic run does not have from the config's
/// errors, and returns one warning per such `extends`. An `extends` of a package nothing declares keeps its error.
///
/// The config is still read without the extended options, so what it inherits is not applied.
fn forgive_declared_extends(
    config: &mut ParsedCommandLine,
    config_path: &str,
    host_config: &Path,
    declared: &Declared,
) -> Vec<Raw> {
    let mut warnings = Vec::new();
    config.errors.retain(|diagnostic| {
        if diagnostic.code() != FILE_NOT_FOUND {
            return true;
        }
        let Some(specifier) = diagnostic.message_args().first() else {
            return true;
        };
        let Some(package) = package_name(specifier) else {
            return true;
        };
        if !declared.declares(host_config, package) {
            return true;
        }
        warnings.push(Raw {
            file: Some(config_path.to_owned()),
            span: Some((1, 1, 1, 1)),
            code: FILE_NOT_FOUND,
            error: false,
            message: format!(
                "extended config {specifier} is not available without node_modules; compiler options from it were \
                 not applied"
            ),
            chain: Vec::new(),
            argument: None,
        });
        false
    });
    warnings
}

/// Drops the `types` entries of packages the project declares but the hermetic run does not have.
///
/// Each would be a `Cannot find type definition file` program diagnostic, which the findings drop as a missing
/// install; but tsc stops at the first group with any diagnostic, so left in, it would end the check before the
/// semantic pass and hide every type error.
fn drop_declared_types(config: &mut ParsedCommandLine, host_config: &Path, declared: &Declared) {
    let Some(base) = config.compiler_options() else {
        return;
    };
    let Some(types) = &base.types else {
        return;
    };
    let kept: Vec<String> = types
        .iter()
        .filter(|entry| !declared.is_missing_install(host_config, TYPE_FILE_CODE, Some(entry), false))
        .cloned()
        .collect();
    if kept.len() == types.len() {
        return;
    }
    config.parsed_config.compiler_options = Some(P::new(CompilerOptions {
        types: Some(kept),
        ..(*base).clone()
    }));
}

/// What a check is about: the `tsconfig` (as tsrs names it), its host path, and the files whose diagnostics are
/// wanted (tsrs names; `None` for every file).
struct Subject<'a> {
    config: &'a str,
    host_config: &'a Path,
    targets: Option<&'a [String]>,
}

fn check(
    subject: &Subject<'_>,
    host: &ProgramHost,
    files: &Arc<ProgramFiles>,
    settings: &Settings,
    declared: &Declared,
) -> anyhow::Result<Outcome> {
    let &Subject {
        config: config_path,
        host_config,
        targets,
    } = subject;
    let threads = settings.threads.get();
    let mut lines = Lines::default();
    let mut warnings = Vec::new();
    let request = BuildRequest {
        config_path,
        checkers: threads,
        single_threaded: threads == 1,
        list_files: false,
        // The findings are copied out before the program is dropped, so the diagnostics go back with it.
        diagnostics_in_checker_regions: true,
    };
    let built = host.build(&request, Arc::clone(files), None, |config| {
        warnings = forgive_declared_extends(config, config_path, host_config, declared);
        if !settings.report_unresolved_modules {
            drop_declared_types(config, host_config, declared);
        }
        apply_settings(config, settings);
    });
    let BuiltProgram {
        owner, loader_regions, ..
    } = match built {
        Ok(built) => built,
        Err(diagnostics) => {
            return Ok(Outcome {
                config: config_path.to_owned(),
                allow_js: false,
                jsx: false,
                diagnostics: config_failure(config_path, &diagnostics, &mut lines),
            });
        }
    };

    let program = owner.program();
    let narrowed = narrowed(program, config_path, targets, threads);
    let mut raw: Vec<Raw> = owner
        .diagnostics(narrowed.as_deref())
        .iter()
        .filter_map(|diagnostic| lines.raw(diagnostic))
        .collect();
    let allow_js = program.options().allow_js.is_true();
    let jsx = program.options().jsx.0 != 0;
    drop(lines);
    // Dropped after the program: nothing that outlives the program points into its loader regions.
    drop(owner);
    drop(loader_regions);

    raw.extend(warnings);
    raw.sort_by(|a, b| (&a.file, a.span, a.code, &a.message).cmp(&(&b.file, b.span, b.code, &b.message)));
    Ok(Outcome {
        config: config_path.to_owned(),
        allow_js,
        jsx,
        diagnostics: raw,
    })
}

/// The program's source files among `targets` (tsrs names), the files whose diagnostics are computed; `None` for
/// every file.
///
/// Every file it is when no targets are given, when the config is a target (it carries the program's file-less
/// diagnostics), and when any file of the program has a syntax error: `tsc` then reports the syntax errors alone, and
/// a check of the targets would go on to their semantic errors. A target the program does not hold (a JSON file it
/// does not import, an excluded file) is not checked. The program itself is the same either way, so a target's
/// imports resolve, and its findings are those of the full check.
///
/// Every file it is, too, when the targets are too many for narrowing to pay ([`worth_narrowing`]).
fn narrowed<'a>(
    program: &Program,
    config_path: &str,
    targets: Option<&'a [String]>,
    threads: usize,
) -> Option<Vec<&'a str>> {
    let targets = targets?;
    if targets.iter().any(|target| target == config_path)
        || !program.get_syntactic_diagnostics(&Context::default(), None).is_empty()
    {
        return None;
    }
    let files: Vec<&str> = targets
        .iter()
        .map(String::as_str)
        .filter(|target| program.get_source_file(target).is_some())
        .collect();
    let libraries = polylint_tsrs::lib_path();
    let sources = program
        .get_source_files()
        .iter()
        .filter(|file| !file.file_name().starts_with(&libraries))
        .count();
    worth_narrowing(files.len(), sources, threads).then_some(files)
}

/// Whether checking `targets` files alone is less work in wall time than checking all `sources` of the program.
///
/// tsrs computes the diagnostics of named files one file after another on one checker, and those of every file on
/// `threads` checkers at once. With the cost of a file taken as even, the named files take `targets` units and the
/// full pass `sources / threads`, so narrowing pays while `targets * threads < sources`: a hook over a few staged
/// files narrows, `poly lint src` over most of the program does not, and the host keeps the findings it named.
fn worth_narrowing(targets: usize, sources: usize, threads: usize) -> bool {
    targets.saturating_mul(threads) < sources
}

impl Raw {
    fn global(code: i32, message: String) -> Self {
        Self {
            file: None,
            span: None,
            code,
            error: true,
            message,
            chain: Vec::new(),
            argument: None,
        }
    }
}

/// Line indexes of the files diagnostics point into, built once per file.
#[derive(Default)]
struct Lines {
    by_file: HashMap<String, LineIndex<'static>>,
}

impl Lines {
    /// The diagnostic as owned data, or `None` for one that is not an error or a warning: tsrs only reports
    /// suggestions and messages through the suggestion API, which is not called.
    fn raw(&mut self, diagnostic: &Diagnostic) -> Option<Raw> {
        let error = match diagnostic.category() {
            Category::Error => true,
            Category::Warning => false,
            _ => return None,
        };
        let (file, span) = match diagnostic.file() {
            Some(file) => {
                let name = file.file_name().to_owned();
                let span = diagnostic.pos().map(|pos| {
                    let end = diagnostic.end().unwrap_or(pos).max(pos);
                    self.span(&name, file.text(), pos, end)
                });
                (Some(name), span)
            }
            None => (None, None),
        };
        let mut chain = Vec::new();
        flatten(diagnostic.message_chain(), 0, &mut chain);
        Some(Raw {
            file,
            span,
            code: diagnostic.code(),
            error,
            message: diagnostic.localize(),
            chain,
            argument: diagnostic.message_args().first().cloned(),
        })
    }

    /// tsrs positions are UTF-8 byte offsets into the text it read, which is the text of the request with a leading
    /// byte order mark dropped.
    fn span(&mut self, name: &str, text: &'static str, pos: u32, end: u32) -> (u32, u32, u32, u32) {
        let index = self
            .by_file
            .entry(name.to_owned())
            .or_insert_with(|| LineIndex::new(text));
        let (start_line, start_col) = index.line_col(pos as usize);
        let (end_line, end_col) = index.line_col(end as usize);
        (start_line, start_col, end_line, end_col)
    }
}

/// The message chain of a diagnostic, one line per link, indented by its depth.
fn flatten(chain: &[P<Diagnostic>], depth: usize, out: &mut Vec<String>) {
    for link in chain {
        out.push(format!("{}{}", "  ".repeat(depth), link.localize()));
        flatten(link.message_chain(), depth + 1, out);
    }
}

fn panic_text(payload: &(dyn std::any::Any + Send)) -> String {
    payload
        .downcast_ref::<&str>()
        .map(|message| (*message).to_owned())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "a panic without a message".to_owned())
}

#[cfg(test)]
mod tests {
    use poly_engine_api::language::Language;

    use super::*;

    fn file(path: &str) -> ProjectFile {
        ProjectFile {
            path: PathBuf::from(path),
            language: Language::Other("tsconfig.json".into()),
            content: "".into(),
        }
    }

    #[test]
    fn narrowing_pays_while_the_targets_times_the_checkers_are_fewer_than_the_sources() {
        for (targets, sources, threads, narrow) in [
            (1, 400, 8, true),
            (49, 400, 8, true),
            (50, 400, 8, false),
            (300, 400, 8, false),
            (399, 400, 1, true),
            (400, 400, 1, false),
            (0, 3, 64, true),
            (1, 3, 4, false),
            (usize::MAX, 3, 2, false),
        ] {
            assert_eq!(
                worth_narrowing(targets, sources, threads),
                narrow,
                "{targets} of {sources} on {threads}"
            );
        }
    }

    #[test]
    fn the_host_path_of_a_tsrs_name_is_the_path_the_host_handed_over() {
        let files = [file("/w/a/tsconfig.json"), file("/w/b/tsconfig.json")];
        let name = tsrs_path(Path::new("/w/b/tsconfig.json"));
        let root = Path::new("/w");
        assert_eq!(host_path_of(root, &files, &name), PathBuf::from("/w/b/tsconfig.json"));
        assert_eq!(
            host_path_of(root, &files, "/x/tsconfig.json"),
            PathBuf::from("/x/tsconfig.json")
        );
        let relative = [file("b/tsconfig.json")];
        assert_eq!(host_path_of(root, &relative, &name), PathBuf::from("b/tsconfig.json"));
    }
}
