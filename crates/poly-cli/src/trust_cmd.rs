//! `poly trust` — allow a repository's config keys that run programs (ADR 0057).
//!
//! `poly trust [PATH]` records the repository holding PATH (default: the working
//! directory) in the per-user trust store, bound to a digest of every
//! exec-capable key its configs set; `--revoke` forgets it, `--list` prints every
//! trusted repository, and `--show` reports what would be gated and whether the
//! current values are trusted. `poly hooks install` records trust too.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{Context, Result, bail};
use clap::Args;
use poly_config::trust::{self, RepoAssessment, TrustStore};

use crate::config_sources::RemoteExtendsResolver;

/// `poly trust` arguments.
#[derive(Args)]
pub struct TrustArgs {
    /// A path inside the repository to act on (default: the working directory).
    pub path: Option<PathBuf>,

    /// Forget the repository: its exec-capable keys are ignored again.
    #[arg(long, conflicts_with_all = ["list", "show"])]
    pub revoke: bool,

    /// List every trusted repository.
    #[arg(long, conflicts_with_all = ["revoke", "show"])]
    pub list: bool,

    /// Show the exec-capable keys the repository sets and whether they are trusted, changing nothing.
    #[arg(long, conflicts_with_all = ["revoke", "list"])]
    pub show: bool,
}

/// Run `poly trust`, mapping any error to exit code 2.
pub fn run_trust(args: TrustArgs) -> ExitCode {
    match trust_command(&args) {
        Ok(code) => code,
        Err(error) => {
            eprintln!("poly trust: {error:#}");
            ExitCode::from(2)
        }
    }
}

fn trust_command(args: &TrustArgs) -> Result<ExitCode> {
    if args.list {
        return list();
    }
    let start = match &args.path {
        Some(path) => path.clone(),
        None => std::env::current_dir().context("resolving the working directory")?,
    };
    if args.revoke {
        return revoke(&start);
    }
    if args.show {
        return show(&start);
    }
    let assessment = trust_repo(&start)?;
    let keys = gated_keys(&assessment);
    println!("trusted {}", assessment.root.display());
    if keys.is_empty() {
        println!("  (no config sets a key that runs programs)");
    }
    for line in keys {
        println!("  {line}");
    }
    Ok(ExitCode::SUCCESS)
}

/// Record the repository holding `start` as trusted with its current
/// exec-capable keys, and make this process honour them from now on.
///
/// # Errors
///
/// When no `poly.toml` governs `start`, a config cannot be resolved, or the store
/// cannot be written.
pub fn trust_repo(start: &Path) -> Result<RepoAssessment> {
    let assessment = assess(start)?;
    let mut store = TrustStore::load()?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs());
    store.insert(&assessment.root, assessment.record(now));
    store.save()?;
    trust::use_store(store);
    Ok(assessment)
}

fn revoke(start: &Path) -> Result<ExitCode> {
    let root = repository_root(start)?;
    let mut store = TrustStore::load()?;
    if store.remove(&root) {
        store.save()?;
        println!("revoked trust for {}", root.display());
    } else {
        println!("{} was not trusted", root.display());
    }
    Ok(ExitCode::SUCCESS)
}

fn list() -> Result<ExitCode> {
    let store = TrustStore::load()?;
    let mut any = false;
    for (root, record) in store.iter() {
        any = true;
        println!("{root} (trusted at {} s since the epoch)", record.trusted_at);
    }
    if !any {
        println!("no repository is trusted");
    }
    Ok(ExitCode::SUCCESS)
}

fn show(start: &Path) -> Result<ExitCode> {
    let assessment = assess(start)?;
    let store = TrustStore::load()?;
    let record = store.get(&assessment.root);
    let keys = gated_keys(&assessment);
    let status = if keys.is_empty() {
        "nothing to trust: no config sets a key that runs programs"
    } else if assessment.is_trusted_by(record) {
        "trusted: these keys are honoured"
    } else if record.is_some() {
        "untrusted: the keys changed since `poly trust`, so they are ignored"
    } else {
        "untrusted: these keys are ignored until `poly trust`"
    };
    println!("{}: {status}", assessment.root.display());
    for line in keys {
        println!("  {line}");
    }
    Ok(ExitCode::SUCCESS)
}

/// One line per config that sets exec-capable keys, plus one per out-of-repository `extends` base.
fn gated_keys(assessment: &RepoAssessment) -> Vec<String> {
    let mut lines: Vec<String> = assessment
        .configs
        .iter()
        .filter(|config| !config.keys.is_empty())
        .map(|config| {
            let file = config.dir.join(poly_config::CONFIG_FILE_NAMES[0]);
            let shown = file.strip_prefix(&assessment.root).unwrap_or(&file);
            format!("{}: {}", shown.display(), config.keys.join(", "))
        })
        .collect();
    lines.extend(
        assessment
            .extends_outside
            .iter()
            .map(|base| format!("extends base outside the repository: {base}")),
    );
    lines
}

/// Assess every config of the repository holding `start`.
fn assess(start: &Path) -> Result<RepoAssessment> {
    let root = repository_root(start)?;
    let resolver = RemoteExtendsResolver::new(&root)?;
    let dirs = config_dirs(&root);
    trust::assess(&root, &dirs, &resolver)
}

/// The trust root of the config governing `start`: its git root, else the config's directory.
fn repository_root(start: &Path) -> Result<PathBuf> {
    let dir = if start.is_file() {
        start.parent().unwrap_or(start)
    } else {
        start
    };
    match poly_config::find_config(dir) {
        Some(config) => Ok(trust::trust_root(config.parent().unwrap_or(dir))),
        None => bail!("no {} at or above {}", poly_config::CONFIG_FILE_NAMES[0], dir.display()),
    }
}

/// Every directory under `root` holding a `poly.toml`, honouring `.gitignore`.
fn config_dirs(root: &Path) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = ignore::WalkBuilder::new(root)
        .hidden(false)
        .filter_entry(|entry| entry.file_name() != ".git")
        .build()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_some_and(|kind| kind.is_file()))
        .filter(|entry| entry.file_name() == poly_config::CONFIG_FILE_NAMES[0])
        .filter_map(|entry| entry.path().parent().map(Path::to_path_buf))
        .collect();
    dirs.sort();
    dirs.dedup();
    dirs
}

/// Print the run's note about keys an untrusted repository's config could not use.
pub(crate) fn eprint_untrusted_note() {
    for line in trust::notes::take() {
        eprintln!("{line}");
    }
}

/// Refuse to run `[hooks]` from a config whose exec-capable keys were withheld.
///
/// # Errors
///
/// When `config` is not trusted.
pub(crate) fn require_trusted(config: &poly_config::PolyConfig, what: &str) -> Result<()> {
    if !config.trust.is_restricted() {
        return Ok(());
    }
    let root = config
        .trust
        .root
        .as_ref()
        .map_or_else(|| "this repository".to_owned(), |root| root.display().to_string());
    bail!(
        "refusing to run {what}: {root} is not trusted and its config sets keys that run programs ({}). \
         Review them with `poly trust --show`, then run `poly trust` (or `poly hooks install`), or set {}=1 \
         for one run",
        config.trust.withheld.join(", "),
        trust::TRUST_ENV
    )
}
