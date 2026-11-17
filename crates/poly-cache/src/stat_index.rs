//! Stat-validated file verdict index: the warm-run fast path.
//!
//! The content-addressed result cache answers "what did engine E say about
//! *these bytes*", which still costs reading and hashing every file before the
//! first lookup. For the overwhelmingly common outcome — the file is clean and
//! has not changed — that work is the whole warm run. This index answers the
//! question one level up, without opening the file: "was this path, with this
//! `stat`, already judged clean under this exact plan?".
//!
//! # Records
//!
//! A [`Record`] holds, per path: the `stat` it was observed under, a digest of
//! the bytes, an overall [`Verdict`], and one [`Segment`] per engine — the key
//! that engine ran under and its raw output. That makes it both the fast path
//! and the per-engine result cache:
//!
//! - **`stat` hit** — the stat and *every* engine key equal the recorded ones,
//!   and the record answers for the run's scope key: the verdict is replayed without opening the file.
//! - **content hit** — the file was read and its digest equals the recorded one:
//!   each engine whose key still matches reuses its segment, so editing one
//!   engine's configuration re-runs only that engine, and a touched-but-identical
//!   file re-runs none.
//!
//! # Worktrees
//!
//! Every checkout of a repository (each `git worktree`) has its own index file,
//! because one record per path cannot serve two checkouts at once: an engine
//! whose key folds in an absolute path records different segments in each, and
//! a shared record would make every alternating run a miss. A checkout's first
//! run is *seeded* read-only from the most recently written sibling index, keyed
//! by checkout-relative path, so byte-identical files reuse the other
//! checkout's segments. A seeded record is never trusted by `stat` — a
//! [`FileStat`] names the checkout it was taken in — only by its digest.
//!
//! The engine key folds in the cache format, the build identity, the engine's id,
//! version and arguments, and its declared cross-file inputs, so a config edit,
//! an engine upgrade or a changed sibling `__init__.py` is a miss, never a stale
//! hit.
//!
//! # Racy timestamps
//!
//! `stat` equality proves "unchanged" only when a modification would have moved
//! the stat. It would not if the file was rewritten within the filesystem's
//! timestamp granularity of the earlier write, keeping size and mtime. Git's
//! answer is to distrust entries whose mtime is not strictly older than the
//! index; [`StatIndex::put`] applies the same rule at record time: a file whose
//! mtime or ctime is within [`RACY_WINDOW_NS`] of *now* is recorded with
//! [`FileStat::UNTRUSTED`], so only a content check can vouch for it on the next
//! run, by which time the timestamp has aged out of the window. ctime and inode
//! are part of the stat as a second line of defence — a same-size, same-mtime
//! rewrite (`touch -r`, `utimes`) still moves ctime, which no userspace call can
//! set.
//!
//! # Storage
//!
//! One file per namespace and checkout, `<root>/index/<checkout>/<namespace>.idx`
//! (`<root>/index/<namespace>.idx` for a cache opened without one): a 4-byte magic and
//! length-prefixed records sorted by path key. A clean file costs ~100 bytes plus
//! 20 per engine, rather than one filesystem entry per engine. Reads load the
//! whole file once; writes merge this run's records into the loaded set (or, if
//! another process replaced the file meanwhile, into what is on disk now) and
//! replace the file atomically, so concurrent runs lose at most each other's
//! newest records.

use rustc_hash::FxHashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::checkout::Checkout;

const MAGIC: &[u8; 4] = b"PSI5";

const HEADER_LEN: usize = MAGIC.len();
const ID_LEN: usize = 16;

/// How recently a file must have been modified, relative to the moment it is
/// recorded, for its record to be withheld. Two seconds covers the coarsest
/// timestamp granularity poly meets (FAT) and any clock skew between a network
/// filesystem and this host.
pub const RACY_WINDOW_NS: i64 = 2_000_000_000;

type Id = [u8; ID_LEN];

fn truncated(hash: &blake3::Hash) -> Id {
    let mut id = [0u8; ID_LEN];
    id.copy_from_slice(&hash.as_bytes()[..ID_LEN]);
    id
}

/// What `stat` reports about a file, compared whole, plus the checkout it was
/// taken in (see [`FileStat::in_checkout`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileStat {
    size: u64,
    mtime_ns: i64,
    ctime_ns: i64,
    ino: u64,
    checkout: u64,
}

impl FileStat {
    /// Stat `path`, following symlinks as a read would.
    pub fn of(path: &Path) -> std::io::Result<Self> {
        std::fs::metadata(path).map(|metadata| Self::from_metadata(&metadata))
    }

    /// The file's size in bytes.
    pub fn size(&self) -> u64 {
        self.size
    }

    /// Build from metadata already in hand.
    pub fn from_metadata(metadata: &std::fs::Metadata) -> Self {
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            Self {
                size: metadata.len(),
                mtime_ns: metadata.mtime().saturating_mul(1_000_000_000) + metadata.mtime_nsec(),
                ctime_ns: metadata.ctime().saturating_mul(1_000_000_000) + metadata.ctime_nsec(),
                ino: metadata.ino(),
                checkout: 0,
            }
        }
        #[cfg(not(unix))]
        {
            Self {
                size: metadata.len(),
                mtime_ns: metadata.modified().map_or(0, system_time_ns),
                ctime_ns: 0,
                ino: 0,
                checkout: 0,
            }
        }
    }

    /// This stat tagged with the checkout it was taken in, so a record written
    /// from one worktree is never vouched for by `stat` in another.
    #[must_use]
    pub fn in_checkout(self, checkout: u64) -> Self {
        Self { checkout, ..self }
    }

    /// A stat no real file has: a record carrying it is never vouched for by
    /// `stat` alone, only by its content digest.
    pub const UNTRUSTED: FileStat = FileStat {
        size: u64::MAX,
        mtime_ns: i64::MIN,
        ctime_ns: i64::MIN,
        ino: u64::MAX,
        checkout: u64::MAX,
    };

    #[cfg(test)]
    pub(crate) fn synthetic(size: u64, mtime_ns: i64, ctime_ns: i64, ino: u64) -> Self {
        Self {
            size,
            mtime_ns,
            ctime_ns,
            ino,
            checkout: 0,
        }
    }

    /// Whether a change made at `now_ns` could leave this stat unchanged.
    fn is_racy(&self, now_ns: i64) -> bool {
        let newest = self.mtime_ns.max(self.ctime_ns);
        newest > now_ns.saturating_sub(RACY_WINDOW_NS)
    }
}

#[cfg(not(unix))]
fn system_time_ns(time: SystemTime) -> i64 {
    time.duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| i64::try_from(elapsed.as_nanos()).unwrap_or(i64::MAX))
}

fn now_ns() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| i64::try_from(elapsed.as_nanos()).unwrap_or(i64::MAX))
}

/// `path` as [`std::path::absolute`] spells it, borrowed unchanged when that is already its
/// spelling: on Unix an absolute path with no empty or `.` component, which is nearly every
/// discovered path and would otherwise be re-allocated once per file.
fn absolute_form(path: &Path) -> std::borrow::Cow<'_, Path> {
    if is_normalized_absolute(path) {
        return std::borrow::Cow::Borrowed(path);
    }
    std::borrow::Cow::Owned(std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf()))
}

#[cfg(unix)]
fn is_normalized_absolute(path: &Path) -> bool {
    let bytes = path.as_os_str().as_encoded_bytes();
    if bytes.first() != Some(&b'/') || bytes.last() == Some(&b'/') {
        return false;
    }
    !bytes.iter().enumerate().any(|(index, byte)| {
        *byte == b'/'
            && match bytes.get(index + 1) {
                Some(b'/') => true,
                Some(b'.') => matches!(bytes.get(index + 2), None | Some(b'/')),
                _ => false,
            }
    })
}

#[cfg(not(unix))]
fn is_normalized_absolute(_path: &Path) -> bool {
    false
}

/// A path reduced to a fixed-width key, made absolute so the same file reached
/// as `./a.py` and `a.py` is one record.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PathKey(Id);

impl PathKey {
    /// Key `path` by its absolute form.
    pub fn of(path: &Path) -> Self {
        let absolute = absolute_form(path);
        Self(truncated(&blake3::hash(absolute.as_os_str().as_encoded_bytes())))
    }

    /// Key `path` relative to `checkout` when it lies inside it, so the same
    /// file in two worktrees of one repository is one key; by its absolute form
    /// otherwise. The two forms are domain-separated and cannot collide.
    pub(crate) fn within(path: &Path, checkout: Option<&Checkout>) -> Self {
        let absolute = absolute_form(path);
        match checkout.and_then(|checkout| checkout.relative(&absolute)) {
            Some(relative) => {
                let mut hasher = blake3::Hasher::new();
                hasher.update(b"checkout-relative\0");
                hasher.update(relative.as_os_str().as_encoded_bytes());
                Self(truncated(&hasher.finalize()))
            }
            None => Self(truncated(&blake3::hash(absolute.as_os_str().as_encoded_bytes()))),
        }
    }
}

/// Digest of a file's bytes, for the touched-but-identical path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContentDigest(Id);

impl ContentDigest {
    /// Digest `bytes`.
    pub fn of(bytes: &[u8]) -> Self {
        Self(truncated(&blake3::hash(bytes)))
    }
}

/// Everything besides a file's bytes that decided its verdict.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlanKey(Id);

impl PlanKey {
    /// Start a key for `namespace`, pre-seeded with the cache format and the
    /// running build's identity so no record outlives either.
    pub fn builder(namespace: crate::Namespace) -> PlanKeyBuilder {
        let mut hasher = blake3::Hasher::new();
        for part in [crate::CACHE_FORMAT_VERSION, crate::build_identity(), namespace.as_dir()] {
            feed(&mut hasher, part.as_bytes());
        }
        PlanKeyBuilder { hasher }
    }
}

impl PlanKey {
    /// This key extended with more inputs, for the ones only a single file has.
    pub fn extended<'a>(&self, parts: impl IntoIterator<Item = &'a str>) -> PlanKey {
        let mut hasher = blake3::Hasher::new();
        hasher.update(&self.0);
        for part in parts {
            feed(&mut hasher, part.as_bytes());
        }
        PlanKey(truncated(&hasher.finalize()))
    }
}

/// Accumulates the parts of a [`PlanKey`]; every part is length-prefixed so two
/// different splits of the same bytes cannot collide.
#[derive(Debug, Clone)]
pub struct PlanKeyBuilder {
    hasher: blake3::Hasher,
}

fn feed(hasher: &mut blake3::Hasher, part: &[u8]) {
    hasher.update(&(part.len() as u64).to_le_bytes());
    hasher.update(part);
}

impl PlanKeyBuilder {
    /// Fold in one more input.
    pub fn part(&mut self, part: impl AsRef<[u8]>) -> &mut Self {
        feed(&mut self.hasher, part.as_ref());
        self
    }

    /// Finish the key.
    pub fn finish(&self) -> PlanKey {
        PlanKey(truncated(&self.hasher.finalize()))
    }
}

/// What a recorded file was judged to be.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// Every engine ran and had nothing to report (lint) or nothing to change (fmt).
    Clean,
    /// A binary file, skipped before any engine was consulted.
    Binary,
    /// At least one engine reported findings; they are in the segments.
    Diagnostics,
}

impl Verdict {
    fn to_byte(self) -> u8 {
        match self {
            Verdict::Clean => 1,
            Verdict::Binary => 2,
            Verdict::Diagnostics => 3,
        }
    }

    fn from_byte(byte: u8) -> Option<Self> {
        match byte {
            1 => Some(Verdict::Clean),
            2 => Some(Verdict::Binary),
            3 => Some(Verdict::Diagnostics),
            _ => None,
        }
    }
}

/// One engine's contribution to a file's record: the key it ran under and what
/// it reported. An empty payload means it found nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Segment {
    /// Identifies the engine, its version, its arguments and its per-file
    /// inputs; a segment is only reused under an equal key.
    pub key: PlanKey,
    /// The engine's raw output, in whatever encoding the caller chose.
    pub payload: Vec<u8>,
}

/// Everything remembered about one path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Record {
    /// `stat` taken before the bytes were read, or [`FileStat::UNTRUSTED`].
    pub stat: FileStat,
    /// The run-level switches the verdict depends on, besides the engines.
    pub scope: PlanKey,
    /// Other scopes a run found this record right for: one of them judged the same bytes, its
    /// engines' output equal to the segments held under their keys (see [`Record::answers`]).
    /// Most recent first, at most [`MAX_CONFIRMED_SCOPES`].
    pub confirmed: Vec<PlanKey>,
    /// Digest of the bytes the segments were computed from.
    pub digest: ContentDigest,
    /// The overall verdict.
    pub verdict: Verdict,
    /// Whether the verdict can be replayed without reading the file: no inline
    /// suppression directives, whose handling needs the content.
    pub replayable: bool,
    /// One per engine of the plan that wrote the record, in plan order, followed by segments
    /// retained from earlier records of the same bytes whose engines that plan did not run (see
    /// [`Record::merged`]).
    pub segments: Vec<Segment>,
}

/// How many segments of earlier plans a record keeps beyond its own plan's, so a record whose
/// content never changes cannot grow without bound across config edits.
pub const MAX_RETAINED_SEGMENTS: usize = 32;

/// How many scopes besides its own a record answers for, so alternating narrowed runs
/// (`--only ruff`, `--only oxc`, …) each keep the record without growing it without bound.
pub const MAX_CONFIRMED_SCOPES: usize = 8;

impl Record {
    /// Whether a recorded `stat` vouches for a file that now looks like `stat`.
    pub fn trusts(&self, stat: &FileStat) -> bool {
        self.stat == *stat
    }

    /// Whether the plan that wrote this record ran exactly these engine keys, in order: its leading
    /// segments are theirs. Segments retained after them belong to other plans and do not count.
    pub fn has_keys(&self, keys: &[PlanKey]) -> bool {
        self.segments.len() >= keys.len() && self.segments.iter().zip(keys).all(|(s, k)| s.key == *k)
    }

    /// Whether a run under `scope` may read this record: `scope` wrote it, or a run under `scope`
    /// confirmed it. Which segments such a run may read is still decided by its engine keys.
    pub fn answers(&self, scope: &PlanKey) -> bool {
        self.scope == *scope || self.confirmed.contains(scope)
    }

    /// This record, answering for `scope` as well: a run under `scope` judged the same bytes and
    /// found the segments its engines read right.
    #[must_use]
    pub fn confirmed_for(mut self, scope: PlanKey) -> Self {
        if !self.answers(&scope) {
            self.confirmed.insert(0, scope);
            self.confirmed.truncate(MAX_CONFIRMED_SCOPES);
        }
        self
    }

    /// The segment recorded under `key`, wherever it sits.
    pub fn segment(&self, key: &PlanKey) -> Option<&Segment> {
        self.segments.iter().find(|segment| segment.key == *key)
    }

    /// This record with the segments of `earlier` it does not have appended, when `earlier`
    /// judged the same bytes, so a run with a narrower plan (`--only ruff`) does not discard what
    /// the other engines found. At most [`MAX_RETAINED_SEGMENTS`] are kept beyond this record's own.
    ///
    /// The scopes `earlier` answered for are kept too: a scope stands for run-level switches
    /// that decide a verdict from the bytes alone, so it holds for the same bytes whichever
    /// record carries it, and the segments those runs read are kept or replaced under the same
    /// keys.
    #[must_use]
    pub fn merged(mut self, earlier: Option<&Record>) -> Self {
        let Some(earlier) = earlier.filter(|earlier| earlier.digest == self.digest) else {
            return self;
        };
        let own = self.segments.len();
        for segment in &earlier.segments {
            if self.segments.len() - own >= MAX_RETAINED_SEGMENTS {
                break;
            }
            if self.segment(&segment.key).is_none() {
                self.segments.push(segment.clone());
            }
        }
        for scope in std::iter::once(&earlier.scope).chain(&earlier.confirmed) {
            if self.confirmed.len() >= MAX_CONFIRMED_SCOPES {
                break;
            }
            if !self.answers(scope) {
                self.confirmed.push(*scope);
            }
        }
        self
    }
}

/// The per-namespace record store. See the [module docs](self).
#[derive(Debug)]
pub struct StatIndex {
    file: Option<PathBuf>,
    loaded: FxHashMap<PathKey, Record>,
    /// Another checkout's records, consulted only where this one has none.
    seed: FxHashMap<PathKey, Record>,
    seen: Option<FileFingerprint>,
    pending: Mutex<FxHashMap<PathKey, Record>>,
    vouched: Mutex<FxHashMap<PathKey, Vouch>>,
}

/// A content check that vouched for an existing record under a new `stat`.
#[derive(Debug, Clone, Copy)]
struct Vouch {
    stat: FileStat,
    digest: ContentDigest,
}

/// Enough of a file's metadata to tell whether another process replaced it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FileFingerprint {
    len: u64,
    mtime_ns: i64,
}

fn fingerprint(file: &Path) -> Option<FileFingerprint> {
    let metadata = std::fs::metadata(file).ok()?;
    Some(FileFingerprint {
        len: metadata.len(),
        mtime_ns: FileStat::from_metadata(&metadata).mtime_ns,
    })
}

impl StatIndex {
    /// An index that never hits and never records, for a disabled cache.
    pub fn disabled() -> Self {
        Self {
            file: None,
            loaded: FxHashMap::default(),
            seed: FxHashMap::default(),
            seen: None,
            pending: Mutex::new(FxHashMap::default()),
            vouched: Mutex::new(FxHashMap::default()),
        }
    }

    /// Load the index stored at `file`; a missing, truncated or foreign file is
    /// an empty index rather than an error.
    pub fn load(file: PathBuf) -> Self {
        Self::load_seeded(file, None)
    }

    /// [`load`](Self::load), with `seed` — another checkout's index of the same
    /// repository — answering for paths `file` has no record of yet.
    pub fn load_seeded(file: PathBuf, seed: Option<&Path>) -> Self {
        let seen = fingerprint(&file);
        let loaded = read_records(&file);
        Self {
            file: Some(file),
            loaded,
            seed: seed.map(read_records).unwrap_or_default(),
            seen,
            pending: Mutex::new(FxHashMap::default()),
            vouched: Mutex::new(FxHashMap::default()),
        }
    }

    /// The record loaded for `path`, if any: this checkout's own, else the
    /// seed's.
    pub fn get(&self, path: &PathKey) -> Option<&Record> {
        self.loaded.get(path).or_else(|| self.seed.get(path))
    }

    /// Remember `record` for `path`, observed with a `stat` taken **before** the
    /// bytes behind its digest were read. A file modified within the racy window
    /// is still remembered — its digest and segments stay reusable by content —
    /// but with [`FileStat::UNTRUSTED`], so `stat` alone never vouches for it.
    pub fn put(&self, path: PathKey, record: Record) {
        self.put_at(now_ns(), path, record);
    }

    pub(crate) fn put_at(&self, now_ns: i64, path: PathKey, mut record: Record) {
        if self.file.is_none() {
            return;
        }
        if record.stat.is_racy(now_ns) {
            record.stat = FileStat::UNTRUSTED;
        }
        self.pending
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(path, record);
    }

    /// Re-record the loaded record for `path` (this checkout's own, or the
    /// seed's) under a new `stat`, after a content check found its bytes still
    /// digest to `digest`.
    ///
    /// Cheaper than [`put`](Self::put) with a copy of the record: nothing but the
    /// `stat` is held until [`flush`](Self::flush), which writes it only onto a
    /// record that still has that digest.
    pub fn vouch(&self, path: PathKey, stat: FileStat, digest: ContentDigest) {
        self.vouch_at(now_ns(), path, stat, digest);
    }

    pub(crate) fn vouch_at(&self, now_ns: i64, path: PathKey, stat: FileStat, digest: ContentDigest) {
        if self.file.is_none() {
            return;
        }
        let stat = if stat.is_racy(now_ns) {
            FileStat::UNTRUSTED
        } else {
            stat
        };
        self.vouched
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(path, Vouch { stat, digest });
    }

    /// Merge this run's records into the file on disk and replace it atomically.
    /// Does nothing when no record was added or vouched for.
    pub fn flush(&self) -> anyhow::Result<()> {
        let Some(file) = &self.file else {
            return Ok(());
        };
        let pending = std::mem::take(&mut *self.pending.lock().unwrap_or_else(std::sync::PoisonError::into_inner));
        let vouched = std::mem::take(&mut *self.vouched.lock().unwrap_or_else(std::sync::PoisonError::into_inner));
        if pending.is_empty() && vouched.is_empty() {
            return Ok(());
        }
        if fingerprint(file) == self.seen {
            write_records(file, &self.loaded, &self.seed, &pending, &vouched)
        } else {
            let on_disk = read_records(file);
            write_records(file, &on_disk, &self.seed, &pending, &vouched)
        }
    }
}

fn read_records(file: &Path) -> FxHashMap<PathKey, Record> {
    let Ok(bytes) = std::fs::read(file) else {
        return FxHashMap::default();
    };
    let Some(mut rest) = bytes.strip_prefix(MAGIC.as_slice()) else {
        return FxHashMap::default();
    };
    // A record with one segment is 113 bytes before its payload, so this sizes the
    // map once instead of rehashing it through every doubling on a large index.
    let mut records = FxHashMap::with_capacity_and_hasher(rest.len() / 113, Default::default());
    while !rest.is_empty() {
        let Some((path, record, after)) = decode(rest) else {
            return FxHashMap::default();
        };
        records.insert(path, record);
        rest = after;
    }
    records
}

struct Cursor<'a>(&'a [u8]);

impl<'a> Cursor<'a> {
    fn take(&mut self, len: usize) -> Option<&'a [u8]> {
        let (head, tail) = self.0.split_at_checked(len)?;
        self.0 = tail;
        Some(head)
    }

    fn id(&mut self) -> Option<Id> {
        self.take(ID_LEN)?.try_into().ok()
    }

    fn u64(&mut self) -> Option<u64> {
        Some(u64::from_le_bytes(self.take(8)?.try_into().ok()?))
    }

    fn u32(&mut self) -> Option<u32> {
        Some(u32::from_le_bytes(self.take(4)?.try_into().ok()?))
    }

    fn u16(&mut self) -> Option<u16> {
        Some(u16::from_le_bytes(self.take(2)?.try_into().ok()?))
    }

    fn u8(&mut self) -> Option<u8> {
        Some(self.take(1)?[0])
    }
}

fn decode(bytes: &[u8]) -> Option<(PathKey, Record, &[u8])> {
    let mut cursor = Cursor(bytes);
    let path = PathKey(cursor.id()?);
    let scope = PlanKey(cursor.id()?);
    let confirmed = (0..cursor.u8()?)
        .map(|_| cursor.id().map(PlanKey))
        .collect::<Option<Vec<_>>>()?;
    let digest = ContentDigest(cursor.id()?);
    let stat = FileStat {
        size: cursor.u64()?,
        mtime_ns: cursor.u64()? as i64,
        ctime_ns: cursor.u64()? as i64,
        ino: cursor.u64()?,
        checkout: cursor.u64()?,
    };
    let verdict = Verdict::from_byte(cursor.u8()?)?;
    let replayable = cursor.u8()? != 0;
    let count = usize::from(cursor.u16()?);
    let mut segments = Vec::with_capacity(count);
    for _ in 0..count {
        let key = PlanKey(cursor.id()?);
        let len = cursor.u32()? as usize;
        segments.push(Segment {
            key,
            payload: cursor.take(len)?.to_vec(),
        });
    }
    let record = Record {
        stat,
        scope,
        confirmed,
        digest,
        verdict,
        replayable,
        segments,
    };
    Some((path, record, cursor.0))
}

/// Encode `record` for `path`, under `stat` rather than its own.
fn encode(out: &mut Vec<u8>, path: &PathKey, record: &Record, stat: &FileStat) -> Option<()> {
    out.extend_from_slice(&path.0);
    out.extend_from_slice(&record.scope.0);
    out.push(u8::try_from(record.confirmed.len()).ok()?);
    for scope in &record.confirmed {
        out.extend_from_slice(&scope.0);
    }
    out.extend_from_slice(&record.digest.0);
    out.extend_from_slice(&stat.size.to_le_bytes());
    out.extend_from_slice(&stat.mtime_ns.to_le_bytes());
    out.extend_from_slice(&stat.ctime_ns.to_le_bytes());
    out.extend_from_slice(&stat.ino.to_le_bytes());
    out.extend_from_slice(&stat.checkout.to_le_bytes());
    out.push(record.verdict.to_byte());
    out.push(u8::from(record.replayable));
    out.extend_from_slice(&u16::try_from(record.segments.len()).ok()?.to_le_bytes());
    for segment in &record.segments {
        out.extend_from_slice(&segment.key.0);
        out.extend_from_slice(&u32::try_from(segment.payload.len()).ok()?.to_le_bytes());
        out.extend_from_slice(&segment.payload);
    }
    Some(())
}

/// Write `base` with `pending` replacing same-path entries and each `vouched`
/// stat applied to the record it vouched for — from `base`, else from `seed` —
/// sorted by path key so the file's bytes depend only on its contents.
fn write_records(
    file: &Path,
    base: &FxHashMap<PathKey, Record>,
    seed: &FxHashMap<PathKey, Record>,
    pending: &FxHashMap<PathKey, Record>,
    vouched: &FxHashMap<PathKey, Vouch>,
) -> anyhow::Result<()> {
    let from_seed = vouched
        .iter()
        .filter(|(path, _)| !pending.contains_key(path) && !base.contains_key(path))
        .filter_map(|(path, vouch)| {
            seed.get_key_value(path)
                .filter(|(_, record)| record.digest == vouch.digest)
        });
    let mut ordered: Vec<(&PathKey, &Record)> = base
        .iter()
        .filter(|(path, _)| !pending.contains_key(path))
        .chain(pending.iter())
        .chain(from_seed)
        .collect();
    ordered.sort_unstable_by_key(|(path, _)| path.0);
    let mut out = Vec::with_capacity(HEADER_LEN + ordered.len() * 200);
    out.extend_from_slice(MAGIC);
    for (path, record) in ordered {
        let stat = vouched
            .get(path)
            .filter(|vouch| vouch.digest == record.digest)
            .map_or(&record.stat, |vouch| &vouch.stat);
        if encode(&mut out, path, record, stat).is_none() {
            tracing::debug!("a record exceeded the index encoding limits and was dropped");
        }
    }
    let parent = file.parent().unwrap_or_else(|| Path::new("."));
    crate::permissions::create_dir_all_private(parent)
        .map_err(|e| anyhow::anyhow!("failed to create index dir {}: {e}", parent.display()))?;
    let tmp = parent.join(format!(
        ".{}.{}.tmp",
        file.file_name().and_then(|n| n.to_str()).unwrap_or("index"),
        std::process::id()
    ));
    std::fs::write(&tmp, &out).map_err(|e| anyhow::anyhow!("index write {}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, file).map_err(|e| anyhow::anyhow!("index rename to {}: {e}", file.display()))
}

#[cfg(test)]
mod tests;
