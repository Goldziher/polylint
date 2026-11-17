//! The parts of a project check's cache key that are about where things are and what the check can
//! read besides the files it is handed.
//!
//! - **Paths are checkout-relative.** The project root is keyed relative to the checkout holding it
//!   (the nearest `.git`, as the result cache itself is located), served files and reported findings
//!   relative to the project root, and an interpreter or `site-packages` directory relative to the
//!   checkout when it lies inside it (a project `.venv`). Two worktrees of a repository, or an absolute
//!   and a relative invocation, then share entries. A path outside the checkout keeps its full form,
//!   which cannot collide with a relative one.
//! - **What a Python check reads from disk is keyed.** Python's import system resolves modules from
//!   the disk, and both Python checkers do so through their search paths rather than the served set
//!   alone. So for a plan that serves Python the key also folds in [`site_packages_fingerprint`]
//!   (what `pip install` changes) and [`unserved_python`] (first-party modules the check can import
//!   but was not handed: gitignored or excluded files, and every other module of a path-scoped run).
//!   Keying them was chosen over making the checkers read only what they are handed, because the
//!   latter would hide those modules from the check and turn their imports into errors.
//! - **An engine's own environment namespace is keyed.** A component may read `<NAME>_*` variables
//!   (tsrs reads `TSRS_*`, ADR 0051); poly never sets them, so they are folded in as found.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use ignore::WalkBuilder;
use rustc_hash::{FxHashMap, FxHashSet};

use crate::engine::{Diagnostic, ProjectEnvironment, ProjectFile};

/// Directories never searched for unserved Python modules: tool state, and trees no import of the
/// project resolves into (a virtual environment is recognised by its `pyvenv.cfg` instead).
const PRUNED: &[&str] = &[
    ".git",
    "node_modules",
    "__pycache__",
    "target",
    "site-packages",
    "dist-packages",
    ".tox",
    ".nox",
    ".mypy_cache",
    ".pytest_cache",
    ".ruff_cache",
    ".eggs",
    ".hg",
    ".svn",
];

/// Whether the directory `dir` is a tree of its own that no import of the project resolves into: a
/// virtual environment (`pyvenv.cfg`), a conda environment (`conda-meta`), or another checkout (a
/// `.git` entry: a nested repository, a submodule, or a worktree under `.worktrees/`, whose edits
/// must not invalidate this project's check).
fn is_foreign_tree(dir: &Path) -> bool {
    dir.join("pyvenv.cfg").is_file() || dir.join("conda-meta").is_dir() || dir.join(".git").exists()
}

/// A file in `site-packages` larger than this is keyed by its size alone. Top-level modules and `.pth`
/// files are small; the cap only bounds the cost of an odd large file.
const MAX_HASHED_BYTES: u64 = 1024 * 1024;

/// `path` as key text: relative to `base` when it lies beneath it (`.` for `base` itself), its full
/// form otherwise.
pub(super) fn relative_text(path: &Path, base: &Path) -> String {
    let path = absolute(path);
    match path.strip_prefix(absolute(base)) {
        Ok(rest) if rest.as_os_str().is_empty() => ".".to_owned(),
        Ok(rest) => rest.to_string_lossy().replace('\\', "/"),
        Err(_) => path.to_string_lossy().into_owned(),
    }
}

fn absolute(path: &Path) -> PathBuf {
    std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf())
}

/// The checkout holding `root`: what [`relative_text`] keys the root and the environment against.
pub(super) fn checkout_of(root: &Path) -> PathBuf {
    poly_cache::repo_anchor(&absolute(root))
}

/// The environment as key text: its wire pairs with every path made checkout-relative.
pub(super) fn environment_text(environment: &ProjectEnvironment, checkout: &Path) -> String {
    let Some(python) = &environment.python else {
        return String::new();
    };
    let sites: Vec<String> = python
        .site_packages
        .iter()
        .map(|site| relative_text(site, checkout))
        .collect();
    format!(
        "python.interpreter={}\0python.site_packages={}\0python.version={}",
        relative_text(&python.interpreter, checkout),
        sites.join("\n"),
        python.version.as_deref().unwrap_or("-"),
    )
}

/// What a Python check reads from disk besides the files it is handed: the installed packages and the
/// project's own unserved modules. Empty without a Python environment and served Python files.
pub(super) fn python_disk_inputs(environment: &ProjectEnvironment, root: &Path, served: &[ProjectFile]) -> String {
    if !served
        .iter()
        .any(|file| file.language == crate::language::Language::Python)
    {
        return String::new();
    }
    let mut hasher = blake3::Hasher::new();
    if let Some(python) = &environment.python {
        for site in &python.site_packages {
            hasher.update(site_packages_fingerprint(site).as_bytes());
        }
    }
    hasher.update(b"\0unserved\0");
    hasher.update(unserved_python(root, served).as_bytes());
    hasher.finalize().to_hex().to_string()
}

/// A fingerprint of what is installed in the `site-packages` directory `site`.
///
/// Every top-level entry's name, the bytes of each top-level file (`.pth` files, single-module
/// packages such as `six.py`) and the bytes of each `*.dist-info/RECORD` (`*.egg-info/SOURCES.txt` for
/// a legacy install). `RECORD` lists every file the distribution installed with its hash and size, so
/// installing, upgrading or removing a package with `pip` or `uv` changes it; hashing those few small
/// files is far cheaper than walking the packages themselves. A file edited by hand inside an installed
/// package is not seen, as no installer would leave it so.
pub(super) fn site_packages_fingerprint(site: &Path) -> String {
    let mut hasher = blake3::Hasher::new();
    let Ok(entries) = std::fs::read_dir(site) else {
        return "absent".to_owned();
    };
    let mut entries: Vec<(String, PathBuf, bool)> = entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let name = entry.file_name().into_string().ok()?;
            let is_dir = entry.file_type().is_ok_and(|kind| kind.is_dir());
            Some((name, entry.path(), is_dir))
        })
        .collect();
    entries.sort();
    for (name, path, is_dir) in &entries {
        hasher.update(name.as_bytes());
        hasher.update(b"\0");
        let manifest = if name.ends_with(".dist-info") {
            Some(path.join("RECORD"))
        } else if name.ends_with(".egg-info") && *is_dir {
            Some(path.join("SOURCES.txt"))
        } else if *is_dir {
            None
        } else {
            Some(path.clone())
        };
        if let Some(manifest) = manifest {
            hash_file(&mut hasher, &manifest);
        }
    }
    hasher.finalize().to_hex().to_string()
}

fn hash_file(hasher: &mut blake3::Hasher, path: &Path) {
    match std::fs::metadata(path) {
        Ok(metadata) if metadata.len() > MAX_HASHED_BYTES => {
            hasher.update(b"size:");
            hasher.update(&metadata.len().to_le_bytes());
        }
        Ok(_) => {
            hasher.update(&std::fs::read(path).unwrap_or_default());
        }
        Err(_) => {
            hasher.update(b"<missing>");
        }
    }
    hasher.update(b"\0");
}

/// A digest of every `.py` / `.pyi` file under `root` that is not in `served`, by root-relative path
/// and content, ignored files included: the checker resolves an import to it whether or not git
/// tracks it. Environments, nested checkouts ([`is_foreign_tree`]) and [`PRUNED`] trees are not
/// searched.
pub(super) fn unserved_python(root: &Path, served: &[ProjectFile]) -> String {
    let root = absolute(root);
    let served: FxHashSet<PathBuf> = served.iter().map(|file| absolute(&file.path)).collect();
    let mut unserved: Vec<(String, PathBuf)> = WalkBuilder::new(&root)
        .standard_filters(false)
        .follow_links(false)
        .filter_entry(|entry| {
            let is_dir = entry.file_type().is_some_and(|kind| kind.is_dir());
            !is_dir
                || entry.depth() == 0
                || (entry.file_name().to_str().is_none_or(|name| !PRUNED.contains(&name))
                    && !is_foreign_tree(entry.path()))
        })
        .build()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_some_and(|kind| kind.is_file()))
        .map(ignore::DirEntry::into_path)
        .filter(|path| {
            path.extension()
                .and_then(|extension| extension.to_str())
                .is_some_and(|extension| extension == "py" || extension == "pyi")
        })
        .filter(|path| !served.contains(path))
        .map(|path| (relative_text(&path, &root), path))
        .collect();
    unserved.sort();
    let mut hasher = blake3::Hasher::new();
    for (name, path) in &unserved {
        hasher.update(name.as_bytes());
        hasher.update(b"\0");
        hash_file(&mut hasher, path);
    }
    hasher.finalize().to_hex().to_string()
}

/// The `<NAME>_*` environment variables of the engine named `engine` (`tsrs` reads `TSRS_*`), as key
/// text independent of their order.
///
/// Takes [`std::env::vars_os`]: `vars` panics on a variable that is not Unicode. Such a name is in
/// no engine's namespace; such a value is keyed by a digest of its bytes.
pub(super) fn engine_env_text(engine: &str, vars: impl Iterator<Item = (OsString, OsString)>) -> String {
    let prefix = format!("{}_", engine.to_ascii_uppercase().replace('-', "_"));
    let mut found: Vec<(String, String)> = vars
        .filter_map(|(name, value)| {
            let name = name.into_string().ok().filter(|name| name.starts_with(&prefix))?;
            let value = value
                .into_string()
                .unwrap_or_else(|raw| format!("bytes:{}", blake3::hash(raw.as_encoded_bytes()).to_hex()));
            Some((name, value))
        })
        .collect();
    found.sort();
    found
        .iter()
        .map(|(name, value)| format!("{name}={value}"))
        .collect::<Vec<_>>()
        .join("\0")
}

/// `reported` with each path under `root` made relative to it, so an entry stored from one checkout
/// names the files of whichever checkout replays it. A path outside `root` is stored in its absolute
/// form: a relative stored path always means "under the root".
pub(super) fn to_stored<'a>(
    root: &Path,
    reported: &'a [(PathBuf, Vec<Diagnostic>)],
) -> Vec<(PathBuf, &'a [Diagnostic])> {
    let root = absolute(root);
    reported
        .iter()
        .map(|(path, diagnostics)| {
            let path = absolute(path);
            let stored = path.strip_prefix(&root).map(Path::to_path_buf).unwrap_or(path);
            (stored, diagnostics.as_slice())
        })
        .collect()
}

/// The inverse of [`to_stored`]. A relative path that names one of the `served` files comes back
/// spelled as this run spells that file, which is what a check reports and what the per-file results
/// it is merged with are keyed by; any other is joined onto `root`. An absolute path is left as it is.
pub(super) fn from_stored(
    root: &Path,
    served: &[ProjectFile],
    stored: Vec<(PathBuf, Vec<Diagnostic>)>,
) -> Vec<(PathBuf, Vec<Diagnostic>)> {
    let mut spelled: Option<FxHashMap<String, &Path>> = None;
    stored
        .into_iter()
        .map(|(path, diagnostics)| {
            if path.is_absolute() {
                return (path, diagnostics);
            }
            let spelled = spelled.get_or_insert_with(|| {
                served
                    .iter()
                    .map(|file| (relative_text(&file.path, root), file.path.as_path()))
                    .collect()
            });
            let name = path.to_string_lossy().replace('\\', "/");
            let path = spelled
                .get(name.as_str())
                .map_or_else(|| root.join(&path), |served| served.to_path_buf());
            (path, diagnostics)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::engine::PythonEnvironment;
    use crate::language::Language;

    fn python(path: &Path, content: &str) -> ProjectFile {
        ProjectFile {
            path: path.to_path_buf(),
            language: Language::Python,
            content: Arc::from(content),
        }
    }

    /// S3: installing a package into `site-packages` changes the key.
    #[test]
    fn installing_a_package_changes_the_site_packages_fingerprint() {
        let dir = tempfile::tempdir().expect("tempdir");
        let site = dir.path().join("site-packages");
        std::fs::create_dir(&site).expect("site");
        let before = site_packages_fingerprint(&site);
        std::fs::create_dir_all(site.join("foopkg")).expect("pkg");
        std::fs::write(site.join("foopkg/__init__.py"), "X: int = 1\n").expect("module");
        let with_package = site_packages_fingerprint(&site);
        assert_ne!(before, with_package);
        std::fs::create_dir(site.join("foopkg-1.0.dist-info")).expect("dist-info");
        std::fs::write(
            site.join("foopkg-1.0.dist-info/RECORD"),
            "foopkg/__init__.py,sha256=a,11\n",
        )
        .expect("record");
        let recorded = site_packages_fingerprint(&site);
        std::fs::write(
            site.join("foopkg-1.0.dist-info/RECORD"),
            "foopkg/__init__.py,sha256=b,12\n",
        )
        .expect("upgrade");
        assert_ne!(recorded, site_packages_fingerprint(&site), "an upgrade rewrites RECORD");
    }

    /// S4: a module the check can import but was not handed is keyed, and removing it changes the key.
    #[test]
    fn an_unserved_module_is_keyed_and_served_ones_are_not() {
        let dir = tempfile::tempdir().expect("tempdir");
        let served = [python(&dir.path().join("m.py"), "import genmod\n")];
        std::fs::write(dir.path().join("m.py"), "import genmod\n").expect("m");
        std::fs::write(dir.path().join(".gitignore"), "genmod.py\n").expect("gitignore");
        let empty = unserved_python(dir.path(), &served);
        std::fs::write(dir.path().join("genmod.py"), "Y: int = 1\n").expect("genmod");
        let with_module = unserved_python(dir.path(), &served);
        assert_ne!(empty, with_module, "a gitignored module is an input");
        std::fs::write(dir.path().join("m.py"), "import genmod\nprint(genmod.Y)\n").expect("edit served");
        assert_eq!(
            with_module,
            unserved_python(dir.path(), &served),
            "served files are keyed by content"
        );
        std::fs::create_dir_all(dir.path().join(".venv/lib")).expect("venv");
        std::fs::write(dir.path().join(".venv/pyvenv.cfg"), "version = 3.12\n").expect("cfg");
        std::fs::write(dir.path().join(".venv/lib/x.py"), "").expect("venv module");
        assert_eq!(
            with_module,
            unserved_python(dir.path(), &served),
            "a venv is not walked"
        );
        std::fs::create_dir_all(dir.path().join(".worktrees/other")).expect("worktree");
        std::fs::write(dir.path().join(".worktrees/other/.git"), "gitdir: elsewhere\n").expect("git file");
        std::fs::write(dir.path().join(".worktrees/other/w.py"), "").expect("worktree module");
        std::fs::create_dir_all(dir.path().join("env/conda-meta")).expect("conda");
        std::fs::write(dir.path().join("env/c.py"), "").expect("conda module");
        assert_eq!(
            with_module,
            unserved_python(dir.path(), &served),
            "a nested checkout and a conda environment are not walked"
        );
    }

    #[test]
    fn environment_paths_inside_the_checkout_are_relative() {
        let environment = |checkout: &Path| ProjectEnvironment {
            python: Some(PythonEnvironment {
                interpreter: checkout.join(".venv/bin/python"),
                site_packages: vec![checkout.join(".venv/lib/python3.12/site-packages")],
                version: Some("3.12.4".to_owned()),
            }),
        };
        let one = Path::new("/work/one");
        let two = Path::new("/elsewhere/two");
        assert_eq!(
            environment_text(&environment(one), one),
            environment_text(&environment(two), two)
        );
        let outside = std::path::absolute("/usr/bin/python3").expect("absolute");
        let system = ProjectEnvironment {
            python: Some(PythonEnvironment {
                interpreter: outside.clone(),
                ..PythonEnvironment::default()
            }),
        };
        let text = environment_text(&system, one);
        assert!(
            text.contains(&format!("python.interpreter={}\0", outside.display())),
            "{text:?}"
        );
    }

    #[test]
    fn engine_env_text_takes_the_engine_namespace_only() {
        let vars = || {
            [("TSRS_TRACE", "1"), ("PATH", "/bin"), ("TSRSX", "y")]
                .into_iter()
                .map(|(name, value)| (OsString::from(name), OsString::from(value)))
        };
        assert_eq!(engine_env_text("tsrs", vars()), "TSRS_TRACE=1");
        assert_eq!(engine_env_text("python-types", vars()), "");
    }

    /// A variable that is not Unicode is keyed, not a panic.
    #[cfg(unix)]
    #[test]
    fn engine_env_text_keys_a_value_that_is_not_unicode() {
        use std::os::unix::ffi::OsStringExt;
        let vars = |value: &[u8]| {
            [
                (OsString::from("TSRS_PATH"), OsString::from_vec(value.to_vec())),
                (OsString::from_vec(vec![0xff, b'_', b'X']), OsString::from("1")),
            ]
            .into_iter()
        };
        let one = engine_env_text("tsrs", vars(&[0xff, 0x01]));
        assert!(one.starts_with("TSRS_PATH=bytes:"), "{one}");
        assert_ne!(one, engine_env_text("tsrs", vars(&[0xff, 0x02])));
    }

    fn round_trip(
        stored_from: &Path,
        reported: &Path,
        replay_root: &Path,
        served: &[ProjectFile],
    ) -> (PathBuf, PathBuf) {
        let reported = vec![(reported.to_path_buf(), Vec::new())];
        let stored: Vec<(PathBuf, Vec<Diagnostic>)> = to_stored(stored_from, &reported)
            .into_iter()
            .map(|(path, diagnostics)| (path, diagnostics.to_vec()))
            .collect();
        let name = stored[0].0.clone();
        (name, from_stored(replay_root, served, stored).remove(0).0)
    }

    #[test]
    fn stored_findings_are_root_relative_and_restored_under_the_replaying_root() {
        let (stored, replayed) = round_trip(
            Path::new("/a/repo"),
            Path::new("/a/repo/pkg/m.py"),
            Path::new("/b/wt"),
            &[],
        );
        assert_eq!(stored, PathBuf::from("pkg/m.py"));
        assert_eq!(replayed, PathBuf::from("/b/wt/pkg/m.py"));
    }

    /// A replayed finding on a served file is spelled as the run spells that file, as a cold run's is.
    #[test]
    fn a_replayed_finding_takes_the_served_files_spelling() {
        let here = std::env::current_dir().expect("cwd");
        let root = here.parent().unwrap_or(&here).to_path_buf();
        let served_path = Path::new("./m.py");
        let served = [python(served_path, "")];
        let (stored, replayed) = round_trip(&root, served_path, &root, &served);
        assert!(stored.is_relative() && stored.ends_with("m.py"), "{stored:?}");
        assert_eq!(replayed, served_path);
    }

    /// A finding outside the root is not stored as if it were under it.
    #[test]
    fn a_finding_outside_the_root_keeps_its_own_file() {
        let here = std::env::current_dir().expect("cwd");
        let outside = Path::new("elsewhere.py");
        let (stored, replayed) = round_trip(&here.join("project"), outside, &here.join("project"), &[]);
        assert_eq!(stored, here.join("elsewhere.py"));
        assert_eq!(replayed, here.join("elsewhere.py"));
    }
}
