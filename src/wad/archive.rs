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
//! [`write_file_atomic`] move archives to and from disk, landing a synced
//! unique sibling file on the target (by `rename`, or by a `hard_link` that
//! refuses an existing name) so a crash never truncates it and concurrent
//! writes never share a staging file. Nothing here knows about MCP or
//! sessions.
//!
//! Two pieces of the format live in modules beside this file: `directory`
//! reads the header's two numbers and the raw entries [`Wad::parse`] walks,
//! and `io` holds the staging file and the path errors
//! [`write_file_atomic`] and [`Wad::read_file`] report through.

mod directory;
mod io;

use alloc::sync::Arc;
use std::collections::HashMap;
use std::path::Path;

use serde::Serialize;

use crate::error::CyberError;
use crate::wad::bytes::{Writer, usize_to_u32};
use crate::wad::name::LumpName;

pub use io::write_file_atomic;

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
        directory::parse(bytes)
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

    /// Writes `data` as this archive's global lump `name`: over the last
    /// copy in place, or appended as a new one when the archive holds
    /// none. This is the one global-lump write every copy-on-write edit of
    /// `PNAMES`, `TEXTURE1`/`TEXTURE2`, `PLAYPAL` and `COLORMAP` goes
    /// through.
    ///
    /// # Errors
    ///
    /// [`CyberError::Internal`] when [`Wad::find_last`] names an index
    /// this archive has no lump at, which only a concurrent modification
    /// could cause.
    #[inline]
    pub fn put_global(&mut self, name: LumpName, data: Vec<u8>) -> Result<(), CyberError> {
        let Some(index) = self.find_last(name) else {
            self.lumps.push(Lump::new(name, data));
            return Ok(());
        };
        let Some(lump) = self.lumps.get_mut(index) else {
            return Err(CyberError::Internal(format!(
                "no lump at index {index} of the working archive: {name} was expected there"
            )));
        };
        lump.data = Arc::from(data);
        Ok(())
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
        let bytes = io::read_bytes(path)?;
        Self::parse(&bytes)
    }
}

#[cfg(test)]
mod tests;
