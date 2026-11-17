//! The hash map and set of the analysis: `FxHash`, which has no per-process seed, so nothing that leaks map order
//! (a tie in a sort) changes between two runs on the same input. The keys are paths and names read from the host's own
//! project, so a random seed would buy no protection worth that.

/// A [`std::collections::HashMap`] hashed with `FxHash`.
pub(crate) type HashMap<K, V> = rustc_hash::FxHashMap<K, V>;

/// A [`std::collections::HashSet`] hashed with `FxHash`.
pub(crate) type HashSet<T> = rustc_hash::FxHashSet<T>;
