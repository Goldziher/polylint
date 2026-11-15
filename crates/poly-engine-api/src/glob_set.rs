//! One glob-set builder shared by every call site, so a malformed pattern is treated alike everywhere.

use std::collections::HashSet;
use std::sync::{Mutex, OnceLock};

use globset::{Glob, GlobSet, GlobSetBuilder};

/// A glob pattern that failed to parse, or a set that failed to build.
#[derive(Debug)]
pub enum GlobSetError {
    /// One pattern is not a valid glob.
    Pattern {
        /// The offending pattern.
        pattern: String,
        /// The parse failure.
        source: globset::Error,
    },
    /// The patterns parsed but the combined set could not be built.
    Build(globset::Error),
}

impl std::fmt::Display for GlobSetError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Pattern { pattern, source } => write!(f, "invalid glob pattern `{pattern}`: {source}"),
            Self::Build(source) => write!(f, "could not build glob set: {source}"),
        }
    }
}

impl std::error::Error for GlobSetError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Pattern { source, .. } | Self::Build(source) => Some(source),
        }
    }
}

/// Build a set from `patterns`, failing on the first invalid one (named in the error).
pub fn build_glob_set(patterns: &[impl AsRef<str>]) -> Result<GlobSet, GlobSetError> {
    let mut builder = GlobSetBuilder::new();
    for pattern in patterns {
        let pattern = pattern.as_ref();
        let glob = Glob::new(pattern).map_err(|source| GlobSetError::Pattern {
            pattern: pattern.to_owned(),
            source,
        })?;
        builder.add(glob);
    }
    builder.build().map_err(GlobSetError::Build)
}

/// Build a set from the valid `patterns`, logging each invalid one once per process.
pub fn build_glob_set_lossy(patterns: &[impl AsRef<str>]) -> GlobSet {
    static WARNED: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    let mut builder = GlobSetBuilder::new();
    for pattern in patterns {
        let pattern = pattern.as_ref();
        match Glob::new(pattern) {
            Ok(glob) => {
                builder.add(glob);
            }
            Err(error) => {
                let first = WARNED
                    .get_or_init(Mutex::default)
                    .lock()
                    .map(|mut seen| seen.insert(pattern.to_owned()))
                    .unwrap_or(true);
                if first {
                    tracing::warn!(%pattern, %error, "skipping invalid glob pattern");
                }
            }
        }
    }
    builder.build().unwrap_or_else(|_| GlobSet::empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strict_builds_valid_patterns() {
        let set = build_glob_set(&["*.rs", "docs/**"]).unwrap();
        assert!(set.is_match("a.rs"));
        assert!(set.is_match("docs/x/y.md"));
        assert!(!set.is_match("a.py"));
    }

    #[test]
    fn strict_names_the_invalid_pattern() {
        let error = build_glob_set(&["*.rs", "[unclosed"]).unwrap_err();
        assert!(matches!(&error, GlobSetError::Pattern { pattern, .. } if pattern == "[unclosed"));
        assert!(error.to_string().contains("[unclosed"));
    }

    #[test]
    fn strict_empty_matches_nothing() {
        let set = build_glob_set(&[] as &[&str]).unwrap();
        assert!(set.is_empty());
        assert!(!set.is_match("a.rs"));
    }

    #[test]
    fn lossy_skips_only_the_invalid_pattern() {
        let set = build_glob_set_lossy(&["*.rs", "[unclosed", "*.toml"]);
        assert!(set.is_match("a.rs"));
        assert!(set.is_match("Cargo.toml"));
        assert!(!set.is_match("a.py"));
    }

    #[test]
    fn lossy_empty_matches_nothing() {
        let set = build_glob_set_lossy(&[] as &[&str]);
        assert!(set.is_empty());
    }

    #[test]
    fn lossy_all_invalid_matches_nothing() {
        let set = build_glob_set_lossy(&["[a", "[b"]);
        assert!(set.is_empty());
    }
}
