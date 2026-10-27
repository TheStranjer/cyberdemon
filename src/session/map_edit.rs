//! The shared map-editing step behind every map editing tool.
//!
//! One undoable [`crate::session::Session::edit`] brings the map into the
//! working archive, edits it through the typed [`Map`] model, invalidates
//! generated node lumps when geometry changed and splices the re-encoded
//! block back at its directory position.
//!
//! [`crate::session::Session::edit_map`] is the entry point; this module
//! holds its machinery as pure functions over the working archive so the
//! copy, read, edit, invalidate and write steps can be tested together.
//! The rule for node invalidation: the generated lumps (`SEGS`,
//! `SSECTORS`, `NODES`, `REJECT`, `BLOCKMAP`, `ZNODES` and the `GL` run)
//! only stay trusted while the geometry they were built from is unchanged.
//! Geometry means the vertex list up to the highest index a linedef
//! references, each linedef's `(v1, v2, front, back)` references, each
//! sidedef's sector assignment and the sector count; vertices trailing
//! past the last reference are invisible to a node build (`build_nodes`
//! appends its split points there, and an `add_vertex` leaves a fresh
//! point there until a line ends at it), so they stay out of the signature,
//! and everything else (textures, heights, light levels, flags, specials,
//! things) is a property edit that keeps the nodes. After a geometry edit
//! vanilla `Doom` needs `build_nodes` before the map plays — while
//! `ZDoom`-family ports rebuild missing nodes themselves.
//!
//! The rule for the vertices an invalidation takes with it: only the points
//! the discarded run itself named. A classic `build_nodes` appends its split
//! vertices after the map's own and names every one of them in its `SEGS`
//! records, so the step's `strip_split_vertices` sheds exactly that trailing
//! run — and nothing else. A vertex the run never named belongs to the
//! caller: a point `add_vertex` appended that no linedef ends at yet
//! survives every geometry edit, whatever the block's nodes do.

mod vertices;

use crate::error::CyberError;
use crate::session::MergedLump;
use crate::session::cow::copy_map_block;
use crate::wad::archive::{Lump, Wad};
use crate::wad::classify::{Analysis, MapBlock, analyze};
use crate::wad::lumps;
use crate::wad::map::model::Map;
use crate::wad::map::{read_map, write_map};
use crate::wad::name::LumpName;

use vertices::{signature, strip_split_vertices};

/// What one [`crate::session::Session::edit_map`] call did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MapEditReport {
    /// The map block that was edited.
    pub map: LumpName,
    /// Whether the block was copied from the merged view into the working
    /// archive before this edit.
    pub copied_to_working: bool,
    /// Whether the edit changed geometry, which emptied the node lumps of
    /// the block and dropped its `GL` run.
    pub nodes_invalidated: bool,
}

/// The working archive's block named `map`, the last of any same-named
/// runs (the way [`crate::wad::map`] readers resolve the name).
///
/// # Errors
///
/// [`CyberError::NotFound`] with a `list_maps` hint when the archive holds
/// no such block.
fn locate(analysis: &Analysis, map: LumpName) -> Result<&MapBlock, CyberError> {
    analysis
        .maps
        .iter()
        .rev()
        .find(|block| block.name == map)
        .ok_or_else(|| {
            CyberError::NotFound(format!(
                "no map block named {map} in the working archive: list_maps shows the maps of \
                 every scope"
            ))
        })
}

/// Empties the generated node lumps of `model.other` in place and drops
/// the `UDMF` `ZNODES` run: after this the map carries no node data.
fn clear_node_lumps(model: &mut Map) {
    for lump in &mut model.other {
        if lumps::BINARY_NODE_LUMPS.contains(&lump.name) {
            *lump = Lump::marker(lump.name);
        }
    }
    model.other.retain(|lump| lump.name != lumps::ZNODES);
}

/// Runs one map edit against `wad`: the body of a
/// [`crate::session::Session::edit_map`] step, given the merged view
/// `merged` to copy a read-only block from.
///
/// The block is copied in when `wad` lacks it, decoded, handed to `edit`,
/// and re-encoded over its own directory range. A changed geometry
/// signature sheds the split vertices the block's own `SEGS` run appended —
/// read off that run before it goes — then empties the node lumps and drops
/// the block's `GL` run from `wad` before the write. See
/// [`strip_split_vertices`].
///
/// # Errors
///
/// [`CyberError::NotFound`] when neither `wad` nor `merged` holds the map,
/// [`CyberError::Format`] when the block does not decode, whatever `edit`
/// reports, and [`crate::wad::map::write_map`]'s encoder errors. On any of
/// these `wad` is left as it was (the caller's undo wrapper guarantees it,
/// and this function only mutates after `edit` succeeded).
pub(crate) fn edit_in_place<T, F>(
    wad: &mut Wad,
    merged: &[MergedLump],
    map: LumpName,
    edit: F,
) -> Result<(T, MapEditReport), CyberError>
where
    F: FnOnce(&mut Map) -> Result<T, CyberError>,
{
    let copied_to_working = copy_map_block(wad, merged, map)?;
    let analysis = analyze(&wad.lumps);
    let block = locate(&analysis, map)?;
    let (header, end, gl) = (block.header, block.end, block.gl);
    let mut model = read_map(&wad.lumps, block)?;
    let before = signature(&model);
    let value = edit(&mut model)?;
    let nodes_invalidated = before != signature(&model);
    if nodes_invalidated {
        strip_split_vertices(&mut model);
        clear_node_lumps(&mut model);
        if let Some((start, stop)) = gl {
            wad.lumps.drain(start..stop);
        }
    }
    let written = write_map(&model)?;
    wad.lumps.splice(header..end, written);
    Ok((
        value,
        MapEditReport {
            map,
            copied_to_working,
            nodes_invalidated,
        },
    ))
}

/// Runs one map edit that rewrites the block's *format*:
/// [`edit_in_place`]'s steps, then the block's `GL` node run goes as well.
///
/// A format change leaves the geometry signature alone, so
/// [`edit_in_place`]'s own invalidation would not fire and a `GL` run built
/// by the old format would survive the rewrite. Node data is written in the
/// map's own format, so a converted block can carry none of it: the edit
/// closure drops the node lumps the model sees (see
/// [`crate::wad::map::convert`]) and this step drops the `GL` run the model
/// does not, reporting the block's node data as invalidated either way.
///
/// # Errors
///
/// [`edit_in_place`]'s errors; `wad` is left as it was behind every one.
pub(crate) fn rewrite_in_place<T, F>(
    wad: &mut Wad,
    merged: &[MergedLump],
    map: LumpName,
    edit: F,
) -> Result<(T, MapEditReport), CyberError>
where
    F: FnOnce(&mut Map) -> Result<T, CyberError>,
{
    let (value, report) = edit_in_place(wad, merged, map, edit)?;
    drop_gl_run(wad, map);
    Ok((
        value,
        MapEditReport {
            nodes_invalidated: true,
            ..report
        },
    ))
}

/// Removes the `GL` node run attached to `wad`'s block named `map`, if it
/// still has one. The block has just been re-encoded in place, so the run is
/// located in the directory as it now stands.
fn drop_gl_run(wad: &mut Wad, map: LumpName) {
    let analysis = analyze(&wad.lumps);
    let Some((start, stop)) = locate(&analysis, map).ok().and_then(|block| block.gl) else {
        return;
    };
    wad.lumps.drain(start..stop);
}

#[cfg(test)]
mod tests;
