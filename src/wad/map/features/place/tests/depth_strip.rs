//! Recess drawing tests: the [`outward_depth`] raycast through void,
//! bare walls and same-sector splits, the [`away_side`] choice, the
//! [`door_rect`] rectangle windings and the [`void_strip`] bevelled
//! bands with their refusals.
//!
//! [`away_side`]: crate::wad::map::features::place::depth::away_side

use crate::wad::map::common::fixtures::{side, vertex};
use crate::wad::map::features::place::depth::away_side;
use crate::wad::map::features::place::{
    BoundaryStrip, ORIGIN, door_rect, endpoints, outward_depth, void_strip,
};
use crate::wad::map::model::Map;

use super::{area2, line, point, recess_map, refuses, sample_wall, shared_l_map, wide_wall_map};

#[test]
fn outward_depth_reaches_the_room_behind_the_far_wall() {
    let mut map = recess_map();
    map.sidedefs.push(side("-", "-", "-", 2));
    map.linedefs.push(line(2, 3, Some(2), None));
    let hit = outward_depth(&map, 0, point(128, 128), 64_i32).expect("the far wall");
    assert_eq!(hit.0, 32_i32, "the hall is 32 units deep");
    assert_eq!(hit.1, 2_usize, "the sector beyond the wall");
}

#[test]
fn outward_depth_finds_nothing_over_open_void_or_behind_a_bare_wall() {
    let void = recess_map();
    assert_eq!(
        outward_depth(&void, 0, point(128, 128), 256_i32),
        None,
        "nothing bounds a sector south of the boundary",
    );
    let mut faced_at_ray = recess_map();
    faced_at_ray.sidedefs.push(side("-", "-", "-", 2));
    faced_at_ray.linedefs.push(line(3, 2, Some(2), None));
    assert_eq!(
        outward_depth(&faced_at_ray, 0, point(128, 128), 64_i32),
        None,
        "a wall faced at the ray bounds nothing beyond it",
    );
    let mut back_only = recess_map();
    back_only.sidedefs.push(side("-", "-", "-", 2));
    back_only.linedefs.push(line(2, 3, None, Some(2)));
    assert_eq!(
        outward_depth(&back_only, 0, point(128, 128), 64_i32),
        None,
        "the far side is void there",
    );
}

#[test]
fn outward_depth_skips_lines_that_bound_the_crossed_sector() {
    let mut map = recess_map();
    map.sidedefs.push(side("-", "-", "-", 1));
    map.sidedefs.push(side("-", "-", "-", 1));
    map.linedefs.push(line(2, 3, Some(2), Some(3))); // a split inside the hall
    map.sidedefs.push(side("-", "-", "-", 2));
    map.linedefs.push(line(2, 3, Some(4), None)); // the far wall
    let hit = outward_depth(&map, 0, point(128, 128), 64_i32).expect("the far wall");
    assert_eq!(
        hit,
        (32_i32, 2_usize),
        "the same-sector split line does not bound the recess",
    );
}

#[test]
fn outward_depth_reports_the_nearest_qualifying_wall() {
    let mut map = recess_map();
    map.vertices.push(vertex(0, 112));
    map.vertices.push(vertex(256, 112));
    map.sidedefs.push(side("-", "-", "-", 3));
    map.linedefs.push(line(4, 5, Some(2), None)); // near, sector 3
    map.sidedefs.push(side("-", "-", "-", 2));
    map.linedefs.push(line(2, 3, Some(3), None)); // far, sector 2
    let hit = outward_depth(&map, 0, point(128, 128), 64_i32).expect("a wall");
    assert_eq!(hit.0, 16_i32, "the closer wall wins");
    assert_eq!(hit.1, 3_usize, "and reports its sector");
}

#[test]
fn outward_depth_stops_at_the_reach_and_on_unresolved_lines() {
    let mut map = recess_map();
    map.sidedefs.push(side("-", "-", "-", 2));
    map.linedefs.push(line(2, 3, Some(2), None));
    assert_eq!(
        outward_depth(&map, 0, point(128, 128), 16_i32),
        None,
        "32 units is past the 16-unit reach",
    );
    assert_eq!(
        outward_depth(&map, 0, point(128, 128), 0_i32),
        None,
        "no reach, no answer",
    );
    assert_eq!(
        outward_depth(&map, 9, point(128, 128), 64_i32),
        None,
        "no such boundary line",
    );
    assert_eq!(
        outward_depth(&Map::default(), 0, point(128, 128), 64_i32),
        None,
        "an empty map casts nowhere",
    );
}

#[test]
fn away_side_names_the_front_for_a_left_of_line_origin() {
    let probe = line(0, 1, Some(7), Some(9));
    assert_eq!(away_side(1_i128, &probe), Some(7_usize), "left: front");
    assert_eq!(away_side(-1_i128, &probe), Some(9_usize), "right: back");
    assert_eq!(away_side(0_i128, &probe), None, "grazing: no side");
}

#[test]
fn door_rect_grows_away_from_the_room_and_winds_consistently() {
    let wall = sample_wall();
    let south = door_rect(&wall, point(0, -128), 16_i32, false);
    assert_eq!(
        south,
        vec![point(32, 0), point(32, -16), point(96, -16), point(96, 0)],
        "near-left first, counter-clockwise toward the far side",
    );
    assert!(area2(&south) > 0_i128, "counter-clockwise");
}

#[test]
fn door_rect_flips_across_the_line_when_the_room_is_in_front() {
    let wall = sample_wall();
    let north = door_rect(&wall, point(0, -128), 16_i32, true);
    assert_eq!(
        north,
        vec![point(32, 0), point(96, 0), point(96, 16), point(32, 16)],
        "the same walk grown along the -normal",
    );
    assert!(area2(&north) > 0_i128, "and wound the same way");
}

#[test]
fn door_rect_collapses_onto_the_cut_without_a_normal() {
    let wall = sample_wall();
    let flat = door_rect(&wall, ORIGIN, 16_i32, false);
    assert_eq!(flat.first(), flat.last(), "the rectangle degenerated");
    assert_eq!(area2(&flat), 0_i128, "no area to paint");
}

#[test]
fn void_strip_offsets_a_straight_boundary_to_the_full_thickness() {
    let map = wide_wall_map();
    let strip = void_strip(&map, &[0_usize], 16_i32).expect("one clean line");
    assert_eq!(
        strip,
        BoundaryStrip {
            polygon: vec![point(0, -8), point(128, -8), point(128, 8), point(0, 8)],
        },
        "a 128 by 16 band centred on the wall",
    );
}

#[test]
fn void_strip_bevels_an_l_shared_boundary_into_one_polygon() {
    let map = shared_l_map();
    let strip = void_strip(&map, &[0_usize, 1], 16_i32).expect("a connected L");
    assert_eq!(
        strip.polygon,
        vec![
            point(0, -8),
            point(132, -4),
            point(136, 128),
            point(120, 128),
            point(124, 4),
            point(0, 8),
        ],
        "one bevelled hexagon, not two crossing rectangles",
    );
    assert!(area2(&strip.polygon) > 0_i128, "a real area");
    let flipped = void_strip(&map, &[1_usize, 0], 16_i32).expect("reversed input");
    assert_eq!(
        flipped.polygon, strip.polygon,
        "the chain joins either way round",
    );
}

#[test]
fn void_strip_refuses_empty_thin_and_broken_boundaries() {
    let map = wide_wall_map();
    let empty = void_strip(&map, &[], 16_i32).expect_err("nothing to thicken");
    assert!(refuses(empty, "longest_boundary"), "the fix is named");
    let thin = void_strip(&map, &[0_usize], 0_i32).expect_err("no such thickness");
    assert!(refuses(thin, "positive"), "the rule is named");
    let missing = void_strip(&map, &[9_usize], 16_i32).expect_err("no linedef 9");
    assert!(refuses(missing, "linedef 9"), "the index is named");
    let flat = Map {
        vertices: vec![vertex(0, 0), vertex(0, 0)],
        ..wide_wall_map()
    };
    let degenerate = void_strip(&flat, &[0_usize], 16_i32).expect_err("a point, not a wall");
    assert!(refuses(degenerate, "zero-length"), "the degenerate line");
    let broken = Map {
        vertices: vec![
            vertex(0, 0),
            vertex(128, 0),
            vertex(500, 500),
            vertex(700, 700),
        ],
        linedefs: vec![line(0, 1, Some(0), Some(1)), line(2, 3, Some(0), Some(1))],
        ..wide_wall_map()
    };
    let apart = void_strip(&broken, &[0_usize, 1], 16_i32).expect_err("two runs");
    assert!(refuses(apart, "void_strip call"), "carve each run apart");
}

#[test]
fn endpoints_resolve_only_complete_lines() {
    let map = wide_wall_map();
    assert_eq!(
        endpoints(&map, 0),
        Some((point(0, 0), point(128, 0))),
        "a resolvable line",
    );
    assert_eq!(endpoints(&map, 1), None, "no linedef 1");
}
