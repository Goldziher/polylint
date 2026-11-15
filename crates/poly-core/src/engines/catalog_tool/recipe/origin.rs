//! Which hosts a locked download may come from.
//!
//! `poly.lock` is a file a pull request can change on its own, so its asset URLs
//! are not trusted by themselves: a locked URL is used only when it is https and
//! its origin is one the tool's catalog entry already names.

use std::collections::BTreeMap;

use poly_catalog::{Release, Source};

use super::resolve::{API_ENV, Origin};

const GITHUB_RELEASES: &str = "https://github.com/";

/// Check that the locked `url` of `tool` at `version` may be downloaded.
///
/// The allowed origins are the GitHub release host for a GitHub source, the
/// declared download host of a URL source, the hosts of the catalog's seeded
/// releases, and the host named by `POLY_GITHUB_API_URL` when the user set it.
///
/// # Errors
///
/// Returns a message pointing at `poly lock` when the URL is not https, carries
/// userinfo, or sits on an origin the catalog does not allow.
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

fn check(tool: &str, url: &str, allowed: &[Origin]) -> Result<(), String> {
    match Origin::of(url) {
        Some(origin) if origin.is_https() && allowed.contains(&origin) => Ok(()),
        _ => Err(format!(
            "poly.lock pins tool `{tool}` to {url}, which is not https or not hosted where the catalog publishes \
             it; refusing to download it. Run `poly lock --upgrade` to regenerate the lock, or set `url` and \
             `sha256` in [tools.{tool}] to trust another host"
        )),
    }
}

fn allowed_origins(
    version: &str,
    source: Option<&Source>,
    seeds: Option<&BTreeMap<String, Release>>,
    api_override: Option<&str>,
) -> Vec<Origin> {
    let mut origins = Vec::new();
    if let Some(source) = source {
        if source.github.is_some() {
            origins.extend(Origin::of(GITHUB_RELEASES));
        }
        origins.extend(source.download_base(version).as_deref().and_then(Origin::of));
    }
    for release in seeds.into_iter().flat_map(BTreeMap::values) {
        origins.extend(release.assets.values().filter_map(|asset| Origin::of(&asset.url)));
    }
    origins.extend(api_override.and_then(Origin::of));
    origins
}

#[cfg(test)]
mod tests {
    use super::*;

    fn github() -> Vec<Origin> {
        vec![Origin::of(GITHUB_RELEASES).expect("origin")]
    }

    #[test]
    fn a_matching_https_origin_is_accepted() {
        assert!(check("t", "https://github.com/o/r/releases/download/v1/t.zip", &github()).is_ok());
        assert!(check("t", "https://GitHub.com:443/o/r/t.zip", &github()).is_ok());
    }

    #[test]
    fn a_foreign_host_is_refused_with_the_lock_hint() {
        let error = check("t", "https://attacker.example/t.zip", &github()).expect_err("foreign");
        assert!(
            error.contains("poly lock") && error.contains("attacker.example"),
            "{error}"
        );
        assert!(check("t", "https://github.com.evil.example/t.zip", &github()).is_err());
        assert!(check("t", "https://github.com:8443/t.zip", &github()).is_err());
    }

    #[test]
    fn http_and_userinfo_are_refused_even_on_an_allowed_host() {
        assert!(check("t", "http://github.com/o/r/t.zip", &github()).is_err());
        assert!(check("t", "https://user@github.com/o/r/t.zip", &github()).is_err());
        assert!(check("t", "file:///tmp/t.zip", &github()).is_err());
    }

    #[test]
    fn an_empty_allow_list_refuses_everything() {
        assert!(check("t", "https://github.com/t.zip", &[]).is_err());
    }

    #[test]
    fn a_url_source_allows_only_its_declared_host() {
        let allowed = vec![Origin::of("https://get.helm.sh").expect("origin")];
        assert!(check("helm", "https://get.helm.sh/helm-v1.tar.gz", &allowed).is_ok());
        assert!(check("helm", "https://github.com/helm.tar.gz", &allowed).is_err());
    }

    #[test]
    fn the_api_override_host_is_allowed_only_when_set() {
        let origins = allowed_origins("1.0.0", None, None, Some("https://ghe.example/api/v3"));
        assert!(check("t", "https://ghe.example/o/r/t.zip", &origins).is_ok());
        assert!(
            check(
                "t",
                "https://ghe.example/t.zip",
                &allowed_origins("1.0.0", None, None, None)
            )
            .is_err()
        );
    }
}
