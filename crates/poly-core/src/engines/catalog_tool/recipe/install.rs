//! Provisioning: downloading a pinned, checksummed release of a tool into poly's
//! cache.
//!
//! Only ever reached when the config sets `install = "auto"`; poly does not
//! fetch executables on its own initiative. Which release and digest to install
//! is decided by [`super::resolve`], the lock, or the config's own `url`.
//! The download is verified against a SHA-256 digest before it is made
//! executable and renamed into place, so a truncated or substituted file is never
//! left where a later run would trust it. An asset that names a `member` is a
//! `.tar.gz` / `.tgz` / `.zip`: the digest covers the archive as downloaded, and
//! only that member is installed.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use poly_catalog::{Asset, Release, ReleaseKind};
use tempfile::NamedTempFile;

use super::archive::{self, Format};
use super::net::Http;
use super::resolve::FileLock;

/// Platform key of the running host in the catalog's asset tables.
pub(super) fn platform() -> String {
    let os = match std::env::consts::OS {
        "macos" => "darwin",
        other => other,
    };
    format!("{os}-{}", std::env::consts::ARCH)
}

/// The asset of `release` for this host, falling back to the platform-neutral
/// `any` entry.
pub(super) fn asset_for(release: &Release) -> Option<&Asset> {
    release.assets.get(&platform()).or_else(|| release.assets.get("any"))
}

/// Where a release of `tool` is stored: `<cache>/tools/<tool>/<version>/<file>`,
/// where `<file>` is the archive member's name, or the download's own name.
pub(super) fn destination(tool: &str, version: &str, asset: &Asset) -> Result<PathBuf> {
    if !super::resolve::valid_version(version) {
        bail!("`{version}` is not a version");
    }
    let named = match &asset.member {
        Some(member) => Path::new(member).file_name().and_then(|name| name.to_str()),
        None => asset.url.rsplit('/').next(),
    };
    let file = named
        .filter(|name| !matches!(*name, "" | "." | ".."))
        .with_context(|| format!("{} does not name a file to install", asset.url))?;
    Ok(poly_cache::cache_home()?
        .join("tools")
        .join(tool)
        .join(version)
        .join(file))
}

/// Whether `path` is a complete, verified install of `asset`.
///
/// A stamp beside the file records the digest the download was verified against
/// together with the file's size and mtime, so later runs compare a short stamp
/// instead of hashing 90 MB, yet a file swapped after verification no longer
/// matches it. The directory must also be private to the current user: anyone who
/// can write there could replace the binary and its stamp together.
pub(super) fn is_installed(path: &Path, asset: &Asset) -> bool {
    let Ok(meta) = std::fs::metadata(path) else {
        return false;
    };
    meta.is_file()
        && path.parent().is_some_and(dir_is_private)
        && std::fs::read_to_string(stamp(path)).is_ok_and(|text| text == stamp_text(&asset.sha256, &meta))
}

fn stamp(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(".sha256");
    PathBuf::from(name)
}

fn stamp_text(sha256: &str, meta: &std::fs::Metadata) -> String {
    let mtime_ns = meta
        .modified()
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |elapsed| elapsed.as_nanos());
    format!("sha256={sha256}\nsize={}\nmtime_ns={mtime_ns}\n", meta.len())
}

fn record_stamp(path: &Path, sha256: &str) -> Result<()> {
    let meta = std::fs::metadata(path).with_context(|| format!("reading {}", path.display()))?;
    std::fs::write(stamp(path), stamp_text(sha256, &meta)).context("recording the install digest")
}

/// Whether `directory` is owned by the current user and writable by nobody else.
#[cfg(unix)]
fn dir_is_private(directory: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    std::fs::symlink_metadata(directory)
        .is_ok_and(|meta| meta.is_dir() && meta.uid() == current_uid() && meta.mode() & 0o022 == 0)
}

#[cfg(not(unix))]
fn dir_is_private(_directory: &Path) -> bool {
    true
}

#[cfg(unix)]
fn current_uid() -> u32 {
    // SAFETY: geteuid has no preconditions and cannot fail.
    unsafe { libc::geteuid() }
}

/// Create `directory` (mode 0700 where new) and make sure only the current user
/// can write to it. Returns whether an existing directory had to be tightened,
/// in which case nothing already inside it can be trusted.
///
/// # Errors
///
/// Returns `Err` when the directory cannot be created, is a symlink, or belongs
/// to another user.
#[cfg(unix)]
fn secure_dir(directory: &Path) -> Result<bool> {
    use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(directory)
        .with_context(|| format!("creating {}", directory.display()))?;
    let meta = std::fs::symlink_metadata(directory).with_context(|| format!("reading {}", directory.display()))?;
    if !meta.is_dir() || meta.uid() != current_uid() {
        bail!(
            "{} is not a directory owned by the current user; refusing to install a tool into it",
            directory.display()
        );
    }
    if meta.mode() & 0o022 == 0 {
        return Ok(false);
    }
    std::fs::set_permissions(directory, std::fs::Permissions::from_mode(0o700))
        .with_context(|| format!("restricting {}", directory.display()))?;
    Ok(true)
}

#[cfg(not(unix))]
fn secure_dir(directory: &Path) -> Result<bool> {
    std::fs::create_dir_all(directory).with_context(|| format!("creating {}", directory.display()))?;
    Ok(false)
}

/// Download `asset` to `path` unless it is already installed, verifying the
/// digest. Returns the installed path.
///
/// Parallel poly processes installing the same file serialize on a lock beside
/// it, so the second finds the first's result instead of downloading again.
///
/// # Errors
///
/// Returns `Err` on a network failure, a digest mismatch, or an unwritable
/// cache directory.
pub(super) fn ensure(path: &Path, asset: &Asset, kind: ReleaseKind) -> Result<()> {
    ensure_when(poly_config::network::offline(), path, asset, kind)
}

/// [`ensure`] with the offline decision passed in, so it can be tested without
/// flipping the process-wide switch.
fn ensure_when(offline: bool, path: &Path, asset: &Asset, kind: ReleaseKind) -> Result<()> {
    ensure_with(&Http::production(), offline, path, asset, kind)
}

fn ensure_with(http: &Http, offline: bool, path: &Path, asset: &Asset, kind: ReleaseKind) -> Result<()> {
    if is_installed(path, asset) {
        return Ok(());
    }
    if offline {
        bail!(
            "{}",
            poly_config::network::offline_message(&format!("download {}", asset.url))
        );
    }
    let directory = path.parent().context("install path has no parent directory")?;
    let tightened = secure_dir(directory)?;
    let _held = FileLock::acquire(&directory.join(".install.lock"))?;
    if !tightened && is_installed(path, asset) {
        return Ok(());
    }
    let partial = tempfile::Builder::new()
        .prefix(".download-")
        .tempfile_in(directory)
        .with_context(|| format!("creating a download file in {}", directory.display()))?;
    let digest = http
        .download(&asset.url, partial.as_file())
        .map_err(|error| anyhow::anyhow!("downloading {}: {error}", asset.url))?;
    if digest != asset.sha256 {
        bail!(
            "downloaded {} has sha256 {digest}, expected {}; refusing to install it and discarding the file. \
             The asset may have been re-published upstream; if you trust the new file, update the pinned digest \
             (`poly lock --upgrade`, or `sha256` in [tools])",
            asset.url,
            asset.sha256
        );
    }
    let staged = match &asset.member {
        Some(member) => unpack(&partial, &asset.url, member, directory)?,
        None => partial,
    };
    #[cfg(unix)]
    if kind == ReleaseKind::Binary {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(staged.path(), std::fs::Permissions::from_mode(0o755))
            .context("marking the download executable")?;
    }
    let _ = kind;
    staged
        .persist(path)
        .with_context(|| format!("installing to {}", path.display()))?;
    record_stamp(path, &asset.sha256)
}

/// Extract `member` from the verified `archive` download into a fresh file
/// beside it, so the final rename stays on one filesystem.
fn unpack(download: &NamedTempFile, url: &str, member: &str, directory: &Path) -> Result<NamedTempFile> {
    let format = Format::of(url).with_context(|| format!("{url} is not a .tar.gz, .tgz or .zip archive"))?;
    let source = download.reopen().context("reopening the download")?;
    let mut extracted = tempfile::Builder::new()
        .prefix(".extract-")
        .tempfile_in(directory)
        .with_context(|| format!("creating an extraction file in {}", directory.display()))?;
    archive::extract_member(source, format, member, extracted.as_file_mut())
        .with_context(|| format!("extracting `{member}` from {url}"))?;
    Ok(extracted)
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};
    use std::net::TcpListener;

    use sha2::{Digest, Sha256};

    use super::super::net::hex;
    use super::super::testserver::{Canned, Server};
    use super::*;

    fn serve(body: &'static [u8]) -> String {
        serve_as(body, "tool-1.0")
    }

    fn serve_as(body: &'static [u8], name: &str) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let address = listener.local_addr().expect("addr");
        std::thread::spawn(move || {
            for stream in listener.incoming().take(2) {
                let mut stream = stream.expect("accept");
                let mut request = [0u8; 1024];
                let _ = stream.read(&mut request);
                let head = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                stream.write_all(head.as_bytes()).expect("head");
                stream.write_all(body).expect("body");
            }
        });
        format!("http://{address}/{name}")
    }

    fn sha(body: &[u8]) -> String {
        hex(&Sha256::digest(body))
    }

    #[test]
    fn a_verified_download_is_installed_executable_and_stamped() {
        let url = serve(b"#!/bin/sh\nexit 0\n");
        let asset = Asset {
            url,
            sha256: sha(b"#!/bin/sh\nexit 0\n"),
            member: None,
        };
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("tool-1.0");
        ensure(&path, &asset, ReleaseKind::Binary).expect("install");
        assert!(is_installed(&path, &asset));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&path).expect("meta").permissions().mode() & 0o111,
                0o111
            );
        }
    }

    fn archive_asset(archive: Vec<u8>, name: &str, member: &str) -> Asset {
        let body: &'static [u8] = Box::leak(archive.into_boxed_slice());
        Asset {
            url: serve_as(body, name),
            sha256: sha(body),
            member: Some(member.to_owned()),
        }
    }

    #[test]
    fn an_archive_installs_only_its_member_under_the_member_name() {
        let archive = archive::tar_gz(&[
            ("pkg/LICENSE", tar::EntryType::Regular, "license"),
            ("pkg/bin/tool", tar::EntryType::Regular, "#!/bin/sh\nexit 0\n"),
        ]);
        let asset = archive_asset(archive, "tool-1.0.tar.gz", "pkg/bin/tool");
        let dir = tempfile::tempdir().expect("tempdir");
        let version_dir = dir.path().join("1.0");
        let path = version_dir.join("tool");
        ensure(&path, &asset, ReleaseKind::Binary).expect("install");
        assert_eq!(std::fs::read(&path).expect("read"), b"#!/bin/sh\nexit 0\n");
        assert!(is_installed(&path, &asset));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&path).expect("meta").permissions().mode() & 0o111,
                0o111
            );
        }
        let mut left: Vec<_> = std::fs::read_dir(&version_dir)
            .expect("read_dir")
            .map(|entry| entry.expect("entry").file_name())
            .filter(|name| name != ".install.lock")
            .collect();
        left.sort();
        assert_eq!(left, ["tool", "tool.sha256"], "no download or extraction files remain");
    }

    #[test]
    fn the_cache_file_of_an_archive_asset_is_named_after_the_member() {
        let asset = Asset {
            url: "https://example.invalid/helm-v4.tar.gz".to_owned(),
            sha256: String::new(),
            member: Some("linux-amd64/helm".to_owned()),
        };
        let path = destination("helm", "4.0.0", &asset).expect("destination");
        assert!(path.ends_with("tools/helm/4.0.0/helm"), "{}", path.display());
    }

    #[test]
    fn a_download_that_names_no_file_or_a_hostile_version_has_no_destination() {
        let asset = |url: &str| Asset {
            url: url.to_owned(),
            sha256: String::new(),
            member: None,
        };
        for url in [
            "https://example.invalid/dl/",
            "https://example.invalid/..",
            "https://example.invalid/.",
        ] {
            let error = destination("helm", "4.0.0", &asset(url)).expect_err(url);
            assert!(error.to_string().contains("does not name a file"), "{url}: {error:#}");
        }
        assert!(destination("helm", "../x", &asset("https://example.invalid/helm")).is_err());
        assert!(destination("helm", "4.0.0", &asset("https://example.invalid/helm")).is_ok());
    }

    #[test]
    fn an_archive_missing_its_member_installs_nothing() {
        let archive = archive::tar_gz(&[("other", tar::EntryType::Regular, "x")]);
        let asset = archive_asset(archive, "tool-1.0.tar.gz", "tool");
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("tool");
        let error = ensure(&path, &asset, ReleaseKind::Binary).expect_err("no member");
        assert!(format!("{error:#}").contains("no member"), "{error:#}");
        assert!(!path.exists());
        assert!(!is_installed(&path, &asset));
    }

    #[test]
    fn an_archive_with_an_unknown_extension_is_refused() {
        let archive = archive::tar_gz(&[("tool", tar::EntryType::Regular, "x")]);
        let asset = archive_asset(archive, "tool-1.0.tar.xz", "tool");
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("tool");
        let error = ensure(&path, &asset, ReleaseKind::Binary).expect_err("unknown format");
        assert!(format!("{error:#}").contains("not a .tar.gz"), "{error:#}");
        assert!(!path.exists());
    }

    #[test]
    fn a_tampered_archive_is_refused_before_it_is_opened() {
        let archive = archive::tar_gz(&[("tool", tar::EntryType::Regular, "x")]);
        let mut asset = archive_asset(archive, "tool-1.0.tar.gz", "tool");
        asset.sha256 = sha(b"what was pinned");
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("tool");
        let error = ensure(&path, &asset, ReleaseKind::Binary).expect_err("digest mismatch");
        assert!(error.to_string().contains("refusing to install"), "{error:#}");
        assert!(!path.exists());
    }

    #[test]
    fn a_digest_mismatch_installs_nothing() {
        let url = serve(b"not what was pinned");
        let asset = Asset {
            url,
            sha256: sha(b"what was pinned"),
            member: None,
        };
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("tool-1.0");
        let error = ensure(&path, &asset, ReleaseKind::Binary).expect_err("must refuse");
        assert!(error.to_string().contains("refusing to install"), "{error:#}");
        assert!(!path.exists(), "a mismatched download must not be left behind");
        assert!(!is_installed(&path, &asset));
    }

    #[test]
    fn an_installed_tool_is_not_downloaded_again() {
        let body: &'static [u8] = b"payload";
        let asset = Asset {
            url: "http://127.0.0.1:1/unreachable".to_owned(),
            sha256: sha(body),
            member: None,
        };
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("tool");
        std::fs::write(&path, body).expect("write");
        record_stamp(&path, &asset.sha256).expect("stamp");
        ensure(&path, &asset, ReleaseKind::Binary).expect("already installed, so no network is needed");
    }

    #[test]
    fn offline_refuses_a_missing_install_without_touching_the_network() {
        let asset = Asset {
            url: "http://127.0.0.1:1/never-requested".to_owned(),
            sha256: sha(b"x"),
            member: None,
        };
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("sub").join("tool");
        let error = ensure_when(true, &path, &asset, ReleaseKind::Binary).expect_err("offline must refuse");
        assert!(error.to_string().contains("offline mode"), "{error:#}");
        assert!(!path.parent().unwrap().exists(), "nothing may be created offline");
    }

    #[test]
    fn offline_still_accepts_an_installed_tool() {
        let body: &'static [u8] = b"payload";
        let asset = Asset {
            url: "http://127.0.0.1:1/x".to_owned(),
            sha256: sha(body),
            member: None,
        };
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("tool");
        std::fs::write(&path, body).expect("write");
        record_stamp(&path, &asset.sha256).expect("stamp");
        ensure_when(true, &path, &asset, ReleaseKind::Binary).expect("installed tools need no network");
    }

    fn quick_http() -> Http {
        use super::super::net::Policy;
        use super::super::unit::Retry;
        let policy = Policy {
            retry: Retry::new(3, 0, 0),
            connect: std::time::Duration::from_secs(2),
            stall: std::time::Duration::from_millis(300),
            request: std::time::Duration::from_millis(600),
            download: std::time::Duration::from_secs(5),
            max_retry_after: std::time::Duration::from_secs(30),
        };
        Http::with(policy, std::sync::Arc::new(|_| {}))
    }

    fn payload_asset(server: &Server, body: &[u8]) -> Asset {
        Asset {
            url: server.url("/tool-1.0"),
            sha256: sha(body),
            member: None,
        }
    }

    #[test]
    fn a_503_during_the_download_is_retried_and_the_install_completes() {
        let body = b"#!/bin/sh\nexit 0\n";
        let server = Server::start(vec![(
            "/tool-1.0",
            vec![Canned::new(503, "busy"), Canned::ok(body.to_vec())],
        )]);
        let asset = payload_asset(&server, body);
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("tool");
        ensure_with(&quick_http(), false, &path, &asset, ReleaseKind::Binary).expect("recovers");
        assert!(is_installed(&path, &asset));
        assert_eq!(server.hits("/tool-1.0"), 2);
    }

    #[test]
    fn a_transfer_cut_short_is_retried_from_the_start() {
        let body = b"complete payload";
        let server = Server::start(vec![(
            "/tool-1.0",
            vec![Canned::ok(body.to_vec()).truncated(), Canned::ok(body.to_vec())],
        )]);
        let asset = payload_asset(&server, body);
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("tool");
        ensure_with(&quick_http(), false, &path, &asset, ReleaseKind::Binary).expect("second attempt completes");
        assert_eq!(std::fs::read(&path).expect("read"), body);
        assert_eq!(server.hits("/tool-1.0"), 2);
    }

    #[test]
    fn a_stalled_download_times_out_and_installs_nothing() {
        let server = Server::start(vec![(
            "/tool-1.0",
            vec![Canned::ok(b"late".to_vec()).delayed(std::time::Duration::from_secs(2))],
        )]);
        let asset = payload_asset(&server, b"late");
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("tool");
        let started = std::time::Instant::now();
        let error = ensure_with(&quick_http(), false, &path, &asset, ReleaseKind::Binary).expect_err("stalls");
        assert!(format!("{error:#}").contains("timed out"), "{error:#}");
        assert!(
            started.elapsed() < std::time::Duration::from_secs(4),
            "{:?}",
            started.elapsed()
        );
        assert!(!path.exists());
    }

    #[test]
    fn a_corrupt_download_is_refused_once_with_both_digests_and_leaves_nothing() {
        let server = Server::start(vec![("/tool-1.0", vec![Canned::ok(b"corrupted".to_vec())])]);
        let asset = payload_asset(&server, b"what upstream published");
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("tool");
        let error = ensure_with(&quick_http(), false, &path, &asset, ReleaseKind::Binary).expect_err("mismatch");
        let text = format!("{error:#}");
        assert!(
            text.contains(&sha(b"corrupted")) && text.contains(&asset.sha256),
            "{text}"
        );
        assert!(text.contains("re-published"), "{text}");
        assert_eq!(
            server.hits("/tool-1.0"),
            1,
            "a digest mismatch is not a transient failure"
        );
        let leftovers: Vec<_> = std::fs::read_dir(dir.path())
            .expect("read_dir")
            .map(|entry| entry.expect("entry").file_name())
            .filter(|name| name != ".install.lock")
            .collect();
        assert!(
            leftovers.is_empty(),
            "the bad file must not stay in the cache: {leftovers:?}"
        );
    }

    #[test]
    fn concurrent_installs_of_one_tool_download_it_once() {
        let body = b"shared payload";
        let server = Server::start(vec![(
            "/tool-1.0",
            vec![Canned::ok(body.to_vec()).delayed(std::time::Duration::from_millis(150))],
        )]);
        let asset = payload_asset(&server, body);
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("1.0").join("tool");
        std::thread::scope(|scope| {
            for _ in 0..4 {
                scope.spawn(|| {
                    ensure_with(&quick_http(), false, &path, &asset, ReleaseKind::Binary).expect("installs");
                });
            }
        });
        assert!(is_installed(&path, &asset));
        assert_eq!(
            server.hits("/tool-1.0"),
            1,
            "the later installers find the first one's result"
        );
    }

    fn installed(body: &'static [u8]) -> (tempfile::TempDir, PathBuf, Asset) {
        let asset = Asset {
            url: "http://127.0.0.1:1/unreachable".to_owned(),
            sha256: sha(body),
            member: None,
        };
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("tool");
        std::fs::write(&path, body).expect("write");
        record_stamp(&path, &asset.sha256).expect("stamp");
        (dir, path, asset)
    }

    #[test]
    fn a_stamp_that_is_only_a_digest_does_not_vouch_for_a_file() {
        let (_dir, path, asset) = installed(b"payload");
        assert!(is_installed(&path, &asset));
        std::fs::write(stamp(&path), format!("{}\n", asset.sha256)).expect("plant");
        assert!(
            !is_installed(&path, &asset),
            "a planted binary with a bare digest stamp"
        );
    }

    #[test]
    fn replacing_the_binary_invalidates_its_stamp() {
        let (_dir, path, asset) = installed(b"payload");
        std::fs::write(&path, b"longer payload").expect("swap");
        assert!(!is_installed(&path, &asset), "different size");
        let (_dir, path, asset) = installed(b"payload");
        let file = std::fs::OpenOptions::new().write(true).open(&path).expect("open");
        file.set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_secs(1))
            .expect("set mtime");
        assert!(!is_installed(&path, &asset), "same size, different mtime");
    }

    #[cfg(unix)]
    #[test]
    fn a_writable_cache_directory_is_never_trusted_and_is_reinstalled_into() {
        use std::os::unix::fs::PermissionsExt;
        let body: &'static [u8] = b"#!/bin/sh\nexit 0\n";
        let url = serve(body);
        let asset = Asset {
            url,
            sha256: sha(body),
            member: None,
        };
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("tool-1.0");
        std::fs::write(&path, b"planted").expect("plant");
        record_stamp(&path, &asset.sha256).expect("stamp");
        assert!(is_installed(&path, &asset));
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o777)).expect("chmod");
        assert!(!is_installed(&path, &asset), "world-writable directory");
        ensure(&path, &asset, ReleaseKind::Binary).expect("reinstall");
        assert_eq!(
            std::fs::read(&path).expect("read"),
            body,
            "the planted file was replaced"
        );
        assert_eq!(
            std::fs::metadata(dir.path()).expect("meta").permissions().mode() & 0o777,
            0o700
        );
        assert!(is_installed(&path, &asset));
    }

    #[cfg(unix)]
    #[test]
    fn new_cache_directories_are_private() {
        use std::os::unix::fs::PermissionsExt;
        let body: &'static [u8] = b"payload";
        let asset = Asset {
            url: serve(body),
            sha256: sha(body),
            member: None,
        };
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("tools").join("tool").join("1.0").join("tool-1.0");
        ensure(&path, &asset, ReleaseKind::Binary).expect("install");
        for directory in [path.parent().unwrap(), path.parent().unwrap().parent().unwrap()] {
            let mode = std::fs::metadata(directory).expect("meta").permissions().mode();
            assert_eq!(mode & 0o077, 0, "{}: {mode:o}", directory.display());
        }
    }

    #[test]
    fn platform_key_matches_the_catalog_spelling() {
        let key = platform();
        assert!(key.contains('-'), "{key}");
        assert!(!key.starts_with("macos"), "the catalog says darwin: {key}");
    }
}
