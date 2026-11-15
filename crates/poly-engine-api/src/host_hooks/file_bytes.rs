//! Reading a config file for a cache key without letting a read failure look like an empty file.

use std::path::Path;

/// The bytes of `path` to fold into a cache key.
///
/// A file that cannot be read yields a marker naming the error kind rather than empty bytes, so a
/// config that became unreadable (permissions, a transient I/O error) keys differently from an
/// empty or absent one and a result computed without it is never replayed once it can be read.
#[must_use]
pub fn for_digest(path: &Path) -> Vec<u8> {
    match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) => format!("\0unreadable:{:?}", error.kind()).into_bytes(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unreadable_file_does_not_digest_like_an_empty_one() {
        let dir = tempfile::tempdir().unwrap();
        let empty = dir.path().join("empty.json");
        std::fs::write(&empty, "").unwrap();
        // A directory in place of the file: reading it fails on every platform.
        let unreadable = dir.path().join("dir.json");
        std::fs::create_dir(&unreadable).unwrap();

        assert!(for_digest(&empty).is_empty());
        assert!(!for_digest(&unreadable).is_empty());
        assert_ne!(for_digest(&unreadable), for_digest(&dir.path().join("missing.json")));
    }
}
