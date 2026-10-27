//! The one undoable map-edit step, run over a session's archives: the
//! copy-in and property edit that keeps the nodes, the geometry edit that
//! empties the node lumps and sheds the split vertices the shed run appended
//! while keeping every point the run never named, the `GL` run and `ZNODES`
//! that go with them, the failing edit that changes nothing, the directory
//! position the block keeps, and `rewrite_in_place`'s forced invalidation.

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

/// An edit closure that appends one unreferenced vertex and names it in a
/// written `SEGS` record, the way `build_nodes` lands its split points.
fn append_split_vertex(map: &mut Map) -> Result<(), CyberError> {
    let split = map.vertices.len();
    map.vertices.push(vertex(128, 0));
    let segs = map
        .other
        .iter_mut()
        .find(|lump| lump.name.as_bytes() == b"SEGS")
        .ok_or_else(|| CyberError::InvalidArgument("the fixture has no SEGS".to_owned()))?;
    let index = u16::try_from(split).map_err(|error| {
        CyberError::InvalidArgument(format!("the fixture outgrew a seg record: {error}"))
    })?;
    *segs = Lump::new(name("SEGS"), seg_record(0, index));
    Ok(())
}

/// One vanilla `SEGS` record running from vertex `start` to vertex `end`:
/// the two endpoint indices, then the angle, linedef, side and offset the
/// vertex shed never looks at.
fn seg_record(start: u16, end: u16) -> Vec<u8> {
    let mut bytes = start.to_le_bytes().to_vec();
    bytes.extend_from_slice(&end.to_le_bytes());
    bytes.extend_from_slice(&[0_u8; 8]);
    bytes
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

/// The empty node lumps a fresh `create_map` block carries: the five
/// placeholders, all of them zero bytes.
fn empty_run() -> Vec<Lump> {
    ["SEGS", "SSECTORS", "NODES", "REJECT", "BLOCKMAP"]
        .into_iter()
        .map(marker)
        .collect()
}

/// The hand-built `MAP01`: three points no linedef ends at, no lines and no
/// sectors at all, over the empty node lumps a fresh `create_map` block
/// carries. The ticket's repro shape: `add_vertex` three times, then
/// `add_sector`.
fn handbuilt_map() -> Map {
    Map {
        name: name("MAP01"),
        format: MapFormat::Doom,
        udmf_namespace: None,
        things: Vec::new(),
        vertices: vec![vertex(0, 0), vertex(128, 0), vertex(128, 96)],
        linedefs: Vec::new(),
        sidedefs: Vec::new(),
        sectors: Vec::new(),
        other: empty_run(),
    }
}

/// A base archive of one hand-built `MAP01`.
fn handbuilt_lumps() -> Vec<Lump> {
    write_map(&handbuilt_map()).expect("the hand-built fixture encodes")
}

/// An edit closure that appends one sector, the way `add_sector` does: the
/// sector count moves and no geometry with it. Fails on a fixture that holds
/// sectors already, which would make the counts below assert nothing.
fn add_sector(map: &mut Map) -> Result<(), CyberError> {
    if !map.sectors.is_empty() {
        return Err(CyberError::InvalidArgument(
            "the fixture already holds sectors".to_owned(),
        ));
    }
    map.sectors.push(room(0, 176));
    Ok(())
}

#[test]
fn a_map_without_a_node_run_keeps_its_unwired_vertices() {
    let mut session = base_session(handbuilt_lumps());
    let ((), report) = session
        .edit_map(name("MAP01"), "add sector", add_sector)
        .expect("the sector lands");
    assert!(
        report.nodes_invalidated,
        "the sector count sizes REJECT, so the block's node data is trusted no more"
    );
    let (map, _) = working_block(&session, "MAP01");
    assert_eq!(
        map.vertices.len(),
        3,
        "and the three points no line ends at still stand: the block never built one to shed"
    );
    assert_eq!(map.sectors.len(), 1, "the sector came in");
}

#[test]
fn the_sector_fixture_refuses_a_map_that_already_holds_one() {
    let mut taken = handbuilt_map();
    taken.sectors.push(room(0, 176));
    let error = add_sector(&mut taken).expect_err("the fixture guards the counts it is checked by");
    assert_eq!(code(&error), "invalid_argument", "guard code");
    assert_eq!(taken.sectors.len(), 1, "and nothing came in");
}

#[test]
fn a_sector_edit_is_followed_by_a_line_ending_at_the_points_it_kept() {
    let mut session = base_session(handbuilt_lumps());
    session
        .edit_map(name("MAP01"), "add sector", add_sector)
        .expect("the sector lands");
    let ((), wired) = session
        .edit_map(name("MAP01"), "wire a wall", |map| {
            map.sidedefs.push(side(0));
            map.linedefs.push(line(0, 1, Some(0), None));
            Ok(())
        })
        .expect("a linedef can end at a kept vertex");
    assert!(wired.nodes_invalidated, "a new line is geometry");
    let (map, _) = working_block(&session, "MAP01");
    assert_eq!(map.vertices.len(), 3, "all three points are still there");
    assert_eq!(
        map.linedefs.first().expect("the new line").v2,
        1,
        "and the line reaches the second of them"
    );
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
        "the stale split point went with the nodes the run wrote it for"
    );
    assert_eq!(other_size(&session, "SEGS"), 0, "and the run emptied");
}

#[test]
fn a_callers_own_vertex_holds_the_split_points_behind_it() {
    let mut session = base_session(iwad_lumps(MapFormat::Doom));
    session
        .edit_map(name("MAP01"), "split corner", append_split_vertex)
        .expect("the build-like edit lands");
    session
        .edit_map(name("MAP01"), "add a point", |map| {
            map.vertices.push(vertex(64, 64));
            Ok(())
        })
        .expect("the append lands");
    let ((), report) = session
        .edit_map(name("MAP01"), "move corner", move_vertex)
        .expect("the move lands");
    assert!(report.nodes_invalidated, "moving a used vertex is geometry");
    let (map, _) = working_block(&session, "MAP01");
    assert_eq!(
        map.vertices.len(),
        6,
        "the point the run never named stays, and the shed stops at it rather than \
         renumber the caller's index"
    );
    let last = map.vertices.get(5).expect("both appends stand");
    assert_eq!(
        (last.x, last.y),
        (64_i32, 64_i32),
        "and it is still the caller's point"
    );
    assert_eq!(other_size(&session, "SEGS"), 0, "the run itself emptied");
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
    lumps.extend(write_map(&demo_map("MAP02", MapFormat::Doom)).expect("the second map encodes"));
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
