//! The seam a host's cache plugs into: a store of opaque entries by namespace and key, and the envelope every entry
//! is wrapped in. The crate defines the seam only; what is cached, and when an entry may be replayed, is the host's.

use std::sync::Arc;

/// A family of cache entries with one meaning and one encoding. Entries of one namespace never answer for another,
/// and an entry written under another `schema` is a miss.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CacheNamespace {
    /// The name of the family, unique among the namespaces of a store.
    pub name: &'static str,
    /// The version of the entries' encoding; raise it whenever the encoding or its meaning changes.
    pub schema: u32,
}

/// A host's store of cache entries. The store keeps bytes and knows nothing of them: it may drop any entry at any
/// time, and whatever it returns is checked ([`open`]) before it is used, so a store that returns a damaged, stale
/// or foreign entry costs a miss, never a wrong answer.
pub trait CacheStore: Send + Sync {
    /// The entry of `key` in `ns`, if the store has one.
    fn get(&self, ns: CacheNamespace, key: &[u8; 32]) -> Option<Arc<[u8]>>;

    /// Keeps `value` as the entry of `key` in `ns`, replacing an earlier one. A store that cannot keep it drops it.
    fn put(&self, ns: CacheNamespace, key: &[u8; 32], value: Arc<[u8]>);
}

/// The first bytes of every entry.
const MAGIC: [u8; 4] = *b"TSRC";
/// The version of the envelope itself.
const ENVELOPE: u32 = 1;
/// The bytes before the payload: magic, envelope version, schema, name digest, key, payload length, payload digest.
const HEADER: usize = 4 + 4 + 4 + 8 + 32 + 8 + 8;

/// FNV-1a over `bytes`: an integrity check against truncation and damage, not an authentication of the entry.
fn digest(bytes: &[u8]) -> u64 {
    const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;
    bytes
        .iter()
        .fold(OFFSET, |hash, &byte| (hash ^ u64::from(byte)).wrapping_mul(PRIME))
}

/// `payload` as the entry of `key` in `ns`: the payload behind a header that names the envelope version, the
/// namespace, the key and the payload's length and digest.
#[must_use]
pub fn seal(ns: CacheNamespace, key: &[u8; 32], payload: &[u8]) -> Arc<[u8]> {
    let mut entry = Vec::with_capacity(HEADER + payload.len());
    entry.extend_from_slice(&MAGIC);
    entry.extend_from_slice(&ENVELOPE.to_le_bytes());
    entry.extend_from_slice(&ns.schema.to_le_bytes());
    entry.extend_from_slice(&digest(ns.name.as_bytes()).to_le_bytes());
    entry.extend_from_slice(key);
    entry.extend_from_slice(&(payload.len() as u64).to_le_bytes());
    entry.extend_from_slice(&digest(payload).to_le_bytes());
    entry.extend_from_slice(payload);
    entry.into()
}

/// The payload of `entry` if it is a whole entry of `key` in `ns` ([`seal`]); `None` for anything else: bytes cut
/// short or extended, damaged, written by another envelope version, under another schema or namespace, or for
/// another key.
#[must_use]
pub fn open<'a>(ns: CacheNamespace, key: &[u8; 32], entry: &'a [u8]) -> Option<&'a [u8]> {
    let (header, payload) = entry.split_at_checked(HEADER)?;
    let (magic, rest) = header.split_at(4);
    let (envelope, rest) = rest.split_at(4);
    let (schema, rest) = rest.split_at(4);
    let (name, rest) = rest.split_at(8);
    let (entry_key, rest) = rest.split_at(32);
    let (length, payload_digest) = rest.split_at(8);
    let valid = magic == MAGIC
        && envelope == ENVELOPE.to_le_bytes()
        && schema == ns.schema.to_le_bytes()
        && name == digest(ns.name.as_bytes()).to_le_bytes()
        && entry_key == key
        && length == (payload.len() as u64).to_le_bytes()
        && payload_digest == digest(payload).to_le_bytes();
    valid.then_some(payload)
}

/// The payload `store` holds for `key` in `ns`, or `None` when it holds none or what it holds is not a valid entry.
#[must_use]
pub fn read(store: &dyn CacheStore, ns: CacheNamespace, key: &[u8; 32]) -> Option<Vec<u8>> {
    let entry = store.get(ns, key)?;
    open(ns, key, &entry).map(<[u8]>::to_vec)
}

/// Puts `payload` into `store` as the entry of `key` in `ns`.
pub fn write(store: &dyn CacheStore, ns: CacheNamespace, key: &[u8; 32], payload: &[u8]) {
    store.put(ns, key, seal(ns, key, payload));
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use std::collections::BTreeMap;
    use std::sync::Mutex;

    use super::*;

    const FACTS: CacheNamespace = CacheNamespace {
        name: "facts",
        schema: 3,
    };
    const KEY: [u8; 32] = [7; 32];
    const PAYLOAD: &[u8] = b"what the program answered";

    type Entries = BTreeMap<(&'static str, [u8; 32]), Arc<[u8]>>;

    /// A store in memory, which keeps whatever it is given.
    #[derive(Default)]
    struct Memory {
        entries: Mutex<Entries>,
    }

    impl CacheStore for Memory {
        fn get(&self, ns: CacheNamespace, key: &[u8; 32]) -> Option<Arc<[u8]>> {
            self.entries.lock().unwrap().get(&(ns.name, *key)).cloned()
        }

        fn put(&self, ns: CacheNamespace, key: &[u8; 32], value: Arc<[u8]>) {
            self.entries.lock().unwrap().insert((ns.name, *key), value);
        }
    }

    #[test]
    fn a_sealed_entry_opens_to_its_payload() {
        let entry = seal(FACTS, &KEY, PAYLOAD);
        assert_eq!(open(FACTS, &KEY, &entry), Some(PAYLOAD));
        let empty = seal(FACTS, &KEY, b"");
        assert_eq!(open(FACTS, &KEY, &empty), Some(&b""[..]));
    }

    #[test]
    fn an_entry_of_another_schema_namespace_or_key_is_a_miss() {
        let entry = seal(FACTS, &KEY, PAYLOAD);
        let newer = CacheNamespace { schema: 4, ..FACTS };
        let other = CacheNamespace { name: "plans", ..FACTS };
        assert_eq!(open(newer, &KEY, &entry), None, "another schema");
        assert_eq!(open(other, &KEY, &entry), None, "another namespace");
        assert_eq!(open(FACTS, &[8; 32], &entry), None, "another key");
    }

    #[test]
    fn a_damaged_entry_is_a_miss_wherever_the_damage_is() {
        let entry = seal(FACTS, &KEY, PAYLOAD);
        for index in 0..entry.len() {
            let mut damaged = entry.to_vec();
            damaged[index] ^= 0x40;
            assert_eq!(open(FACTS, &KEY, &damaged), None, "byte {index} flipped");
        }
        for length in 0..entry.len() {
            assert_eq!(open(FACTS, &KEY, &entry[..length]), None, "cut to {length} bytes");
        }
        let mut longer = entry.to_vec();
        longer.push(0);
        assert_eq!(open(FACTS, &KEY, &longer), None, "bytes added");
        assert_eq!(open(FACTS, &KEY, b""), None);
    }

    #[test]
    fn a_store_gives_back_what_was_written_and_an_invalid_entry_is_a_miss() {
        let store = Memory::default();
        assert_eq!(read(&store, FACTS, &KEY), None, "nothing written");
        write(&store, FACTS, &KEY, PAYLOAD);
        assert_eq!(read(&store, FACTS, &KEY).as_deref(), Some(PAYLOAD));
        let newer = CacheNamespace { schema: 4, ..FACTS };
        assert_eq!(
            read(&store, newer, &KEY),
            None,
            "the store kept the entry of an older schema"
        );
        store.put(FACTS, &KEY, Arc::from(&b"not an entry"[..]));
        assert_eq!(read(&store, FACTS, &KEY), None, "the store returned foreign bytes");
    }
}
