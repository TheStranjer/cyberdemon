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
//! appends its split points there), so they stay out of the signature,
//! and everything else (textures, heights, light levels, flags, specials,
//! things) is a property edit that keeps the nodes. After a geometry edit
//! vanilla `Doom` needs `build_nodes` before the map plays — which also
//! strips the stale split vertices, no longer referenced by anything —
//! while `ZDoom`-family ports rebuild missing nodes themselves.

use crate::error::CyberError;
use crate::session::MergedLump;
use crate::session::cow::copy_map_block;
use crate::wad::archive::{Lump, Wad};
use crate::wad::classify::{Analysis, MapBlock, analyze};
use crate::wad::map::model::Map;
use crate::wad::map::{read_map, write_map};
use crate::wad::name::LumpName;

/// The generated node lumps of a binary block: kept as (empty) placeholders
/// through an invalidation because the vanilla readers require them.
const NODE_LUMPS: [&[u8]; 5] = [b"SEGS", b"SSECTORS", b"NODES", b"REJECT", b"BLOCKMAP"];

/// The geometry a node build reads from one linedef.
type LineRef = (usize, usize, Option<usize>, Option<usize>);

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

/// The geometry facts the generated node lumps depend on: two maps share
/// one signature exactly as long as a node rebuild would read the same
/// geometry from either.
#[derive(PartialEq, Eq)]
struct Signature {
    /// The coordinates of the referenced vertices, in index order: the
    /// trailing unreferenced ones a node build leaves behind are not
    /// geometry, and get stripped whenever the nodes go.
    vertices: Vec<(i32, i32)>,
    /// Every linedef's endpoints and side assignments, in index order.
    linedefs: Vec<LineRef>,
    /// Every sidedef's sector index, in index order.
    sidedefs: Vec<usize>,
    /// How many sectors the map holds.
    sector_count: usize,
}

/// The geometry signature of `map`.
fn signature(map: &Map) -> Signature {
    Signature {
        vertices: map
            .vertices
            .iter()
            .take(used_vertices(map))
            .map(|v| (v.x, v.y))
            .collect(),
        linedefs: map
            .linedefs
            .iter()
            .map(|line| (line.v1, line.v2, line.front, line.back))
            .collect(),
        sidedefs: map.sidedefs.iter().map(|side| side.sector).collect(),
        sector_count: map.sectors.len(),
    }
}

/// How many vertices the linedefs reference: one past the highest
/// endpoint index. Vertices trailing past it are invisible to a node
/// build (`build_nodes` keeps its split points there, and removal
/// cascades leave orphans there), so they stay out of the signature.
fn used_vertices(map: &Map) -> usize {
    map.linedefs
        .iter()
        .flat_map(|line| [line.v1, line.v2])
        .max()
        .and_then(|top| top.checked_add(1))
        .unwrap_or(0_usize)
}

/// Drops trailing vertices no linedef references: the split points an
/// earlier `build_nodes` appended belong to its node lumps, which have
/// just been invalidated.
fn strip_split_vertices(map: &mut Map) {
    map.vertices.truncate(used_vertices(map));
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
        if NODE_LUMPS.contains(&lump.name.as_bytes()) {
            *lump = Lump::marker(lump.name);
        }
    }
    model.other.retain(|lump| lump.name.as_bytes() != b"ZNODES");
}

/// Runs one map edit against `wad`: the body of a
/// [`crate::session::Session::edit_map`] step, given the merged view
/// `merged` to copy a read-only block from.
///
/// The block is copied in when `wad` lacks it, decoded, handed to `edit`,
/// and re-encoded over its own directory range. A changed geometry
/// signature empties the node lumps, strips the unreferenced trailing
/// (split) vertices and drops the block's `GL` run from `wad` before the
/// write.
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
        clear_node_lumps(&mut model);
        strip_split_vertices(&mut model);
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
mod tests {
    use std::path::PathBuf;

    use super::{MapEditReport, edit_in_place, locate};
    use crate::error::CyberError;
    use crate::session::{Layer, LayerRole, Session};
    use crate::wad::archive::{Lump, Wad, WadKind};
    use crate::wad::classify::{MapFormat, analyze};
    use crate::wad::map::model::{Linedef, Map, Sector, Sidedef, Thing, Vertex};
    use crate::wad::map::{read_map, write_map};
    use crate::wad::name::LumpName;

    /// A valid parsed name for test data.
    fn name(text: &str) -> LumpName {
        LumpName::parse(text).expect("test names are valid")
    }

    /// An empty marker lump named `text`.
    fn marker(text: &str) -> Lump {
        Lump::marker(name(text))
    }

    /// A lump named `text` holding dummy node bytes.
    fn dummy(text: &str) -> Lump {
        Lump::new(name(text), vec![0xA5_u8; 8])
    }

    /// A vertex at `x`/`y`.
    fn vertex(x: i32, y: i32) -> Vertex {
        Vertex {
            x,
            y,
            ..Default::default()
        }
    }

    /// A player-start spawn at `x`/`y`.
    fn spawn(x: i32, y: i32) -> Thing {
        Thing {
            kind: 1,
            x,
            y,
            angle: 0,
            flags: 7,
            ..Default::default()
        }
    }

    /// A line from `v1` to `v2` with the given sides.
    fn line(v1: usize, v2: usize, front: Option<usize>, back: Option<usize>) -> Linedef {
        Linedef {
            v1,
            v2,
            flags: 1,
            front,
            back,
            ..Default::default()
        }
    }

    /// A sidedef over `sector`.
    fn side(sector: usize) -> Sidedef {
        Sidedef {
            sector,
            ..Default::default()
        }
    }

    /// A sector with the given floor height and light.
    fn room(floor_height: i32, light: i32) -> Sector {
        Sector {
            floor_height,
            ceiling_height: 128,
            floor_flat: "FLOOR1".to_owned(),
            ceiling_flat: "FLOOR1".to_owned(),
            light,
            ..Default::default()
        }
    }

    /// A tiny two-room map named `text` in `format`: four vertices, two
    /// walls, one divider line and one spawn, plus dummy generated node
    /// lumps (binary node stubs or `UDMF` `ZNODES`).
    fn demo_map(text: &str, format: MapFormat) -> Map {
        let other = match format {
            MapFormat::Doom | MapFormat::Hexen => vec![
                dummy("SEGS"),
                dummy("SSECTORS"),
                dummy("NODES"),
                dummy("REJECT"),
                dummy("BLOCKMAP"),
            ],
            MapFormat::Udmf => vec![dummy("ZNODES")],
        };
        Map {
            name: name(text),
            format,
            udmf_namespace: (format == MapFormat::Udmf).then(|| "zdoom".to_owned()),
            things: vec![spawn(64, 128)],
            vertices: vec![
                vertex(0, 0),
                vertex(256, 0),
                vertex(256, 256),
                vertex(0, 256),
            ],
            linedefs: vec![
                line(0, 1, Some(0), None),
                line(1, 2, Some(0), Some(1)),
                line(2, 3, Some(1), None),
                line(3, 0, Some(1), None),
            ],
            sidedefs: vec![side(0), side(1)],
            sectors: vec![room(0, 160), room(32, 192)],
            other,
        }
    }

    /// The `GL` node run of a `MAP01` block, with dummy bytes.
    fn gl_run() -> Vec<Lump> {
        vec![marker("GL_MAP01"), dummy("GL_VERT"), dummy("GL_NODES")]
    }

    /// A session whose only layer is a base archive of `lumps`.
    fn base_session(lumps: Vec<Lump>) -> Session {
        let mut session = Session::default();
        session.set_iwad(Layer::new(
            PathBuf::from("base.wad"),
            LayerRole::Iwad,
            Wad {
                kind: WadKind::Iwad,
                lumps,
            },
        ));
        session
    }

    /// A base archive: a `MAP01` block in `format` with its `GL` run and a
    /// trailing loose lump.
    fn iwad_lumps(format: MapFormat) -> Vec<Lump> {
        let mut lumps = write_map(&demo_map("MAP01", format)).expect("the fixture encodes");
        lumps.extend(gl_run());
        lumps.push(marker("ENDPAK"));
        lumps
    }

    /// The directory names of an archive, in order.
    fn names(wad: &Wad) -> Vec<String> {
        wad.lumps.iter().map(|lump| lump.name.to_string()).collect()
    }

    /// The payload size of the first lump named `text`.
    fn size_of(wad: &Wad, text: &str) -> usize {
        wad.lumps
            .iter()
            .find(|lump| lump.name == name(text))
            .unwrap_or_else(|| panic!("no lump named {text} in {:?}", names(wad)))
            .size()
    }

    /// The working archive's decoded block named `text`, plus the block's
    /// `GL` range when it still has one.
    type BlockWithGl = (Map, Option<(usize, usize)>);

    fn working_block(session: &Session, text: &str) -> BlockWithGl {
        let analysis = analyze(&session.working().lumps);
        let block = analysis
            .maps
            .iter()
            .rev()
            .find(|block| block.name == name(text))
            .unwrap_or_else(|| panic!("no {text} block in the working archive"));
        let gl = block.gl;
        (
            read_map(&session.working().lumps, block).expect("the edited block decodes"),
            gl,
        )
    }

    /// The error code of a [`CyberError`], for compact asserts.
    fn code(error: &CyberError) -> &str {
        error.code()
    }

    /// The edit closure of a format rewrite: it removes the node lumps the
    /// way [`crate::wad::map::convert`] does and changes no geometry, so the
    /// signature comparison alone would never invalidate the block. Fails on
    /// a fixture that carries no node lumps, which would make the test below
    /// check nothing.
    fn strip_nodes(map: &mut Map) -> Result<(), CyberError> {
        let is_node = |lump: &Lump| crate::wad::map::convert::is_node_lump(lump.name.as_bytes());
        if !map.other.iter().any(is_node) {
            return Err(CyberError::InvalidArgument(
                "the fixture carries no node lump to strip".to_owned(),
            ));
        }
        map.other.retain(|lump| !is_node(lump));
        Ok(())
    }

    /// An edit closure that raises sector 0's floor to 48, or refuses when
    /// the map has no sectors.
    fn raise_floor(map: &mut Map) -> Result<(), CyberError> {
        let sector = map
            .sectors
            .first_mut()
            .ok_or_else(|| CyberError::InvalidArgument("the map has no sectors".to_owned()))?;
        sector.floor_height = 48_i32;
        Ok(())
    }

    /// An edit closure that moves vertex 0 by 100 units in `x`, or refuses
    /// when the map has no vertices.
    fn move_vertex(map: &mut Map) -> Result<(), CyberError> {
        let vertex = map
            .vertices
            .first_mut()
            .ok_or_else(|| CyberError::InvalidArgument("the map has no vertices".to_owned()))?;
        vertex.x = vertex.x.checked_add(100_i32).expect("the move fits");
        Ok(())
    }

    /// An edit closure that fails without touching `map`.
    fn failing(_map: &mut Map) -> Result<(), CyberError> {
        Err(CyberError::InvalidArgument("nope".to_owned()))
    }

    /// An edit closure that appends one unreferenced vertex and writes a
    /// `SEGS` record over the placeholder, the way `build_nodes` lands
    /// its split points.
    fn append_split_vertex(map: &mut Map) -> Result<(), CyberError> {
        map.vertices.push(vertex(128, 0));
        let segs = map
            .other
            .iter_mut()
            .find(|lump| lump.name.as_bytes() == b"SEGS")
            .ok_or_else(|| CyberError::InvalidArgument("the fixture has no SEGS".to_owned()))?;
        *segs = Lump::new(name("SEGS"), vec![0_u8; 12]);
        Ok(())
    }

    /// The payload size of the `other` lump named `text` in the decoded
    /// working `MAP01` block.
    fn other_size(session: &Session, text: &str) -> usize {
        let (map, _) = working_block(session, "MAP01");
        map.other
            .iter()
            .find(|lump| lump.name == name(text))
            .unwrap_or_else(|| panic!("no {text} in the block"))
            .size()
    }

    #[test]
    fn trailing_unreferenced_vertices_are_not_geometry() {
        let mut session = base_session(iwad_lumps(MapFormat::Doom));
        let ((), report) = session
            .edit_map(name("MAP01"), "split corner", append_split_vertex)
            .expect("the append lands");
        assert!(
            !report.nodes_invalidated,
            "no linedef points at the appended vertex, so the node builder sees no change"
        );
        let (map, _) = working_block(&session, "MAP01");
        assert_eq!(map.vertices.len(), 5, "the split point stays");
        assert_eq!(other_size(&session, "SEGS"), 12, "and the run is trusted");
    }

    #[test]
    fn a_geometry_edit_strips_the_split_vertices_with_the_nodes() {
        let mut session = base_session(iwad_lumps(MapFormat::Doom));
        session
            .edit_map(name("MAP01"), "split corner", append_split_vertex)
            .expect("the build-like edit lands");
        let ((), report) = session
            .edit_map(name("MAP01"), "move corner", move_vertex)
            .expect("the move lands");
        assert!(report.nodes_invalidated, "moving a used vertex is geometry");
        let (map, _) = working_block(&session, "MAP01");
        assert_eq!(
            map.vertices.len(),
            4,
            "the stale split point went with the nodes"
        );
        assert_eq!(other_size(&session, "SEGS"), 0, "and the run emptied");
    }

    #[test]
    fn a_property_edit_copies_in_and_keeps_the_nodes() {
        let mut session = base_session(iwad_lumps(MapFormat::Doom));
        let ((), report) = session
            .edit_map(name("MAP01"), "raise floor", raise_floor)
            .expect("the edit lands");
        assert_eq!(
            report,
            MapEditReport {
                map: name("MAP01"),
                copied_to_working: true,
                nodes_invalidated: false,
            },
            "the map came from the layer and the nodes stay trusted"
        );
        let (map, gl) = working_block(&session, "MAP01");
        assert_eq!(map.sectors.first().expect("sector 0").floor_height, 48_i32);
        assert_eq!(map.things.len(), 1, "the rest came along untouched");
        assert!(gl.is_some(), "the GL run is kept by a property edit");
        for lump in ["SEGS", "SSECTORS", "NODES", "REJECT", "BLOCKMAP"] {
            assert_eq!(
                size_of(session.working(), lump),
                8_usize,
                "{lump} keeps its bytes"
            );
        }
        assert!(
            !names(session.working()).contains(&"ENDPAK".to_owned()),
            "only the block and its GL run were copied"
        );
    }

    #[test]
    fn a_property_edit_undoes_as_one_step_and_leaves_the_iwad_alone() {
        let mut session = base_session(iwad_lumps(MapFormat::Doom));
        session
            .edit_map(name("MAP01"), "raise floor", raise_floor)
            .expect("the edit lands");
        assert_eq!(
            session.undo_labels(),
            vec!["raise floor"],
            "one undo entry for the whole step"
        );
        assert!(session.is_dirty(), "and unsaved work");
        let layer = session.layers()[0];
        let layer_map = {
            let analysis = analyze(&layer.wad.lumps);
            let block = analysis.maps.first().expect("the layer holds MAP01");
            read_map(&layer.wad.lumps, block).expect("the layer block decodes")
        };
        assert_eq!(
            layer_map.sectors.first().expect("sector 0").floor_height,
            0_i32,
            "the layer was never written to"
        );
        assert_eq!(
            session.undo().expect("undo the edit"),
            "raise floor",
            "the label comes back"
        );
        assert!(session.working().is_empty(), "the copy rolled back too");
        assert!(!session.is_dirty(), "back to the saved state");
    }

    #[test]
    fn a_geometry_edit_empties_node_lumps_and_drops_the_gl_run() {
        let mut session = base_session(iwad_lumps(MapFormat::Doom));
        let ((), report) = session
            .edit_map(name("MAP01"), "move corner", move_vertex)
            .expect("the edit lands");
        assert!(report.nodes_invalidated, "moving a vertex is geometry");
        let (map, gl) = working_block(&session, "MAP01");
        assert_eq!(map.vertices.first().expect("vertex 0").x, 100_i32);
        assert_eq!(gl, None, "the GL block is gone from the working archive");
        for lump in names(session.working()) {
            assert!(!lump.starts_with("GL_"), "no GL lump survived: {lump}");
        }
        for member in ["SEGS", "SSECTORS", "NODES", "REJECT", "BLOCKMAP"] {
            assert_eq!(
                size_of(session.working(), member),
                0_usize,
                "{member} was emptied but kept"
            );
        }
    }

    #[test]
    fn a_geometry_edit_drops_znodes_from_a_udmf_block() {
        let mut session = base_session(iwad_lumps(MapFormat::Udmf));
        let ((), report) = session
            .edit_map(name("MAP01"), "move corner", move_vertex)
            .expect("the edit lands");
        assert!(report.nodes_invalidated, "moving a vertex is geometry");
        assert!(
            !names(session.working()).contains(&"ZNODES".to_owned()),
            "ZNODES is dropped for UDMF, not emptied: {:?}",
            names(session.working())
        );
        let (map, _) = working_block(&session, "MAP01");
        assert_eq!(map.format, MapFormat::Udmf, "the block still round trips");
        assert_eq!(map.udmf_namespace.as_deref(), Some("zdoom"));
    }

    #[test]
    fn a_failing_edit_changes_the_archive_and_history_nothing() {
        let mut session = base_session(iwad_lumps(MapFormat::Doom));
        let error = session
            .edit_map(name("MAP01"), "broken", failing)
            .expect_err("the closure refuses");
        assert_eq!(code(&error), "invalid_argument", "the error passes through");
        assert!(session.working().is_empty(), "not even the copy landed");
        assert!(session.undo_labels().is_empty(), "no history entry");
        assert!(!session.is_dirty(), "the session stayed clean");
    }

    #[test]
    fn a_second_edit_copies_nothing_and_replaces_the_block_in_place() {
        let mut lumps = iwad_lumps(MapFormat::Doom);
        lumps.extend(
            write_map(&demo_map("MAP02", MapFormat::Doom)).expect("the second map encodes"),
        );
        let mut session = base_session(lumps);
        session
            .edit_map(name("MAP02"), "raise floor", raise_floor)
            .expect("the first map edit");
        session
            .edit_map(name("MAP01"), "light room", |map| {
                let sector = map.sectors.first_mut().expect("the map has sectors");
                sector.light = 200_i32;
                Ok(())
            })
            .expect("the second map copies in");
        let after_copy = names(session.working());
        let first_map_at = after_copy
            .iter()
            .position(|text| text == "MAP01")
            .expect("MAP01 was appended");
        let ((), report) = session
            .edit_map(name("MAP01"), "darken room", |map| {
                let sector = map.sectors.first_mut().expect("the map has sectors");
                sector.light = 96_i32;
                Ok(())
            })
            .expect("the same map edits again");
        assert!(!report.copied_to_working, "the block was already editable");
        let after_edit = names(session.working());
        assert_eq!(
            after_edit.iter().position(|text| text == "MAP01"),
            Some(first_map_at),
            "the block kept its directory position"
        );
        assert_eq!(
            after_edit.iter().take(first_map_at).collect::<Vec<_>>(),
            after_copy.iter().take(first_map_at).collect::<Vec<_>>(),
            "every lump before the block stands"
        );
        assert_eq!(after_edit.len(), after_copy.len(), "and nothing trailed in");
        let (map, _) = working_block(&session, "MAP01");
        assert_eq!(map.sectors.first().expect("sector 0").light, 96_i32);
        assert_eq!(
            session.undo_labels(),
            vec!["darken room", "light room", "raise floor"],
            "three edits, three steps"
        );
    }

    #[test]
    fn edit_map_reports_not_found_for_a_map_no_archive_holds() {
        let mut session = base_session(iwad_lumps(MapFormat::Doom));
        let error = session
            .edit_map(name("MAP07"), "raise floor", raise_floor)
            .expect_err("there is no MAP07");
        assert_eq!(code(&error), "not_found", "missing map code");
        assert!(
            error.to_string().contains("MAP07") && error.to_string().contains("list_lumps"),
            "the message names the map and the fix: {error}"
        );
        assert!(session.working().is_empty(), "nothing changed");
        assert!(session.undo_labels().is_empty(), "and nothing was recorded");
    }

    #[test]
    fn locate_refuses_a_directory_without_the_block() {
        let analysis = analyze(&[]);
        let error = locate(&analysis, name("MAP01")).expect_err("nothing is classified");
        assert_eq!(code(&error), "not_found", "missing block code");
        assert!(
            error.to_string().contains("MAP01") && error.to_string().contains("working"),
            "the message names the map and the archive it checked: {error}"
        );
    }

    #[test]
    fn edit_in_place_reuses_the_sessions_working_archive_shape() {
        let base = iwad_lumps(MapFormat::Doom);
        let analysis = analyze(&base);
        let merged: Vec<crate::session::MergedLump> = base
            .iter()
            .enumerate()
            .map(|(index, lump)| crate::session::MergedLump {
                lump: lump.clone(),
                source: crate::session::Source::Layer(0),
                source_index: index,
            })
            .collect();
        assert_eq!(analysis.maps.len(), 1, "the fixture is one block");
        let mut working = Wad::default();
        let ((), report) = edit_in_place(&mut working, &merged, name("MAP01"), raise_floor)
            .expect("the step runs outside a session too");
        assert!(report.copied_to_working, "the copy is part of the step");
        assert_eq!(
            working.lumps.len(),
            11_usize.saturating_add(gl_run().len()),
            "header and ten members, plus the GL run, copied in"
        );
    }

    #[test]
    fn a_rewrite_copies_in_drops_the_gl_run_and_invalidates_the_block() {
        let mut session = base_session(iwad_lumps(MapFormat::Doom));
        let ((), report) = session
            .convert_map(name("MAP01"), "convert to UDMF", strip_nodes)
            .expect("the rewrite lands");
        assert!(
            report.copied_to_working,
            "the layer's block was copied in first"
        );
        assert!(
            report.nodes_invalidated,
            "a format change invalidates the node data whatever the geometry says"
        );
        let (map, gl) = working_block(&session, "MAP01");
        assert_eq!(gl, None, "the GL run left the archive");
        for lump in names(session.working()) {
            assert!(!lump.starts_with("GL_"), "no GL lump survived: {lump}");
        }
        for member in ["SEGS", "SSECTORS", "NODES", "REJECT", "BLOCKMAP"] {
            assert_eq!(
                size_of(session.working(), member),
                0_usize,
                "{member} came back as the encoder's empty placeholder"
            );
        }
        assert_eq!(
            session.undo_labels(),
            vec!["convert to UDMF"],
            "copy and rewrite are one undo step"
        );
        assert_eq!(map.vertices.len(), 4, "the geometry came along");
        session.undo().expect("undo the rewrite");
        assert!(
            session.working().is_empty(),
            "copy, block and run rolled back"
        );
    }

    #[test]
    fn edit_map_keeps_the_gl_run_a_rewrite_drops() {
        let mut plain = base_session(iwad_lumps(MapFormat::Udmf));
        let ((), kept) = plain
            .edit_map(name("MAP01"), "strip only", strip_nodes)
            .expect("a plain edit lands");
        assert!(
            !kept.nodes_invalidated,
            "no geometry changed, so the signature alone invalidates nothing"
        );
        let (_, gl) = working_block(&plain, "MAP01");
        assert!(gl.is_some(), "the plain edit leaves the GL run in place");
        let mut rewritten = base_session(iwad_lumps(MapFormat::Udmf));
        let ((), forced) = rewritten
            .convert_map(name("MAP01"), "convert to Doom", strip_nodes)
            .expect("a rewrite lands");
        assert!(forced.nodes_invalidated, "a rewrite always invalidates");
        let (map, rewritten_gl) = working_block(&rewritten, "MAP01");
        assert_eq!(rewritten_gl, None, "and the GL run goes with it");
        assert!(
            !map.other
                .iter()
                .any(|lump| lump.name.as_bytes() == b"ZNODES"),
            "the closure's node removal survived the re-encode"
        );
    }

    #[test]
    fn a_rewrite_of_a_block_without_a_gl_run_changes_only_the_block() {
        let lumps = write_map(&demo_map("MAP01", MapFormat::Doom)).expect("the fixture encodes");
        let mut session = base_session(lumps);
        let ((), report) = session
            .convert_map(name("MAP01"), "convert to UDMF", strip_nodes)
            .expect("a rewrite without a GL run lands");
        assert!(
            report.nodes_invalidated,
            "the report still says the nodes went"
        );
        let (_, gl) = working_block(&session, "MAP01");
        assert_eq!(gl, None, "and there was no run to drop");
        assert_eq!(
            size_of(session.working(), "NODES"),
            0_usize,
            "the vanilla placeholder is empty"
        );
    }

    #[test]
    fn a_failing_rewrite_changes_the_archive_and_history_nothing() {
        let mut session = base_session(iwad_lumps(MapFormat::Doom));
        let error = session
            .convert_map(name("MAP01"), "broken", failing)
            .expect_err("the closure refuses");
        assert_eq!(code(&error), "invalid_argument", "the error passes through");
        assert!(session.working().is_empty(), "not even the copy landed");
        assert!(session.undo_labels().is_empty(), "no history entry");
    }
}
