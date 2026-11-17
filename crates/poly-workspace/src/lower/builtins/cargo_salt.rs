//! What besides the declared input files decides a cargo builtin's verdict, folded into its result-cache key.
//!
//! The group is cached on its declared inputs (sources, manifests, the lockfile, the toolchain pin, the clippy and
//! cargo config files). That misses what lives outside the tracked tree:
//!
//! - **the toolchain and tool versions**: a `rustup update` adds clippy lints, a new `cargo-machete` reports more.
//!   `rustc -vV` and `cargo -V` (run from the repository root, so a `rust-toolchain.toml` selects the same toolchain
//!   the hook will use) plus the tool's own `--version`;
//! - **the environment cargo reads**: `RUSTFLAGS`, `CARGO_BUILD_*`, `CARGO_PROFILE_*`, `CLIPPY_CONF_DIR`, …
//!   ([`relevant_env`]);
//! - **config files the tracked listing cannot see**: an untracked `clippy.toml`, `.cargo/config.toml` or
//!   `tomlfmt.toml` (cargo-sort's) in the tree, one in a directory above the repository (cargo and clippy both search
//!   upward) and `$CARGO_HOME/config.toml`.
//!
//! The versions cost a process spawn each, once per repository root per process and only for a cached group: the two
//! shared probes run concurrently, and a tool's own version is probed the first time that tool's salt is asked for.

use std::collections::HashMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Mutex, OnceLock, PoisonError};

/// The config files cargo, clippy and cargo-sort read, wherever they are found.
const CONFIG_NAMES: &[&str] = &[
    "clippy.toml",
    ".clippy.toml",
    ".cargo/config.toml",
    ".cargo/config",
    "tomlfmt.toml",
    ".tomlfmt.toml",
];

/// The version probe for each cached cargo builtin, by hook id.
const TOOL_VERSIONS: &[(&str, &[&str])] = &[
    ("cargo-clippy", &["cargo", "clippy", "-V"]),
    ("cargo-sort", &["cargo-sort", "--version"]),
    ("cargo-machete", &["cargo-machete", "--version"]),
];

/// Probes shared by every tool of the group.
const SHARED_PROBES: &[&[&str]] = &[&["rustc", "-vV"], &["cargo", "-V"]];

/// Per repository root: the shared part, and each tool's salt once asked for.
type Salts = Mutex<HashMap<PathBuf, (String, HashMap<&'static str, String>)>>;

/// The cache salt of the cargo builtin `tool` for the repository at `root`: the shared part, then the tool's own
/// version. An id that is not a cached cargo builtin gets the shared part alone.
pub(super) fn salt(root: &Path, tool: &str) -> String {
    static SALTS: OnceLock<Salts> = OnceLock::new();
    let salts = SALTS.get_or_init(Salts::default);
    let mut salts = salts.lock().unwrap_or_else(PoisonError::into_inner);
    let (shared, by_tool) = salts
        .entry(root.to_path_buf())
        .or_insert_with(|| (shared_salt(root), HashMap::new()));
    let Some((id, argv)) = TOOL_VERSIONS.iter().find(|(id, _)| *id == tool) else {
        return shared.clone();
    };
    by_tool
        .entry(*id)
        .or_insert_with(|| format!("{shared}|{id}={}", probe(root, argv).trim()))
        .clone()
}

/// What every tool of the group depends on: the toolchain, the environment and the config files.
fn shared_salt(root: &Path) -> String {
    let versions: Vec<String> = std::thread::scope(|scope| {
        let handles: Vec<_> = SHARED_PROBES
            .iter()
            .map(|argv| scope.spawn(move || probe(root, argv)))
            .collect();
        handles
            .into_iter()
            .map(|handle| handle.join().unwrap_or_else(|_| "panicked".to_owned()))
            .collect()
    });
    let mut hasher = blake3::Hasher::new();
    for version in &versions {
        feed(&mut hasher, version.as_bytes());
    }
    feed(&mut hasher, env_digest(std::env::vars_os()).as_bytes());
    feed(&mut hasher, config_digest(root).as_bytes());
    hasher.finalize().to_hex().to_string()
}

fn feed(hasher: &mut blake3::Hasher, part: &[u8]) {
    hasher.update(&(part.len() as u64).to_le_bytes());
    hasher.update(part);
}

/// The output of `argv` run from `root`, or a marker when it cannot run (an absent tool keys apart from a present one).
fn probe(root: &Path, argv: &[&str]) -> String {
    let Some((program, args)) = argv.split_first() else {
        return String::new();
    };
    match Command::new(program)
        .args(args)
        .current_dir(root)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
    {
        Ok(output) if output.status.success() => String::from_utf8_lossy(&output.stdout).into_owned(),
        Ok(output) => format!("failed:{}", output.status),
        Err(_) => "absent".to_owned(),
    }
}

/// Whether the environment variable `name` changes what cargo, rustc or clippy report.
///
/// `CARGO_*` is cargo's config-by-environment namespace, less what changes no verdict: how output looks
/// (`CARGO_TERM_*`), where cargo lives (`CARGO_HOME`, whose config is keyed by content instead) or builds
/// (`CARGO_TARGET_DIR`, which a worktree may set for itself), how much it builds at once, and what cargo itself sets
/// for the processes it runs (`CARGO`, `CARGO_PKG_*`, `CARGO_MANIFEST_*`, …), which describe the caller rather than
/// configure the hook.
fn relevant_env(name: &str) -> bool {
    const NOT_AN_INPUT: &[&str] = &[
        "CARGO_PKG_",
        "CARGO_MANIFEST_",
        "CARGO_CRATE_",
        "CARGO_BIN_",
        "CARGO_PRIMARY_PACKAGE",
        "CARGO_TARGET_TMPDIR",
        "CARGO_TERM_",
        "CARGO_MAKEFLAGS",
    ];
    const NOT_AN_INPUT_EXACT: &[&str] = &[
        "CARGO_HOME",
        "CARGO_TARGET_DIR",
        "CARGO_BUILD_TARGET_DIR",
        "CARGO_BUILD_JOBS",
        "CARGO_INCREMENTAL",
        "CARGO_BUILD_INCREMENTAL",
    ];
    if let Some(rest) = name.strip_prefix("CARGO_") {
        return !rest.is_empty()
            && !NOT_AN_INPUT_EXACT.contains(&name)
            && !NOT_AN_INPUT.iter().any(|set| name.starts_with(set));
    }
    matches!(
        name,
        "RUSTFLAGS" | "RUSTDOCFLAGS" | "RUSTC" | "RUSTC_BOOTSTRAP" | "RUSTUP_TOOLCHAIN"
    ) || name.starts_with("CLIPPY_")
}

/// A digest of the relevant variables in `vars`, independent of their order. Takes [`std::env::vars_os`]: `vars`
/// panics on a variable that is not Unicode. A name that is not Unicode is none of cargo's; a value is hashed as bytes.
fn env_digest(vars: impl Iterator<Item = (OsString, OsString)>) -> String {
    let mut relevant: Vec<(String, OsString)> = vars
        .filter_map(|(name, value)| Some((name.into_string().ok().filter(|name| relevant_env(name))?, value)))
        .collect();
    relevant.sort();
    let mut hasher = blake3::Hasher::new();
    for (name, value) in &relevant {
        feed(&mut hasher, name.as_bytes());
        feed(&mut hasher, value.as_encoded_bytes());
    }
    hasher.finalize().to_hex().to_string()
}

/// A digest of every clippy and cargo config file that can govern the repository at `root`: in the tree, tracked or
/// not (but not ignored); in each directory above it; and in `$CARGO_HOME`. Each is named by where it sits relative
/// to `root`, never by absolute path, so two checkouts key alike.
fn config_digest(root: &Path) -> String {
    let mut found: Vec<(String, PathBuf)> = in_tree_configs(root)
        .into_iter()
        .map(|relative| (relative.to_string_lossy().replace('\\', "/"), root.join(&relative)))
        .collect();
    for (depth, dir) in root.ancestors().enumerate().skip(1) {
        for name in CONFIG_NAMES {
            let candidate = dir.join(name);
            if candidate.is_file() {
                found.push((format!("{}{name}", "../".repeat(depth)), candidate));
            }
        }
    }
    if let Some(home) = cargo_home() {
        for name in ["config.toml", "config"] {
            let candidate = home.join(name);
            if candidate.is_file() {
                found.push((format!("$CARGO_HOME/{name}"), candidate));
            }
        }
    }
    found.sort();
    let mut hasher = blake3::Hasher::new();
    for (label, path) in &found {
        feed(&mut hasher, label.as_bytes());
        feed(&mut hasher, &std::fs::read(path).unwrap_or_default());
    }
    hasher.finalize().to_hex().to_string()
}

/// The config files under `root`, tracked or untracked but not ignored, relative to `root`. Without git, only `root`
/// itself is looked at.
fn in_tree_configs(root: &Path) -> Vec<PathBuf> {
    let pathspecs = CONFIG_NAMES.iter().map(|name| format!(":(glob)**/{name}"));
    let listed = Command::new("git")
        .args(["ls-files", "-z", "--cached", "--others", "--exclude-standard", "--"])
        .args(pathspecs)
        .current_dir(root)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output();
    match listed {
        Ok(output) if output.status.success() => {
            let mut paths: Vec<PathBuf> = output
                .stdout
                .split(|byte| *byte == 0)
                .filter(|entry| !entry.is_empty())
                .map(|entry| PathBuf::from(String::from_utf8_lossy(entry).into_owned()))
                .collect();
            paths.sort();
            paths.dedup();
            paths
        }
        _ => CONFIG_NAMES
            .iter()
            .map(PathBuf::from)
            .filter(|name| root.join(name).is_file())
            .collect(),
    }
}

fn cargo_home() -> Option<PathBuf> {
    std::env::var_os("CARGO_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".cargo")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn git(dir: &Path, args: &[&str]) {
        let status = Command::new("git")
            .args(args)
            .current_dir(dir)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .expect("git runs");
        assert!(status.success(), "git {args:?}");
    }

    /// The audit's S7 case: an untracked `clippy.toml` changes what clippy reports, so it must change the key.
    #[test]
    fn an_untracked_clippy_toml_changes_the_config_digest() {
        let dir = tempfile::tempdir().expect("tempdir");
        git(dir.path(), &["init", "-q"]);
        std::fs::write(dir.path().join(".gitignore"), "target/\n").expect("gitignore");
        let before = config_digest(dir.path());
        std::fs::write(dir.path().join("clippy.toml"), "too-many-arguments-threshold = 2\n").expect("clippy.toml");
        let with_clippy = config_digest(dir.path());
        assert_ne!(before, with_clippy);
        std::fs::create_dir_all(dir.path().join("crates/a/.cargo")).expect("mkdir");
        std::fs::write(
            dir.path().join("crates/a/.cargo/config.toml"),
            "[build]\nrustflags = []\n",
        )
        .expect("config");
        assert_ne!(
            with_clippy,
            config_digest(dir.path()),
            "a nested .cargo/config.toml counts"
        );
        std::fs::create_dir_all(dir.path().join("target")).expect("target");
        let settled = config_digest(dir.path());
        std::fs::write(dir.path().join("target/clippy.toml"), "x = 1\n").expect("ignored");
        assert_eq!(settled, config_digest(dir.path()), "an ignored file is not an input");
    }

    #[test]
    fn the_env_digest_follows_cargo_configuration_only() {
        let digest = |vars: &[(&str, &str)]| {
            env_digest(
                vars.iter()
                    .map(|(name, value)| (OsString::from(name), OsString::from(value))),
            )
        };
        let base = digest(&[("PATH", "/bin")]);
        assert_ne!(base, digest(&[("PATH", "/bin"), ("RUSTFLAGS", "-Dwarnings")]));
        assert_ne!(base, digest(&[("CARGO_BUILD_TARGET", "x86_64-unknown-linux-musl")]));
        assert_ne!(base, digest(&[("CLIPPY_CONF_DIR", "conf")]));
        assert_eq!(base, digest(&[("CARGO_PKG_NAME", "x"), ("CARGO_TERM_COLOR", "always")]));
        assert_eq!(base, digest(&[("PATH", "/usr/bin"), ("HOME", "/root")]));
        assert_eq!(
            base,
            digest(&[
                ("CARGO_TARGET_DIR", "/elsewhere/target"),
                ("CARGO_BUILD_JOBS", "2"),
                ("CARGO_INCREMENTAL", "0"),
                ("CARGO", "/toolchain/bin/cargo"),
            ]),
            "where and how fast cargo builds changes no verdict"
        );
    }

    /// A variable that is not Unicode is hashed, not a panic.
    #[cfg(unix)]
    #[test]
    fn the_env_digest_takes_a_value_that_is_not_unicode() {
        use std::os::unix::ffi::OsStringExt;
        let digest = |value: &[u8]| {
            env_digest(
                [
                    (OsString::from("RUSTFLAGS"), OsString::from_vec(value.to_vec())),
                    (OsString::from_vec(vec![0xff]), OsString::from("x")),
                ]
                .into_iter(),
            )
        };
        assert_ne!(digest(&[0xff, 0x01]), digest(&[0xff, 0x02]));
    }

    #[test]
    fn a_tool_salt_is_the_shared_part_and_the_tools_own_version() {
        let dir = tempfile::tempdir().expect("tempdir");
        let shared = salt(dir.path(), "");
        assert_eq!(shared.len(), blake3::OUT_LEN * 2, "{shared}");
        for id in ["cargo-clippy", "cargo-sort"] {
            let own = salt(dir.path(), id);
            assert!(own.starts_with(&format!("{shared}|{id}=")), "{own}");
            assert_eq!(own, salt(dir.path(), id), "memoised");
        }
    }

    /// cargo-sort's config is an input: `tomlfmt.toml` changes what it accepts.
    #[test]
    fn a_tomlfmt_config_changes_the_config_digest() {
        let dir = tempfile::tempdir().expect("tempdir");
        git(dir.path(), &["init", "-q"]);
        let before = config_digest(dir.path());
        std::fs::write(dir.path().join("tomlfmt.toml"), "table_order = [\"package\"]\n").expect("tomlfmt");
        assert_ne!(before, config_digest(dir.path()));
    }
}
