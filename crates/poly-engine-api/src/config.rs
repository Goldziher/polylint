//! The per-engine configuration slice handed to every [`Engine`](crate::engine::Engine) call.

use std::cell::RefCell;

pub use poly_config::{GlobalDefaults, LineEnding};

/// The slice of config handed to one engine for one file.
#[derive(Debug, Clone)]
pub struct EngineConfig {
    /// Global opinionated defaults.
    pub globals: GlobalDefaults,
    /// Indent width for this file's language.
    pub indent_width: usize,
    /// Tool-specific options from `[<kind>.<lang>.<engine>]` (engine merges its own defaults).
    pub options: toml::Table,
}

impl EngineConfig {
    /// A stable 32-byte blake3 digest of everything in this config: the global
    /// defaults, `indent_width` and the `options` table.
    ///
    /// Component hosts use it to key per-config state (a configured engine
    /// instance, a cached option-key answer) without re-sending or re-serializing
    /// the config. The options table serializes in key order (`toml::Table` is
    /// ordered), so equal configs hash equal across processes.
    ///
    /// Computed on demand rather than memoised in a field: `EngineConfig` is built
    /// with struct literals in hundreds of places and its fields are public, so a field cache
    /// would break them and go stale on mutation. Each thread instead keeps a few recently
    /// hashed configs and returns the stored digest only when the config compares equal to
    /// one of them, so a mutated clone can never read a stale digest and the per-file path
    /// pays an equality check rather than a serialization and a hash.
    #[must_use]
    pub fn fingerprint(&self) -> [u8; 32] {
        MEMO.with(|memo| {
            let mut memo = memo.borrow_mut();
            if let Some((_, digest)) = memo.iter().find(|(cfg, _)| cfg.same_as(self)) {
                return *digest;
            }
            let digest = self.compute_fingerprint();
            if memo.len() == MEMO_CAPACITY {
                memo.remove(0);
            }
            memo.push((self.clone(), digest));
            digest
        })
    }

    fn same_as(&self, other: &Self) -> bool {
        self.indent_width == other.indent_width
            && self.globals.line_length == other.globals.line_length
            && self.globals.line_ending == other.globals.line_ending
            && self.globals.final_newline == other.globals.final_newline
            && self.globals.trim_trailing_whitespace == other.globals.trim_trailing_whitespace
            && self.options == other.options
    }

    fn compute_fingerprint(&self) -> [u8; 32] {
        let mut hasher = blake3::Hasher::new();
        hasher.update(format!("{:?}", self.globals).as_bytes());
        hasher.update(b"\0");
        hasher.update(&(self.indent_width as u64).to_le_bytes());
        hasher.update(b"\0");
        match toml::to_string(&self.options) {
            Ok(text) => hasher.update(text.as_bytes()),
            // Unserializable tables still hash deterministically through Debug.
            Err(_) => hasher.update(format!("{:?}", self.options).as_bytes()),
        };
        *hasher.finalize().as_bytes()
    }
}

/// Configs remembered per thread; a run has a handful of distinct engine configs.
const MEMO_CAPACITY: usize = 16;

thread_local! {
    static MEMO: RefCell<Vec<(EngineConfig, [u8; 32])>> = const { RefCell::new(Vec::new()) };
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(options: &str) -> EngineConfig {
        EngineConfig {
            globals: GlobalDefaults::default(),
            indent_width: 4,
            options: toml::from_str(options).expect("valid toml"),
        }
    }

    #[test]
    fn fingerprint_is_stable_and_clone_independent() {
        let a = cfg("a = 1\nb = \"x\"");
        let mut b = a.clone();
        assert_eq!(a.fingerprint(), b.fingerprint());
        b.options.insert("c".into(), toml::Value::Boolean(true));
        assert_ne!(a.fingerprint(), b.fingerprint());
    }

    #[test]
    fn fingerprint_ignores_key_order_and_sees_indent() {
        let a = cfg("a = 1\nb = 2");
        let b = cfg("b = 2\na = 1");
        assert_eq!(a.fingerprint(), b.fingerprint());
        let mut c = a.clone();
        c.indent_width = 2;
        assert_ne!(a.fingerprint(), c.fingerprint());
    }

    #[test]
    fn memo_never_serves_a_stale_digest_after_mutation() {
        let mut a = cfg("a = 1");
        let before = a.fingerprint();
        assert_eq!(before, a.fingerprint());
        a.options.insert("a".into(), toml::Value::Integer(2));
        assert_ne!(before, a.fingerprint());
        a.options.insert("a".into(), toml::Value::Integer(1));
        assert_eq!(before, a.fingerprint());
        a.globals.line_length += 1;
        assert_ne!(before, a.fingerprint());
    }

    #[test]
    fn memoised_digest_matches_a_fresh_computation() {
        let a = cfg("a = 1");
        assert_eq!(a.fingerprint(), a.compute_fingerprint());
        for i in 0..(MEMO_CAPACITY * 2) {
            let c = cfg(&format!("k = {i}"));
            assert_eq!(c.fingerprint(), c.compute_fingerprint());
        }
        assert_eq!(a.fingerprint(), a.compute_fingerprint());
    }
}
