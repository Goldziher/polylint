//! `poly adr`: ADR log maintenance through the bundled adrlint engine (ADR 0061).
//!
//! The host half of `poly adr lock`, `unlock` and `new`. [`AdrLog`] reads the adrlint settings from
//! `poly.toml` (`[engines.adrlint]`, or the v1 `[lint.markdown.adrlint]` table) through the same option
//! keys and defaults the engine declares for its project check, gathers the files an operation reads,
//! and hands them to the engine as content. The engine answers what to write; [`AdrLog::apply`] writes
//! it, only inside the project root and never through a symlink.
//!
//! A build that links the adrlint engine registers it with [`register_linked_planner`] and is answered
//! in-process; any other build asks the `web` component with `Request::Adr`, installing it on demand
//! under the usual `--offline` and `[components] install` policy. Either way the same function plans the
//! change, so the two builds cannot drift.

use std::fs::{self, OpenOptions};
use std::io::{ErrorKind, Write as _};
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use poly_component_protocol::{Payload, Request, WireAdrOp, WireAdrPlan, WireFileSlice};
use poly_core::components::{ComponentInjection, call_engine_component_with_body, engine_project_inputs};
use poly_core::{Config, Kind, Language};
use poly_engine_api::component_wire::engine_config_to_wire;
use poly_engine_api::config::EngineConfig;
use poly_engine_api::engine::ProjectFile;

/// The engine that answers ADR operations.
pub const ENGINE: &str = "adrlint";

/// Plans an ADR operation in-process: the adrlint engine's own planner, for a build that links it.
pub type LinkedPlanner = fn(&WireAdrOp, &EngineConfig, &[ProjectFile]) -> anyhow::Result<WireAdrPlan>;

static LINKED: OnceLock<LinkedPlanner> = OnceLock::new();

/// Declares that this binary links the adrlint engine and plans ADR operations with `planner`, so no
/// component is consulted. Call it once at startup, next to the engine provider registration. Returns
/// `false` when a planner was already registered (the first wins).
pub fn register_linked_planner(planner: LinkedPlanner) -> bool {
    LINKED.set(planner).is_ok()
}

/// One project's ADR log, as its `poly.toml` describes it.
#[derive(Debug)]
pub struct AdrLog {
    /// The project root, canonical.
    root: PathBuf,
    /// The resolved adrlint config.
    config: EngineConfig,
    /// Whether the engine's gate option (`enabled`) is on.
    enabled: bool,
    /// The ADR directories, relative to the root (empty for the root itself).
    dirs: Vec<PathBuf>,
    /// Whether the directories' subdirectories belong to the log.
    recursive: bool,
    /// The lock manifest, relative to the root.
    lock: PathBuf,
}

impl AdrLog {
    /// Reads the log's settings for the project at `root` from `config`.
    ///
    /// The option keys and their defaults (`dir = "docs/adr"`, `paths`, `recursive`,
    /// `lock = "adrlint.lock"`) are the ones the engine declares in `Engine::project_inputs`, read from
    /// the component index, so `poly adr` resolves exactly the directories `poly lint` judges.
    ///
    /// # Errors
    ///
    /// When the root cannot be resolved, no component describes the adrlint engine, or an option has
    /// the wrong type or names a path outside the root.
    pub fn resolve(config: &Config, root: &Path, injection: Option<&ComponentInjection>) -> Result<Self> {
        let root = fs::canonicalize(root).with_context(|| format!("resolving the project root {}", root.display()))?;
        let engine_config = config.engine_config(&Language::Markdown, ENGINE, Kind::Lint);
        let inputs = engine_project_inputs(injection, ENGINE).context("no engine component describes adrlint")?;
        let declared = inputs
            .dirs
            .context("the adrlint engine declares no ADR directory option")?;
        let side = inputs
            .files
            .first()
            .context("the adrlint engine declares no lock file option")?;
        let options = &engine_config.options;

        let mut named: Vec<String> = Vec::new();
        if let Some(key) = declared.paths_key.as_deref() {
            for entry in option(options, key, "an array of strings", toml::Value::as_array)?
                .into_iter()
                .flatten()
            {
                let entry = entry
                    .as_str()
                    .with_context(|| format!("option `{key}` must hold only strings"))?;
                named.push(entry.to_owned());
            }
        }
        if named.is_empty() {
            let dir = option(options, &declared.dir_key, "a string", toml::Value::as_str)?;
            named.push(dir.unwrap_or(&declared.dir_default).to_owned());
        }
        let mut dirs = named
            .iter()
            .map(|dir| inside_root(dir).with_context(|| format!("directory `{dir}` must stay inside the project root")))
            .collect::<Result<Vec<_>>>()?;
        dirs.sort();
        dirs.dedup();

        let recursive = match declared.recursive_key.as_deref() {
            Some(key) => option(options, key, "a boolean", toml::Value::as_bool)?.unwrap_or(declared.recursive_default),
            None => declared.recursive_default,
        };
        let lock_text =
            option(options, &side.option_key, "a string", toml::Value::as_str)?.unwrap_or(&side.default_path);
        let lock = inside_root(lock_text)
            .filter(|lock| !lock.as_os_str().is_empty())
            .with_context(|| format!("the lock file `{lock_text}` must stay inside the project root"))?;
        let enabled = inputs
            .gate_key
            .as_deref()
            .is_none_or(|key| options.get(key).and_then(toml::Value::as_bool) == Some(true));

        Ok(Self {
            root,
            config: engine_config,
            enabled,
            dirs,
            recursive,
            lock,
        })
    }

    /// The canonical project root every path of the log is relative to.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Whether `[engines.adrlint] enabled = true` (or the v1 table's) switches the engine on.
    #[must_use]
    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// The lock manifest, relative to the root.
    #[must_use]
    pub fn lock_path(&self) -> &Path {
        &self.lock
    }

    /// The configured ADR directories that exist, as absolute paths. A symlinked directory is left out,
    /// as the project check leaves it out.
    #[must_use]
    pub fn directories(&self) -> Vec<PathBuf> {
        self.dirs
            .iter()
            .map(|dir| self.root.join(dir))
            .filter(|dir| fs::symlink_metadata(dir).is_ok_and(|metadata| metadata.is_dir()))
            .collect()
    }

    /// `path` (absolute, or relative to the working directory) relative to the project root.
    ///
    /// # Errors
    ///
    /// When it lies outside the root.
    pub fn relative(&self, path: &Path) -> Result<PathBuf> {
        let absolute = if path.is_absolute() {
            path.to_path_buf()
        } else {
            std::env::current_dir()
                .context("resolving the working directory")?
                .join(path)
        };
        // A record need not exist to be unlocked (it may have been deleted); its directory must.
        let resolved = fs::canonicalize(&absolute).ok().or_else(|| {
            let (parent, name) = absolute.parent().zip(absolute.file_name())?;
            fs::canonicalize(parent).ok().map(|parent| parent.join(name))
        });
        resolved
            .and_then(|resolved| resolved.strip_prefix(&self.root).ok().map(Path::to_path_buf))
            .with_context(|| format!("{} is outside the project root {}", path.display(), self.root.display()))
    }

    /// The directory a new record goes into: `chosen` when given, else the one configured directory.
    ///
    /// # Errors
    ///
    /// When `chosen` is outside the root or not a directory, several directories are configured and
    /// none was chosen, or the configured one does not exist.
    pub fn new_record_dir(&self, chosen: Option<&Path>) -> Result<PathBuf> {
        let dir = match chosen {
            Some(chosen) => self.relative(chosen)?,
            None => match self.dirs.as_slice() {
                [only] => only.clone(),
                several => bail!(
                    "several ADR directories are configured ({}); pick one with --dir",
                    several.iter().map(|dir| wire_path(dir)).collect::<Vec<_>>().join(", ")
                ),
            },
        };
        let absolute = self.root.join(&dir);
        if !fs::symlink_metadata(&absolute).is_ok_and(|metadata| metadata.is_dir()) {
            bail!(
                "the ADR directory {} does not exist; create it first",
                absolute.display()
            );
        }
        Ok(dir)
    }

    /// Plans writing or refreshing the lock manifest. With `all`, every numbered record is locked.
    ///
    /// # Errors
    ///
    /// When a file cannot be read, or the engine refuses (see `poly_engines_web::adrlint::ops`).
    pub fn plan_lock(&self, all: bool, config: &Config, injection: Option<&ComponentInjection>) -> Result<WireAdrPlan> {
        let mut files = Vec::new();
        for dir in &self.dirs {
            self.markdown_under(dir, &mut files)?;
        }
        files.extend(self.manifest()?);
        self.call(WireAdrOp::Lock { all }, files, config, injection)
    }

    /// Plans removing `record` (relative to the root) from the lock manifest.
    ///
    /// # Errors
    ///
    /// When the manifest cannot be read, or has no entry for `record`.
    pub fn plan_unlock(
        &self,
        record: &Path,
        config: &Config,
        injection: Option<&ComponentInjection>,
    ) -> Result<WireAdrPlan> {
        let files = self.manifest()?.into_iter().collect();
        let op = WireAdrOp::Unlock {
            path: wire_path(record),
        };
        self.call(op, files, config, injection)
    }

    /// Plans creating the next numbered record titled `title` in `dir` (relative to the root), dated
    /// `date`.
    ///
    /// # Errors
    ///
    /// When the directory cannot be read, or the engine rejects the title.
    pub fn plan_new(
        &self,
        dir: &Path,
        title: &str,
        date: &str,
        config: &Config,
        injection: Option<&ComponentInjection>,
    ) -> Result<WireAdrPlan> {
        let mut files = Vec::new();
        self.markdown_in(dir, false, &mut files)?;
        let op = WireAdrOp::New {
            dir: wire_path(dir),
            title: title.to_owned(),
            date: date.to_owned(),
        };
        self.call(op, files, config, injection)
    }

    /// Writes `plan`, returning the absolute paths written. A path outside the root, a symlink, or (for
    /// a new record) an existing file is refused before anything is written for that file.
    ///
    /// # Errors
    ///
    /// When a write is refused or fails.
    pub fn apply(&self, plan: &WireAdrPlan) -> Result<Vec<PathBuf>> {
        let mut written = Vec::with_capacity(plan.writes.len());
        for write in &plan.writes {
            let relative = inside_root(&write.path)
                .filter(|path| !path.as_os_str().is_empty())
                .with_context(|| format!("refusing to write {}: it is outside the project root", write.path))?;
            let path = self.root.join(&relative);
            let parent = path.parent().context("a write with no parent directory")?;
            let parent = fs::canonicalize(parent).with_context(|| format!("resolving {}", parent.display()))?;
            if !parent.starts_with(&self.root) {
                bail!(
                    "refusing to write {}: its directory resolves outside the project root",
                    write.path
                );
            }
            if fs::symlink_metadata(&path).is_ok_and(|metadata| metadata.file_type().is_symlink()) {
                bail!("refusing to write through the symlink at {}", path.display());
            }
            if write.create {
                create_new(&path, &write.content)?;
            } else {
                replace(&path, &write.content)?;
            }
            written.push(path);
        }
        Ok(written)
    }

    /// Every Markdown file under `dir`, recursing when the log is recursive.
    fn markdown_under(&self, dir: &Path, out: &mut Vec<ProjectFile>) -> Result<()> {
        self.markdown_in(dir, self.recursive, out)
    }

    /// Every Markdown file in `dir` (relative to the root), and its subdirectories with `recursive`.
    /// Symlinks are never followed; a directory that does not exist contributes nothing.
    fn markdown_in(&self, dir: &Path, recursive: bool, out: &mut Vec<ProjectFile>) -> Result<()> {
        let absolute = self.root.join(dir);
        if fs::symlink_metadata(&absolute).is_ok_and(|metadata| metadata.file_type().is_symlink()) {
            return Ok(());
        }
        let entries = match fs::read_dir(&absolute) {
            Ok(entries) => entries,
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error).with_context(|| format!("reading {}", absolute.display())),
        };
        let mut names = Vec::new();
        for entry in entries {
            let entry = entry.with_context(|| format!("reading {}", absolute.display()))?;
            let kind = entry
                .file_type()
                .with_context(|| format!("reading {}", entry.path().display()))?;
            names.push((entry.file_name(), kind));
        }
        names.sort_by(|a, b| a.0.cmp(&b.0));
        for (name, kind) in names {
            let relative = dir.join(&name);
            if kind.is_dir() {
                if recursive {
                    self.markdown_in(&relative, true, out)?;
                }
            } else if kind.is_file() && relative.extension().is_some_and(|extension| extension == "md") {
                out.push(self.read(&relative, Language::Markdown)?);
            }
        }
        Ok(())
    }

    /// The lock manifest, or `None` when the project has none yet.
    fn manifest(&self) -> Result<Option<ProjectFile>> {
        let path = self.root.join(&self.lock);
        match fs::symlink_metadata(&path) {
            Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error).with_context(|| format!("reading {}", path.display())),
            Ok(metadata) if metadata.file_type().is_symlink() => {
                bail!(
                    "refusing to read the lock file through the symlink at {}",
                    path.display()
                )
            }
            Ok(_) => {
                let name = wire_path(&self.lock);
                self.read(&self.lock, Language::Other(name)).map(Some)
            }
        }
    }

    fn read(&self, relative: &Path, language: Language) -> Result<ProjectFile> {
        let path = self.root.join(relative);
        let content = fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
        Ok(ProjectFile {
            path: relative.to_path_buf(),
            language,
            content: Arc::from(content),
        })
    }

    /// Asks the engine to plan `op` over `files`: in-process when this build links it, otherwise through
    /// the component that hosts it.
    fn call(
        &self,
        op: WireAdrOp,
        files: Vec<ProjectFile>,
        config: &Config,
        injection: Option<&ComponentInjection>,
    ) -> Result<WireAdrPlan> {
        if injection.is_none()
            && let Some(planner) = LINKED.get()
        {
            return planner(&op, &self.config, &files);
        }
        let mut body = Vec::new();
        let mut slices = Vec::with_capacity(files.len());
        for file in &files {
            slices.push(WireFileSlice {
                path: wire_path(&file.path),
                language: file.language.id().to_owned(),
                offset: body.len() as u64,
                len: file.content.len() as u64,
            });
            body.extend_from_slice(file.content.as_bytes());
        }
        let request = Request::Adr {
            op,
            config: engine_config_to_wire(&self.config),
            files: slices,
        };
        match call_engine_component_with_body(injection, config, ENGINE, &request, &body)? {
            Payload::Adr(plan) => Ok(plan),
            other => bail!("the adrlint component answered Adr with an unexpected payload: {other:?}"),
        }
    }
}

/// An option of the expected type, `None` when unset. A wrong type is an error naming the key, never a
/// silent fall back to the default, which would act on the wrong log.
fn option<'a, T>(
    options: &'a toml::Table,
    key: &str,
    expected: &str,
    read: impl FnOnce(&'a toml::Value) -> Option<T>,
) -> Result<Option<T>> {
    match options.get(key) {
        None => Ok(None),
        Some(value) => read(value)
            .map(Some)
            .with_context(|| format!("option `{key}` must be {expected}, found {}", value.type_str())),
    }
}

/// `path` with `.` removed, or `None` when it is absolute or climbs out with `..`.
fn inside_root(path: &str) -> Option<PathBuf> {
    let path = Path::new(path);
    let escapes = path.is_absolute()
        || path
            .components()
            .any(|part| matches!(part, Component::ParentDir | Component::Prefix(_) | Component::RootDir));
    if escapes {
        return None;
    }
    Some(path.components().filter(|part| *part != Component::CurDir).collect())
}

/// A root-relative path as the wire spells it: `/`-separated, with no `.` component.
fn wire_path(path: &Path) -> String {
    path.components()
        .filter_map(|part| match part {
            Component::Normal(name) => Some(name.to_string_lossy()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("/")
}

/// Writes a new file, failing when it already exists.
fn create_new(path: &Path, content: &str) -> Result<()> {
    let mut file = match OpenOptions::new().write(true).create_new(true).open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == ErrorKind::AlreadyExists => bail!("{} already exists", path.display()),
        Err(error) => return Err(error).with_context(|| format!("creating {}", path.display())),
    };
    file.write_all(content.as_bytes())
        .with_context(|| format!("writing {}", path.display()))
}

/// Replaces `path` atomically: the content goes to a temporary file beside it, which is then renamed
/// over it, so an interrupted write never leaves a half-written manifest.
fn replace(path: &Path, content: &str) -> Result<()> {
    let parent = path.parent().context("a write with no parent directory")?;
    let name = path.file_name().and_then(|name| name.to_str()).unwrap_or("adrlint");
    let temporary = parent.join(format!(".{name}.{}.tmp", std::process::id()));
    let result = (|| -> Result<()> {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
            .with_context(|| format!("creating {}", temporary.display()))?;
        file.write_all(content.as_bytes())
            .with_context(|| format!("writing {}", temporary.display()))?;
        if let Ok(metadata) = fs::metadata(path) {
            fs::set_permissions(&temporary, metadata.permissions())
                .with_context(|| format!("setting the permissions of {}", temporary.display()))?;
        }
        fs::rename(&temporary, path).with_context(|| format!("replacing {}", path.display()))
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

/// Today's date in UTC, `YYYY-MM-DD`, for a new record's front matter.
#[must_use]
pub fn today() -> String {
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs());
    let (year, month, day) = civil_from_days(i64::try_from(seconds / 86_400).unwrap_or(0));
    format!("{year:04}-{month:02}-{day:02}")
}

/// The proleptic Gregorian date `days` after 1970-01-01 (Howard Hinnant's `civil_from_days`).
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year, u32::try_from(month).unwrap_or(1), u32::try_from(day).unwrap_or(1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn civil_dates_match_known_days() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(11_016), (2000, 2, 29));
        assert_eq!(civil_from_days(20_772), (2026, 11, 15));
    }

    #[test]
    fn a_path_must_stay_inside_the_root() {
        assert_eq!(inside_root("./docs/adr"), Some(PathBuf::from("docs/adr")));
        assert_eq!(inside_root("."), Some(PathBuf::new()));
        assert_eq!(inside_root("../elsewhere"), None);
        assert_eq!(inside_root("/etc"), None);
    }

    #[test]
    fn wire_paths_use_forward_slashes() {
        assert_eq!(
            wire_path(&Path::new("docs").join("adr").join("0001-x.md")),
            "docs/adr/0001-x.md"
        );
        assert_eq!(wire_path(Path::new("./adrlint.lock")), "adrlint.lock");
    }
}
