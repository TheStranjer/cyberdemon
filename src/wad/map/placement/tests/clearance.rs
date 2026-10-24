//! Clearance cases: what fits where, the exact distance test and the grid
//! walkers, whole [`distribute`] runs on the square, L, box and circle
//! areas, and the existing bodies that keep their room off the grid.

use alloc::vec::Vec;

use crate::wad::map::Thing;
use crate::wad::map::placement::distribute::distribute;
use crate::wad::map::placement::{
    Area, CLEARANCE, DEFAULT_RADIUS, axis_points, fits, keeps_distance, thing_clearance,
};

use super::{doom_definitions, l_room, points, spec, square_rooms};

#[test]
fn fits_requires_the_sector_and_the_clearance_from_every_wall() {
    let map = square_rooms(&[(0_i32, 100_i32), (400_i32, 500_i32)]);
    assert!(fits(&map, 0, (50_i64, 50_i64), CLEARANCE));
    assert!(
        fits(&map, 0, (20_i64, 50_i64), 20_i64),
        "exactly the clearance fits"
    );
    assert!(
        !fits(&map, 0, (19_i64, 50_i64), 20_i64),
        "one short does not"
    );
    assert!(
        !fits(&map, 0, (50_i64, 450_i64), CLEARANCE),
        "a point in the other room's sector is not this one's"
    );
    assert!(
        !fits(&map, 0, (5000_i64, 5000_i64), CLEARANCE),
        "a point in no sector at all"
    );
}

#[test]
fn the_distance_test_measures_perpendicular_and_endpoint_cases() {
    assert!(keeps_distance(
        (0_i128, 0_i128),
        (100_i128, 0_i128),
        (50_i128, 28_i128),
        28_i128
    ));
    assert!(!keeps_distance(
        (0_i128, 0_i128),
        (100_i128, 0_i128),
        (50_i128, 27_i128),
        28_i128
    ));
    assert!(
        !keeps_distance(
            (0_i128, 0_i128),
            (100_i128, 0_i128),
            (-20_i128, 20_i128),
            29_i128
        ),
        "beyond the endpoint the distance is to the corner: sqrt(800) < 29"
    );
    assert!(
        keeps_distance(
            (0_i128, 0_i128),
            (100_i128, 0_i128),
            (-20_i128, 20_i128),
            28_i128
        ),
        "and sqrt(800) >= 28"
    );
}

#[test]
fn axis_points_includes_the_far_bound_and_is_capped() {
    assert_eq!(
        axis_points(20_i64, 100_i64, 40_i64),
        vec![20_i64, 60_i64, 100_i64]
    );
    assert_eq!(axis_points(50_i64, 40_i64, 40_i64), Vec::<i64>::new());
    assert_eq!(
        axis_points(0_i64, i64::MAX, 1_i64).len(),
        4_096_usize,
        "a huge axis stops at the cap"
    );
}

#[test]
fn clearance_uses_the_defined_radius_or_the_given_fallback() {
    let definitions = doom_definitions();
    assert_eq!(
        thing_clearance(&definitions, 3001_u32, 20_i64),
        20_i64.checked_add(CLEARANCE).expect("imp radius"),
        "the Imp's 20-unit radius"
    );
    assert_eq!(
        thing_clearance(&definitions, 999_999_u32, DEFAULT_RADIUS),
        DEFAULT_RADIUS
            .checked_add(CLEARANCE)
            .expect("default radius"),
        "an unlisted type gets the caller's fallback"
    );
}

#[test]
fn the_centred_grid_lays_evenly_in_a_rectangle_and_orders_centre_outward() {
    let map = square_rooms(&[(0_i32, 180_i32)]);
    let definitions = doom_definitions();
    let out = distribute(&map, &definitions, &spec(Area::Sector(0), 9_usize, 20_i64));
    assert_eq!(
        points(&out),
        vec![
            (90_i64, 90_i64),
            (90_i64, 50_i64),
            (50_i64, 90_i64),
            (130_i64, 90_i64),
            (90_i64, 130_i64),
            (50_i64, 50_i64),
            (130_i64, 50_i64),
            (50_i64, 130_i64),
            (130_i64, 130_i64)
        ],
        "step 40 centred in the 0..180 box: the slack 20 splits 10/10 \
         and the 10/170 rails fall inside the wall clearance"
    );
    assert!(out.warnings.is_empty(), "the room held everything asked");
}

#[test]
fn the_spread_walks_every_kth_candidate() {
    let map = square_rooms(&[(0_i32, 180_i32)]);
    let definitions = doom_definitions();
    let out = distribute(&map, &definitions, &spec(Area::Sector(0), 3_usize, 20_i64));
    assert_eq!(
        points(&out),
        vec![(90_i64, 90_i64), (130_i64, 90_i64), (130_i64, 50_i64)],
        "nine kept, three needed: every third of the centre-outward order"
    );
}

#[test]
fn the_grid_drops_points_outside_an_l_shaped_area() {
    let map = l_room();
    let definitions = doom_definitions();
    let out = distribute(&map, &definitions, &spec(Area::Sector(0), 16_usize, 28_i64));
    assert_eq!(
        points(&out),
        vec![
            (128_i64, 72_i64),
            (72_i64, 128_i64),
            (72_i64, 72_i64),
            (184_i64, 72_i64),
            (72_i64, 184_i64)
        ],
        "the five points the L leaves at least 28 clear of every wall, \
         ordered by distance from the centre (128, 128)"
    );
    assert_eq!(
        out.warnings,
        vec!["sector 0 fits only 5 of 16 things".to_owned()],
        "the shortfall is a note, not an error"
    );
}

#[test]
fn existing_bodies_keep_their_clearance_off_the_grid() {
    let mut map = square_rooms(&[(0_i32, 200_i32)]);
    let definitions = doom_definitions();
    map.things.push(Thing {
        x: 120_i32,
        y: 80_i32,
        kind: 3001_i32,
        ..Default::default()
    });
    let out = distribute(&map, &definitions, &spec(Area::Sector(0), 16_usize, 20_i64));
    assert!(
        !points(&out).contains(&(120_i64, 80_i64)),
        "the imp's own spot is gone: 0 < 20 + 20"
    );
    assert!(
        points(&out).contains(&(80_i64, 80_i64)),
        "the 40-unit neighbour is exactly 20 + 20 away and stays"
    );
    assert_eq!(points(&out).len(), 15, "sixteen grid points, one taken");
    assert_eq!(
        out.warnings,
        vec!["sector 0 fits only 15 of 16 things".to_owned()]
    );
}

#[test]
fn an_open_box_must_also_sit_in_some_sector() {
    let map = square_rooms(&[(0_i32, 200_i32)]);
    let definitions = doom_definitions();
    let inside = distribute(
        &map,
        &definitions,
        &spec(
            Area::Rect {
                min_x: 40_i64,
                min_y: 40_i64,
                max_x: 160_i64,
                max_y: 160_i64,
            },
            2_usize,
            20_i64,
        ),
    );
    assert_eq!(
        points(&inside),
        vec![(80_i64, 80_i64), (40_i64, 120_i64)],
        "the box centred in the room keeps its points, spread every eighth"
    );
    let outside = distribute(
        &map,
        &definitions,
        &spec(
            Area::Rect {
                min_x: 1000_i64,
                min_y: 1000_i64,
                max_x: 1200_i64,
                max_y: 1200_i64,
            },
            2_usize,
            20_i64,
        ),
    );
    assert!(
        outside.placed.is_empty(),
        "a box in the void holds no sector point"
    );
    assert_eq!(
        outside.warnings,
        vec!["the bounding box fits only 0 of 2 things".to_owned()]
    );
}

#[test]
fn a_circle_area_keeps_its_radius_and_orders_its_picks() {
    let map = square_rooms(&[(0_i32, 200_i32)]);
    let definitions = doom_definitions();
    let out = distribute(
        &map,
        &definitions,
        &spec(
            Area::Circle {
                centre: (100_i64, 100_i64),
                radius: 50_i64,
            },
            2_usize,
            20_i64,
        ),
    );
    assert_eq!(
        points(&out),
        vec![(100_i64, 100_i64), (60_i64, 100_i64)],
        "five points sit within the radius; every second, from the centre"
    );
    assert!(out.warnings.is_empty(), "five kept for two needed");
}
