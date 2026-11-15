//! Which hosts a locked download may come from.
//!
//! `poly.lock` is a file a pull request can change on its own, so its asset URLs
//! are not trusted by themselves: a locked URL is used only when it is https and
//! its origin is one the tool's catalog entry already names.

use std::collections::BTreeMap;

use poly_catalog::{Release, Source};

use super::resolve::{Origin, API_ENV};

const GITHUB_RELEASES: &str = "https://github.com/";

/// Hosts a GitHub release download is served from after its redirect.
const GITHUB_CDN_HOSTS: [&str; 4] = [
    "https://github.com",
    "https://objects.githubusercontent.com",
    "https://release-assets.githubusercontent.com",
    "https://github-releases.githubusercontent.com",
];

/// An origin and the path prefix a download URL must sit under.
#[derive(Debug, PartialEq, Eq)]
struct Allowed {
    origin: Origin,
    prefix: String,
    /// GitHub treats owner and repo names case-insensitively.
    fold_case: bool,
}

/// Check that the locked `url` of `tool` at `version` may be downloaded.
///
/// The allowed locations are the `releases/download/` path of the catalog's
/// repository for a GitHub source, the declared download base of a URL source,
/// the directories of the catalog's seeded releases, and the host named by
/// `POLY_GITHUB_API_URL` when the user set it.
///
/// # Errors
///
/// Returns a message pointing at `poly lock` when the URL is not https, carries
/// userinfo, or sits outside every location the catalog allows.
pub(super) fn check_locked_url(
    tool: &str,
    url: &str,
    version: &str,
    source: Option<&Source>,
    seeds: Option<&BTreeMap<String, Release>>,
) -> Result<(), String> {
    let allowed = allowed_origins(version, source, seeds, std::env::var(API_ENV).ok().as_deref());
    check(tool, url, &allowed)
}

fn check(tool: &str, url: &str, allowed: &[Allowed]) -> Result<(), String> {
    match Origin::of(url) {
        Some(origin)
            if origin.is_https() && clean_path(url) && allowed.iter().any(|entry| entry.admits(&origin, url)) =>
        {
            Ok(())
        }
        _ => Err(format!(
            "poly.lock pins tool `{tool}` to {url}, which is not https or not hosted where the catalog publishes \
             it; refusing to download it. Run `poly lock --upgrade` to regenerate the lock, or set `url` and \
             `sha256` in [tools.{tool}] to trust another host"
        )),
    }
}

impl Allowed {
    fn new(url: &str, prefix: String, fold_case: bool) -> Option<Allowed> {
        Some(Allowed {
            origin: Origin::of(url)?,
            prefix,
            fold_case,
        })
    }

    fn admits(&self, origin: &Origin, url: &str) -> bool {
        if &self.origin != origin {
            return false;
        }
        let path = path_of(url);
        if self.fold_case {
            path.to_ascii_lowercase().starts_with(&self.prefix.to_ascii_lowercase())
        } else {
            path.starts_with(&self.prefix)
        }
    }
}

/// The path of `url`, without query or fragment; `/` when there is none.
fn path_of(url: &str) -> &str {
    let rest = url.split_once("://").map_or(url, |(_, rest)| rest);
    let path = rest.find('/').map_or("/", |at| &rest[at..]);
    path.split(['?', '#']).next().unwrap_or("/")
}

/// Whether the path has no dot segment or encoded separator that could climb out of a prefix.
fn clean_path(url: &str) -> bool {
    let path = path_of(url);
    let lowered = path.to_ascii_lowercase();
    !path.contains('\\')
        && !lowered.contains("%2e")
        && !lowered.contains("%2f")
        && !lowered.contains("%5c")
        && path.split('/').all(|segment| segment != ".." && segment != ".")
}

fn with_slash(path: &str) -> String {
    if path.ends_with('/') {
        path.to_owned()
    } else {
        format!("{path}/")
    }
}

fn allowed_origins(
    version: &str,
    source: Option<&Source>,
    seeds: Option<&BTreeMap<String, Release>>,
    api_override: Option<&str>,
) -> Vec<Allowed> {
    let mut allowed = Vec::new();
    if let Some(source) = source {
        if let Some(repo) = &source.github {
            allowed.extend(Allowed::new(
                GITHUB_RELEASES,
                format!("/{}/releases/download/", repo.trim_matches('/')),
                true,
            ));
        } else if let Some(base) = source.download_base(version) {
            allowed.extend(Allowed::new(&base, with_slash(path_of(&base)), false));
        }
    }
    for release in seeds.into_iter().flat_map(BTreeMap::values) {
        for asset in release.assets.values() {
            let directory = path_of(&asset.url).rsplit_once('/').map_or("/", |(head, _)| head);
            allowed.extend(Allowed::new(&asset.url, with_slash(directory), false));
        }
    }
    allowed.extend(api_override.and_then(|url| Allowed::new(url, "/".to_owned(), false)));
    allowed
}

/// Whether a download redirect from `from` to `to` may be followed: https only,
/// and within GitHub's release hosts when `from` is one, else the same origin.
///
/// A plain-http `from` (a local test server) may only stay on its own origin.
pub(super) fn redirect_permitted(from: &str, to: &str) -> bool {
    let (Some(source), Some(target)) = (Origin::of(from), Origin::of(to)) else {
        return false;
    };
    if source == target {
        return true;
    }
    target.is_https()
        && source.is_https()
        && GITHUB_CDN_HOSTS
            .iter()
            .filter_map(|host| Origin::of(host))
            .any(|host| host == source)
        && GITHUB_CDN_HOSTS
            .iter()
            .filter_map(|host| Origin::of(host))
            .any(|host| host == target)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn github() -> Vec<Allowed> {
        vec![Allowed::new(GITHUB_RELEASES, "/o/r/releases/download/".to_owned(), true).expect("origin")]
    }

    #[test]
    fn a_matching_https_origin_is_accepted() {
        assert!(check("t", "https://github.com/o/r/releases/download/v1/t.zip", &github()).is_ok());
        assert!(check("t", "https://GitHub.com:443/o/r/releases/download/v1/t.zip", &github()).is_ok());
    }

    #[test]
    fn owner_and_repo_match_case_insensitively() {
        assert!(check("t", "https://github.com/O/R/releases/download/v1/t.zip", &github()).is_ok());
    }

    #[test]
    fn a_poisoned_lock_path_on_the_right_host_is_refused() {
        for url in [
            "https://github.com/attacker/x/releases/download/v1/t.zip",
            "https://github.com/o/r2/releases/download/v1/t.zip",
            "https://github.com/o/r/archive/v1.zip",
            "https://github.com/o/r/releases/download/../../../attacker/x/t.zip",
            "https://github.com/o/r/releases/download/%2e%2e/%2e%2e/x/t.zip",
        ] {
            assert!(check("t", url, &github()).is_err(), "{url}");
        }
    }

    #[test]
    fn a_foreign_host_is_refused_with_the_lock_hint() {
        let error = check("t", "https://attacker.example/t.zip", &github()).expect_err("foreign");
        assert!(
            error.contains("poly lock") && error.contains("attacker.example"),
            "{error}"
        );
        assert!(check(
            "t",
            "https://github.com.evil.example/o/r/releases/download/t.zip",
            &github()
        )
        .is_err());
        assert!(check("t", "https://github.com:8443/o/r/releases/download/t.zip", &github()).is_err());
    }

    #[test]
    fn http_and_userinfo_are_refused_even_on_an_allowed_host() {
        assert!(check("t", "http://github.com/o/r/releases/download/t.zip", &github()).is_err());
        assert!(check("t", "https://user@github.com/o/r/releases/download/t.zip", &github()).is_err());
        assert!(check("t", "file:///tmp/t.zip", &github()).is_err());
    }

    #[test]
    fn an_empty_allow_list_refuses_everything() {
        assert!(check("t", "https://github.com/t.zip", &[]).is_err());
    }

    #[test]
    fn a_url_source_allows_only_its_declared_host_and_path() {
        let allowed = vec![Allowed::new("https://get.helm.sh/dl", "/dl/".to_owned(), false).expect("origin")];
        assert!(check("helm", "https://get.helm.sh/dl/helm-v1.tar.gz", &allowed).is_ok());
        assert!(check("helm", "https://get.helm.sh/other/helm-v1.tar.gz", &allowed).is_err());
        assert!(check("helm", "https://get.helm.sh/DL/helm-v1.tar.gz", &allowed).is_err());
        assert!(check("helm", "https://github.com/helm.tar.gz", &allowed).is_err());
    }

    #[test]
    fn the_api_override_host_is_allowed_only_when_set() {
        let origins = allowed_origins("1.0.0", None, None, Some("https://ghe.example/api/v3"));
        assert!(check("t", "https://ghe.example/o/r/t.zip", &origins).is_ok());
        assert!(check(
            "t",
            "https://ghe.example/t.zip",
            &allowed_origins("1.0.0", None, None, None)
        )
        .is_err());
    }

    #[test]
    fn redirects_stay_on_github_hosts_or_the_same_origin() {
        let from = "https://github.com/o/r/releases/download/v1/t.zip";
        assert!(redirect_permitted(from, "https://objects.githubusercontent.com/x"));
        assert!(redirect_permitted(
            from,
            "https://release-assets.githubusercontent.com/x"
        ));
        assert!(!redirect_permitted(from, "https://attacker.example/x"));
        assert!(!redirect_permitted(from, "http://objects.githubusercontent.com/x"));
        assert!(redirect_permitted("https://get.helm.sh/a", "https://get.helm.sh/b"));
        assert!(!redirect_permitted(
            "https://get.helm.sh/a",
            "https://objects.githubusercontent.com/b"
        ));
        assert!(!redirect_permitted("https://get.helm.sh/a", "http://get.helm.sh/b"));
    }
}
