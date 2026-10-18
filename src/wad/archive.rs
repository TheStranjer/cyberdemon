//! The in-memory `WAD` archive: parsing, serialization, lookup and file I/O.
//!
//! [`Wad`] is the whole archive held as a directory of [`Lump`]s with their
//! data resolved into shared byte buffers, so cloning one for an undo
//! snapshot copies pointers, not bytes. [`Wad::parse`] accepts every layout
//! real engines produce, including overlapping data and a directory placed
//! before the payloads; [`Wad::to_bytes`] writes the canonical layout (data
//! in directory order, then the directory) and [`Wad::to_bytes_dedup`] the
//! same with duplicate payloads stored once at shared offsets.
//! [`Wad::read_file`] and
//! [`write_file_atomic`] move archives to and from disk, replacing the
//! target through a synced sibling file so a crash never truncates it.
//! Nothing here knows about MCP or sessions.

use alloc::sync::Arc;
use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};
use std::process;

use serde::Serialize;

use crate::error::CyberError;
use crate::wad::bytes::{Reader, Writer, i32_at, usize_to_u32};
use crate::wad::name::LumpName;

/// Bytes in a `WAD` header: magic, lump count and directory offset.
const HEADER_LEN: usize = 12;
/// Bytes in one directory entry: position, size and eight-byte name.
const ENTRY_LEN: usize = 16;
/// Offset of the lump count field in the header.
const LUMP_COUNT_AT: usize = 4;
/// Offset of the directory offset field in the header.
const DIRECTORY_AT_OFFSET: usize = 8;

/// Most lumps a `WAD` this crate parses may declare.
pub const MAX_LUMPS: i32 = 1_000_000;

/// Which magic a `WAD` header carries.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum WadKind {
    /// A full game archive, the `IWAD` magic.
    #[serde(rename = "IWAD")]
    Iwad,
    /// A patch or content archive, the `PWAD` magic.
    #[serde(rename = "PWAD")]
    Pwad,
}

impl WadKind {
    /// The four header bytes that spell this kind.
    #[must_use]
    #[inline]
    pub const fn magic(self) -> &'static [u8; 4] {
        match self {
            Self::Iwad => b"IWAD",
            Self::Pwad => b"PWAD",
        }
    }
}

/// The kind the header bytes denote, `None` for any other magic.
fn kind_of(magic: &[u8]) -> Option<WadKind> {
    if magic == WadKind::Iwad.magic().as_slice() {
        Some(WadKind::Iwad)
    } else if magic == WadKind::Pwad.magic().as_slice() {
        Some(WadKind::Pwad)
    } else {
        None
    }
}

/// A named, immutable `WAD` lump payload.
///
/// The data is an `Arc<[u8]>` so cloning a whole directory (undo snapshots
/// rely on this) shares the payloads.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Lump {
    /// The directory name, uppercase and `NUL`-trimmed.
    pub name: LumpName,
    /// The payload bytes, shared between every clone.
    pub data: Arc<[u8]>,
}

impl Lump {
    /// A lump named `name` holding `data`.
    #[must_use]
    #[inline]
    pub fn new(name: LumpName, data: Vec<u8>) -> Self {
        Self {
            name,
            data: data.into(),
        }
    }

    /// An empty marker lump, such as `F_START` or a map header.
    #[must_use]
    #[inline]
    pub fn marker(name: LumpName) -> Self {
        Self {
            name,
            data: Vec::new().into(),
        }
    }

    /// The payload size in bytes.
    #[must_use]
    #[inline]
    pub fn size(&self) -> usize {
        self.data.len()
    }
}

/// One raw directory entry before its data range is resolved.
#[derive(Clone, Copy, Debug)]
struct DirectoryEntry {
    /// The lump's byte offset in the file, as stored.
    filepos: i32,
    /// The lump's size in bytes, as stored.
    size: i32,
    /// The name, normalised through [`LumpName::from_raw`].
    name: LumpName,
}

impl DirectoryEntry {
    /// Reads the next sixteen directory bytes from `reader`.
    fn read(reader: &mut Reader<'_>) -> Result<Self, CyberError> {
        Ok(Self {
            filepos: reader.read_i32()?,
            size: reader.read_i32()?,
            name: reader.read_name()?,
        })
    }

    /// Resolves this entry against `bytes` into a [`Lump`]; `index` names
    /// the entry in errors. A zero size wins regardless of `filepos`,
    /// because engines leave marker positions as garbage.
    fn into_lump(self, bytes: &[u8], index: usize) -> Result<Lump, CyberError> {
        let name = self.name;
        if self.size == 0_i32 {
            return Ok(Lump::marker(name));
        }
        if self.size < 0_i32 {
            return Err(CyberError::Format(format!(
                "lump {index} ({name}) has negative size {}",
                self.size
            )));
        }
        if self.filepos < 0_i32 {
            return Err(CyberError::Format(format!(
                "lump {index} ({name}) has negative file position {}",
                self.filepos
            )));
        }
        let start = usize::try_from(self.filepos).map_err(|error| {
            CyberError::Format(format!(
                "lump {index} ({name}) file position {} does not fit an index on this platform: {error}",
                self.filepos
            ))
        })?;
        let width = usize::try_from(self.size).map_err(|error| {
            CyberError::Format(format!(
                "lump {index} ({name}) size {} does not fit an index on this platform: {error}",
                self.size
            ))
        })?;
        let end = start.checked_add(width).ok_or_else(|| {
            CyberError::Format(format!(
                "lump {index} ({name}) range {} + {width} overflows the address space",
                self.filepos
            ))
        })?;
        let data = bytes.get(start..end).ok_or_else(|| {
            CyberError::Format(format!(
                "lump {index} ({name}) at offset {start} with size {width} runs past the end of the {}-byte file",
                bytes.len()
            ))
        })?;
        Ok(Lump {
            name,
            data: Arc::from(data),
        })
    }
}

/// A `WAD` archive held entirely in memory: the header kind plus the
/// directory with every lump's data resolved.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Wad {
    /// The header magic, `IWAD` or `PWAD`.
    pub kind: WadKind,
    /// The directory in file order, duplicates and markers included.
    pub lumps: Vec<Lump>,
}

/// The layout one [`Wad::to_bytes_dedup`] write plans.
struct DedupPlan<'wad> {
    /// The `filepos` of every lump, duplicate payloads sharing an offset.
    offsets: Vec<u32>,
    /// The byte the directory starts at.
    directory_at: u32,
    /// The unique non-empty payloads, in the order they are written.
    payloads: Vec<&'wad [u8]>,
}

impl Default for Wad {
    /// An empty `PWAD`: the starting point for building a new archive.
    #[inline]
    fn default() -> Self {
        Self {
            kind: WadKind::Pwad,
            lumps: Vec::new(),
        }
    }
}

/// The position after `width` bytes of lump data at `position`, checked.
fn advance(position: usize, index: usize, width: usize) -> Result<usize, CyberError> {
    position.checked_add(width).ok_or_else(|| {
        CyberError::LimitExceeded(format!(
            "lump {index} data at offset {position} plus {width} bytes exceeds the addressable range"
        ))
    })
}

/// Walks the deduplicating layout [`Wad::to_bytes_dedup`] writes: the
/// shared `filepos` table, the directory position and the unique
/// non-empty payloads in write order, every position checked against the
/// format's `i32` field limits.
fn dedup_plan(lumps: &[Lump]) -> Result<DedupPlan<'_>, CyberError> {
    let mut offsets = Vec::with_capacity(lumps.len());
    let mut payloads: Vec<&[u8]> = Vec::new();
    let mut seen: HashMap<&[u8], u32> = HashMap::new();
    let mut position = HEADER_LEN;
    for (index, lump) in lumps.iter().enumerate() {
        let data: &[u8] = &lump.data;
        let at = if data.is_empty() {
            offset_field(position, &format!("lump {index} data"))?
        } else if let Some(shared) = seen.get(data) {
            *shared
        } else {
            let at = offset_field(position, &format!("lump {index} data"))?;
            seen.insert(data, at);
            payloads.push(data);
            position = advance(position, index, data.len())?;
            at
        };
        offsets.push(at);
    }
    let directory_at = offset_field(position, "directory")?;
    Ok(DedupPlan {
        offsets,
        directory_at,
        payloads,
    })
}

/// The `u32` form of a file position, limited to what a `WAD` can store:
/// every position lives in an `i32` field. `what` names the positioned
/// part so errors point at it.
///
/// # Errors
///
/// [`CyberError::LimitExceeded`] when `position` exceeds `u32::MAX` or
/// `i32::MAX`.
fn offset_field(position: usize, what: &str) -> Result<u32, CyberError> {
    let field = usize_to_u32(position, what)?;
    i32::try_from(field).map_err(|error| {
        CyberError::LimitExceeded(format!(
            "{what} offset {position} is past the i32::MAX limit of a WAD file: {error}"
        ))
    })?;
    Ok(field)
}

impl Wad {
    /// Parses a whole `WAD` file from its bytes.
    ///
    /// Layouts the engines produce and this keeps: lump data in any order
    /// with overlapping ranges, several directory entries covering one
    /// range, and zero-size entries with any `filepos`.
    ///
    /// # Errors
    ///
    /// [`CyberError::LimitExceeded`] when `numlumps` exceeds
    /// [`MAX_LUMPS`]; [`CyberError::Format`] for a file shorter than the
    /// header, an unknown magic, a negative `numlumps` or directory
    /// offset, a directory not fully inside the file, or a lump whose
    /// stored range runs past the end.
    #[inline]
    pub fn parse(bytes: &[u8]) -> Result<Self, CyberError> {
        let length = bytes.len();
        if length < HEADER_LEN {
            return Err(CyberError::Format(format!(
                "WAD file is {length} bytes, shorter than the {HEADER_LEN}-byte header"
            )));
        }
        let magic = bytes.get(..4).unwrap_or(&[]);
        let kind = kind_of(magic).ok_or_else(|| {
            CyberError::Format(format!(
                "unknown WAD magic {magic:02X?}, expected IWAD or PWAD"
            ))
        })?;
        let count = lump_count_at(bytes)?;
        let directory_at = directory_start(bytes, count)?;
        let mut lumps = Vec::with_capacity(count);
        let mut reader = Reader::at(bytes, directory_at);
        for index in 0_usize..count {
            let raw = DirectoryEntry::read(&mut reader)?;
            lumps.push(raw.into_lump(bytes, index)?);
        }
        Ok(Self { kind, lumps })
    }

    /// Serializes to canonical `WAD` bytes: header, then every lump's data
    /// in directory order starting at byte 12, then the directory. A
    /// zero-size lump gets the current write position as its `filepos`.
    ///
    /// # Errors
    ///
    /// [`CyberError::LimitExceeded`] when the lump count, a lump size or
    /// any written position does not fit the format's `i32` fields.
    #[inline]
    pub fn to_bytes(&self) -> Result<Vec<u8>, CyberError> {
        let count = usize_to_u32(self.lumps.len(), "lump")?;
        let (offsets, directory_at) = self.plan()?;
        let mut out = Writer::new();
        out.put_bytes(self.kind.magic());
        out.put_u32(count);
        out.put_u32(directory_at);
        for lump in &self.lumps {
            out.put_bytes(&lump.data);
        }
        for (lump, position) in self.lumps.iter().zip(&offsets) {
            out.put_u32(*position);
            out.put_u32(usize_to_u32(lump.size(), "lump")?);
            out.put_bytes(&lump.name.raw());
        }
        Ok(out.into_inner())
    }

    /// The `filepos` [`to_bytes`](Self::to_bytes) would give each lump, in
    /// directory order; for tools that report positions without
    /// serializing.
    ///
    /// # Errors
    ///
    /// [`CyberError::LimitExceeded`] under the same conditions as
    /// [`to_bytes`](Self::to_bytes).
    #[inline]
    pub fn data_offsets(&self) -> Result<Vec<u32>, CyberError> {
        self.plan().map(|(offsets, _directory_at)| offsets)
    }

    /// Serializes like [`to_bytes`](Self::to_bytes) but writes every
    /// distinct non-empty payload once: later directory entries holding
    /// equal bytes share the first copy's offset, which the `WAD` format
    /// allows (and [`parse`](Self::parse) reads back). Zero-size lumps
    /// keep the current write position as their `filepos`, exactly as the
    /// canonical layout gives them.
    ///
    /// Returns the file bytes and the payload bytes the sharing saved.
    ///
    /// # Errors
    ///
    /// [`CyberError::LimitExceeded`] under the same conditions as
    /// [`to_bytes`](Self::to_bytes).
    #[inline]
    pub fn to_bytes_dedup(&self) -> Result<(Vec<u8>, usize), CyberError> {
        let count = usize_to_u32(self.lumps.len(), "lump")?;
        let planned = dedup_plan(&self.lumps)?;
        let mut out = Writer::new();
        out.put_bytes(self.kind.magic());
        out.put_u32(count);
        out.put_u32(planned.directory_at);
        for payload in &planned.payloads {
            out.put_bytes(payload);
        }
        for (lump, position) in self.lumps.iter().zip(&planned.offsets) {
            out.put_u32(*position);
            out.put_u32(usize_to_u32(lump.size(), "lump")?);
            out.put_bytes(&lump.name.raw());
        }
        let written: usize = planned.payloads.iter().map(|payload| payload.len()).sum();
        let total: usize = self.lumps.iter().map(Lump::size).sum();
        Ok((out.into_inner(), total.saturating_sub(written)))
    }

    /// Walks the layout [`to_bytes`](Self::to_bytes) writes: the `u32`
    /// data position of every lump plus the directory position, each
    /// checked against the format's `i32` field limits.
    fn plan(&self) -> Result<(Vec<u32>, u32), CyberError> {
        let mut offsets = Vec::with_capacity(self.lumps.len());
        let mut position = HEADER_LEN;
        for (index, lump) in self.lumps.iter().enumerate() {
            offsets.push(offset_field(position, &format!("lump {index} data"))?);
            position = advance(position, index, lump.data.len())?;
        }
        let directory_at = offset_field(position, "directory")?;
        Ok((offsets, directory_at))
    }

    /// The index of the last lump named `name`, the lookup `Doom` itself
    /// performs when patches shadow base lumps.
    #[must_use]
    #[inline]
    pub fn find_last(&self, name: LumpName) -> Option<usize> {
        self.lumps.iter().rposition(|lump| lump.name == name)
    }

    /// The index of the first lump named `name`.
    #[must_use]
    #[inline]
    pub fn find_first(&self, name: LumpName) -> Option<usize> {
        self.lumps.iter().position(|lump| lump.name == name)
    }

    /// The indices of every lump named `name`, in directory order.
    #[must_use]
    #[inline]
    pub fn find_all(&self, name: LumpName) -> Vec<usize> {
        self.lumps
            .iter()
            .enumerate()
            .filter_map(|(index, lump)| (lump.name == name).then_some(index))
            .collect()
    }

    /// The index of the first lump named `name` at or after `start`;
    /// `None` when `start` is past the end or no later entry matches.
    #[must_use]
    #[inline]
    pub fn find_from(&self, name: LumpName, start: usize) -> Option<usize> {
        self.lumps
            .iter()
            .skip(start)
            .position(|lump| lump.name == name)
            .and_then(|offset| offset.checked_add(start))
    }

    /// The number of directory entries.
    #[must_use]
    #[inline]
    pub fn len(&self) -> usize {
        self.lumps.len()
    }

    /// Whether the archive holds no lumps.
    #[must_use]
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.lumps.is_empty()
    }

    /// Reads `path` and parses it with [`parse`](Self::parse).
    ///
    /// # Errors
    ///
    /// [`CyberError::Io`] naming the path when the file cannot be read (a
    /// missing file is an I/O failure too), or whatever
    /// [`parse`](Self::parse) reports for malformed contents.
    #[inline]
    pub fn read_file(path: &Path) -> Result<Self, CyberError> {
        let bytes = fs::read(path).map_err(|error| io_error(path, "read", &error))?;
        Self::parse(&bytes)
    }
}

/// The declared lump count as a `usize`, checked for sanity before any
/// memory is reserved for it.
///
/// # Errors
///
/// [`CyberError::Format`] for a negative count,
/// [`CyberError::LimitExceeded`] above [`MAX_LUMPS`].
fn lump_count_at(bytes: &[u8]) -> Result<usize, CyberError> {
    let raw = i32_at(bytes, LUMP_COUNT_AT)?;
    if raw < 0_i32 {
        return Err(CyberError::Format(format!(
            "WAD declares a negative lump count: {raw}"
        )));
    }
    if raw > MAX_LUMPS {
        return Err(CyberError::LimitExceeded(format!(
            "WAD declares {raw} lumps, the limit is {MAX_LUMPS}"
        )));
    }
    usize::try_from(raw).map_err(|error| {
        CyberError::LimitExceeded(format!(
            "WAD lump count {raw} does not fit an index on this platform: {error}"
        ))
    })
}

/// The directory start offset, checked so all `count` entries lie inside
/// `bytes`.
///
/// # Errors
///
/// [`CyberError::Format`] for a negative offset or a directory that does
/// not fit in the file; the messages carry the numbers.
fn directory_start(bytes: &[u8], count: usize) -> Result<usize, CyberError> {
    let raw = i32_at(bytes, DIRECTORY_AT_OFFSET)?;
    if raw < 0_i32 {
        return Err(CyberError::Format(format!(
            "WAD declares a negative directory offset: {raw}"
        )));
    }
    let start = usize::try_from(raw).map_err(|error| {
        CyberError::LimitExceeded(format!(
            "directory offset {raw} does not fit an index on this platform: {error}"
        ))
    })?;
    let width = count.checked_mul(ENTRY_LEN).ok_or_else(|| {
        CyberError::Format(format!(
            "directory of {count} entries is too wide for the file format"
        ))
    })?;
    let end = start.checked_add(width).ok_or_else(|| {
        CyberError::Format(format!(
            "directory of {count} entries at offset {start} overflows the address space"
        ))
    })?;
    if end > bytes.len() {
        return Err(CyberError::Format(format!(
            "directory of {count} entries at offset {start} needs {width} bytes and ends at {end}, past the end of the {}-byte file",
            bytes.len()
        )));
    }
    Ok(start)
}

/// The sibling path `<name>.<pid>.tmp` an atomic write stages through.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] when `path` has no file name to extend.
fn staging_path(path: &Path) -> Result<PathBuf, CyberError> {
    let name = path.file_name().ok_or_else(|| {
        CyberError::InvalidArgument(format!(
            "{} has no file name to stage a sibling write beside",
            path.display()
        ))
    })?;
    let mut staging = name.to_os_string();
    staging.push(".");
    staging.push(process::id().to_string());
    staging.push(".tmp");
    Ok(path.with_file_name(staging))
}

/// An `Io` failure naming the path and what was attempted on it.
fn io_error(path: &Path, action: &str, error: &io::Error) -> CyberError {
    CyberError::Io(format!("cannot {action} {}: {error}", path.display()))
}

/// A staging-file creation failure: a missing parent directory is
/// `NotFound`, anything else `Io`.
fn staging_error(path: &Path, error: &io::Error) -> CyberError {
    if error.kind() == io::ErrorKind::NotFound {
        return CyberError::NotFound(format!(
            "cannot write {}: its parent directory does not exist ({error})",
            path.display()
        ));
    }
    io_error(path, "create a temporary file beside", error)
}

/// Removes a staging file after a failed write. Cleanup cannot add
/// information: the operation already decided its error.
fn remove_staging(staging: &Path) {
    let removal = fs::remove_file(staging);
    drop(removal);
}

/// Writes `bytes` to `path` atomically via a sibling staging file.
///
/// The bytes land in `<name>.<pid>.tmp`, are `sync_all`-ed, then the
/// staging file is `rename`d over the target, so readers see either the
/// old file or the complete new one. The staging file is removed when
/// any step after creation fails. The parent directory of `path` must
/// exist.
///
/// # Errors
///
/// [`CyberError::NotFound`] when the parent directory of `path` does not
/// exist; [`CyberError::Io`] naming the path for write, sync or rename
/// failures; [`CyberError::InvalidArgument`] when `path` has no file name.
#[inline]
pub fn write_file_atomic(path: &Path, bytes: &[u8]) -> Result<(), CyberError> {
    let staging = staging_path(path)?;
    let mut file = File::create(&staging).map_err(|error| staging_error(path, &error))?;
    let written = file
        .write_all(bytes)
        .and_then(|()| file.sync_all())
        .map_err(|error| io_error(path, "write", &error));
    drop(file);
    let renamed = written.and_then(|()| {
        fs::rename(&staging, path).map_err(|error| io_error(path, "replace", &error))
    });
    if renamed.is_err() {
        remove_staging(&staging);
    }
    renamed
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use serde_json::json;

    use super::{
        DirectoryEntry, HEADER_LEN, Lump, MAX_LUMPS, Wad, WadKind, directory_start, offset_field,
        write_file_atomic,
    };
    use crate::wad::name::LumpName;

    /// A valid parsed name for test data.
    fn name(text: &str) -> LumpName {
        LumpName::parse(text).expect("test names are valid")
    }

    /// One raw sixteen-byte directory entry.
    fn raw_entry(filepos: i32, size: i32, entry_name: [u8; 8]) -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&filepos.to_le_bytes());
        bytes.extend_from_slice(&size.to_le_bytes());
        bytes.extend_from_slice(&entry_name);
        bytes
    }

    /// A whole file: a header for `count` entries at `directory_at`, then
    /// `parts` concatenated behind it.
    fn build_file(kind: WadKind, count: i32, directory_at: i32, parts: &[&[u8]]) -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(kind.magic());
        bytes.extend_from_slice(&count.to_le_bytes());
        bytes.extend_from_slice(&directory_at.to_le_bytes());
        for part in parts {
            bytes.extend_from_slice(part);
        }
        bytes
    }

    /// A `Wad` of the given kind with markers, payloads and a duplicate
    /// name.
    fn sample(kind: WadKind) -> Wad {
        Wad {
            kind,
            lumps: vec![
                Lump::marker(name("F_START")),
                Lump::new(name("TEXTURE1"), b"dir-bytes".to_vec()),
                Lump::marker(name("P_START")),
                Lump::new(name("S_SKY1"), vec![0_u8, 1, 2]),
                Lump::new(name("texture1"), b"patched".to_vec()),
                Lump::marker(name("P_END")),
            ],
        }
    }

    /// The directory entries of an encoded file, as (filepos, size).
    fn written_entries(wad_bytes: &[u8], count: usize) -> Vec<(i32, i32)> {
        let width = count.checked_mul(16).expect("test directories are small");
        let start = wad_bytes
            .len()
            .checked_sub(width)
            .expect("the test directory fits");
        wad_bytes[start..]
            .chunks_exact(16)
            .map(|chunk| {
                assert!(chunk.len() > 7, "directory chunks are sixteen bytes");
                let filepos = i32::from_le_bytes(chunk[0..4].try_into().expect("four bytes"));
                let size = i32::from_le_bytes(chunk[4..8].try_into().expect("four bytes"));
                (filepos, size)
            })
            .collect()
    }

    /// The names of the entries directly inside `dir`, sorted.
    fn fs_entries(dir: &Path) -> Vec<String> {
        let mut names = fs::read_dir(dir)
            .expect("read the scratch directory")
            .map(|entry| {
                entry
                    .expect("a readable directory entry")
                    .file_name()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect::<Vec<String>>();
        names.sort();
        names
    }

    #[test]
    fn kind_magic_and_serialization() {
        assert_eq!(*WadKind::Iwad.magic(), *b"IWAD", "IWAD bytes");
        assert_eq!(*WadKind::Pwad.magic(), *b"PWAD", "PWAD bytes");
        assert_eq!(
            serde_json::to_value(WadKind::Iwad).expect("serializes"),
            json!("IWAD"),
            "IWAD serializes as the magic"
        );
        assert_eq!(
            serde_json::to_value(WadKind::Pwad).expect("serializes"),
            json!("PWAD"),
            "PWAD serializes as the magic"
        );
    }

    #[test]
    fn lump_constructors_and_size() {
        let filled = Lump::new(name("SPLY3"), vec![b'a'; 4]);
        assert_eq!(filled.size(), 4, "payload size");
        assert_eq!(filled.data.as_ref(), b"aaaa", "payload bytes");
        let marker = Lump::marker(name("F_END"));
        assert_eq!(marker.size(), 0, "markers are empty");
        assert!(marker.data.is_empty(), "markers hold no bytes");
        assert_eq!(marker.name, name("f_end"), "names are case-folded");
    }

    #[test]
    fn default_is_an_empty_pwad() {
        let wad = Wad::default();
        assert_eq!(wad.kind, WadKind::Pwad, "default kind");
        assert!(wad.is_empty(), "default has no lumps");
        assert_eq!(wad.len(), 0, "default length");
    }

    #[test]
    fn round_trips_through_bytes_for_both_kinds() {
        for kind in [WadKind::Iwad, WadKind::Pwad] {
            let wad = sample(kind);
            let encoded = wad.to_bytes().expect("a small WAD serializes");
            assert_eq!(&encoded[..4], kind.magic(), "magic is first");
            let parsed = Wad::parse(&encoded).expect("the encoding parses back");
            assert_eq!(parsed, wad, "round trip is exact for {kind:?}");
        }
    }

    #[test]
    fn empty_wad_round_trips_as_a_bare_header() {
        let encoded = Wad::default().to_bytes().expect("empty serializes");
        assert_eq!(encoded.len(), HEADER_LEN, "just the header");
        assert_eq!(
            Wad::parse(&encoded).expect("bare header parses"),
            Wad::default(),
            "empty round trip"
        );
    }

    #[test]
    fn to_bytes_lays_data_then_the_directory_in_order() {
        let wad = sample(WadKind::Pwad);
        let encoded = wad.to_bytes().expect("serializes");
        let entries = written_entries(&encoded, wad.len());
        let mut expected = HEADER_LEN;
        for (index, lump) in wad.lumps.iter().enumerate() {
            let (filepos, size) = entries[index];
            let start = usize::try_from(filepos).expect("test files are small");
            let width = usize::try_from(size).expect("test files are small");
            assert_eq!(
                start, expected,
                "entry {index} records the running position"
            );
            assert_eq!(width, lump.size(), "entry {index} records the size");
            let end = start.checked_add(width).expect("test files are small");
            assert_eq!(
                &encoded[start..end],
                lump.data.as_ref(),
                "entry {index} payload sits at its recorded offset"
            );
            expected = end;
        }
        let directory_at = i32::from_le_bytes(encoded[8..12].try_into().expect("four bytes"));
        assert_eq!(
            usize::try_from(directory_at).expect("test files are small"),
            expected,
            "the header names the directory start"
        );
    }

    #[test]
    fn zero_size_lumps_get_the_current_position() {
        let wad = Wad {
            kind: WadKind::Pwad,
            lumps: vec![
                Lump::marker(name("A_START")),
                Lump::new(name("DATA"), vec![b'x'; 3]),
                Lump::marker(name("A_END")),
            ],
        };
        let offsets = wad.data_offsets().expect("small WAD offsets");
        assert_eq!(offsets[0], 12, "marker before the data gets 12");
        assert_eq!(offsets[1], 12, "the payload follows at 12");
        assert_eq!(offsets[2], 15, "the end marker sits after the payload");
        let encoded = wad.to_bytes().expect("serializes");
        let entries = written_entries(&encoded, offsets.len());
        for (index, offset) in offsets.iter().enumerate() {
            let stored = u32::try_from(entries[index].0).expect("test files are small");
            assert_eq!(stored, *offset, "entry {index} was written as planned");
        }
    }

    /// A `Wad` carrying two byte-identical payloads, a third distinct one,
    /// and a marker between the twins.
    fn duplicated() -> Wad {
        Wad {
            kind: WadKind::Pwad,
            lumps: vec![
                Lump::new(name("FIRST"), b"same-same".to_vec()),
                Lump::marker(name("NOTE")),
                Lump::new(name("SECOND"), b"same-same".to_vec()),
                Lump::new(name("THIRD"), b"different!".to_vec()),
                Lump::new(name("FOURTH"), b"same-same".to_vec()),
            ],
        }
    }

    #[test]
    fn to_bytes_dedup_round_trips_through_parse_to_an_equal_wad() {
        let wad = duplicated();
        let (bytes, saved) = wad.to_bytes_dedup().expect("a small WAD serializes");
        assert_eq!(
            Wad::parse(&bytes).expect("shared offsets parse back"),
            wad,
            "the round trip is exact even though entries share data ranges"
        );
        assert_eq!(&bytes[..4], b"PWAD", "the magic is written as usual");
        assert_eq!(saved, 18, "the two later copies of the 9-byte payload");
    }

    #[test]
    fn to_bytes_dedup_is_smaller_with_duplicates_and_identical_without() {
        let wad = duplicated();
        let plain = wad.to_bytes().expect("serializes");
        let (deduped, saved) = wad.to_bytes_dedup().expect("serializes");
        assert!(
            deduped.len() < plain.len(),
            "sharing the payload shrinks the file: {} vs {saved} saved",
            deduped.len()
        );
        assert_eq!(
            plain.len().saturating_sub(deduped.len()),
            saved,
            "the reported saving is the size difference"
        );
        let unique = sample(WadKind::Pwad);
        let (unique_dedup, unique_saved) = unique.to_bytes_dedup().expect("serializes");
        assert_eq!(
            unique_dedup,
            unique.to_bytes().expect("serializes"),
            "no duplicates means the plain layout byte for byte"
        );
        assert_eq!(unique_saved, 0_usize, "and nothing is saved");
    }

    #[test]
    fn to_bytes_dedup_gives_the_twins_one_shared_offset() {
        let wad = duplicated();
        let (bytes, _saved) = wad.to_bytes_dedup().expect("serializes");
        let entries = written_entries(&bytes, wad.len());
        assert_eq!(
            entries[0].0, entries[2].0,
            "the twin shares the first offset"
        );
        assert_eq!(entries[0].0, entries[4].0, "and so does the third copy");
        assert_ne!(
            entries[0].0, entries[3].0,
            "the distinct payload sits elsewhere"
        );
        assert_eq!(
            entries[2].1, 9_i32,
            "the size field still names the whole payload"
        );
        let start = usize::try_from(entries[0].0).expect("test files are small");
        let end = start.checked_add(9_usize).expect("test files are small");
        assert_eq!(
            &bytes[start..end],
            b"same-same",
            "at the first copy's bytes"
        );
    }

    #[test]
    fn data_offsets_match_the_written_directory() {
        let wad = sample(WadKind::Iwad);
        let offsets = wad.data_offsets().expect("offsets");
        let encoded = wad.to_bytes().expect("serializes");
        let entries = written_entries(&encoded, offsets.len());
        for (index, (entry, offset)) in entries.iter().zip(offsets.iter()).enumerate() {
            let stored = u32::try_from(entry.0).expect("test files are small");
            assert_eq!(stored, *offset, "offset {index} matches the plan");
        }
    }

    #[test]
    fn parse_accepts_overlapping_and_shared_ranges() {
        let mut directory = Vec::new();
        directory.extend(raw_entry(12, 4, *b"FIRST\0\0\0"));
        directory.extend(raw_entry(14, 4, *b"SECOND\0\0"));
        directory.extend(raw_entry(12, 4, *b"FIRST\0\0\0"));
        directory.extend(raw_entry(18, 0, *b"-1\0\0\0\0\0\0"));
        let file = build_file(WadKind::Pwad, 4, 20, &[b"ABCDEFGH", &directory]);
        let wad = Wad::parse(&file).expect("overlaps are legal");
        assert_eq!(wad.lumps[0].data.as_ref(), b"ABCD", "first window");
        assert_eq!(wad.lumps[1].data.as_ref(), b"CDEF", "overlapping window");
        assert_eq!(wad.lumps[2].data.as_ref(), b"ABCD", "shared offset window");
        assert_eq!(wad.lumps[2].name, wad.lumps[0].name, "same name too");
        assert!(wad.lumps[3].data.is_empty(), "garbage position, zero size");
        assert_eq!(wad.lumps[3].name.raw()[..2], *b"-1", "odd bytes kept");
    }

    #[test]
    fn parse_accepts_a_negative_filepos_on_an_empty_lump() {
        let directory = raw_entry(-1_234_567, 0, *b"MAP01\0\0\0");
        let file = build_file(WadKind::Iwad, 1, 12, &[&directory]);
        let wad = Wad::parse(&file).expect("markers may hold any filepos");
        assert_eq!(wad.lumps[0], Lump::marker(name("MAP01")), "empty marker");
    }

    #[test]
    fn parse_accepts_a_directory_before_the_data() {
        let directory = raw_entry(28, 4, *b"TAIL\0\0\0\0");
        let file = build_file(WadKind::Pwad, 1, 12, &[&directory, b"DATA"]);
        let wad = Wad::parse(&file).expect("the directory may sit before payloads");
        assert_eq!(
            wad.lumps[0].data.as_ref(),
            b"DATA",
            "payload written after the directory"
        );
    }

    #[test]
    fn parse_rejects_a_short_header() {
        let error = Wad::parse(b"PWA").expect_err("three bytes are no WAD");
        assert_eq!(error.code(), "format_error", "short header code");
        assert!(
            error.to_string().contains("shorter than"),
            "short header message: {error}"
        );
    }

    #[test]
    fn parse_rejects_an_unknown_magic() {
        let error =
            Wad::parse(b"SLAD\0\0\0\0\x0c\0\0\0").expect_err("an upside-down magic is not a WAD");
        assert_eq!(error.code(), "format_error", "bad magic code");
        assert!(
            error.to_string().contains("unknown WAD magic"),
            "bad magic message: {error}"
        );
    }

    #[test]
    fn parse_rejects_a_negative_lump_count() {
        let file = build_file(WadKind::Pwad, -1, 12, &[]);
        let error = Wad::parse(&file).expect_err("negative counts fail");
        assert_eq!(error.code(), "format_error", "negative count code");
        assert!(
            error.to_string().contains("-1"),
            "the message carries the number: {error}"
        );
    }

    #[test]
    fn parse_rejects_a_negative_directory_offset() {
        let file = build_file(WadKind::Pwad, 0, -16, &[]);
        let error = Wad::parse(&file).expect_err("negative offsets fail");
        assert_eq!(error.code(), "format_error", "negative offset code");
        assert!(
            error.to_string().contains("negative directory offset"),
            "offset message: {error}"
        );
    }

    #[test]
    fn parse_rejects_a_directory_past_the_end() {
        let file = build_file(WadKind::Pwad, 4, 1_000, &[]);
        let error = Wad::parse(&file).expect_err("the directory must fit");
        assert_eq!(error.code(), "format_error", "directory bound code");
        let message = error.to_string();
        assert!(
            message.contains("directory") && message.contains("1000") && message.contains('4'),
            "the message carries the numbers: {message}"
        );
    }

    #[test]
    fn parse_rejects_an_oversized_lump_count() {
        let file = build_file(WadKind::Pwad, MAX_LUMPS + 1, 12, &[]);
        let error = Wad::parse(&file).expect_err("huge counts are refused");
        assert_eq!(error.code(), "limit_exceeded", "count limit code");
        assert!(
            error.to_string().contains("limit"),
            "the message names the limit: {error}"
        );
    }

    #[test]
    fn parse_rejects_a_lump_running_past_the_end() {
        let directory = raw_entry(8, 100, *b"TOOBIG\0\0");
        let file = build_file(WadKind::Pwad, 1, 16, &[b"data", &directory]);
        let error = Wad::parse(&file).expect_err("ranges must fit the file");
        assert_eq!(error.code(), "format_error", "lump bound code");
        let message = error.to_string();
        assert!(
            message.contains("TOOBIG") && message.contains("lump 0"),
            "the message names the index and name: {message}"
        );
    }

    #[test]
    fn parse_rejects_a_negative_size() {
        let directory = raw_entry(0, -5, *b"NEGSIZE\0");
        let file = build_file(WadKind::Pwad, 1, 13, &[b"x", &directory]);
        let error = Wad::parse(&file).expect_err("sizes cannot be negative with data");
        assert_eq!(error.code(), "format_error", "negative size code");
        assert!(
            error.to_string().contains("negative size"),
            "negative size message: {error}"
        );
    }

    #[test]
    fn parse_rejects_a_negative_filepos_on_a_data_lump() {
        let directory = raw_entry(-4, 3, *b"NEGPOS\0\0");
        let file = build_file(WadKind::Pwad, 1, 15, &[b"abc", &directory]);
        let error = Wad::parse(&file).expect_err("data cannot start before the header");
        assert_eq!(error.code(), "format_error", "negative position code");
        let message = error.to_string();
        assert!(
            message.contains("NEGPOS") && message.contains("negative file position"),
            "the message names the lump: {message}"
        );
    }

    #[test]
    fn directory_width_checks_are_exercised_directly() {
        let file = build_file(WadKind::Pwad, 0, 12, &[]);
        let wide = usize::MAX.checked_div(8).expect("divides");
        let overflow = directory_start(&file, wide).expect_err("16 times this count overflows");
        assert_eq!(overflow.code(), "format_error", "overflowing width code");
        assert!(
            overflow.to_string().contains("too wide"),
            "wide directory message: {overflow}"
        );
        let huge = usize::MAX.checked_div(16).expect("divides");
        let far = build_file(WadKind::Pwad, 0, i32::MAX, &[]);
        let sum = directory_start(&far, huge).expect_err("i32::MAX plus this width overflows");
        assert!(
            sum.to_string().contains("overflows the address space"),
            "address space message: {sum}"
        );
        let past = usize::try_from(i32::MAX)
            .expect("fits")
            .checked_mul(4)
            .expect("test count fits");
        let error = directory_start(&file, past).expect_err("the directory must fit the file");
        assert!(
            error.to_string().contains("past the end"),
            "past-the-end message: {error}"
        );
    }

    #[test]
    fn offset_field_refuses_positions_past_the_format() {
        assert_eq!(
            offset_field(12, "lump 0 data").expect("12 fits"),
            12_u32,
            "small positions pass"
        );
        let over_signed = usize::try_from(i32::MAX)
            .expect("fits")
            .checked_add(1)
            .expect("one more");
        let wide_error =
            offset_field(over_signed, "lump 1 data").expect_err("i32::MAX plus one fails");
        assert_eq!(wide_error.code(), "limit_exceeded", "past-i32 code");
        assert!(
            wide_error.to_string().contains("i32::MAX"),
            "past-i32 message: {wide_error}"
        );
        let over_wide = usize::try_from(u32::MAX)
            .expect("fits")
            .checked_add(1)
            .expect("one more");
        let huge_error = offset_field(over_wide, "directory").expect_err("u32::MAX plus one fails");
        assert_eq!(huge_error.code(), "limit_exceeded", "past-u32 code");
        assert!(
            huge_error.to_string().contains("32 bits"),
            "past-u32 message: {huge_error}"
        );
    }

    #[test]
    fn entry_into_lump_copies_the_bytes_it_names() {
        let raw = DirectoryEntry {
            filepos: 2,
            size: 3,
            name: name("WINDOW"),
        };
        let lump = raw.into_lump(b"0123456789", 7).expect("the window fits");
        assert_eq!(lump.data.as_ref(), b"234", "the addressed window");
        assert_eq!(lump.name, name("WINDOW"), "the name carries over");
    }

    #[test]
    fn lookups_find_duplicates_in_engine_order() {
        let wad = sample(WadKind::Pwad);
        let texture = name("TEXTURE1");
        assert_eq!(wad.find_first(texture), Some(1), "first hit");
        assert_eq!(
            wad.find_last(texture),
            Some(4),
            "the patch shadows the base"
        );
        assert_eq!(wad.find_all(texture), vec![1, 4], "every hit");
        assert_eq!(wad.find_from(texture, 2), Some(4), "from a later start");
        assert_eq!(wad.find_from(texture, 5), None, "past the last hit");
        assert_eq!(wad.find_from(texture, 99), None, "past the end");
        assert_eq!(wad.find_first(name("NOPE")), None, "missing name");
        assert_eq!(wad.len(), 6, "directory length");
        assert!(!wad.is_empty(), "six lumps are not empty");
    }

    #[test]
    fn read_file_round_trips_a_written_archive() {
        let dir = tempfile::tempdir().expect("create a scratch directory");
        let path = dir.path().join("sample.wad");
        let wad = sample(WadKind::Iwad);
        let encoded = wad.to_bytes().expect("serializes");
        write_file_atomic(&path, &encoded).expect("the first write succeeds");
        assert_eq!(
            Wad::read_file(&path).expect("read back"),
            wad,
            "file round trip"
        );
        let replacement = Wad::default().to_bytes().expect("serializes");
        write_file_atomic(&path, &replacement).expect("an existing target is replaced");
        assert_eq!(
            Wad::read_file(&path).expect("reread"),
            Wad::default(),
            "the target holds the new archive"
        );
        assert_eq!(
            fs_entries(dir.path()),
            vec!["sample.wad".to_owned()],
            "no staging files remain"
        );
    }

    #[test]
    fn read_file_reports_missing_paths_as_io() {
        let dir = tempfile::tempdir().expect("create a scratch directory");
        let error = Wad::read_file(&dir.path().join("absent.wad"))
            .expect_err("absent files cannot be read");
        assert_eq!(error.code(), "io_error", "missing file code");
        assert!(
            error.to_string().contains("absent.wad"),
            "the message names the path: {error}"
        );
    }

    #[test]
    fn write_file_atomic_needs_an_existing_parent() {
        let dir = tempfile::tempdir().expect("create a scratch directory");
        let path = dir.path().join("gone").join("out.wad");
        let error = write_file_atomic(&path, b"PWAD").expect_err("no parent directory");
        assert_eq!(error.code(), "not_found", "missing parent code");
        assert!(
            error.to_string().contains("parent directory"),
            "missing parent message: {error}"
        );
    }

    #[test]
    fn write_file_atomic_removes_its_staging_file_when_rename_fails() {
        let blocker = tempfile::tempdir().expect("a directory cannot be renamed over");
        let target = blocker.path().to_path_buf();
        let error = write_file_atomic(&target, b"PWAD").expect_err("rename onto a directory");
        assert_eq!(error.code(), "io_error", "failed rename code");
        assert_eq!(
            fs_entries(&target),
            Vec::<String>::new(),
            "no staging files remain"
        );
    }
}
