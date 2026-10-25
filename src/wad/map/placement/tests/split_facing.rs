//! Split-and-facing cases: the proportional needs and their remainder
//! ties, the angle snap and the bearings the facing modes follow, the
//! seeded compass walk, the face-the-start and face-the-centre modes, the
//! occupant radii that widen the blockers and the exact shape helpers.

use alloc::vec::Vec;

use crate::fixed::midpoint;
use crate::wad::classify::MapFormat;
use crate::wad::map::Map;
use crate::wad::map::Thing;
use crate::wad::map::placement::distribute::{distribute, sector_area};
use crate::wad::map::placement::facing::{bearing, next_u32};
use crate::wad::map::placement::{
    Area, Facing, Placed, occupants, player_start, snap_angle, split_proportional,
};

use super::{angles, doom_definitions, map_name, spec, square_rooms};

#[test]
fn proportional_needs_follow_the_bounding_box_areas() {
    let map = square_rooms(&[(0_i32, 100_i32), (200_i32, 400_i32)]);
    assert_eq!(sector_area(&map, 0), 10_000_i128);
    assert_eq!(
        split_proportional(&map, &[0_usize, 1], 5_usize),
        vec![1_usize, 4_usize],
        "the 1:4 area ratio splits 5 exactly"
    );
    assert_eq!(
        split_proportional(&map, &[0_usize, 1], 4_usize),
        vec![1_usize, 3_usize],
        "0.8 and 3.2: the bigger remainder rounds the small sector up"
    );
    assert_eq!(
        split_proportional(&map, &[1, 0], 4_usize),
        vec![3_usize, 1_usize],
        "and the result stays parallel to the requested order"
    );
}

#[test]
fn remainder_ties_go_to_the_lower_sector_index() {
    let map = square_rooms(&[(0_i32, 100_i32), (200_i32, 300_i32)]);
    assert_eq!(
        split_proportional(&map, &[0_usize, 1], 3_usize),
        vec![2_usize, 1_usize],
        "equal areas: the extra copy goes to the lower index"
    );
    assert_eq!(
        split_proportional(&map, &[1], 2_usize),
        vec![2_usize],
        "one area takes the whole count"
    );
    let empty = Map::new_empty(map_name("MAP01"), MapFormat::Doom);
    assert_eq!(
        split_proportional(&empty, &[0_usize, 1], 3_usize),
        vec![2_usize, 1_usize],
        "sectors bounding nothing split evenly, lower index first"
    );
}

#[test]
fn angles_snap_to_the_nearest_compass_point() {
    assert_eq!(snap_angle(0_i32), 0_i32);
    assert_eq!(snap_angle(22_i32), 0_i32);
    assert_eq!(snap_angle(23_i32), 45_i32);
    assert_eq!(snap_angle(70_i32), 90_i32);
    assert_eq!(snap_angle(350_i32), 0_i32, "350 rounds up to 360 = 0");
    assert_eq!(snap_angle(-23_i32), 315_i32, "-23 turns to 337");
    assert_eq!(snap_angle(400_i32), 45_i32, "400 turns to 40");
}

#[test]
fn bearings_point_where_the_facing_modes_follow() {
    assert_eq!(bearing((0_i64, 0_i64), (100_i64, 0_i64)), 0_i32);
    assert_eq!(bearing((0_i64, 0_i64), (0_i64, 100_i64)), 90_i32);
    assert_eq!(
        snap_angle(bearing((0_i64, 0_i64), (100_i64, 90_i64))),
        45_i32
    );
}

#[test]
fn the_seeded_facing_is_a_deterministic_compass_walk() {
    let map = square_rooms(&[(0_i32, 400_i32)]);
    let definitions = doom_definitions();
    let mut seeded = spec(Area::Sector(0), 6_usize, 20_i64);
    seeded.facing = Facing::Seeded(1_u32);
    let first = distribute(&map, &definitions, &seeded);
    let second = distribute(&map, &definitions, &seeded);
    assert_eq!(
        first.placed, second.placed,
        "the same seed places the same angles on the same points"
    );
    let mut state = 1_u32;
    let mut expected: Vec<i32> = Vec::new();
    for _ in 0..6_usize {
        state = next_u32(state);
        let octant = state.checked_rem(8_u32).expect("eight is non-zero");
        expected.push(i32::try_from(octant.checked_mul(45_u32).expect("small")).expect("fit"));
    }
    assert_eq!(angles(&first), expected, "the documented xorshift32 walk");
    seeded.facing = Facing::Seeded(2_u32);
    let third = distribute(&map, &definitions, &seeded);
    assert_ne!(
        angles(&first),
        angles(&third),
        "another seed walks elsewhere"
    );
    seeded.facing = Facing::Seeded(0_u32);
    let zero = distribute(&map, &definitions, &seeded);
    assert_eq!(
        angles(&zero),
        angles(&first),
        "the zero state is replaced by one: seed 0 walks like seed 1"
    );
}

#[test]
fn the_face_modes_follow_the_start_and_the_centre() {
    let mut map = square_rooms(&[(0_i32, 400_i32)]);
    let definitions = doom_definitions();
    map.things.push(Thing {
        x: 20_i32,
        y: 200_i32,
        kind: 1_i32,
        ..Default::default()
    });
    let start = player_start(&map).expect("the map carries a start");
    assert_eq!(start, (20_i64, 200_i64));
    let mut to_start = spec(Area::Sector(0), 1_usize, 20_i64);
    to_start.facing = Facing::PlayerStart;
    let out = distribute(&map, &definitions, &to_start);
    assert_eq!(
        out.placed,
        vec![Placed {
            at: (200_i64, 200_i64),
            angle: 180_i32
        }],
        "the centre pick sits due east of the start and faces back west"
    );
    let mut to_centre = spec(Area::Sector(0), 1_usize, 20_i64);
    to_centre.facing = Facing::Centre;
    let centred = distribute(&map, &definitions, &to_centre);
    assert_eq!(
        centred.placed,
        vec![Placed {
            at: (200_i64, 200_i64),
            angle: 0_i32
        }],
        "the centre faces itself: a zero bearing from its own point"
    );
}

#[test]
fn blockers_widen_with_the_occupant_radius() {
    let map = square_rooms(&[(0_i32, 200_i32)]);
    let definitions = doom_definitions();
    let blockers = occupants(&map, &definitions, 20_i64);
    assert!(blockers.is_empty(), "the bare rooms hold no things");
    let mut crowded = map;
    crowded.things.push(Thing {
        x: 100_i32,
        y: 100_i32,
        kind: 999_999_i32,
        ..Default::default()
    });
    crowded.things.push(Thing {
        x: 20_i32,
        y: 20_i32,
        kind: 3001_i32,
        ..Default::default()
    });
    let armed = occupants(&crowded, &definitions, 20_i64);
    assert_eq!(
        armed
            .iter()
            .map(|blocker| blocker.radius)
            .collect::<Vec<i64>>(),
        vec![20_i64, 20_i64],
        "the unknown type takes the fallback, the Imp its defined radius"
    );
}

#[test]
fn the_midpoint_and_area_shape_helpers_stay_exact() {
    assert_eq!(midpoint(0_i64, 100_i64), 50_i64);
    assert_eq!(midpoint(-300_i64, 300_i64), 0_i64);
    let circle = Area::Circle {
        centre: (0_i64, 0_i64),
        radius: 64_i64,
    };
    let bounds = square_rooms(&[(0_i32, 100_i32)]);
    assert_eq!(
        circle.bbox(&bounds),
        Some((-64_i64, -64_i64, 64_i64, 64_i64))
    );
    assert_eq!(circle.centre(&bounds), Some((0_i64, 0_i64)));
    assert_eq!(circle.label(), "the circle");
    assert_eq!(Area::Sector(7).label(), "sector 7");
    assert_eq!(
        Area::Rect {
            min_x: 0_i64,
            min_y: 0_i64,
            max_x: 1_i64,
            max_y: 1_i64,
        }
        .label(),
        "the bounding box"
    );
    assert_eq!(next_u32(0_u32), next_u32(1_u32), "zero starts at one");
}

#[test]
fn an_area_with_no_need_places_nothing() {
    let map = square_rooms(&[(0_i32, 200_i32)]);
    let definitions = doom_definitions();
    let out = distribute(&map, &definitions, &spec(Area::Sector(0), 0_usize, 20_i64));
    assert!(out.placed.is_empty(), "need 0 asks for nothing");
    assert!(out.warnings.is_empty(), "and gets no note");
}
