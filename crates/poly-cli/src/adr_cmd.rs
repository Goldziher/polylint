//! `poly adr` — maintain an architecture decision record log with the bundled adrlint (ADR 0061).
//!
//! - `poly adr lock [--all] [--check]` — write or refresh `adrlint.lock` for every accepted or
//!   rejected record (every numbered record with `--all`). It refuses while `poly lint` reports an
//!   adrlint error in the log, so `[per-file-ignores]` and inline suppressions decide what blocks it.
//! - `poly adr unlock <FILE> --reason <TEXT>` — release one record from the manifest for editing.
//! - `poly adr new <TITLE> [--dir <DIR>]` — create the next numbered record from the log's template
//!   (`template.md` or `NNNN-template.md`), else MADR's.
//!
//! All three read the adrlint settings from `poly.toml` (`[engines.adrlint]`, or the v1
//! `[lint.markdown.adrlint]`): `dir` / `paths` / `recursive` name the log, `lock` the manifest and
//! `start` the first number. They write only inside the project root, the directory of the nearest
//! `poly.toml`. The planning is the adrlint engine's: in-process in a build that links it, otherwise
//! the `web` component, installed on demand unless `--offline` or `[components] install = "never"`.

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use clap::{Args, Subcommand};
use poly_core::{Config, RunOptions, Severity};
use poly_host_services::adr::{AdrLog, ENGINE, today};

use crate::config_sources::{self, RemoteExtendsResolver};

pub use poly_host_services::adr::register_linked_planner;

/// `poly adr` arguments.
#[derive(Args)]
pub struct AdrArgs {
    /// The ADR operation to perform.
    #[command(subcommand)]
    pub command: AdrCommand,
}

/// The `poly adr` subcommands.
#[derive(Subcommand)]
pub enum AdrCommand {
    /// Write or refresh adrlint.lock for every accepted or rejected record.
    ///
    /// Refuses while `poly lint` reports an adrlint error in the log; `[per-file-ignores]` and inline
    /// `poly: allow[...]` directives apply, so a template sharing a number with the first record can
    /// be exempted from MADR43 for that pair alone.
    Lock(AdrLockArgs),
    /// Release one record from adrlint.lock so it can be edited.
    Unlock(AdrUnlockArgs),
    /// Create the next numbered record from the log's template.
    New(AdrNewArgs),
}

/// `poly adr lock` arguments.
#[derive(Args)]
pub struct AdrLockArgs {
    /// Lock every numbered record, whatever its status; also refreshes a manifest written by an
    /// older adrlint.
    #[arg(long)]
    pub all: bool,

    /// Write nothing; exit 1 when adrlint.lock would change.
    #[arg(long)]
    pub check: bool,
}

/// `poly adr unlock` arguments.
#[derive(Args)]
pub struct AdrUnlockArgs {
    /// The record to release, as a path from the working directory.
    #[arg(value_name = "FILE")]
    pub file: PathBuf,

    /// Why the record is being unlocked; printed for the commit message.
    #[arg(long, value_name = "TEXT")]
    pub reason: String,
}

/// `poly adr new` arguments.
#[derive(Args)]
pub struct AdrNewArgs {
    /// The decision's title; the file name is its slug.
    #[arg(value_name = "TITLE")]
    pub title: String,

    /// The ADR directory to create the record in (default: the one configured directory).
    #[arg(long, value_name = "DIR")]
    pub dir: Option<PathBuf>,
}

/// Run `poly adr`: exit 0 on success, 1 when `lock` is refused by lint errors or `--check` finds the
/// manifest out of date, 2 on any other error.
pub fn run_adr(args: AdrArgs) -> ExitCode {
    match run(args) {
        Ok(code) => code,
        Err(error) => {
            eprintln!("poly adr: {error:#}");
            ExitCode::from(2)
        }
    }
}

fn run(args: AdrArgs) -> Result<ExitCode> {
    // Checked before any config or component work, so a missing reason never costs a download.
    if let AdrCommand::Unlock(unlock) = &args.command
        && unlock.reason.trim().is_empty()
    {
        bail!("--reason must not be empty");
    }
    let project = Project::load()?;
    match args.command {
        AdrCommand::Lock(lock) => project.lock(&lock),
        AdrCommand::Unlock(unlock) => project.unlock(&unlock),
        AdrCommand::New(new) => project.create(&new),
    }
}

/// The resolved config and the ADR log it describes.
struct Project {
    config: Config,
    log: AdrLog,
    resolver: Arc<RemoteExtendsResolver>,
}

impl Project {
    fn load() -> Result<Self> {
        let resolver = Arc::new(config_sources::resolver()?);
        let poly_config = crate::load_poly_config(crate::global().config.as_deref(), resolver.as_ref())
            .context("failed to load config")?;
        let config = Config::from(poly_config);
        let log = AdrLog::resolve(&config, &project_root()?, None)?;
        Ok(Self { config, log, resolver })
    }

    fn lock(self, args: &AdrLockArgs) -> Result<ExitCode> {
        if !self.log.is_enabled() {
            bail!("adrlint is not enabled; set `[engines.adrlint] enabled = true` in poly.toml");
        }
        let dirs = self.log.directories();
        if dirs.is_empty() {
            bail!("no ADR directory exists under {}", self.log.root().display());
        }
        let blocking = self.lint_errors(&dirs)?;
        if !blocking.is_empty() {
            for finding in &blocking {
                eprintln!("{finding}");
            }
            eprintln!(
                "poly adr lock: refusing to lock: fix {} adrlint error(s) first",
                blocking.len()
            );
            return Ok(ExitCode::from(1));
        }

        let plan = self.log.plan_lock(args.all, &self.config, None)?;
        if args.check {
            if plan.writes.is_empty() {
                println!("{}", plan.summary);
                return Ok(ExitCode::SUCCESS);
            }
            for change in &plan.changes {
                eprintln!("would {change}");
            }
            eprintln!(
                "poly adr lock: {} is out of date; run `poly adr lock`",
                self.log.lock_path().display()
            );
            return Ok(ExitCode::from(1));
        }
        self.log.apply(&plan)?;
        println!("{}", plan.summary);
        Ok(ExitCode::SUCCESS)
    }

    fn unlock(self, args: &AdrUnlockArgs) -> Result<ExitCode> {
        let record = self.log.relative(&args.file)?;
        let plan = self.log.plan_unlock(&record, &self.config, None)?;
        self.log.apply(&plan)?;
        println!("{}: {}", plan.summary, args.reason.trim());
        Ok(ExitCode::SUCCESS)
    }

    fn create(self, args: &AdrNewArgs) -> Result<ExitCode> {
        let dir = self.log.new_record_dir(args.dir.as_deref())?;
        let plan = self.log.plan_new(&dir, &args.title, &today(), &self.config, None)?;
        self.log.apply(&plan)?;
        println!("{}", plan.summary);
        Ok(ExitCode::SUCCESS)
    }

    /// The error-severity adrlint findings `poly lint` reports over `dirs`, one line each, after every
    /// suppression applies. The whole-project phase does not run; a component that cannot be had fails
    /// the check rather than skipping it, and a run that checked nothing is an error, so the gate never
    /// passes because it examined nothing.
    fn lint_errors(&self, dirs: &[PathBuf]) -> Result<Vec<String>> {
        let options = RunOptions {
            force_exclude: self.config.force_exclude,
            explicit_config: crate::global().config.is_some(),
            config_resolver: Some(self.resolver.clone()),
            only: vec![ENGINE.to_owned()],
            on_missing: Some(poly_config::v2::OnMissing::Fail),
            ..RunOptions::default()
        };
        let run = poly_core::lint_run(dirs, &self.config, &options, false, false)
            .context("linting the ADR log before locking it")?;
        if let Some(error) = run.errors.first() {
            bail!("could not lint {}: {}", error.path.display(), error.message);
        }
        if run.checked == 0 {
            bail!("linting the ADR log checked no file, so nothing shows it is safe to lock");
        }
        Ok(run
            .results
            .iter()
            .flat_map(|result| {
                result
                    .diagnostics
                    .iter()
                    .filter(|diagnostic| diagnostic.engine == ENGINE && diagnostic.severity == Severity::Error)
                    .map(|diagnostic| {
                        let line = diagnostic.span.map_or(1, |span| span.start_line);
                        let code = diagnostic.code.as_deref().unwrap_or("adrlint");
                        format!("{}:{line}: {code} {}", result.path.display(), diagnostic.title)
                    })
            })
            .collect())
    }
}

/// The project root: the directory of `--config`, else of the nearest `poly.toml`, else the working
/// directory.
fn project_root() -> Result<PathBuf> {
    if let Some(explicit) = &crate::global().config {
        let parent = explicit.parent().filter(|parent| !parent.as_os_str().is_empty());
        return Ok(parent.map_or_else(|| PathBuf::from("."), Path::to_path_buf));
    }
    let cwd = std::env::current_dir().context("resolving the working directory")?;
    Ok(poly_config::find_config(&cwd)
        .and_then(|config| config.parent().map(Path::to_path_buf))
        .unwrap_or(cwd))
}
