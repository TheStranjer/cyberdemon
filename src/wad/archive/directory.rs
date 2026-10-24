//! The directory half of the in-memory archive: the raw [`DirectoryEntry`]
//! record and the two header checks that bound it.
//!
//! [`parse`] is the whole-archive walk [`Wad::parse`](super::Wad::parse)
//! delegates to: it reads the magic, checks the declared lump count and the
//! directory offset, then resolves every entry against the file bytes, so
//! overlapping ranges and shared offsets come out as separate [`Lump`]s. A
//! zero-size entry keeps whatever garbage `filepos` the writer left; a data
//! entry whose stored range leaves the file is a format error naming its
//! index and name. Nothing here touches the filesystem.

use alloc::sync::Arc;

use super::{
    DIRECTORY_AT_OFFSET, ENTRY_LEN, HEADER_LEN, LUMP_COUNT_AT, Lump, MAX_LUMPS, Wad, kind_of,
};
use crate::error::CyberError;
use crate::wad::bytes::{Reader, i32_at};
use crate::wad::name::LumpName;

/// One raw directory entry before its data range is resolved.
#[derive(Clone, Copy, Debug)]
pub(super) struct DirectoryEntry {
    /// The lump's byte offset in the file, as stored.
    pub(super) filepos: i32,
    /// The lump's size in bytes, as stored.
    pub(super) size: i32,
    /// The name, normalised through [`LumpName::from_raw`].
    pub(super) name: LumpName,
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
    pub(super) fn into_lump(self, bytes: &[u8], index: usize) -> Result<Lump, CyberError> {
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

/// Parses a whole `WAD` file from its bytes into the [`Wad`](super::Wad)
/// the archive holds: the header kind plus every directory entry resolved
/// against `bytes`.
///
/// Layouts the engines produce and this keeps: lump data in any order with
/// overlapping ranges, several directory entries covering one range, and
/// zero-size entries with any `filepos`.
///
/// # Errors
///
/// [`CyberError::LimitExceeded`] when `numlumps` exceeds [`MAX_LUMPS`];
/// [`CyberError::Format`] for a file shorter than the header, an unknown
/// magic, a negative `numlumps` or directory offset, a directory not fully
/// inside the file, or a lump whose stored range runs past the end.
pub(super) fn parse(bytes: &[u8]) -> Result<Wad, CyberError> {
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
    Ok(Wad { kind, lumps })
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
pub(super) fn directory_start(bytes: &[u8], count: usize) -> Result<usize, CyberError> {
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
