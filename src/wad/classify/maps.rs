//! The map-block pass: the `find_maps` walk that records every [`MapBlock`]
//! with its [`MapFormat`] and attached `GL` range.
//!
//! The member names of a binary `Doom`/`Hexen` block and of a `GL` node
//! block, and the header tests that tell a real block from a loose
//! `THINGS`, are `find_maps`' own business; `map_owners` answers which block
//! owns every directory index in one sweep, `gl_marker_for` is the one
//! naming rule every `GL` writer shares: `GL_<map>`, or the name-less
//! `GL_LEVEL` marker for longer names. [`super::analyze`] runs `find_maps`
//! once over the directory and reads the owner table per lump. Marker names are
//! [`super::namespaces`]' business: `is_marker_name` keeps `S_START`-style
//! lumps from being read as map headers. The [`MapBlock`] and
//! [`MapFormat`] shapes themselves stay in the root.

use crate::wad::archive::Lump;
use crate::wad::lumps;
use crate::wad::name::{LumpName, NAME_LEN};

use super::namespaces::is_marker_name;
use super::{MapBlock, MapFormat};

/// The member lump names of a `GL` node block.
const GL_MEMBERS: [&[u8]; 5] = [b"GL_VERT", b"GL_SEGS", b"GL_SSECT", b"GL_NODES", b"GL_PVS"];
/// The `GL_VERT` first member every `GL` block starts with.
const GL_VERT: LumpName = LumpName::padded(*b"GL_VERT\0");
/// The prefix every `GL` node block marker starts with.
const GL_PREFIX: &[u8] = b"GL_";
/// Longest map name whose `GL_<name>` marker still fits a directory name.
const GL_NAME_MAX: usize = NAME_LEN - GL_PREFIX.len();
/// The `GL` marker that carries no map name and attaches to the map block
/// right above it: the fallback [`gl_marker_for`] answers with when the
/// map name leaves no room behind the [`GL_PREFIX`].
pub(crate) const GL_LEVEL: LumpName = LumpName::padded(*b"GL_LEVEL");

/// The marker name one map's `GL` node block carries: `GL_<map>` when the
/// map name fits behind the [`GL_PREFIX`], [`GL_LEVEL`] otherwise. The
/// fallback carries no name, so [`attach_gl`] hangs it off the preceding
/// map block: whoever writes a `GL_LEVEL` marker must keep the run right
/// after its block. This is the one rule every `GL` writer follows.
#[must_use]
pub(crate) fn gl_marker_for(map: LumpName) -> LumpName {
    if map.len() > GL_NAME_MAX {
        return GL_LEVEL;
    }
    let mut raw = [0_u8; NAME_LEN];
    for (slot, byte) in raw.iter_mut().zip(GL_PREFIX.iter().chain(map.raw().iter())) {
        *slot = *byte;
    }
    LumpName::from_raw(raw)
}

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
    name.starts_with(GL_PREFIX) && !is_gl_member(name) && matches_next(lumps, index, GL_VERT)
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
/// `GL_<mapname>`, to the preceding map for [`GL_LEVEL`].
fn attach_gl(maps: &mut [MapBlock], lumps: &[Lump], start: usize, end: usize) {
    let Some(head) = lumps.get(start).map(|lump| lump.name) else {
        return;
    };
    let target = if head == GL_LEVEL {
        maps.last_mut()
    } else {
        let wanted = head.as_bytes().strip_prefix(GL_PREFIX).unwrap_or_default();
        maps.iter_mut()
            .find(|block| block.name.as_bytes() == wanted)
    };
    if let Some(block) = target {
        block.gl = Some((start, end));
    }
}

/// The owner of every directory position, index by index, in one sweep:
/// each block claims its header (the `true` flag), its member run
/// (`header + 1..end`) and its attached `GL` run, and a position keeps its
/// first claimant — the block earliest in directory order that owns it, the
/// same answer the original per-lump linear scan gave by returning the
/// first matching block, in the same block order. This is what turns
/// [`super::analyze`]'s per-lump scan from O(lumps × maps) into
/// O(lumps + claimed positions).
pub(crate) fn map_owners(maps: &[MapBlock], len: usize) -> Vec<Option<(LumpName, bool)>> {
    let mut owners: Vec<Option<(LumpName, bool)>> = core::iter::repeat_n(None, len).collect();
    for block in maps {
        claim(&mut owners, block.header, block.name);
        claim_run(
            &mut owners,
            block.header.saturating_add(1)..block.end,
            block.name,
        );
        if let Some((start, end)) = block.gl {
            claim_run(&mut owners, start..end, block.name);
        }
    }
    owners
}

/// Claims position `index` for `map`'s block header while nothing owns it.
fn claim(owners: &mut [Option<(LumpName, bool)>], index: usize, map: LumpName) {
    if let Some(slot) = owners.get_mut(index).filter(|held| held.is_none()) {
        *slot = Some((map, true));
    }
}

/// Claims every still-unowned position of `range` for `map`'s data.
fn claim_run(
    owners: &mut [Option<(LumpName, bool)>],
    range: core::ops::Range<usize>,
    map: LumpName,
) {
    let Some(head) = owners.get_mut(range.start..) else {
        return;
    };
    for slot in head.iter_mut().take(range.end.saturating_sub(range.start)) {
        if slot.is_none() {
            *slot = Some((map, false));
        }
    }
}
