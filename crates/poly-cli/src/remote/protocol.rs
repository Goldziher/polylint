//! Which git transports a remote `extends` base or hook source may be fetched over.
//!
//! The URL comes from a config file, which may itself be an untrusted base or a cloned
//! repository's own `poly.toml`. `git://` and `http://` are unauthenticated and unencrypted,
//! `file://` reads the local disk and `ext::` runs a command, so by default only `https`, `ssh`
//! (including scp-like `host:path`) and plain local paths are fetched. `POLY_GIT_ALLOW_PROTOCOLS`
//! (a `:`-separated list, passed through as git's `GIT_ALLOW_PROTOCOL`) opts into others.

use std::process::Command;

use anyhow::bail;

/// Environment variable that replaces the default allowed transports, or extends them when the
/// value starts with `+` (`+http:git`).
pub const ALLOW_PROTOCOLS_ENV: &str = "POLY_GIT_ALLOW_PROTOCOLS";

/// Transports allowed when [`ALLOW_PROTOCOLS_ENV`] is unset.
const DEFAULT_ALLOWED: &str = "https:ssh";

/// The transport git would pick for a URL.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Transport {
    /// `https://`.
    Https,
    /// `ssh://` or scp-like `[user@]host:path`.
    Ssh,
    /// A filesystem path with no scheme.
    Local,
    /// Any other scheme or remote helper (`git`, `http`, `file`, `ext`, ...).
    Other(String),
}

/// Classify `url` the way git does: `scheme://`, then `helper::`, then scp-like, else a path.
pub fn classify(url: &str) -> Transport {
    if let Some((scheme, _)) = url.split_once("://") {
        return match scheme.to_ascii_lowercase().as_str() {
            "https" => Transport::Https,
            "ssh" => Transport::Ssh,
            other => Transport::Other(other.to_owned()),
        };
    }
    let first_colon = url.find(':');
    let first_slash = url.find('/');
    match first_colon {
        Some(colon) if first_slash.is_none_or(|slash| colon < slash) => {
            if url[colon..].starts_with("::") {
                return Transport::Other(url[..colon].to_ascii_lowercase());
            }
            // `C:\repo` and `C:/repo` are drive paths, not hosts.
            if colon == 1 && url.as_bytes()[0].is_ascii_alphabetic() {
                return Transport::Local;
            }
            Transport::Ssh
        }
        _ => Transport::Local,
    }
}

fn protocol_name(transport: &Transport) -> Option<&str> {
    match transport {
        Transport::Https => Some("https"),
        Transport::Ssh => Some("ssh"),
        Transport::Local => None,
        Transport::Other(name) => Some(name),
    }
}

/// The allow-list an [`ALLOW_PROTOCOLS_ENV`] value names: a leading `+` adds its entries to the
/// defaults, anything else replaces them.
fn expand(opt_in: Option<&str>) -> Option<String> {
    opt_in.map(|value| match value.strip_prefix('+') {
        Some(extra) => format!("{DEFAULT_ALLOWED}:{}", extra.trim_matches(':')),
        None => value.to_owned(),
    })
}

/// Reject `url` unless its transport is allowed, with `opt_in` the value of
/// [`ALLOW_PROTOCOLS_ENV`] when set.
pub fn check_url_with(url: &str, opt_in: Option<&str>) -> anyhow::Result<()> {
    if url.starts_with('-') {
        bail!("git source URL {url:?} must not start with `-`");
    }
    let expanded = expand(opt_in);
    let allowed = expanded.as_deref().unwrap_or(DEFAULT_ALLOWED);
    let Some(name) = protocol_name(&classify(url)).map(str::to_owned) else {
        return Ok(());
    };
    if allowed.split(':').any(|entry| entry.eq_ignore_ascii_case(&name)) {
        return Ok(());
    }
    bail!(
        "git source URL {url:?} uses the `{name}` transport, which is not allowed; allowed: {}. \
         Use an https or ssh URL, or set {ALLOW_PROTOCOLS_ENV} (for example `{ALLOW_PROTOCOLS_ENV}={DEFAULT_ALLOWED}:{name}`) to opt in",
        allowed.replace(':', ", ")
    );
}

fn opt_in_from_env() -> Option<String> {
    std::env::var(ALLOW_PROTOCOLS_ENV)
        .ok()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

/// [`check_url_with`] against the process environment.
pub fn check_url(url: &str) -> anyhow::Result<()> {
    check_url_with(url, opt_in_from_env().as_deref())
}

/// The `GIT_ALLOW_PROTOCOL` value and whether `protocol.file.allow=never` applies for `url`.
///
/// A plain local path is the file transport, so it is the one case where `file` is let through;
/// every other URL gets the allow-list alone, plus `file` refused outright by default.
pub fn git_policy(url: &str, opt_in: Option<&str>) -> (String, bool) {
    let expanded = expand(opt_in);
    let list = expanded.as_deref().unwrap_or(DEFAULT_ALLOWED);
    if classify(url) == Transport::Local {
        return (format!("{list}:file"), false);
    }
    (list.to_owned(), opt_in.is_none())
}

/// Apply the transport policy for `url` to a git command that will contact it.
///
/// `GIT_ALLOW_PROTOCOL` also covers redirects and submodules, which the URL check alone does not.
pub fn restrict(command: &mut Command, url: &str) {
    let (list, deny_file) = git_policy(url, opt_in_from_env().as_deref());
    command.env("GIT_ALLOW_PROTOCOL", list);
    if deny_file {
        command.args(["-c", "protocol.file.allow=never"]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_leading_plus_extends_the_defaults_instead_of_replacing_them() {
        assert!(check_url_with("http://example.com/a.git", Some("+http:git")).is_ok());
        assert!(check_url_with("git://example.com/a.git", Some("+http:git")).is_ok());
        assert!(check_url_with("https://example.com/a.git", Some("+http")).is_ok());
        assert!(check_url_with("ssh://git@example.com/a.git", Some("+http")).is_ok());
        assert!(check_url_with("ext::sh -c id", Some("+http")).is_err());
        assert!(check_url_with("https://example.com/a.git", Some("http")).is_err());
        assert_eq!(git_policy("https://x/y", Some("+http")).0, "https:ssh:http");
    }

    #[test]
    fn classifies_common_url_shapes() {
        assert_eq!(classify("https://github.com/a/b.git"), Transport::Https);
        assert_eq!(classify("ssh://git@github.com/a/b.git"), Transport::Ssh);
        assert_eq!(classify("git@github.com:a/b.git"), Transport::Ssh);
        assert_eq!(classify("github.com:a/b.git"), Transport::Ssh);
        assert_eq!(classify("git://example.com/a.git"), Transport::Other("git".into()));
        assert_eq!(classify("HTTP://example.com/a.git"), Transport::Other("http".into()));
        assert_eq!(classify("file:///srv/repo"), Transport::Other("file".into()));
        assert_eq!(classify("ext::sh -c 'id'"), Transport::Other("ext".into()));
        assert_eq!(classify("/srv/repo"), Transport::Local);
        assert_eq!(classify("../repo"), Transport::Local);
        assert_eq!(classify("./a:b/repo"), Transport::Local);
        assert_eq!(classify(r"C:\repo"), Transport::Local);
    }

    #[test]
    fn defaults_accept_https_ssh_scp_and_local() {
        for url in [
            "https://github.com/a/b.git",
            "ssh://git@github.com/a/b.git",
            "git@github.com:a/b.git",
            "/srv/repo",
        ] {
            check_url_with(url, None).unwrap_or_else(|error| panic!("{url}: {error:#}"));
        }
    }

    #[test]
    fn defaults_reject_git_http_file_and_ext() {
        for url in [
            "git://example.com/a.git",
            "http://example.com/a.git",
            "file:///srv/repo",
            "ext::sh -c id",
            "-oProxyCommand=id",
        ] {
            assert!(check_url_with(url, None).is_err(), "{url}");
        }
        let message = check_url_with("git://example.com/a.git", None).unwrap_err().to_string();
        assert!(
            message.contains("https, ssh") && message.contains(ALLOW_PROTOCOLS_ENV),
            "{message}"
        );
    }

    #[test]
    fn opt_in_replaces_the_allow_list() {
        check_url_with("git://example.com/a.git", Some("https:ssh:git")).unwrap();
        assert!(check_url_with("http://example.com/a.git", Some("https:ssh:git")).is_err());
        assert!(check_url_with("ssh://h/a.git", Some("https")).is_err());
    }

    #[test]
    fn policy_lets_file_through_only_for_local_paths() {
        assert_eq!(git_policy("https://h/a.git", None), ("https:ssh".to_owned(), true));
        assert_eq!(git_policy("/srv/repo", None), ("https:ssh:file".to_owned(), false));
        assert_eq!(git_policy("git://h/a", Some("git")), ("git".to_owned(), false));
    }
}
