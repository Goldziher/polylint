//! Unpacking a downloaded release archive down to the one executable inside it.
//!
//! Nothing here writes a path taken from the archive: the named member is
//! streamed into a file the caller already created, so an entry called
//! `../../.ssh/authorized_keys` has nowhere to go. Entries are matched by their
//! exact normalised path, entries that escape the archive root never match, only
//! regular files are extracted, and the extracted size is capped so a
//! decompression bomb cannot fill the disk.

use std::io::{Read, Seek, Write};
use std::path::{Component, Path, PathBuf};

use anyhow::{Context, Result, bail};

/// The largest executable poly will extract from an archive.
const MAX_MEMBER_BYTES: u64 = 512 * 1024 * 1024;

/// The packaging of a downloaded archive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Format {
    TarGz,
    Zip,
}

impl Format {
    /// The format named by the extension of `url`, ignoring a query or fragment.
    pub(super) fn of(url: &str) -> Option<Self> {
        let path = url.split(['?', '#']).next().unwrap_or(url).to_ascii_lowercase();
        if path.ends_with(".tar.gz") || path.ends_with(".tgz") {
            Some(Self::TarGz)
        } else if path.ends_with(".zip") {
            Some(Self::Zip)
        } else {
            None
        }
    }
}

/// Copy the regular file `member` out of the archive `source` into `out`,
/// returning the number of bytes written.
///
/// # Errors
///
/// Returns `Err` when `member` is not a plain relative path, is absent or not a
/// regular file, exceeds the size cap, or the archive is corrupt.
pub(super) fn extract_member<R: Read + Seek, W: Write>(
    source: R,
    format: Format,
    member: &str,
    out: &mut W,
) -> Result<u64> {
    extract_capped(source, format, member, out, MAX_MEMBER_BYTES)
}

fn extract_capped<R: Read + Seek, W: Write>(
    source: R,
    format: Format,
    member: &str,
    out: &mut W,
    cap: u64,
) -> Result<u64> {
    let wanted = member_path(member)?;
    match format {
        Format::TarGz => from_tar_gz(source, &wanted, out, cap),
        Format::Zip => from_zip(source, &wanted, out, cap),
    }
}

pub(super) fn member_path(member: &str) -> Result<PathBuf> {
    let path = Path::new(member);
    if member.is_empty() || !path.components().all(|part| matches!(part, Component::Normal(_))) {
        bail!("archive member `{member}` must be a relative path without `.` or `..` components");
    }
    Ok(path.to_path_buf())
}

fn normalised(path: &Path) -> PathBuf {
    path.components()
        .filter(|part| !matches!(part, Component::CurDir))
        .collect()
}

fn from_tar_gz<R: Read, W: Write>(source: R, wanted: &Path, out: &mut W, cap: u64) -> Result<u64> {
    let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(source));
    for entry in archive.entries().context("reading the tar archive")? {
        let mut entry = entry.context("reading a tar entry")?;
        let is_member = entry.path().is_ok_and(|path| normalised(&path) == wanted);
        if !is_member {
            continue;
        }
        if !entry.header().entry_type().is_file() {
            bail!("`{}` in the archive is not a regular file", wanted.display());
        }
        return copy_capped(&mut entry, out, cap);
    }
    bail!("the archive has no member `{}`", wanted.display())
}

fn from_zip<R: Read + Seek, W: Write>(source: R, wanted: &Path, out: &mut W, cap: u64) -> Result<u64> {
    let mut archive = zip::ZipArchive::new(source).context("reading the zip archive")?;
    for index in 0..archive.len() {
        let mut file = archive.by_index(index).context("reading a zip entry")?;
        let is_member = file.enclosed_name().is_some_and(|name| normalised(&name) == wanted);
        if !is_member {
            continue;
        }
        if !file.is_file() {
            bail!("`{}` in the archive is not a regular file", wanted.display());
        }
        return copy_capped(&mut file, out, cap);
    }
    bail!("the archive has no member `{}`", wanted.display())
}

fn copy_capped<R: Read, W: Write>(reader: R, out: &mut W, cap: u64) -> Result<u64> {
    let copied = std::io::copy(&mut reader.take(cap + 1), out).context("extracting the archive member")?;
    if copied > cap {
        bail!("the archive member is larger than the {cap}-byte limit");
    }
    Ok(copied)
}

/// The most entries, and the most bytes in all, poly extracts from a distribution archive.
const MAX_TREE_ENTRIES: usize = 20_000;
const MAX_TREE_BYTES: u64 = 1024 * 1024 * 1024;

/// The caps a tree extraction enforces.
#[derive(Clone, Copy)]
struct TreeLimits {
    entries: usize,
    member_bytes: u64,
    total_bytes: u64,
}

const TREE_LIMITS: TreeLimits = TreeLimits {
    entries: MAX_TREE_ENTRIES,
    member_bytes: MAX_MEMBER_BYTES,
    total_bytes: MAX_TREE_BYTES,
};

/// Extract the whole archive `source` into the existing directory `dest`,
/// returning the number of files written.
///
/// Same guarantees as [`extract_member`], applied to every entry: a path that is
/// absolute or has a `..` component is an error (never silently skipped, so a
/// hostile archive fails loudly), only directories and regular files are
/// accepted (a symlink, hard link or device is an error), and the entry count,
/// each file's size and the total size are capped. Files are created exclusively
/// (`O_EXCL`), so an entry cannot write through anything already in `dest`.
///
/// # Errors
///
/// Returns `Err` on an unsafe or unsupported entry, an exceeded cap, or a corrupt archive.
pub(super) fn extract_tree<R: Read + Seek>(source: R, format: Format, dest: &Path) -> Result<usize> {
    extract_tree_capped(source, format, dest, TREE_LIMITS)
}

fn extract_tree_capped<R: Read + Seek>(source: R, format: Format, dest: &Path, limits: TreeLimits) -> Result<usize> {
    let mut sink = TreeSink {
        dest,
        limits,
        files: 0,
        entries: 0,
        bytes: 0,
    };
    match format {
        Format::TarGz => {
            let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(source));
            for entry in archive.entries().context("reading the tar archive")? {
                let mut entry = entry.context("reading a tar entry")?;
                let path = entry.path().context("reading a tar entry path")?.into_owned();
                let kind = entry.header().entry_type();
                if kind.is_dir() {
                    sink.directory(&path)?;
                } else if kind.is_file() {
                    sink.file(&path, &mut entry)?;
                } else if kind.is_pax_global_extensions() || kind.is_pax_local_extensions() {
                    continue;
                } else {
                    bail!("`{}` in the archive is not a regular file or directory", path.display());
                }
            }
        }
        Format::Zip => {
            let mut archive = zip::ZipArchive::new(source).context("reading the zip archive")?;
            for index in 0..archive.len() {
                let mut file = archive.by_index(index).context("reading a zip entry")?;
                let Some(path) = file.enclosed_name() else {
                    bail!("the zip entry `{}` escapes the archive root", file.name());
                };
                if file.is_symlink() {
                    bail!("`{}` in the archive is a symlink", path.display());
                }
                if file.is_dir() {
                    sink.directory(&path)?;
                } else if file.is_file() {
                    sink.file(&path, &mut file)?;
                } else {
                    bail!("`{}` in the archive is not a regular file or directory", path.display());
                }
            }
        }
    }
    Ok(sink.files)
}

struct TreeSink<'a> {
    dest: &'a Path,
    limits: TreeLimits,
    files: usize,
    entries: usize,
    bytes: u64,
}

impl TreeSink<'_> {
    /// `path` as a relative path of plain components, `.` segments dropped.
    fn target(&mut self, path: &Path) -> Result<PathBuf> {
        self.entries += 1;
        if self.entries > self.limits.entries {
            bail!("the archive has more than {} entries", self.limits.entries);
        }
        let relative = normalised(path);
        if relative.as_os_str().is_empty() {
            return Ok(self.dest.to_path_buf());
        }
        if !relative.components().all(|part| matches!(part, Component::Normal(_))) {
            bail!(
                "the archive entry `{}` is absolute or escapes the archive root",
                path.display()
            );
        }
        Ok(self.dest.join(relative))
    }

    fn directory(&mut self, path: &Path) -> Result<()> {
        let target = self.target(path)?;
        make_dir(&target)
    }

    fn file<R: Read>(&mut self, path: &Path, reader: &mut R) -> Result<()> {
        let target = self.target(path)?;
        if target == self.dest {
            bail!("the archive entry `{}` names the extraction root", path.display());
        }
        if let Some(parent) = target.parent() {
            make_dir(parent)?;
        }
        let mut out = create_file(&target)?;
        let remaining = self.limits.total_bytes - self.bytes;
        let cap = self.limits.member_bytes.min(remaining);
        let copied = copy_capped(reader, &mut out, cap).with_context(|| {
            format!(
                "extracting `{}` (a member or the whole archive is over its size limit)",
                path.display()
            )
        })?;
        self.bytes += copied;
        self.files += 1;
        Ok(())
    }
}

fn make_dir(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        std::fs::DirBuilder::new().recursive(true).mode(0o700).create(path)
    }
    #[cfg(not(unix))]
    { std::fs::create_dir_all(path) }.with_context(|| format!("creating {}", path.display()))
}

fn create_file(path: &Path) -> Result<std::fs::File> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options
        .open(path)
        .with_context(|| format!("creating {} (a duplicate entry?)", path.display()))
}

/// A `.tar.gz` holding `entries`, with names written verbatim (so a test can
/// plant `..` components the tar writer's own API refuses to emit).
#[cfg(test)]
pub(super) fn tar_gz(entries: &[(&str, tar::EntryType, &str)]) -> Vec<u8> {
    let mut builder = tar::Builder::new(Vec::new());
    for (name, kind, data) in entries {
        let mut header = tar::Header::new_gnu();
        header.as_old_mut().name[..name.len()].copy_from_slice(name.as_bytes());
        header.set_size(data.len() as u64);
        header.set_mode(0o755);
        header.set_entry_type(*kind);
        header.set_cksum();
        builder.append(&header, data.as_bytes()).expect("append");
    }
    let tarball = builder.into_inner().expect("tar");
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    encoder.write_all(&tarball).expect("gzip");
    encoder.finish().expect("finish gzip")
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use tar::EntryType;

    use super::*;

    fn zip_of(entries: &[(&str, &str)]) -> Vec<u8> {
        let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
        for (name, data) in entries {
            writer
                .start_file(*name, zip::write::SimpleFileOptions::default())
                .expect("start");
            writer.write_all(data.as_bytes()).expect("write");
        }
        writer.finish().expect("finish").into_inner()
    }

    fn unpack(archive: Vec<u8>, format: Format, member: &str) -> Result<Vec<u8>> {
        let mut out = Vec::new();
        extract_member(Cursor::new(archive), format, member, &mut out)?;
        Ok(out)
    }

    fn regular<'a>(name: &'a str, data: &'a str) -> (&'a str, EntryType, &'a str) {
        (name, EntryType::Regular, data)
    }

    #[test]
    fn the_format_comes_from_the_url_extension() {
        for (url, expected) in [
            ("https://x/helm-v4.tar.gz", Some(Format::TarGz)),
            ("https://x/tool.TGZ", Some(Format::TarGz)),
            ("https://x/tflint_linux.zip?download=1", Some(Format::Zip)),
            ("https://x/tool", None),
            ("https://x/tool.tar.xz", None),
        ] {
            assert_eq!(Format::of(url), expected, "{url}");
        }
    }

    #[test]
    fn a_tar_gz_member_is_selected_by_its_exact_path() {
        let archive = tar_gz(&[
            regular("linux-amd64/LICENSE", "license"),
            regular("linux-amd64/helm", "the binary"),
            regular("helm", "a different file"),
        ]);
        assert_eq!(
            unpack(archive, Format::TarGz, "linux-amd64/helm").expect("extract"),
            b"the binary"
        );
    }

    #[test]
    fn a_leading_dot_slash_in_the_archive_is_ignored() {
        let archive = tar_gz(&[regular("./bin/tool", "payload")]);
        assert_eq!(unpack(archive, Format::TarGz, "bin/tool").expect("extract"), b"payload");
    }

    #[test]
    fn a_tar_entry_that_escapes_the_root_is_never_the_member() {
        let archive = tar_gz(&[regular("../helm", "evil"), regular("dir/../helm", "evil too")]);
        let error = unpack(archive, Format::TarGz, "helm").expect_err("no member");
        assert!(error.to_string().contains("no member"), "{error:#}");
    }

    #[test]
    fn a_member_path_that_traverses_or_is_absolute_is_refused() {
        for member in ["../evil", "a/../../evil", "/etc/passwd", "./tool", ""] {
            let tarball = tar_gz(&[regular("../evil", "x"), regular("tool", "x")]);
            let error = unpack(tarball, Format::TarGz, member).expect_err(member);
            assert!(error.to_string().contains("relative path"), "{member}: {error:#}");
            let zipped = zip_of(&[("tool", "x")]);
            assert!(unpack(zipped, Format::Zip, member).is_err(), "{member}");
        }
    }

    #[test]
    fn a_missing_member_is_an_error() {
        let archive = tar_gz(&[regular("other", "x")]);
        let error = unpack(archive, Format::TarGz, "helm").expect_err("absent");
        assert!(error.to_string().contains("no member `helm`"), "{error:#}");
    }

    #[test]
    fn a_symlink_or_directory_member_is_refused() {
        for kind in [EntryType::Symlink, EntryType::Directory] {
            let archive = tar_gz(&[("helm", kind, "")]);
            let error = unpack(archive, Format::TarGz, "helm").expect_err("not a file");
            assert!(error.to_string().contains("not a regular file"), "{error:#}");
        }
    }

    #[test]
    fn a_member_over_the_size_cap_is_refused() {
        let payload = "x".repeat(64);
        let archive = tar_gz(&[regular("tool", &payload)]);
        let mut out = Vec::new();
        let error = extract_capped(Cursor::new(archive.clone()), Format::TarGz, "tool", &mut out, 63)
            .expect_err("over the cap");
        assert!(error.to_string().contains("limit"), "{error:#}");
        let mut out = Vec::new();
        let written = extract_capped(Cursor::new(archive), Format::TarGz, "tool", &mut out, 64).expect("at the cap");
        assert_eq!((written, out.len()), (64, 64));
    }

    #[test]
    fn a_zip_member_is_selected_by_its_exact_path() {
        let archive = zip_of(&[("README.md", "docs"), ("tflint", "the binary")]);
        assert_eq!(unpack(archive, Format::Zip, "tflint").expect("extract"), b"the binary");
    }

    #[test]
    fn a_zip_entry_that_escapes_the_root_is_never_the_member() {
        let archive = zip_of(&[("../tflint", "evil")]);
        let error = unpack(archive, Format::Zip, "tflint").expect_err("no member");
        assert!(error.to_string().contains("no member"), "{error:#}");
    }

    #[test]
    fn a_zip_member_over_the_size_cap_is_refused() {
        let big = "x".repeat(32);
        let archive = zip_of(&[("tflint", big.as_str())]);
        let mut out = Vec::new();
        let error = extract_capped(Cursor::new(archive), Format::Zip, "tflint", &mut out, 31).expect_err("over");
        assert!(error.to_string().contains("limit"), "{error:#}");
    }

    fn tree_into(archive: Vec<u8>, format: Format, limits: TreeLimits) -> (tempfile::TempDir, Result<usize>) {
        let dest = tempfile::tempdir().expect("dest");
        let outcome = extract_tree_capped(Cursor::new(archive), format, dest.path(), limits);
        (dest, outcome)
    }

    fn files_under(root: &Path) -> Vec<String> {
        let mut found = Vec::new();
        let mut stack = vec![root.to_path_buf()];
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(dir).expect("dir").flatten() {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                } else {
                    found.push(
                        path.strip_prefix(root)
                            .expect("under root")
                            .to_string_lossy()
                            .into_owned(),
                    );
                }
            }
        }
        found.sort();
        found
    }

    #[test]
    fn a_tree_is_extracted_whole_from_tar_gz_and_zip() {
        let tarball = tar_gz(&[
            ("dist/", EntryType::Directory, ""),
            regular("dist/bin/run", "script"),
            regular("dist/lib/a.jar", "jar"),
        ]);
        let zipped = zip_of(&[("dist/bin/run", "script"), ("dist/lib/a.jar", "jar")]);
        for (archive, format) in [(tarball, Format::TarGz), (zipped, Format::Zip)] {
            let (dest, outcome) = tree_into(archive, format, TREE_LIMITS);
            assert_eq!(outcome.expect("extracts"), 2, "{format:?}");
            assert_eq!(
                files_under(dest.path()),
                ["dist/bin/run", "dist/lib/a.jar"],
                "{format:?}"
            );
            assert_eq!(std::fs::read(dest.path().join("dist/lib/a.jar")).expect("read"), b"jar");
        }
    }

    #[test]
    fn a_tree_entry_that_escapes_or_is_absolute_fails_the_whole_extraction() {
        for name in ["../evil", "dist/../../evil", "/tmp/evil"] {
            let archive = tar_gz(&[regular("dist/ok", "x"), regular(name, "evil")]);
            let (dest, outcome) = tree_into(archive, Format::TarGz, TREE_LIMITS);
            let error = outcome.expect_err(name);
            assert!(
                error.to_string().contains("escapes") || error.to_string().contains("absolute"),
                "{name}: {error:#}"
            );
            assert!(!dest.path().parent().expect("parent").join("evil").exists(), "{name}");
        }
        let (_, outcome) = tree_into(zip_of(&[("../evil", "x")]), Format::Zip, TREE_LIMITS);
        assert!(outcome.expect_err("zip traversal").to_string().contains("escapes"));
    }

    #[test]
    fn a_tree_with_a_symlink_hard_link_or_device_is_refused() {
        for kind in [EntryType::Symlink, EntryType::Link, EntryType::Char] {
            let archive = tar_gz(&[regular("dist/ok", "x"), ("dist/link", kind, "")]);
            let (_, outcome) = tree_into(archive, Format::TarGz, TREE_LIMITS);
            let error = outcome.expect_err("not extractable");
            assert!(
                error.to_string().contains("not a regular file or directory"),
                "{error:#}"
            );
        }
        let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
        writer
            .add_symlink("dist/link", "/etc/passwd", zip::write::SimpleFileOptions::default())
            .expect("symlink");
        let archive = writer.finish().expect("finish").into_inner();
        let (_, outcome) = tree_into(archive, Format::Zip, TREE_LIMITS);
        assert!(outcome.expect_err("zip symlink").to_string().contains("symlink"));
    }

    #[test]
    fn a_tree_over_a_size_or_entry_cap_is_refused() {
        let payload = "x".repeat(64);
        let archive = tar_gz(&[regular("a", &payload), regular("b", &payload)]);
        let member = TreeLimits {
            member_bytes: 63,
            ..TREE_LIMITS
        };
        let (_, outcome) = tree_into(archive.clone(), Format::TarGz, member);
        assert!(format!("{:#}", outcome.expect_err("member cap")).contains("limit"));
        let total = TreeLimits {
            total_bytes: 100,
            ..TREE_LIMITS
        };
        let (_, outcome) = tree_into(archive.clone(), Format::TarGz, total);
        assert!(format!("{:#}", outcome.expect_err("total cap")).contains("limit"));
        let few = TreeLimits {
            entries: 1,
            ..TREE_LIMITS
        };
        let (_, outcome) = tree_into(archive.clone(), Format::TarGz, few);
        assert!(
            outcome
                .expect_err("entry cap")
                .to_string()
                .contains("more than 1 entries")
        );
        let (_, outcome) = tree_into(archive, Format::TarGz, TREE_LIMITS);
        assert_eq!(outcome.expect("within the caps"), 2);
    }

    #[test]
    fn a_duplicate_tree_entry_is_refused_rather_than_overwritten() {
        let archive = tar_gz(&[regular("a", "first"), regular("a", "second")]);
        let (dest, outcome) = tree_into(archive, Format::TarGz, TREE_LIMITS);
        assert!(outcome.is_err());
        assert_eq!(std::fs::read(dest.path().join("a")).expect("read"), b"first");
    }

    #[test]
    fn a_corrupt_archive_is_an_error() {
        assert!(unpack(b"not an archive".to_vec(), Format::TarGz, "x").is_err());
        assert!(unpack(b"not an archive".to_vec(), Format::Zip, "x").is_err());
    }
}
