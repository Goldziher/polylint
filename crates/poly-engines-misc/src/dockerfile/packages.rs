//! Package-manager rules over one simple command's words: apt-get, apk, pip and npm.

use poly_engine_api::engine::Severity;

/// A rule match before it is anchored to an instruction span.
pub(super) struct Hit {
    pub code: &'static str,
    pub severity: Severity,
    pub message: String,
}

impl Hit {
    fn new(code: &'static str, severity: Severity, message: impl Into<String>) -> Self {
        Self {
            code,
            severity,
            message: message.into(),
        }
    }
}

/// Splits `args` at the first positional word, skipping flags and the values of `value_flags`.
fn subcommand<'a>(args: &'a [String], value_flags: &[&str]) -> Option<(&'a str, &'a [String])> {
    let mut skip = false;
    for (i, a) in args.iter().enumerate() {
        if skip {
            skip = false;
        } else if a.starts_with('-') {
            skip = value_flags.contains(&a.as_str());
        } else {
            return Some((a.as_str(), &args[i + 1..]));
        }
    }
    None
}

/// Positional words of `args`, skipping flags and the values of `value_flags`.
fn positionals<'a>(args: &'a [String], value_flags: &[&str]) -> Vec<&'a str> {
    let mut out = Vec::new();
    let mut skip = false;
    for a in args {
        if skip {
            skip = false;
        } else if a.starts_with('-') {
            skip = value_flags.contains(&a.as_str());
        } else {
            out.push(a.as_str());
        }
    }
    out
}

fn has_flag(args: &[String], flags: &[&str]) -> bool {
    args.iter().any(|a| flags.contains(&a.as_str()))
}

fn quoted_list(items: &[&str]) -> String {
    items.iter().map(|i| format!("`{i}`")).collect::<Vec<_>>().join(", ")
}

/// Context the package rules need beyond the command itself.
pub(super) struct RunContext<'a> {
    /// The whole `RUN` body, for cleanup and cache-mount checks that span commands.
    pub text: &'a str,
    /// `ENV PIP_NO_CACHE_DIR` was set earlier in the file.
    pub pip_no_cache_env: bool,
}

/// `apt-get` rules: DL3008, DL3014, DL3015.
pub(super) fn apt_get(args: &[String], hits: &mut Vec<Hit>) {
    let Some(("install", rest)) = subcommand(args, &["-o", "-c", "-t", "--target-release", "--option"]) else {
        return;
    };
    let unpinned: Vec<&str> = positionals(rest, &["-o", "-c", "-t", "--target-release", "--option"])
        .into_iter()
        .filter(|p| !p.contains('=') && !p.contains('$') && !p.ends_with(".deb") && !p.starts_with('/'))
        .collect();
    if !unpinned.is_empty() {
        hits.push(Hit::new(
            "DL3008",
            Severity::Warning,
            format!(
                "Pin versions in `apt-get install` (`package=version`): {}.",
                quoted_list(&unpinned)
            ),
        ));
    }
    let assume_yes = args.iter().any(|a| {
        a == "--yes"
            || a == "--assume-yes"
            || a == "-qq"
            || (a.starts_with('-') && !a.starts_with("--") && a.contains('y'))
    });
    if !assume_yes {
        hits.push(Hit::new(
            "DL3014",
            Severity::Warning,
            "Use the `-y` switch to avoid manual input in `apt-get install`.",
        ));
    }
    if !has_flag(args, &["--no-install-recommends"]) && !args.iter().any(|a| a.contains("Install-Recommends=false")) {
        hits.push(Hit::new(
            "DL3015",
            Severity::Info,
            "Avoid additional packages by specifying `--no-install-recommends`.",
        ));
    }
}

/// `apk add` rules: DL3018, DL3019.
pub(super) fn apk(args: &[String], ctx: &RunContext<'_>, hits: &mut Vec<Hit>) {
    const VALUE_FLAGS: &[&str] = &[
        "-t",
        "--virtual",
        "-X",
        "--repository",
        "-p",
        "--root",
        "--keys-dir",
        "--cache-dir",
    ];
    let Some(("add", rest)) = subcommand(args, VALUE_FLAGS) else {
        return;
    };
    let unpinned: Vec<&str> = positionals(rest, VALUE_FLAGS)
        .into_iter()
        .filter(|p| !p.contains(['=', '<', '>', '$']) && !p.ends_with(".apk") && !p.starts_with(['.', '/']))
        .collect();
    if !unpinned.is_empty() {
        hits.push(Hit::new(
            "DL3018",
            Severity::Warning,
            format!(
                "Pin versions in `apk add` (`package=version`): {}.",
                quoted_list(&unpinned)
            ),
        ));
    }
    if !has_flag(rest, &["--no-cache"]) && !ctx.text.contains("/var/cache/apk") {
        hits.push(Hit::new(
            "DL3019",
            Severity::Info,
            "Use `apk add --no-cache` instead of cleaning `/var/cache/apk` manually.",
        ));
    }
}

/// `pip install` rules: DL3013, DL3042.
pub(super) fn pip(args: &[String], ctx: &RunContext<'_>, hits: &mut Vec<Hit>) {
    const VALUE_FLAGS: &[&str] = &[
        "-r",
        "-c",
        "-e",
        "-i",
        "-f",
        "-t",
        "-b",
        "--requirement",
        "--constraint",
        "--editable",
        "--index-url",
        "--extra-index-url",
        "--find-links",
        "--target",
        "--prefix",
        "--root",
        "--platform",
        "--python-version",
        "--implementation",
        "--abi",
        "--src",
        "--build",
        "--cache-dir",
    ];
    let Some(("install", rest)) = subcommand(args, VALUE_FLAGS) else {
        return;
    };
    let unpinned: Vec<&str> = positionals(rest, VALUE_FLAGS)
        .into_iter()
        .filter(|p| !pip_pinned_or_unanalysable(p))
        .collect();
    if !unpinned.is_empty() {
        hits.push(Hit::new(
            "DL3013",
            Severity::Warning,
            format!(
                "Pin versions in `pip install` (`package==version`): {}.",
                quoted_list(&unpinned)
            ),
        ));
    }
    let cache_off = has_flag(rest, &["--no-cache-dir"])
        || ctx.pip_no_cache_env
        || ctx.text.contains("PIP_NO_CACHE_DIR")
        || ctx.text.contains("type=cache");
    if !cache_off {
        hits.push(Hit::new(
            "DL3042",
            Severity::Warning,
            "Avoid the pip cache: use `pip install --no-cache-dir`.",
        ));
    }
}

fn pip_pinned_or_unanalysable(p: &str) -> bool {
    const OPS: &[&str] = &["==", ">=", "<=", "~=", "!=", "<", ">", "@"];
    const ARCHIVES: &[&str] = &[".whl", ".tar.gz", ".tgz", ".zip"];
    OPS.iter().any(|op| p.contains(op))
        || p.contains(['/', '$'])
        || p.starts_with(['.', '~'])
        || ARCHIVES.iter().any(|e| p.ends_with(e))
}

/// `npm install` rule: DL3016.
pub(super) fn npm(args: &[String], hits: &mut Vec<Hit>) {
    const VALUE_FLAGS: &[&str] = &["--prefix", "--registry", "--cache", "--userconfig", "--tag"];
    let Some((sub, rest)) = subcommand(args, VALUE_FLAGS) else {
        return;
    };
    if !matches!(sub, "install" | "i" | "add") {
        return;
    }
    let unpinned: Vec<&str> = positionals(rest, VALUE_FLAGS)
        .into_iter()
        .filter(|p| npm_unpinned(p))
        .collect();
    if !unpinned.is_empty() {
        hits.push(Hit::new(
            "DL3016",
            Severity::Warning,
            format!(
                "Pin versions in `npm install` (`package@version`): {}.",
                quoted_list(&unpinned)
            ),
        ));
    }
}

fn npm_unpinned(p: &str) -> bool {
    const ARCHIVES: &[&str] = &[".tgz", ".tar.gz"];
    if p.starts_with(['.', '/', '$', '~'])
        || p.starts_with("file:")
        || p.starts_with("git+")
        || p.contains("://")
        || ARCHIVES.iter().any(|e| p.ends_with(e))
    {
        return false;
    }
    let scoped = p.starts_with('@');
    if !scoped && p.contains('/') {
        return false;
    }
    !p.strip_prefix('@').unwrap_or(p).contains('@')
}
