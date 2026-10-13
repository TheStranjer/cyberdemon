//! WAD fixtures written to disk with the library itself.
//!
//! Shared by the integration tests: every helper takes the scratch
//! directory (keep the [`tempfile::TempDir`] alive for as long as the paths
//! are used) and returns the path it wrote.

use std::fs;
use std::path::{Path, PathBuf};

use cyberdemon::wad::archive::write_file_atomic;
use cyberdemon::wad::{Lump, LumpName, Wad, WadKind};

/// Bytes in one flat this repo's fixtures use.
const FLAT_BYTES: usize = 4096;
/// Bytes in an `ENDOOM` screen.
const ENDOOM_BYTES: usize = 4000;

/// A lump named `text` carrying `data`.
fn lump(text: &str, data: Vec<u8>) -> Lump {
    Lump::new(
        LumpName::parse(text).expect("fixture names are valid"),
        data,
    )
}

/// An empty marker lump named `text`.
fn marker(text: &str) -> Lump {
    Lump::marker(LumpName::parse(text).expect("fixture names are valid"))
}

/// A map block: the header marker plus empty vanilla member lumps. Later
/// tickets swap real geometry in here.
fn map_block(text: &str) -> Vec<Lump> {
    let mut lumps = vec![marker(text)];
    for member in ["THINGS", "LINEDEFS", "SIDEDEFS", "VERTEXES", "SECTORS"] {
        lumps.push(marker(member));
    }
    lumps
}

/// 14 `PLAYPAL` palettes of 768 bytes; palette zero's colour `i` is
/// `(i, 255 - i, i)` and the other 13 are copies of it.
fn playpal() -> Vec<u8> {
    let mut first = Vec::new();
    for index in 0_u8..=255 {
        first.extend_from_slice(&[index, 255_u8.wrapping_sub(index), index]);
    }
    let mut palettes = Vec::new();
    for _palette in 0_u8..14 {
        palettes.extend_from_slice(&first);
    }
    palettes
}

/// 34 `COLORMAP` tables of 256 bytes, every table the identity map.
fn colormap() -> Vec<u8> {
    let mut tables = Vec::new();
    for _table in 0_u8..34 {
        for index in 0_u8..=255 {
            tables.push(index);
        }
    }
    tables
}

/// Serialize `wad` into `file` inside `dir` and return the path.
fn write_wad(dir: &Path, file: &str, wad: &Wad) -> PathBuf {
    let path = dir.join(file);
    let bytes = wad.to_bytes().expect("fixtures serialize");
    write_file_atomic(&path, &bytes).expect("fixtures write");
    path
}

/// A minimal `DOOM II` style `IWAD`: palettes, one empty `MAP01` block, one
/// flat region with `FLOOR1` and an `ENDOOM`.
///
/// # Panics
///
/// Panics if the fixture cannot be serialized or written.
#[must_use]
#[inline]
pub fn iwad(dir: &Path) -> PathBuf {
    let mut lumps = vec![lump("PLAYPAL", playpal()), lump("COLORMAP", colormap())];
    lumps.extend(map_block("MAP01"));
    lumps.push(marker("F_START"));
    lumps.push(lump("FLOOR1", vec![1_u8; FLAT_BYTES]));
    lumps.push(marker("F_END"));
    lumps.push(lump("ENDOOM", vec![b' '; ENDOOM_BYTES]));
    let wad = Wad {
        kind: WadKind::Iwad,
        lumps,
    };
    write_wad(dir, "iwad-doom2.wad", &wad)
}

/// A `PWAD` overriding `FLOOR1`, adding a `DEHACKED` global and an empty
/// `MAP02` block.
///
/// # Panics
///
/// Panics if the fixture cannot be serialized or written.
#[must_use]
#[inline]
pub fn pwad(dir: &Path) -> PathBuf {
    let mut lumps = vec![
        marker("FF_START"),
        lump("FLOOR1", vec![2_u8; FLAT_BYTES]),
        marker("FF_END"),
        lump("DEHACKED", b"Thing 1 (Troop)\n".to_vec()),
    ];
    lumps.extend(map_block("MAP02"));
    let wad = Wad {
        kind: WadKind::Pwad,
        lumps,
    };
    write_wad(dir, "pwad-extra.wad", &wad)
}

/// A file that is not a `WAD` at all, for `format_error` paths.
///
/// # Panics
///
/// Panics if the file cannot be written.
#[must_use]
#[inline]
pub fn not_a_wad(dir: &Path) -> PathBuf {
    let path = dir.join("notes.txt");
    fs::write(&path, b"this is a text file, not a WAD").expect("fixtures write");
    path
}
