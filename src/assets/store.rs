//! The in-memory least-recently-used store of generated assets.
//!
//! Entries live in a `Vec` ordered oldest-first. Capacity is small (tens by
//! default), so the linear scans here are deliberate.

use alloc::sync::Arc;
use std::env;

use crate::error::CyberError;

/// Assets kept when nothing else configures the store.
pub const DEFAULT_CAPACITY: usize = 50;

/// Largest capacity [`AssetStore::new`] and [`AssetStore::set_capacity`] accept.
pub const MAX_CAPACITY: usize = 10_000;

/// Largest single asset [`AssetStore::insert`] accepts: 256 MiB.
pub const MAX_ASSET_BYTES: usize = 256 * 1024 * 1024;

/// Environment variable [`AssetStore::from_env`] reads the capacity from.
pub const CAPACITY_ENV: &str = "CYBERDEMON_ASSET_CAPACITY";

/// Longest a sanitized filename may get, in characters.
const MAX_FILENAME_CHARS: usize = 64;

/// Filename used when sanitization leaves nothing behind.
const FALLBACK_FILENAME: &str = "asset.bin";

/// Length of a generated asset id, in lowercase hex characters.
const ID_HEX_CHARS: usize = 16;

/// One stored asset: its identity plus its bytes.
#[derive(Clone, Debug)]
pub struct Asset {
    /// Sixteen lowercase hex characters, unique within the store.
    pub id: String,
    /// Sanitized name the asset was inserted under.
    pub filename: String,
    /// `MIME` media type the asset was inserted with.
    pub mime: String,
    /// The bytes, shared so lookups never copy them.
    pub data: Arc<[u8]>,
}

/// A least-recently-used set of the most recent assets, oldest first.
///
/// [`AssetStore::insert`] appends the newest entry and evicts from the front
/// while the store is over capacity; [`AssetStore::get`] moves the asset it
/// returns to the back.
#[derive(Debug)]
pub struct AssetStore {
    /// Maximum number of entries kept.
    capacity: usize,
    /// Current entries, oldest first.
    entries: Vec<Asset>,
}

impl AssetStore {
    /// Creates an empty store keeping at most `capacity` assets.
    ///
    /// # Errors
    ///
    /// [`CyberError::InvalidArgument`] when `capacity` is outside
    /// `1..=MAX_CAPACITY`.
    #[inline]
    pub fn new(capacity: usize) -> Result<Self, CyberError> {
        validate_capacity(capacity)?;
        Ok(Self::with_capacity(capacity))
    }

    /// Creates an empty store sized from [`CAPACITY_ENV`].
    ///
    /// A missing or unparseable value falls back to [`DEFAULT_CAPACITY`]
    /// silently: nothing is printed because stdout carries the protocol.
    /// The parsing helper is unit-tested directly, because setting an
    /// environment variable from a test would need `unsafe`.
    #[must_use]
    #[inline]
    pub fn from_env() -> Self {
        let capacity = capacity_from(env::var(CAPACITY_ENV).ok().as_deref());
        // `capacity_from` only yields values `new` accepts; the fallback
        // exists so no unwrap is needed on the unreachable error path.
        Self::new(capacity).unwrap_or_else(|_| Self::with_capacity(DEFAULT_CAPACITY))
    }

    /// The maximum number of assets this store keeps.
    #[must_use]
    #[inline]
    pub const fn capacity(&self) -> usize {
        self.capacity
    }

    /// Number of assets currently stored.
    #[must_use]
    #[inline]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the store holds no assets.
    #[must_use]
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Total bytes held by the stored assets.
    #[must_use]
    #[inline]
    pub fn total_bytes(&self) -> usize {
        self.entries.iter().map(|entry| entry.data.len()).sum()
    }

    /// Resizes the store, evicting the oldest entries when it shrinks.
    ///
    /// Returns the number of assets evicted by the change.
    ///
    /// # Errors
    ///
    /// [`CyberError::InvalidArgument`] when `capacity` is outside
    /// `1..=MAX_CAPACITY`; the store is then left unchanged.
    #[inline]
    pub fn set_capacity(&mut self, capacity: usize) -> Result<usize, CyberError> {
        validate_capacity(capacity)?;
        self.capacity = capacity;
        Ok(self.evict_to_capacity())
    }

    /// Stores `data` as a new asset and returns it.
    ///
    /// The filename is reduced to the safe alphabet and truncated, the id is
    /// freshly generated random hex, unique within the store. The asset
    /// becomes the most recently used one; the oldest entries are evicted
    /// while the store is over capacity.
    ///
    /// # Errors
    ///
    /// [`CyberError::LimitExceeded`] when `data` is larger than
    /// [`MAX_ASSET_BYTES`].
    #[inline]
    pub fn insert(
        &mut self,
        filename: &str,
        mime: &str,
        data: Vec<u8>,
    ) -> Result<Asset, CyberError> {
        if data.len() > MAX_ASSET_BYTES {
            return Err(CyberError::LimitExceeded(format!(
                "asset is {} bytes, the limit is {MAX_ASSET_BYTES}",
                data.len(),
            )));
        }
        let asset = Asset {
            id: self.next_id(),
            filename: sanitized_filename(filename),
            mime: mime.to_owned(),
            data: Arc::from(data),
        };
        self.entries.push(asset.clone());
        self.evict_to_capacity();
        Ok(asset)
    }

    /// The asset stored under `id`, moved to the most-recently-used end.
    #[must_use]
    #[inline]
    pub fn get(&mut self, id: &str) -> Option<Asset> {
        let position = self.entries.iter().position(|entry| entry.id == id)?;
        let asset = self.entries.remove(position);
        self.entries.push(asset.clone());
        Some(asset)
    }

    /// The asset stored under `id`, without touching recency.
    #[must_use]
    #[inline]
    pub fn peek(&self, id: &str) -> Option<&Asset> {
        self.entries.iter().find(|entry| entry.id == id)
    }

    /// Drops the oldest entries until the store fits its capacity.
    ///
    /// Returns how many entries were evicted.
    fn evict_to_capacity(&mut self) -> usize {
        let evicted = self.entries.len().saturating_sub(self.capacity);
        self.entries.drain(..evicted);
        evicted
    }

    /// A fresh random id, retried until no stored asset uses it.
    fn next_id(&self) -> String {
        let mut id = random_id();
        while self.entries.iter().any(|entry| entry.id == id) {
            id = random_id();
        }
        id
    }

    /// Builds a store at an already-validated capacity.
    const fn with_capacity(capacity: usize) -> Self {
        Self {
            capacity,
            entries: Vec::new(),
        }
    }
}

/// Parse a configured capacity, falling back to [`DEFAULT_CAPACITY`].
///
/// `None` models an unset variable; anything that does not parse as a
/// `usize` in `1..=MAX_CAPACITY` is invalid and also falls back.
fn capacity_from(value: Option<&str>) -> usize {
    value
        .and_then(|raw| raw.parse::<usize>().ok())
        .filter(|capacity| validate_capacity(*capacity).is_ok())
        .unwrap_or(DEFAULT_CAPACITY)
}

/// Check a capacity against the accepted `1..=MAX_CAPACITY` range.
fn validate_capacity(capacity: usize) -> Result<(), CyberError> {
    if capacity == 0 || capacity > MAX_CAPACITY {
        return Err(CyberError::InvalidArgument(format!(
            "asset capacity must be within 1..={MAX_CAPACITY}, got {capacity}"
        )));
    }
    Ok(())
}

/// Reduce a raw filename to `[A-Za-z0-9._-]` (other characters become `_`),
/// at most [`MAX_FILENAME_CHARS`] characters; a result left empty becomes
/// [`FALLBACK_FILENAME`].
fn sanitized_filename(raw: &str) -> String {
    let kept: String = raw
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-') {
                ch
            } else {
                '_'
            }
        })
        .take(MAX_FILENAME_CHARS)
        .collect();
    if kept.is_empty() {
        FALLBACK_FILENAME.to_owned()
    } else {
        kept
    }
}

/// A new id of [`ID_HEX_CHARS`] lowercase hex characters.
///
/// With 64 bits of randomness collisions are effectively impossible, but
/// [`AssetStore::next_id`] still retries, so the store's uniqueness holds no
/// matter what the random source yields.
fn random_id() -> String {
    (0..ID_HEX_CHARS)
        .map(|_| {
            let nibble = fastrand::u8(0..16);
            char::from_digit(u32::from(nibble), 16).unwrap_or('0')
        })
        .collect()
}

/// The media type conventionally used for files with this extension.
///
/// The extension carries no dot and is matched case-sensitively; anything
/// unknown maps to `application/octet-stream`.
#[must_use]
#[inline]
pub fn mime_for_extension(ext: &str) -> &'static str {
    match ext {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "wav" => "audio/wav",
        "ogg" => "audio/ogg",
        "mp3" => "audio/mpeg",
        "flac" => "audio/flac",
        "mid" | "midi" => "audio/midi",
        "mus" => "audio/x-mus",
        "json" => "application/json",
        "txt" => "text/plain; charset=utf-8",
        "wad" => "application/x-doom-wad",
        _ => "application/octet-stream",
    }
}

#[cfg(test)]
mod tests {
    use alloc::sync::Arc;
    use std::collections::HashSet;

    use super::{
        AssetStore, DEFAULT_CAPACITY, MAX_ASSET_BYTES, MAX_CAPACITY, capacity_from,
        mime_for_extension, sanitized_filename,
    };

    /// Insert a one-byte asset and return its id.
    fn put(store: &mut AssetStore, name: &str) -> String {
        store
            .insert(name, "application/octet-stream", vec![b'x'])
            .expect("a one-byte insert always succeeds")
            .id
    }

    /// Add `count` assets to the store, returning their ids oldest first.
    fn fill(store: &mut AssetStore, count: usize) -> Vec<String> {
        (0..count)
            .map(|index| put(store, &format!("asset-{index}.bin")))
            .collect()
    }

    #[test]
    fn new_accepts_the_range_boundaries() {
        let one = AssetStore::new(1).expect("capacity 1 is the minimum");
        assert_eq!(one.capacity(), 1, "minimum capacity kept");
        assert!(one.is_empty(), "a fresh store is empty and reports it");
        assert_eq!(one.len(), 0, "a fresh store holds nothing");
        assert_eq!(one.total_bytes(), 0, "an empty store holds no bytes");
        let max = AssetStore::new(MAX_CAPACITY).expect("MAX_CAPACITY is valid");
        assert_eq!(max.capacity(), MAX_CAPACITY, "maximum capacity kept");
    }

    #[test]
    fn new_rejects_capacities_outside_the_range() {
        let zero = AssetStore::new(0).expect_err("capacity 0 must be rejected");
        assert_eq!(zero.code(), "invalid_argument", "zero capacity code");
        let above = MAX_CAPACITY.saturating_add(1);
        let too_big = AssetStore::new(above).expect_err("capacity above MAX_CAPACITY must fail");
        assert_eq!(
            too_big.code(),
            "invalid_argument",
            "too-large capacity code"
        );
    }

    #[test]
    fn insert_evicts_the_oldest_beyond_capacity() {
        let mut store = AssetStore::new(3).expect("capacity 3 is valid");
        let ids = fill(&mut store, 4);
        assert_eq!(store.len(), 3, "the store never exceeds capacity");
        assert!(store.peek(&ids[0]).is_none(), "the first asset is evicted");
        let survivor = store.peek(&ids[1]).expect("the second asset survives");
        assert_eq!(survivor.filename, "asset-1.bin", "survivor identity");
    }

    #[test]
    fn get_refreshes_recency() {
        let mut store = AssetStore::new(3).expect("capacity 3 is valid");
        let ids = fill(&mut store, 3);
        let touched = store.get(&ids[0]).expect("the asset is present");
        assert_eq!(touched.id, ids[0], "get returns the asset");
        put(&mut store, "newer.bin");
        let survivor = store.peek(&ids[0]).expect("the touched asset survives");
        assert_eq!(survivor.filename, "asset-0.bin", "touched asset kept");
        assert!(
            store.peek(&ids[1]).is_none(),
            "the untouched oldest asset is evicted"
        );
    }

    #[test]
    fn peek_leaves_recency_untouched() {
        let mut store = AssetStore::new(3).expect("capacity 3 is valid");
        let ids = fill(&mut store, 3);
        let seen = store.peek(&ids[0]).expect("the asset is present");
        assert_eq!(seen.filename, "asset-0.bin", "peek returns the asset");
        put(&mut store, "newer.bin");
        assert!(
            store.peek(&ids[0]).is_none(),
            "a merely peeked oldest asset is still evicted"
        );
    }

    #[test]
    fn get_and_peek_miss_the_unknown_id() {
        let mut store = AssetStore::new(3).expect("capacity 3 is valid");
        put(&mut store, "a.bin");
        assert!(store.get("deadbeefdeadbeef").is_none(), "get miss");
        assert!(store.peek("deadbeefdeadbeef").is_none(), "peek miss");
    }

    #[test]
    fn set_capacity_shrinks_and_counts_evictions() {
        let mut store = AssetStore::new(3).expect("capacity 3 is valid");
        let ids = fill(&mut store, 3);
        let evicted = store.set_capacity(1).expect("capacity 1 is valid");
        assert_eq!(evicted, 2, "the two oldest assets are evicted");
        assert_eq!(store.capacity(), 1, "the new capacity is kept");
        assert_eq!(store.len(), 1, "only the newest asset remains");
        let survivor = store.peek(&ids[2]).expect("the newest asset survives");
        assert_eq!(survivor.filename, "asset-2.bin", "survivor identity");
    }

    #[test]
    fn set_capacity_growing_keeps_every_asset() {
        let mut store = AssetStore::new(3).expect("capacity 3 is valid");
        fill(&mut store, 3);
        let evicted = store.set_capacity(10).expect("capacity 10 is valid");
        assert_eq!(evicted, 0, "growing evicts nothing");
        assert_eq!(store.len(), 3, "all assets are kept");
    }

    #[test]
    fn set_capacity_rejects_out_of_range_values() {
        let mut store = AssetStore::new(3).expect("capacity 3 is valid");
        fill(&mut store, 2);
        let error = store
            .set_capacity(0)
            .expect_err("capacity 0 must be rejected");
        assert_eq!(error.code(), "invalid_argument", "invalid resize code");
        assert_eq!(store.capacity(), 3, "a failed resize changes nothing");
        assert_eq!(store.len(), 2, "a failed resize evicts nothing");
    }

    #[test]
    fn total_bytes_tracks_the_entries() {
        let mut store = AssetStore::new(2).expect("capacity 2 is valid");
        store
            .insert("a.bin", "application/octet-stream", vec![7_u8; 3])
            .expect("a three-byte asset fits");
        store
            .insert("b.bin", "application/octet-stream", vec![7_u8; 4])
            .expect("a four-byte asset fits");
        assert_eq!(store.total_bytes(), 7, "both sizes add up");
        store
            .insert("c.bin", "application/octet-stream", vec![7_u8; 5])
            .expect("a five-byte asset fits");
        assert_eq!(
            store.total_bytes(),
            9,
            "the evicted asset leaves the sum behind"
        );
    }

    #[test]
    fn insert_rejects_oversized_data() {
        let mut store = AssetStore::new(2).expect("capacity 2 is valid");
        let huge = vec![0_u8; MAX_ASSET_BYTES.saturating_add(1)];
        let error = store
            .insert("huge.bin", "application/octet-stream", huge)
            .expect_err("data over MAX_ASSET_BYTES must be rejected");
        assert_eq!(error.code(), "limit_exceeded", "oversize error code");
        assert!(store.is_empty(), "rejected data is not stored");
    }

    #[test]
    fn insert_accepts_exactly_the_maximum_size() {
        let mut store = AssetStore::new(1).expect("capacity 1 is valid");
        let maximal = vec![0_u8; MAX_ASSET_BYTES];
        let asset = store
            .insert("max.bin", "application/octet-stream", maximal)
            .expect("MAX_ASSET_BYTES itself is allowed");
        assert_eq!(asset.data.len(), MAX_ASSET_BYTES, "full length stored");
        assert_eq!(store.total_bytes(), MAX_ASSET_BYTES, "size counted");
    }

    #[test]
    fn sanitized_filename_replaces_unsafe_characters() {
        assert_eq!(
            sanitized_filename("my file.png"),
            "my_file.png",
            "spaces become underscores"
        );
        assert_eq!(
            sanitized_filename("../../etc/passwd"),
            ".._.._etc_passwd",
            "slashes become underscores, dots stay"
        );
        assert_eq!(
            sanitized_filename("bad\0name;rm -rf"),
            "bad_name_rm_-rf",
            "control characters and punctuation become underscores"
        );
    }

    #[test]
    fn sanitized_filename_replaces_non_ascii_characters() {
        assert_eq!(
            sanitized_filename("caf\u{e9}.png"),
            "caf_.png",
            "the accented letter becomes one underscore"
        );
        assert_eq!(
            sanitized_filename("\u{1f600}art.png"),
            "_art.png",
            "the astral character becomes one underscore"
        );
    }

    #[test]
    fn sanitized_filename_bounds_length_and_emptiness() {
        assert_eq!(
            sanitized_filename(""),
            "asset.bin",
            "empty input falls back to the default name"
        );
        assert_eq!(
            sanitized_filename("   "),
            "___",
            "blank input is not empty input"
        );
        let long = "a".repeat(70);
        let kept = sanitized_filename(&long);
        assert_eq!(kept.chars().count(), 64, "at most 64 characters");
        assert_eq!(kept, "a".repeat(64), "the first 64 characters are kept");
    }

    #[test]
    fn insert_returns_the_sanitized_asset() {
        let mut store = AssetStore::new(2).expect("capacity 2 is valid");
        let asset = store
            .insert("weird name!.bin", "text/plain", vec![b'x'])
            .expect("a small insert succeeds");
        assert_eq!(asset.filename, "weird_name_.bin", "filename sanitized");
        assert_eq!(asset.mime, "text/plain", "media type kept verbatim");
        let peeked = store.peek(&asset.id).expect("the asset is stored");
        assert_eq!(peeked.filename, asset.filename, "stored identity matches");
        assert!(
            Arc::ptr_eq(&peeked.data, &asset.data),
            "the returned asset aliases the stored bytes"
        );
    }

    #[test]
    fn ids_are_lowercase_hex_and_unique_across_many_inserts() {
        let mut store = AssetStore::new(MAX_CAPACITY).expect("MAX_CAPACITY is valid");
        let mut seen: HashSet<String> = HashSet::new();
        for _ in 0_usize..1_000_usize {
            let asset = store
                .insert("x.bin", "application/octet-stream", vec![b'1'])
                .expect("a one-byte insert succeeds");
            assert_eq!(asset.id.len(), 16, "ids are 16 characters");
            let hex = asset
                .id
                .chars()
                .all(|ch| ch.is_ascii_digit() || ('a'..='f').contains(&ch));
            assert!(hex, "id {} is lowercase hex", asset.id);
            assert!(seen.insert(asset.id), "duplicate id detected");
        }
        assert_eq!(seen.len(), 1_000, "every id is distinct");
    }

    #[test]
    fn capacity_from_accepts_valid_values() {
        assert_eq!(capacity_from(Some("1")), 1, "the minimum is kept");
        assert_eq!(capacity_from(Some("50")), 50, "a plain number is kept");
        assert_eq!(
            capacity_from(Some("10000")),
            MAX_CAPACITY,
            "the maximum is kept"
        );
    }

    #[test]
    fn capacity_from_falls_back_on_invalid_values() {
        assert_eq!(capacity_from(None), DEFAULT_CAPACITY, "unset variable");
        assert_eq!(capacity_from(Some("")), DEFAULT_CAPACITY, "empty value");
        assert_eq!(capacity_from(Some("0")), DEFAULT_CAPACITY, "zero");
        let above = MAX_CAPACITY.saturating_add(1).to_string();
        assert_eq!(
            capacity_from(Some(above.as_str())),
            DEFAULT_CAPACITY,
            "above max"
        );
        assert_eq!(capacity_from(Some("-5")), DEFAULT_CAPACITY, "negative");
        assert_eq!(
            capacity_from(Some("many")),
            DEFAULT_CAPACITY,
            "not a number"
        );
        assert_eq!(
            capacity_from(Some("50 ")),
            DEFAULT_CAPACITY,
            "trailing space"
        );
    }

    #[test]
    fn mime_for_extension_maps_every_known_type() {
        let cases = [
            ("png", "image/png"),
            ("jpg", "image/jpeg"),
            ("jpeg", "image/jpeg"),
            ("gif", "image/gif"),
            ("wav", "audio/wav"),
            ("ogg", "audio/ogg"),
            ("mp3", "audio/mpeg"),
            ("flac", "audio/flac"),
            ("mid", "audio/midi"),
            ("midi", "audio/midi"),
            ("mus", "audio/x-mus"),
            ("json", "application/json"),
            ("txt", "text/plain; charset=utf-8"),
            ("wad", "application/x-doom-wad"),
        ];
        for (ext, expected) in cases {
            assert_eq!(mime_for_extension(ext), expected, "media type for {ext}");
        }
    }

    #[test]
    fn mime_for_extension_falls_back_for_unknowns() {
        assert_eq!(
            mime_for_extension("xyz"),
            "application/octet-stream",
            "unknown extension"
        );
        assert_eq!(
            mime_for_extension(""),
            "application/octet-stream",
            "empty extension"
        );
        assert_eq!(
            mime_for_extension("PNG"),
            "application/octet-stream",
            "matching is case-sensitive"
        );
    }
}
