//! Shared builders and assertions for the `plan::emit` tests: the room,
//! opening and door constructors, the build-place-emit runners, the line
//! and vertex lookups the topics fold their checks through, and the
//! sidedness, orientation and validation-cleanliness invariants.
//!
//! [`geometry`] emits whole plans and checks the sectors, lines and
//! bookkeeping the emission produces.

mod geometry;

use alloc::collections::BTreeSet;

use crate::defs::definitions_for;
use crate::error::CyberError;
use crate::geom::{Label, Piece};
use crate::plan::connectors::{ConnectorGeometry, plan_connectors};
use crate::plan::emit::{Emitted, MapIdentity, emit};
use crate::plan::model::{Connection, ConnectionKind, DoorStyle, Plan, Room, Shape};
use crate::plan::walls::build_walls;
use crate::wad::game::Game;
use crate::wad::map::flags::{BLOCKING, TWO_SIDED};
use crate::wad::map::validate::{Context, Issue, ResourceIndex, run};
use crate::wad::map::{Linedef, Map, MapFormat};
use crate::wad::name::LumpName;

/// A parsed map name for test data.
pub(super) fn map_name(text: &str) -> LumpName {
    LumpName::parse(text).expect("test map names are valid")
}

/// The identity every test emits into.
fn identity() -> MapIdentity {
    MapIdentity {
        name: map_name("MAP01"),
        format: MapFormat::Doom,
    }
}

/// A rectangular room with default sector properties.
pub(super) fn rect_room(id: &str, left_bottom: (i64, i64), width_height: (i64, i64)) -> Room {
    Room {
        id: id.to_owned(),
        shape: Shape::Rect {
            x: left_bottom.0,
            y: left_bottom.1,
            w: width_height.0,
            h: width_height.1,
        },
        floor: 0,
        ceiling: 128,
        light: 160,
        floor_flat: None,
        ceiling_flat: None,
        wall_texture: None,
        outdoor: false,
        secret: false,
        light_effect: None,
    }
}

/// A default-width opening between two rooms by id.
pub(super) fn opening(from: &str, to: &str) -> Connection {
    Connection {
        id: None,
        from: from.to_owned(),
        to: to.to_owned(),
        kind: ConnectionKind::Opening,
        at: None,
        width: 64,
    }
}

/// A styled door between two rooms by id.
pub(super) fn door(from: &str, to: &str, style: DoorStyle) -> Connection {
    let mut passage = opening(from, to);
    passage.id = Some("d1".to_owned());
    passage.kind = ConnectionKind::Door { style };
    passage
}

/// Two 128 x 128 rooms sharing the line x = 128, `thickness` apart.
pub(super) fn two_rooms(thickness: i64, connections: Vec<Connection>) -> Plan {
    Plan {
        rooms: vec![
            rect_room("a", (0, 0), (128, 128)),
            rect_room("b", (128, 0), (128, 128)),
        ],
        connections,
        features: Vec::new(),
        wall_thickness: thickness,
    }
}

/// A plan of `rooms` at `thickness` with no connections.
pub(super) fn rooms_plan(rooms: Vec<Room>, thickness: i64) -> Plan {
    Plan {
        rooms,
        connections: Vec::new(),
        features: Vec::new(),
        wall_thickness: thickness,
    }
}

/// Build, place and emit a plan that must succeed, keeping the
/// placement for the bookkeeping asserts.
#[track_caller]
pub(super) fn compile(plan: &Plan) -> (Emitted, Vec<ConnectorGeometry>) {
    let walls = build_walls(plan).expect("the fixture walls build");
    let connectors = plan_connectors(plan, &walls).expect("the connectors place");
    let emitted = emit(plan, &walls, &connectors, identity()).expect("the plan emits");
    (emitted, connectors)
}

/// Build, place and emit a plan whose emission must fail.
#[track_caller]
pub(super) fn emit_error(plan: &Plan) -> CyberError {
    let walls = build_walls(plan).expect("the fixture walls build");
    let connectors = plan_connectors(plan, &walls).expect("the connectors place");
    emit(plan, &walls, &connectors, identity()).expect_err("the plan must not emit")
}

/// The sectors behind both sides of `line`, `None` when one-sided.
fn side_sectors(map: &Map, line: &Linedef) -> Option<(usize, usize)> {
    let (Some(front), Some(back)) = map.line_sectors(line) else {
        return None;
    };
    Some((front, back))
}

/// The two sectors of the `index`th line, ordered, asserting the line
/// is flagged two-sided.
pub(super) fn door_pair(map: &Map, index: usize) -> (usize, usize) {
    let line = map.linedefs.get(index).expect("recorded index");
    assert_eq!(line.flags, TWO_SIDED, "door lines are two-sided");
    let (front, back) = side_sectors(map, line).expect("door lines have two sides");
    (front.min(back), front.max(back))
}

/// The indices of the two-sided lines joining sectors `one` and
/// `two`, in either order.
pub(super) fn lines_between(map: &Map, one: usize, two: usize) -> Vec<usize> {
    map.linedefs
        .iter()
        .enumerate()
        .filter_map(|(index, line)| joins_sectors(map, line, one, two).then_some(index))
        .collect()
}

/// Whether `line` is two-sided between sectors `one` and `two`.
fn joins_sectors(map: &Map, line: &Linedef, one: usize, two: usize) -> bool {
    let Some((front, back)) = side_sectors(map, line) else {
        return false;
    };
    (front == one && back == two) || (front == two && back == one)
}

/// The flags of the `index`th line.
pub(super) fn line_flags(map: &Map, index: usize) -> u32 {
    map.linedefs.get(index).expect("recorded index").flags
}

/// The shared `x` of a vertical line, `None` otherwise.
fn vertical_x(map: &Map, line: &Linedef) -> Option<i32> {
    let first = map.vertices.get(line.v1)?;
    let second = map.vertices.get(line.v2)?;
    (first.x == second.x).then_some(first.x)
}

/// Whether the `index`th line is vertical at coordinate `x`.
pub(super) fn vertical_at(map: &Map, index: usize, x: i32) -> bool {
    let line = map.linedefs.get(index).expect("recorded index");
    vertical_x(map, line) == Some(x)
}

/// Whether both ends of `line` sit on the same edge of the map's
/// vertex bounds.
pub(super) fn on_map_border(map: &Map, line: &Linedef) -> bool {
    let (min_x, min_y, max_x, max_y) = map.bounds().expect("the map has vertices");
    let first = map.vertices.get(line.v1).expect("test indices");
    let second = map.vertices.get(line.v2).expect("test indices");
    (first.x == min_x && second.x == min_x)
        || (first.x == max_x && second.x == max_x)
        || (first.y == min_y && second.y == min_y)
        || (first.y == max_y && second.y == max_y)
}

/// The stored coordinate of the `index`th vertex, widened to `i64`.
fn point_of(map: &Map, index: usize) -> (i64, i64) {
    let vertex = map.vertices.get(index).expect("test indices");
    (i64::from(vertex.x), i64::from(vertex.y))
}

/// Whether `line` is one-sided and its front faces sector `sector`.
pub(super) fn faces_sector(map: &Map, line: &Linedef, sector: usize) -> bool {
    line.back.is_none() && map.side_sector(line.front) == Some(sector)
}

/// Sidedness and flags agree with the piece faces: void-facing pieces
/// are the blocking one-sided lines, region-vs-region pieces are
/// flagged two-sided with two distinct sidedef rows.
pub(super) fn assert_sidedness_matches_faces(emitted: &Emitted) {
    assert_eq!(
        emitted.map.linedefs.len(),
        emitted.pieces.len(),
        "one line per piece"
    );
    for (piece, line) in emitted.pieces.iter().zip(&emitted.map.linedefs) {
        assert_piece_facing(piece, line);
    }
}

/// The sidedness and flags of one piece's line.
fn assert_piece_facing(piece: &Piece, line: &Linedef) {
    let touches_void = piece.left == Label::Void || piece.right == Label::Void;
    assert_eq!(
        touches_void,
        line.back.is_none(),
        "void-facing pieces are the one-sided lines: {piece:?}"
    );
    let expected = if line.back.is_some() {
        TWO_SIDED
    } else {
        BLOCKING
    };
    assert_eq!(line.flags, expected, "flags match sidedness");
    let front = line.front.expect("every emitted line has a front");
    assert_ne!(line.back, Some(front), "the two sides never share a row");
}

/// One piece becomes exactly its linedef: the front side carries the
/// non-void face's sector and the endpoints follow the flip rule.
pub(super) fn assert_line_orientation(emitted: &Emitted, piece: &Piece, line: &Linedef) {
    assert_ne!(piece.left, piece.right, "arrange drops same-label pieces");
    let flipped = piece.right == Label::Void;
    let front_label = if flipped { piece.left } else { piece.right };
    let expected = emitted
        .sector_of_label
        .get(&front_label)
        .copied()
        .expect("every surviving face owns a sector");
    let front = line.front.expect("every emitted line has a front");
    let row = emitted
        .map
        .side_sector(Some(front))
        .expect("the front row exists");
    assert_eq!(row, expected, "the front side faces the piece's region");
    let (start, stop) = if flipped {
        (piece.b, piece.a)
    } else {
        (piece.a, piece.b)
    };
    assert_eq!(
        point_of(&emitted.map, line.v1),
        start,
        "v1 is the flipped start"
    );
    assert_eq!(
        point_of(&emitted.map, line.v2),
        stop,
        "v2 is the flipped stop"
    );
}

/// Every emitted side is an empty placeholder: the textures slice
/// dresses them later.
pub(super) fn assert_sides_are_placeholders(map: &Map) {
    for side in &map.sidedefs {
        assert_eq!(
            (
                side.upper.as_str(),
                side.lower.as_str(),
                side.middle.as_str()
            ),
            ("-", "-", "-"),
            "textures wait for the textures slice"
        );
        assert_eq!(side.x_offset, 0_i32, "no offsets yet");
        assert_eq!(side.y_offset, 0_i32, "no offsets yet");
    }
}

/// Sectors and lines carry no specials, tags, args or extras yet.
pub(super) fn assert_rows_are_plain(map: &Map) {
    for sector in &map.sectors {
        assert_eq!(sector.special, 0_i32, "no sector specials yet");
        assert_eq!(sector.tag, 0_i32, "no sector tags yet");
        assert!(sector.extra.is_empty(), "no sector extras");
    }
    for line in &map.linedefs {
        assert_eq!(line.special, 0_i32, "no line specials yet");
        assert_eq!(line.tag, 0_i32, "no line tags yet");
        assert!(line.args.iter().all(|&arg| arg == 0_i32), "no line args");
    }
}

/// The issues the geometry and structure validation checks report for
/// `map`, filtered to the arrangement's contract.
pub(super) fn geometry_issues(map: &Map) -> Vec<Issue> {
    let definitions = definitions_for(Game::Doom).expect("the embedded tables parse");
    let resources = ResourceIndex::default();
    let context = Context {
        definitions: &definitions,
        hexen_specials: &definitions,
        resources: &resources,
    };
    let enabled: BTreeSet<&'static str> = [
        "crossing_lines",
        "overlapping_lines",
        "two_sided_flag_mismatch",
        "zero_length_line",
        "duplicate_line",
    ]
    .into_iter()
    .collect();
    run(map, &context, Some(&enabled)).issues
}
