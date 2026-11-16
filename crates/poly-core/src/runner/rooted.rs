//! A run rooted at [`RunOptions::root`] instead of the process working directory
//! (ADR 0073).
//!
//! The run is what a `poly` child started in the root would do: its paths are
//! resolved against the root, every working-directory read below the runner
//! answers the root ([`poly_engine_api::run_root`]), and the paths and messages
//! it reports are spelled relative to the root again, as the child's are.
//!
//! The run executes on a pool of its own whose every worker has the root entered
//! for its whole life, so no task of it, stolen or not, can see another run's
//! root, and no other run's task can see this one's.

use std::path::{MAIN_SEPARATOR, Path, PathBuf};
use std::sync::Arc;

use poly_engine_api::run_root;

use super::types::{FormatRun, LintRun, RunOptions};
use super::{SkippedFile, SuppressedDiagnostic, WORKER_STACK_SIZE};
use crate::discover::DiscoveryReport;

/// Run `body` over `paths` as the run `opts` describes: unchanged when it names no
/// root, else on a pool rooted at it, over the root-joined paths, with the result
/// spelled relative to the root.
pub(super) fn run<T: Relativize + Send>(
    opts: &RunOptions,
    paths: &[PathBuf],
    body: impl FnOnce(&[PathBuf]) -> anyhow::Result<T> + Send,
) -> anyhow::Result<T> {
    let Some(root) = opts.root.as_deref() else {
        return body(paths);
    };
    let root: Arc<Path> = Arc::from(root);
    let joined: Vec<PathBuf> = paths.iter().map(|path| root.join(path)).collect();
    let pinned = Arc::clone(&root);
    let mut builder = rayon::ThreadPoolBuilder::new()
        .stack_size(WORKER_STACK_SIZE)
        .start_handler(move |_| run_root::pin(Arc::clone(&pinned)));
    if let Some(jobs) = opts.jobs.filter(|&jobs| jobs > 0) {
        builder = builder.num_threads(jobs);
    }
    let pool = builder
        .build()
        .map_err(|error| anyhow::anyhow!("cannot start the worker pool for {}: {error}", root.display()))?;
    let mut done = pool.install(|| body(&joined))?;
    done.relativize(&Prefix::new(&root));
    Ok(done)
}

/// The root as it prefixes a path and as it appears inside a message.
pub(super) struct Prefix<'a> {
    root: &'a Path,
    text: String,
}

impl<'a> Prefix<'a> {
    fn new(root: &'a Path) -> Self {
        let mut text = root.display().to_string();
        if !text.ends_with(MAIN_SEPARATOR) {
            text.push(MAIN_SEPARATOR);
        }
        Self { root, text }
    }

    fn path(&self, path: &mut PathBuf) {
        if let Ok(relative) = path.strip_prefix(self.root) {
            *path = relative.to_path_buf();
        }
    }

    fn text(&self, text: &mut String) {
        if text.contains(&self.text) {
            *text = text.replace(&self.text, "");
        }
    }
}

/// A run result whose paths and messages can be re-spelled relative to the root.
pub(super) trait Relativize {
    /// Re-spell every path under the root, and every mention of it in a message,
    /// relative to it.
    fn relativize(&mut self, prefix: &Prefix<'_>);
}

impl Relativize for LintRun {
    fn relativize(&mut self, prefix: &Prefix<'_>) {
        for result in &mut self.results {
            prefix.path(&mut result.path);
            for diagnostic in result.diagnostics.iter_mut() {
                prefix.text(&mut diagnostic.title);
                if let Some(description) = &mut diagnostic.description {
                    prefix.text(description);
                }
            }
            suppressed(&mut result.suppressed, prefix);
            optional(&mut result.skipped, prefix);
            optional(&mut result.error, prefix);
        }
        for error in &mut self.errors {
            prefix.path(&mut error.path);
            prefix.text(&mut error.message);
        }
        skipped(&mut self.skipped, prefix);
        suppressed(&mut self.suppressed, prefix);
        discovery(&mut self.discovery, prefix);
    }
}

impl Relativize for FormatRun {
    fn relativize(&mut self, prefix: &Prefix<'_>) {
        for result in &mut self.results {
            prefix.path(&mut result.path);
            optional(&mut result.skipped, prefix);
            optional(&mut result.error, prefix);
        }
        for error in &mut self.errors {
            prefix.path(&mut error.path);
            prefix.text(&mut error.message);
        }
        skipped(&mut self.skipped, prefix);
        discovery(&mut self.discovery, prefix);
    }
}

fn optional(text: &mut Option<String>, prefix: &Prefix<'_>) {
    if let Some(text) = text {
        prefix.text(text);
    }
}

fn skipped(entries: &mut [SkippedFile], prefix: &Prefix<'_>) {
    for entry in entries {
        prefix.path(&mut entry.path);
        prefix.text(&mut entry.reason);
    }
}

fn suppressed(entries: &mut [SuppressedDiagnostic], prefix: &Prefix<'_>) {
    for entry in entries {
        prefix.path(&mut entry.path);
    }
}

fn discovery(report: &mut DiscoveryReport, prefix: &Prefix<'_>) {
    for sample in &mut report.unrecognized_samples {
        prefix.path(sample);
    }
    for sample in &mut report.pruned_samples {
        prefix.path(&mut sample.path);
    }
    for rule in &mut report.rules {
        for sample in &mut rule.directory_samples {
            prefix.path(&mut sample.path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_rooted_path_and_message_read_as_the_child_spells_them() {
        let root = std::env::temp_dir().join("poly-rooted-test");
        let prefix = Prefix::new(&root);
        let mut path = root.join("src").join("a.py");
        prefix.path(&mut path);
        assert_eq!(path, Path::new("src").join("a.py"));

        let mut outside = PathBuf::from("/elsewhere/b.py");
        prefix.path(&mut outside);
        assert_eq!(outside, PathBuf::from("/elsewhere/b.py"));

        let mut message = format!("reading {}: denied", root.join("a.py").display());
        prefix.text(&mut message);
        assert_eq!(message, "reading a.py: denied");
    }

    #[test]
    fn a_run_without_a_root_runs_on_the_given_paths_unchanged() {
        let paths = vec![PathBuf::from("a.py")];
        let seen = run(&RunOptions::default(), &paths, |given| {
            Ok(Seen(given.to_vec(), run_root::get().is_none()))
        })
        .unwrap();
        assert_eq!(seen.0, paths);
        assert!(seen.1, "an unrooted run enters no root");
    }

    #[test]
    fn a_rooted_run_sees_its_root_on_every_worker_and_reports_relative_paths() {
        let root = tempfile::tempdir().unwrap();
        let opts = RunOptions {
            root: Some(root.path().to_path_buf()),
            jobs: Some(2),
            ..RunOptions::default()
        };
        let paths = vec![PathBuf::from("a.py")];
        let seen = run(&opts, &paths, |given| {
            use rayon::prelude::*;
            let rooted = (0..64)
                .into_par_iter()
                .all(|_| run_root::get().as_deref() == Some(root.path()));
            Ok(Seen(given.to_vec(), rooted))
        })
        .unwrap();
        assert!(seen.1, "every worker of the run sees its root");
        assert_eq!(seen.0, vec![PathBuf::from("a.py")], "reported relative to the root");
        assert!(run_root::get().is_none(), "the caller's thread is left unrooted");
    }

    /// The paths a body received and whether its workers saw the expected root.
    struct Seen(Vec<PathBuf>, bool);

    impl Relativize for Seen {
        fn relativize(&mut self, prefix: &Prefix<'_>) {
            for path in &mut self.0 {
                prefix.path(path);
            }
        }
    }
}
