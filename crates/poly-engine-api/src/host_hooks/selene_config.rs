//! The native selene config (`selene.toml`), resolved by poly.
//!
//! selene reads `selene.toml` from the directory it runs in. poly resolves the file nearest the linted file
//! instead (see [`native_config`]), so a monorepo can carry one per project, and salts the lint cache key
//! with its digest. The `selene` engine reads the parsed table through [`governing`].

use std::path::Path;
use std::sync::Arc;

use super::native_config::{self, Found};

/// The config file name.
pub const FILE_NAME: &str = "selene.toml";

/// The `selene.toml` that governs `path`, if any.
#[must_use]
pub fn governing(path: &Path) -> Option<Arc<Found>> {
    native_config::nearest("selene", &[FILE_NAME], path)
}

/// The cache-key salt for linting `path`: a digest of the config that governs it, if any.
#[must_use]
pub fn salt(path: &Path) -> Option<String> {
    governing(path).map(|found| format!("selene:{}", found.digest))
}
