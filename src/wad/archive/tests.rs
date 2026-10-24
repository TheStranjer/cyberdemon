//! Shared builders for the `wad::archive` tests: the parsed-name maker, the
//! raw header and directory-entry constructors the two topics assemble files
//! with, the sample and duplicated archives they serialize, and the readers
//! of a written directory and of a scratch directory's entries.
//!
//! `bytes` checks which layouts round-trip and which the writer lays out
//! where; `parse_io` walks every header and entry refusal and the trips
//! through the filesystem.

mod bytes;
mod parse_io;

use std::fs;
use std::path::Path;

use super::{Lump, Wad, WadKind};
use crate::wad::name::LumpName;

/// A valid parsed name for test data.
pub(super) fn name(text: &str) -> LumpName {
    LumpName::parse(text).expect("test names are valid")
}

/// One raw sixteen-byte directory entry.
pub(super) fn raw_entry(filepos: i32, size: i32, entry_name: [u8; 8]) -> Vec<u8> {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&filepos.to_le_bytes());
    bytes.extend_from_slice(&size.to_le_bytes());
    bytes.extend_from_slice(&entry_name);
    bytes
}

/// A whole file: a header for `count` entries at `directory_at`, then
/// `parts` concatenated behind it.
pub(super) fn build_file(kind: WadKind, count: i32, directory_at: i32, parts: &[&[u8]]) -> Vec<u8> {
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
pub(super) fn sample(kind: WadKind) -> Wad {
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
pub(super) fn written_entries(wad_bytes: &[u8], count: usize) -> Vec<(i32, i32)> {
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
pub(super) fn fs_entries(dir: &Path) -> Vec<String> {
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

/// A `Wad` carrying two byte-identical payloads, a third distinct one,
/// and a marker between the twins.
pub(super) fn duplicated() -> Wad {
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
