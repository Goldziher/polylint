//! What a component lock must carry before the host loads anything from it.
//!
//! alef skips signature verification for an entry with no `key_id` when the key map is empty,
//! so a lock that arrives without keys would turn every component into an unverified one.

use alef_component_runtime::ComponentLock;
use anyhow::bail;

/// Reject a lock that would load components without verifying their signature.
///
/// A lock with no artifacts is fine. Otherwise it must embed at least one public key and every
/// entry must name a `key_id` among them. The one exception is an unsigned development lock
/// (`mise run components:dev`): no keys, no `key_id`s and every URL a `file://` path, or any
/// unsigned lock when `allow_unsigned` is set.
pub(crate) fn check_lock(lock: &ComponentLock, allow_unsigned: bool) -> anyhow::Result<()> {
    if lock.artifacts.is_empty() {
        return Ok(());
    }
    if is_unsigned_dev_lock(lock) || (allow_unsigned && lock.public_keys.is_empty()) {
        return Ok(());
    }
    if lock.public_keys.is_empty() {
        bail!(
            "the embedded component lock carries no public keys, so component signatures cannot be verified; \
             refusing to load components from it"
        );
    }
    for entry in &lock.artifacts {
        let component = &entry.identity.component;
        if entry.key_id.is_empty() {
            bail!("component `{component}` in the embedded lock has no key_id; refusing to load it unverified");
        }
        if !lock.public_keys.contains_key(&entry.key_id) {
            bail!(
                "component `{component}` in the embedded lock names key `{}`, which the lock does not embed",
                entry.key_id
            );
        }
    }
    Ok(())
}

fn is_unsigned_dev_lock(lock: &ComponentLock) -> bool {
    lock.public_keys.is_empty()
        && lock
            .artifacts
            .iter()
            .all(|entry| entry.key_id.is_empty() && entry.url.starts_with("file://"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lock(keys: &str, key_id: &str, url: &str) -> ComponentLock {
        serde_json::from_str(&format!(
            r#"{{"schema_version":2,"public_keys":{keys},"artifacts":[{{
                "identity":{{"crate_name":"c","component":"demo","version":"1.0.0","target":"t",
                             "feature_hash":"f","contract_hash":"h"}},
                "provides":[],"mode":"download","url":"{url}","sha256":"s","size":1,
                "manifest_sha256":"m","key_id":"{key_id}"}}]}}"#
        ))
        .unwrap()
    }

    const KEYS: &str = r#"{"release":"AAAA"}"#;

    #[test]
    fn an_empty_lock_is_accepted() {
        let lock: ComponentLock =
            serde_json::from_str(r#"{"schema_version":2,"public_keys":{},"artifacts":[]}"#).unwrap();
        check_lock(&lock, false).unwrap();
    }

    #[test]
    fn a_signed_lock_is_accepted() {
        check_lock(&lock(KEYS, "release", "https://h/a.tar.gz"), false).unwrap();
    }

    #[test]
    fn a_lock_without_keys_is_refused() {
        let error = check_lock(&lock("{}", "", "https://h/a.tar.gz"), false).unwrap_err();
        assert!(error.to_string().contains("no public keys"), "{error}");
        check_lock(&lock("{}", "release", "https://h/a.tar.gz"), false).unwrap_err();
    }

    #[test]
    fn an_entry_without_a_key_id_is_refused() {
        let error = check_lock(&lock(KEYS, "", "https://h/a.tar.gz"), false).unwrap_err();
        assert!(error.to_string().contains("no key_id"), "{error}");
    }

    #[test]
    fn an_entry_naming_an_unembedded_key_is_refused() {
        let error = check_lock(&lock(KEYS, "other", "https://h/a.tar.gz"), false).unwrap_err();
        assert!(error.to_string().contains("does not embed"), "{error}");
    }

    #[test]
    fn the_unsigned_dev_lock_is_accepted() {
        check_lock(&lock("{}", "", "file:///tmp/c/demo.tar.gz"), false).unwrap();
    }

    #[test]
    fn the_dev_exception_does_not_cover_a_mixed_or_remote_lock() {
        check_lock(&lock(KEYS, "", "file:///tmp/c/demo.tar.gz"), false).unwrap_err();
        check_lock(&lock("{}", "", "https://h/demo.tar.gz"), false).unwrap_err();
    }

    #[test]
    fn allow_unsigned_accepts_a_keyless_lock_only() {
        check_lock(&lock("{}", "k", "https://h/a.tar.gz"), true).unwrap();
        check_lock(&lock(KEYS, "", "https://h/a.tar.gz"), true).unwrap_err();
    }
}
