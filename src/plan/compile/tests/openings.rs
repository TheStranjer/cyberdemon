//! The openings: an opening's whole dressing run with its band inheriting
//! the rooms at its surfaces, the full-width openings that join two rooms
//! with no band sector at all (with and without wall thickness), the two
//! repro plans measured across a snapped edge and a short one, and the
//! zero-thickness rooms sharing their walls.

use super::{
    assert_no_bare_flats, assert_no_gaps, assert_valid, compiled, lines_between, opening,
    rect_room, two_rooms, wall_faces,
};
use crate::plan::model::{ConnectionKind, Plan, Room, Shape};
use crate::plan::units::{Units, transform_plan};
use crate::wad::map::{Linedef, Map};

#[test]
fn an_opening_between_two_rooms_dresses_every_wall_and_stays_clean() {
    let plan = two_rooms(16, vec![opening("a", "b")]);
    let out = compiled(&plan);
    let map = &out.map;
    assert_eq!(map.sectors.len(), 3, "two rooms plus the band");
    assert_eq!(
        out.room_sectors.get("a"),
        Some(&vec![0_usize]),
        "rooms keep plan order"
    );
    assert_eq!(out.room_sectors.get("b"), Some(&vec![1_usize]));
    let info = out.connector_sectors.first().expect("the opening");
    assert_eq!(info.id, "a->b", "the unnamed connection label");
    assert_eq!(info.kind, ConnectionKind::Opening, "kind rides along");
    assert_eq!(info.sectors, vec![2], "one band sector");
    assert_eq!(info.door_lines.len(), 2, "the band meets each room once");
    assert_eq!(
        one_sided_at(map, 120),
        2,
        "the void wall stands at x=120, split below and above the opening"
    );
    assert_eq!(one_sided_at(map, 136), 2, "and at x=136 likewise");
    for (row, flags) in wall_faces(map) {
        assert_eq!(
            row.middle, "WALROLE",
            "every face to the void (room walls and the opening's jambs) is walled"
        );
        assert_eq!(flags, 1_u32, "one-sided lines just block");
    }
    for index in [0_usize, 1] {
        let space = map.sectors.get(index).expect("a room sector");
        assert_eq!(
            space.floor_flat, "FLOROLE",
            "unset room floors take the role"
        );
        assert_eq!(space.ceiling_flat, "CEIROLE", "and so do the ceilings");
    }
    let band = map.sectors.get(2).expect("the band sector");
    assert_eq!(
        band.floor_flat, "FLOROLE",
        "an opening's band inherits the room standing at its floor"
    );
    assert_eq!(
        band.ceiling_flat, "CEIROLE",
        "and the room standing at its ceiling"
    );
    assert_no_bare_flats(map);
    assert_no_gaps(map);
    assert!(
        out.warnings.is_empty(),
        "the plan is sound: {:?}",
        out.warnings
    );
    assert_valid(map);
}

/// The shared `x` of a vertical line, `None` otherwise.
fn vertical_x(map: &Map, line: &Linedef) -> Option<i32> {
    let first = map.vertices.get(line.v1)?;
    let second = map.vertices.get(line.v2)?;
    (first.x == second.x).then_some(first.x)
}

/// The one-sided lines standing on the vertical grid line `x`.
fn one_sided_at(map: &Map, x: i32) -> usize {
    map.linedefs
        .iter()
        .filter(|line| line.back.is_none() && vertical_x(map, line) == Some(x))
        .count()
}

#[test]
fn a_full_width_opening_joins_two_touching_rooms_with_no_extra_sector() {
    let mut arch = opening("a", "b");
    arch.id = Some("arch".to_owned());
    arch.width = 128_i64;
    let out = compiled(&two_rooms(16, vec![arch]));
    let map = &out.map;
    assert_eq!(map.sectors.len(), 2, "the two rooms and nothing between");
    let info = out.connector_sectors.first().expect("the arch");
    assert_eq!(info.id, "arch", "the connection still reports under its id");
    assert!(
        info.sectors.is_empty(),
        "an opening across the whole run owns no sector"
    );
    assert!(
        info.door_lines.is_empty(),
        "and no line to hang a special on"
    );
    let shared = lines_between(map, 0, 1);
    assert_eq!(
        shared.len(),
        1,
        "the rooms share their whole boundary as one line"
    );
    assert_eq!(
        wall_faces(map).len(),
        6,
        "the outer walls of the two rooms are all that stands"
    );
    assert_eq!(one_sided_at(map, 120), 0, "no stub wall at the inner face");
    assert_eq!(one_sided_at(map, 136), 0, "and none at the other one");
    assert_eq!(
        out.room_sectors.get("a"),
        Some(&vec![0_usize]),
        "both rooms keep their own sector"
    );
    assert_eq!(out.room_sectors.get("b"), Some(&vec![1_usize]));
    assert_no_bare_flats(map);
    assert_no_gaps(map);
    assert!(
        out.warnings.is_empty(),
        "the join needs no note: {:?}",
        out.warnings
    );
    assert_valid(map);
}

#[test]
fn a_full_width_opening_at_zero_thickness_joins_the_rooms_too() {
    let mut arch = opening("a", "b");
    arch.width = 128_i64;
    let out = compiled(&two_rooms(0, vec![arch]));
    let map = &out.map;
    assert_eq!(
        map.sectors.len(),
        2,
        "the band sector that used to sit here is gone"
    );
    assert_eq!(
        lines_between(map, 0, 1).len(),
        1,
        "the rooms share their line whole"
    );
    assert_no_bare_flats(map);
    assert_valid(map);
}

/// The reported repro: an `L`-shaped `north` room and a `south` room
/// touching it along one edge, both drawn in image units and carried
/// into map units by the caller's scale, vertical mirror and `16`
/// snap grid. The shared run lands `304` map units long; `width` is
/// the opening the caller asked for.
fn repro_plan(width: i64) -> Plan {
    let north = Room {
        id: "north".to_owned(),
        shape: Shape::Polygon {
            points: vec![
                (100, 140),
                (725, 140),
                (725, 610),
                (585, 610),
                (585, 335),
                (100, 335),
            ],
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
    };
    let mut plan = Plan {
        rooms: vec![north, rect_room("south", (64, 410), (521, 200))],
        connections: vec![opening("north", "south")],
        features: Vec::new(),
        wall_thickness: 16_i64,
    };
    if let Some(connection) = plan.connections.first_mut() {
        connection.width = width;
    }
    let units = Units {
        scale_num: 3_i64,
        scale_den: 2_i64,
        origin_x: 0_i64,
        origin_y: 0_i64,
        grid: 16_i64,
        flip_y: true,
    };
    transform_plan(&plan, &units, units.grid).expect("the repro transform runs")
}

#[test]
fn a_repro_opening_across_the_whole_snapped_edge_joins_the_rooms() {
    let out = compiled(&repro_plan(304_i64));
    assert_eq!(
        out.map.sectors.len(),
        2,
        "no 16x304 sliver stands between the rooms"
    );
    assert!(
        out.connector_sectors
            .first()
            .expect("the opening")
            .sectors
            .is_empty(),
        "the full-width opening owns no sector"
    );
    assert_eq!(
        lines_between(&out.map, 0, 1).len(),
        1,
        "the rooms share their snapped edge as one open line"
    );
    assert_no_bare_flats(&out.map);
    assert_no_gaps(&out.map);
    assert!(
        out.warnings.is_empty(),
        "the join needs no note: {:?}",
        out.warnings
    );
    assert_valid(&out.map);
}

/// The endpoints of every one-sided vertical line standing on one of
/// the `x` coordinates in `faces`.
fn wall_face_points(map: &Map, faces: &[i32]) -> Vec<(i32, i32)> {
    map.linedefs
        .iter()
        .filter(|row| row.back.is_none())
        .filter(|row| vertical_x(map, row).is_some_and(|x| faces.contains(&x)))
        .flat_map(|row| [row.v1, row.v2])
        .map(|index| {
            let vertex = map.vertices.get(index).expect("test indices");
            (vertex.x, vertex.y)
        })
        .collect()
}

#[test]
fn a_repro_opening_short_of_the_edge_keeps_one_dressed_band() {
    let out = compiled(&repro_plan(288_i64));
    assert_eq!(
        out.map.sectors.len(),
        3,
        "two rooms plus the one band the wall still needs"
    );
    assert_eq!(
        out.connector_sectors.first().expect("the opening").sectors,
        vec![2_usize],
        "the narrower opening keeps its sector"
    );
    let band = out.map.sectors.get(2).expect("the band sector");
    assert_eq!(band.floor_flat, "FLOROLE", "the band inherits the rooms");
    for (x, y) in wall_face_points(&out.map, &[872_i32, 888_i32]) {
        assert!(
            (-912_i32..=(-608_i32)).contains(&y),
            "the wall face at x={x} reaches past the shared run's ends"
        );
    }
    assert_no_bare_flats(&out.map);
    assert_no_gaps(&out.map);
    assert_valid(&out.map);
}

#[test]
fn zero_thickness_rooms_share_their_walls_and_the_opening_still_works() {
    let plan = two_rooms(0, vec![opening("a", "b")]);
    let out = compiled(&plan);
    let map = &out.map;
    assert_eq!(map.sectors.len(), 3, "the band still gets its own sector");
    let shared = lines_between(map, 0, 1);
    assert_eq!(
        shared.len(),
        2,
        "the rooms meet directly on x=128 below and above the band"
    );
    assert!(
        !lines_between(map, 0, 2).is_empty() && !lines_between(map, 2, 1).is_empty(),
        "the depth-16 band still cuts both rooms"
    );
    assert_eq!(
        out.connector_sectors.first().expect("the opening").sectors,
        vec![2]
    );
    assert_no_gaps(map);
    assert_valid(map);
}
