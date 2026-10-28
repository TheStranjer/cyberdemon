//! The engine's one geometry read: the light [`SectorShape`] row measured
//! against the full [`SectorFacts`] fields it stands in for — the area,
//! box, centre and roles of every sector, on the query fixtures and the
//! describe hand-drawn fixtures, for the `Doom` and `Hexen` families —
//! and the sector with a dangling boundary the engine now reads through
//! the stored [`SectorIndex`]: no centre to box, no area to keep, no
//! home for a thing or vertex, `ne` and `not_in` included.
//!
//! [`SectorShape`]: crate::wad::map::describe::SectorShape
//! [`SectorFacts`]: crate::wad::map::describe::SectorFacts
//! [`SectorIndex`]: crate::wad::map::geometry::SectorIndex

use alloc::vec;
use alloc::vec::Vec;

use serde_json::json;

use super::{defs, diagonal_map, field_query, filter, flaggy_map, hits, query, roley_map};
use crate::defs::Definitions;
use crate::wad::classify::MapFormat;
use crate::wad::map::common::fixtures::{name, one_sided, room, side, two_room_map, vertex};
use crate::wad::map::consts::SKY_FLAT;
use crate::wad::map::describe::Describe;
use crate::wad::map::describe::tests::{
    elbow_map, nested_exit_map, split_divider_map, square_map, switch_map,
};
use crate::wad::map::flags::{BLOCKING, TWO_SIDED};
use crate::wad::map::query::{ElementKind, Op, Within};
use crate::wad::map::{Linedef, Map, Sidedef};

/// The two-room map with the step line wired as a manual `Doom` door:
/// special `1` across an untagged line, so the sector behind it wears the
/// `door` role the light `roles` read must state.
fn wired_map() -> Map {
    let mut map = two_room_map();
    let step = map
        .linedefs
        .get_mut(2)
        .expect("the fixture has a step line");
    step.special = 1_i32;
    map
}

/// A two-sided divider from `vertices.0` down to `vertices.1`: its front
/// side faces `rooms.0` with the step's lower texture, its back opens
/// into `rooms.1`.
fn divider(vertices: (usize, usize), rooms: (usize, usize), sides: &mut Vec<Sidedef>) -> Linedef {
    let (v1, v2) = vertices;
    let (front, back) = rooms;
    let first = sides.len();
    sides.push(side("-", "STEP1", "-", front));
    sides.push(side("-", "-", "-", back));
    Linedef {
        v1,
        v2,
        flags: TWO_SIDED,
        front: Some(first),
        back: Some(first.saturating_add(1)),
        ..Default::default()
    }
}

/// Three rising rooms — `0..128` at floor `0`, a 64-wide tread at `16`,
/// `192..320` at `32`, all 256 deep — sharing two-sided dividers: the
/// tread's 64 x 256 footprint with a neighbour 16 units up and another
/// 16 down is exactly what the `stair_step` role read needs.
fn stepped_map() -> Map {
    let mut sides: Vec<Sidedef> = Vec::new();
    let lines = vec![
        one_sided(0, 7, &mut sides, 0),
        one_sided(7, 6, &mut sides, 0),
        divider((6, 1), (0, 1), &mut sides),
        divider((5, 2), (1, 2), &mut sides),
        one_sided(4, 3, &mut sides, 2),
        one_sided(6, 5, &mut sides, 1),
        one_sided(5, 4, &mut sides, 2),
        one_sided(2, 1, &mut sides, 1),
        one_sided(1, 0, &mut sides, 0),
        one_sided(3, 2, &mut sides, 2),
    ];
    let mut map = Map::new_empty(name("MAP01"), MapFormat::Doom);
    map.vertices = vec![
        vertex(0, 0),
        vertex(128, 0),
        vertex(192, 0),
        vertex(320, 0),
        vertex(320, 256),
        vertex(192, 256),
        vertex(128, 256),
        vertex(0, 256),
    ];
    map.linedefs = lines;
    map.sidedefs = sides;
    map.sectors = vec![
        room(0, 128, 160, 0),
        room(16, 128, 160, 0),
        room(32, 128, 160, 0),
    ];
    map
}

/// The two-room map with room 1's boundary left dangling: every sidedef
/// facing sector 1 stores a sector index past the block, so the sector
/// bounds no line, no box, no centre and no area.
fn dangling_map() -> Map {
    let mut map = two_room_map();
    for side_index in [3_usize, 5, 6, 7] {
        map.sidedefs
            .get_mut(side_index)
            .expect("the fixture has eight sides")
            .sector = 99;
    }
    map
}

/// Every fixture the parity walk measures: the query's own maps, the two
/// the engine gates on (a door-wired step line, a stair tread) and the
/// describe hand-drawn rooms.
fn fixtures() -> Vec<(&'static str, Map)> {
    vec![
        ("two rooms", two_room_map()),
        ("role marker", roley_map()),
        ("diagonal", diagonal_map()),
        ("flags and a void thing", flaggy_map()),
        ("wired door", wired_map()),
        ("stair tread", stepped_map()),
        ("square", square_map()),
        ("elbow", elbow_map()),
        ("switch wall", switch_map()),
        ("open dividers", split_divider_map(0, 0)),
        ("blocked dividers", split_divider_map(BLOCKING, BLOCKING)),
        (
            "touched exit",
            nested_exit_map(MapFormat::Doom, 52, BLOCKING),
        ),
        ("hexen portal", nested_exit_map(MapFormat::Hexen, 243, 0)),
    ]
}

#[test]
fn the_light_shape_row_answers_what_the_full_facts_measure() {
    let game_defs = defs();
    for (label, source) in fixtures() {
        compare_shapes(label, &source, &game_defs);
    }
}

/// Compare `source`'s light shape rows with the full facts rows, sector
/// by sector, once read as the `Doom` family and once as the `Hexen`
/// one: the fields the query engine reads must answer identically.
fn compare_shapes(label: &str, source: &Map, game_defs: &Definitions) {
    for layout in [MapFormat::Doom, MapFormat::Hexen] {
        let mut map = source.clone();
        map.format = layout;
        let describe = Describe::new(&map, game_defs, SKY_FLAT);
        for sector in 0..map.sectors.len() {
            let facts = describe
                .sector(sector)
                .expect("the fixtures hold every sector");
            let shape = describe
                .sector_shape(sector)
                .expect("the same sector shapes");
            assert_eq!(
                shape.area, facts.area,
                "{label} sector {sector}'s area at {layout:?}"
            );
            assert_eq!(
                shape.bbox, facts.bbox,
                "{label} sector {sector}'s box at {layout:?}"
            );
            assert_eq!(
                shape.center, facts.center,
                "{label} sector {sector}'s centre at {layout:?}"
            );
            assert_eq!(
                shape.roles, facts.roles,
                "{label} sector {sector}'s roles at {layout:?}"
            );
        }
    }
}

#[test]
fn the_light_read_states_the_roles_the_filters_ask_of_it() {
    let game_defs = defs();
    let roley = roley_map();
    let read = Describe::new(&roley, &game_defs, SKY_FLAT);
    let marker = read.sector_shape(1).expect("the role marker sector");
    assert!(
        marker.roles.iter().any(|role| role == "secret"),
        "special 9 still hides sector 1: {:?}",
        marker.roles
    );
    assert!(
        marker.roles.iter().any(|role| role == "sky"),
        "the F_SKY1 ceiling still opens sector 1"
    );
    let wired = wired_map();
    let door_read = Describe::new(&wired, &game_defs, SKY_FLAT);
    let band = door_read.sector_shape(1).expect("the room behind the door");
    assert!(
        band.roles.iter().any(|role| role == "door"),
        "the manual door still dresses sector 1: {:?}",
        band.roles
    );
}

#[test]
fn the_light_read_measures_the_stair_tread_it_roles() {
    let game_defs = defs();
    let stepped = stepped_map();
    let describe = Describe::new(&stepped, &game_defs, SKY_FLAT);
    let tread = describe.sector_shape(1).expect("the middle tread");
    assert_eq!(tread.area, 0x4000_u64, "the 64 x 256 footprint");
    assert!(
        tread.bbox.is_some_and(|[min_x, min_y, max_x, max_y]| {
            min_x == 128_i32 && max_x == 192_i32 && min_y == 0_i32 && max_y == 256_i32
        }),
        "the tread's own box: {:?}",
        tread.bbox
    );
    assert!(
        tread.center.is_some(),
        "the tread's centre lands inside it for the `bbox` bound"
    );
    assert!(
        tread.roles.iter().any(|role| role == "stair_step"),
        "one neighbour rises and one falls: {:?}",
        tread.roles
    );
    let low = describe.sector_shape(0).expect("the low room");
    assert!(
        !low.roles.iter().any(|role| role == "stair_step"),
        "the 128 x 256 room beside it is no tread"
    );
}

#[test]
fn a_sector_with_a_dangling_boundary_reads_as_no_match() {
    let map = dangling_map();
    let boxed = query(
        ElementKind::Sectors,
        Vec::new(),
        Some(Within::Bbox {
            bbox: [-1_000_i64, -1_000, 1_000, 1_000],
        }),
    );
    assert_eq!(
        hits(&map, &boxed),
        vec![0],
        "the shapeless room has no centre to box in"
    );
    let sized = field_query(ElementKind::Sectors, filter("area", Op::Ge, json!(1_i64)));
    assert_eq!(
        hits(&map, &sized),
        vec![0],
        "and a dangling boundary measures no area"
    );
    let claimed = field_query(ElementKind::Sectors, filter("roles", Op::Has, json!("sky")));
    assert!(
        hits(&map, &claimed).is_empty(),
        "and states no role the box lost"
    );
    let homed = field_query(ElementKind::Things, filter("sector", Op::Eq, json!(1_i64)));
    assert!(
        hits(&map, &homed).is_empty(),
        "the imp's room resolves nowhere"
    );
    let other = field_query(ElementKind::Things, filter("sector", Op::Ne, json!(1_i64)));
    assert_eq!(
        hits(&map, &other),
        vec![0],
        "ne does not rescue the unresolvable"
    );
    let outside = field_query(
        ElementKind::Things,
        filter("sector", Op::NotIn, json!([1_i64])),
    );
    assert_eq!(hits(&map, &outside), vec![0], "nor does not_in");
    let held = query(
        ElementKind::Vertices,
        Vec::new(),
        Some(Within::Sectors { sectors: vec![1] }),
    );
    assert!(
        hits(&map, &held).is_empty(),
        "a sector bounding nothing holds no vertex"
    );
}
