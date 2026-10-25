//! The map-block pass: the member names of a binary `Doom`/`Hexen` block
//! and of a `GL` node block, the header tests that tell a real block from a
//! loose `THINGS`, [`find_maps`]' walk that records every [`MapBlock`] with
//! its [`MapFormat`] and attached `GL` range, and [`map_at`]'s answer to
//! which block owns a directory index.
//!
//! [`super::analyze`] runs [`find_maps`] once over the directory and asks
//! [`map_at`] per lump. Marker names are [`super::namespaces`]' business:
//! [`is_marker_name`] keeps `S_START`-style lumps from being read as map
//! headers. The [`MapBlock`] and [`MapFormat`] shapes themselves stay in
//! the root.

use crate::wad::archive::Lump;
use crate::wad::lumps;
use crate::wad::name::LumpName;

use super::namespaces::is_marker_name;
use super::{MapBlock, MapFormat};

/// The member lump names of a `GL` node block.
const GL_MEMBERS: [&[u8]; 5] = [b"GL_VERT", b"GL_SEGS", b"GL_SSECT", b"GL_NODES", b"GL_PVS"];
/// The `GL_VERT` first member every `GL` block starts with.
const GL_VERT: LumpName = LumpName::padded(*b"GL_VERT\0");

/// Whether `name` is a member of a binary map block, a name from
/// [`lumps::BINARY_BLOCK_MEMBERS`].
fn is_map_member(name: LumpName) -> bool {
    lumps::BINARY_BLOCK_MEMBERS.contains(&name)
}

/// Whether `name` is a member of a `GL` node block.
fn is_gl_member(name: &[u8]) -> bool {
    GL_MEMBERS.contains(&name)
}

/// Whether the lump after `index` is named exactly `wanted`.
fn matches_next(lumps: &[Lump], index: usize, wanted: LumpName) -> bool {
    lumps
        .get(index.saturating_add(1))
        .is_some_and(|lump| lump.name == wanted)
}

/// Whether the lump at `index` starts a binary or `UDMF` map block.
fn is_map_header(lumps: &[Lump], index: usize) -> bool {
    let Some(head) = lumps.get(index) else {
        return false;
    };
    if is_marker_name(head.name.as_bytes()) {
        return false;
    }
    matches_next(lumps, index, lumps::THINGS) || matches_next(lumps, index, lumps::TEXTMAP)
}

/// Whether the lump at `index` starts a `GL` node block: a `GL_`-prefixed
/// name that is not itself a `GL` member, followed by `GL_VERT`.
fn is_gl_header(lumps: &[Lump], index: usize) -> bool {
    let Some(head) = lumps.get(index) else {
        return false;
    };
    let name = head.name.as_bytes();
    name.starts_with(b"GL_") && !is_gl_member(name) && matches_next(lumps, index, GL_VERT)
}

/// Finds the map blocks in directory order, recording warnings.
pub(super) fn find_maps(lumps: &[Lump], warnings: &mut Vec<String>) -> Vec<MapBlock> {
    let mut maps = Vec::new();
    let mut index = 0_usize;
    while let Some(head) = lumps.get(index) {
        if is_map_header(lumps, index) {
            let (end, format) = scan_map(lumps, index, warnings);
            maps.push(MapBlock {
                name: head.name,
                header: index,
                end,
                format,
                gl: None,
            });
            index = end;
        } else if is_gl_header(lumps, index) {
            let end = scan_gl(lumps, index);
            attach_gl(&mut maps, lumps, index, end);
            index = end;
        } else {
            index = index.saturating_add(1);
        }
    }
    maps
}

/// Scans the body of the map whose header sits at `header`.
fn scan_map(lumps: &[Lump], header: usize, warnings: &mut Vec<String>) -> (usize, MapFormat) {
    let start = header.saturating_add(1);
    if lumps
        .get(start)
        .is_some_and(|lump| lump.name == lumps::TEXTMAP)
    {
        udmf_range(lumps, start, warnings)
    } else {
        binary_range(lumps, start)
    }
}

/// The end of a binary member run starting at `start`, plus its format.
fn binary_range(lumps: &[Lump], start: usize) -> (usize, MapFormat) {
    let mut end = start;
    while lumps.get(end).is_some_and(|lump| is_map_member(lump.name)) {
        end = end.saturating_add(1);
    }
    let hexen = lumps
        .iter()
        .skip(start)
        .take(end.saturating_sub(start))
        .any(|lump| lump.name == lumps::BEHAVIOR);
    let format = if hexen {
        MapFormat::Hexen
    } else {
        MapFormat::Doom
    };
    (end, format)
}

/// The end of a `UDMF` block whose `TEXTMAP` sits at `textmap`, plus the
/// fixed `Udmf` format. A missing `ENDMAP` warns and ends at the directory.
fn udmf_range(lumps: &[Lump], textmap: usize, warnings: &mut Vec<String>) -> (usize, MapFormat) {
    let body = textmap.saturating_add(1);
    let found = lumps
        .iter()
        .skip(body)
        .position(|lump| lump.name == lumps::ENDMAP);
    let end = found.map_or_else(
        || {
            warnings.push(format!(
                "map lump at index {textmap} is a TEXTMAP without a following ENDMAP: \
                 the block runs to the end of the directory"
            ));
            lumps.len()
        },
        |offset| body.saturating_add(offset).saturating_add(1),
    );
    (end, MapFormat::Udmf)
}

/// The end of the `GL` member run whose header sits at `header`.
fn scan_gl(lumps: &[Lump], header: usize) -> usize {
    let mut end = header.saturating_add(1);
    while lumps
        .get(end)
        .is_some_and(|lump| is_gl_member(lump.name.as_bytes()))
    {
        end = end.saturating_add(1);
    }
    end
}

/// Attaches the `GL` block spanning `start..end` to its map: by name for
/// `GL_<mapname>`, to the preceding map for `GL_LEVEL`.
fn attach_gl(maps: &mut [MapBlock], lumps: &[Lump], start: usize, end: usize) {
    let Some(head) = lumps.get(start).map(|lump| lump.name) else {
        return;
    };
    let target = if head.as_bytes() == b"GL_LEVEL" {
        maps.last_mut()
    } else {
        let wanted = head.as_bytes().strip_prefix(b"GL_").unwrap_or_default();
        maps.iter_mut()
            .find(|block| block.name.as_bytes() == wanted)
    };
    if let Some(block) = target {
        block.gl = Some((start, end));
    }
}

/// The map a lump at `index` belongs to, with whether it is the header.
pub(super) fn map_at(maps: &[MapBlock], index: usize) -> Option<(LumpName, bool)> {
    maps.iter().find_map(|block| {
        if index == block.header {
            Some((block.name, true))
        } else if (block.header < index && index < block.end)
            || block
                .gl
                .is_some_and(|(start, end)| start <= index && index < end)
        {
            Some((block.name, false))
        } else {
            None
        }
    })
}
