//! Builds the program of one `tsconfig.json` over the request's files and collects the compiler's diagnostics,
//! in the order `tsc --noEmit` reports them: the config's, the syntax errors', then the program's, global and
//! semantic ones, stopping at the first group that has any. With targets, the diagnostics of the targets alone.
//!
//! The program is built from the request alone: tsrs reads through an in-memory file system, so nothing comes from
//! the disk and nothing is written. Everything that leaves this module is owned data; the program is freed first.
//!
//! The process environment still applies to tsrs, which reads its own `TSRS_*` variables and `RAYON_NUM_THREADS`
//! (ADR 0051). This module reads none of them and passes the library path and the checker count explicitly.

use std::collections::HashMap;
use std::panic::{self, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{anyhow, bail, ensure};
use poly_engine_api::engine::ProjectFile;
use poly_engine_api::line_index::LineIndex;
use tsrs_ast::{Diagnostic, SourceFile};
use tsrs_compiler::{
    Context, Program, ProgramOptions, ProgramRegions, get_diagnostics_of_any_program, sort_and_deduplicate_diagnostics,
};
use tsrs_core::arena::Region;
use tsrs_core::{CompilerOptions, P, Tristate, tspath};
use tsrs_diagnostics::Category;
use tsrs_tsoptions::{ParseConfigHost, ParsedCommandLine};
use tsrs_vfs::FS;

use crate::external::{Declared, package_name};
use crate::owner::ProgramOwner;
use crate::settings::Settings;
use crate::vfs::{self, MemoryFs, tsrs_path};

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
    let mut memory = MemoryFs::default();
    for file in request.files {
        memory.insert(tsrs_path(&file.path), Arc::clone(&file.content));
    }
    let Some(config) = locate(request, &memory)? else {
        return Ok(None);
    };
    let cwd = tsrs_path(&request.root.join(&request.settings.dir));
    let host_config = host_path_of(request.files, &config);
    let targets: Option<Vec<String>> = request
        .targets
        .map(|targets| targets.iter().map(|path| tsrs_path(path)).collect());
    let region = Region::new_scratch(SCRATCH_CHUNK);
    let entered = region.enter();
    let result = panic::catch_unwind(AssertUnwindSafe(|| {
        check(
            &Subject {
                config: &config,
                host_config: &host_config,
                targets: targets.as_deref(),
            },
            memory.into_tsrs(),
            request.settings,
            request.declared,
            cwd,
        )
    }));
    drop(entered);
    drop(region);
    match result {
        Ok(outcome) => outcome.map(Some),
        Err(payload) => Err(anyhow!("tsrs panicked: {}", panic_text(payload.as_ref()))),
    }
}

/// The host's path of the file tsrs names `name`, which is how [`Declared`] scopes manifests: tsrs normalises
/// separators and drive prefixes, so its names do not share a prefix with the host's manifest directories on Windows.
fn host_path_of(files: &[ProjectFile], name: &str) -> PathBuf {
    files
        .iter()
        .find(|file| tsrs_path(&file.path) == name)
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

/// The host configs are parsed with: the check's file system and the project directory as the current directory.
struct ConfigHost {
    fs: Arc<dyn FS>,
    cwd: String,
}

impl ParseConfigHost for ConfigHost {
    fn fs(&self) -> &dyn FS {
        &*self.fs
    }

    fn get_current_directory(&self) -> &str {
        &self.cwd
    }
}

fn tristate(value: Option<bool>) -> Option<Tristate> {
    value.map(|on| if on { Tristate::True } else { Tristate::False })
}

/// A parsed `tsconfig`, or the diagnostics that say why there is none.
enum Loaded {
    Ready(Box<ParsedCommandLine>),
    Failed(Vec<Raw>),
}

fn load_config(config_path: &str, host: &ConfigHost, lines: &mut Lines) -> Loaded {
    let (parsed, diagnostics) =
        tsrs_tsoptions::get_parsed_command_line_of_config_file(config_path, None, None, host, None);
    let mut raw: Vec<Raw> = diagnostics
        .iter()
        .filter_map(|diagnostic| lines.raw(diagnostic))
        .collect();
    let Some(config) = parsed else {
        if raw.is_empty() {
            raw.push(Raw::global(
                CANNOT_READ_FILE,
                format!("Cannot read file '{config_path}'."),
            ));
        }
        return Loaded::Failed(raw);
    };
    if let Some(file) = config.config_file {
        raw.extend(
            file.source_file
                .diagnostics()
                .iter()
                .filter_map(|diagnostic| lines.raw(diagnostic)),
        );
    }
    if raw.is_empty() {
        Loaded::Ready(Box::new(config))
    } else {
        Loaded::Failed(raw)
    }
}

/// Sets the options the check owns: `tsc --noEmit`, the checker count and the overrides of `settings`.
fn apply_settings(config: &mut ParsedCommandLine, settings: &Settings, config_path: &str) -> anyhow::Result<()> {
    let Some(base) = config.compiler_options() else {
        bail!("`{config_path}` has no compiler options");
    };
    let threads = settings.threads.get();
    config.parsed_config.compiler_options = Some(P::new(CompilerOptions {
        checkers: Some(i64::try_from(threads).unwrap_or(i64::MAX)),
        single_threaded: if threads == 1 {
            Tristate::True
        } else {
            base.single_threaded
        },
        no_emit: Tristate::True,
        strict: tristate(settings.strict).unwrap_or(base.strict),
        skip_lib_check: tristate(settings.skip_lib_check).unwrap_or(base.skip_lib_check),
        ..(*base).clone()
    }));
    Ok(())
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

/// What a check is about: the `tsconfig` (as tsrs names it), its host path, and the files whose diagnostics are
/// wanted (tsrs names; `None` for every file).
struct Subject<'a> {
    config: &'a str,
    host_config: &'a Path,
    targets: Option<&'a [String]>,
}

fn check(
    subject: &Subject<'_>,
    fs: Arc<dyn FS>,
    settings: &Settings,
    declared: &Declared,
    cwd: String,
) -> anyhow::Result<Outcome> {
    let &Subject {
        config: config_path,
        host_config,
        targets,
    } = subject;
    let host = ConfigHost {
        fs: Arc::clone(&fs),
        cwd,
    };
    let mut lines = Lines::default();
    let mut config = match load_config(config_path, &host, &mut lines) {
        Loaded::Ready(config) => *config,
        Loaded::Failed(diagnostics) => {
            return Ok(Outcome {
                config: config_path.to_owned(),
                allow_js: false,
                jsx: false,
                diagnostics,
            });
        }
    };
    let warnings = forgive_declared_extends(&mut config, config_path, host_config, declared);
    apply_settings(&mut config, settings, config_path)?;

    let threads = settings.threads.get();
    let compiler_host = tsrs_compiler::new_compiler_host(
        &tspath::get_directory_path(config_path),
        Arc::clone(&fs),
        &vfs::lib_path(),
        None,
        None,
        None,
    );
    let mut options = ProgramOptions::new(P::new(config), compiler_host);
    // Referenced projects contribute their sources: their declaration outputs are not among the files.
    options.use_source_of_project_reference = true;
    // Dropped after the program: nothing that outlives the program points into its loader regions.
    let loader_regions: Arc<ProgramRegions> = Arc::default();
    options.regions = Some(Arc::clone(&loader_regions));
    options.checker_regions = true;
    if threads == 1 {
        options.single_threaded = Tristate::True;
    }

    let owner = ProgramOwner::new(options);
    let program = owner.program();
    let ctx = Context::default();
    let narrowed = narrowed(&ctx, program, config_path, targets);
    let found = get_diagnostics_of_any_program(
        &ctx,
        program,
        narrowed.as_deref(),
        false,
        &mut |ctx, file| program.get_bind_diagnostics(ctx, file),
        &mut |ctx, file| program.get_semantic_diagnostics(ctx, file),
    );
    let mut raw: Vec<Raw> = sort_and_deduplicate_diagnostics(&found)
        .iter()
        .filter_map(|diagnostic| lines.raw(diagnostic))
        .collect();
    let allow_js = program.options().allow_js.is_true();
    let jsx = program.options().jsx.0 != 0;
    drop(lines);
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
fn narrowed(
    ctx: &Context,
    program: &'static Program,
    config_path: &str,
    targets: Option<&[String]>,
) -> Option<Vec<P<SourceFile>>> {
    let targets = targets?;
    if targets.iter().any(|target| target == config_path) || !program.get_syntactic_diagnostics(ctx, None).is_empty() {
        return None;
    }
    Some(
        targets
            .iter()
            .filter_map(|target| program.get_source_file(target))
            .collect(),
    )
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
    fn the_host_path_of_a_tsrs_name_is_the_path_the_host_handed_over() {
        let files = [file("/w/a/tsconfig.json"), file("/w/b/tsconfig.json")];
        let name = tsrs_path(Path::new("/w/b/tsconfig.json"));
        assert_eq!(host_path_of(&files, &name), PathBuf::from("/w/b/tsconfig.json"));
        assert_eq!(
            host_path_of(&files, "/x/tsconfig.json"),
            PathBuf::from("/x/tsconfig.json")
        );
    }
}
