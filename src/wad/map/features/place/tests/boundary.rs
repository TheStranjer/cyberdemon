//! Wall selection and projection tests: the shared-boundary listings,
//! the closest-leg choice, the point-to-line projection and the width
//! fit with every refusal it raises.

use crate::wad::map::common::fixtures::{two_room_map, vertex};
use crate::wad::map::features::place::{
    ORIGIN, door_segment, door_wall, longest_boundary, nearest_boundary, project_onto_line,
};
use crate::wad::map::model::Map;

use super::{point, refuses, shared_l_map, shared_line_map, wide_wall_map};

#[test]
fn longest_boundary_lists_the_shared_wall_in_order() {
    let map = two_room_map();
    assert_eq!(
        longest_boundary(&map, 0, 1),
        Some(vec![2_usize]),
        "the two-room fixture shares only the step line",
    );
    assert_eq!(
        longest_boundary(&map, 1, 0),
        Some(vec![2_usize]),
        "sector order does not matter",
    );
    assert_eq!(longest_boundary(&map, 0, 5), None, "no sector 5 exists");
    assert_eq!(
        longest_boundary(&map, 0, 0),
        None,
        "a sector shares no line with itself",
    );
    let l = shared_l_map();
    assert_eq!(
        longest_boundary(&l, 0, 1),
        Some(vec![0_usize, 1]),
        "both legs of the L are the shared boundary",
    );
}

#[test]
fn nearest_boundary_picks_the_closest_leg_and_lowest_index_on_ties() {
    let map = shared_l_map();
    assert_eq!(
        nearest_boundary(&map, 0, 1, point(64, -16)),
        Some(0_usize),
        "the horizontal leg is below",
    );
    assert_eq!(
        nearest_boundary(&map, 0, 1, point(144, 64)),
        Some(1_usize),
        "the vertical leg is right",
    );
    assert_eq!(
        nearest_boundary(&map, 0, 1, point(0, 128)),
        Some(0_usize),
        "128 units to both legs: the lower index wins",
    );
    assert_eq!(
        nearest_boundary(&map, 0, 5, point(0, 0)),
        None,
        "no shared boundary to be near",
    );
    let single = shared_line_map();
    assert_eq!(
        nearest_boundary(&single, 0, 1, point(500, 500)),
        Some(0_usize),
        "the only line is the nearest one",
    );
}

#[test]
fn project_onto_line_places_on_off_and_clamped_points() {
    let map = shared_line_map();
    assert_eq!(
        project_onto_line(&map, 0, Some(point(128, 100))),
        point(128, 100),
        "a point on the line stands",
    );
    assert_eq!(
        project_onto_line(&map, 0, Some(point(64, 100))),
        point(128, 100),
        "an off-line point lands perpendicular",
    );
    assert_eq!(
        project_onto_line(&map, 0, Some(point(10_000, -50))),
        point(128, 0),
        "below v1: clamped to it",
    );
    assert_eq!(
        project_onto_line(&map, 0, Some(point(0, 500))),
        point(128, 256),
        "past v2: clamped to it",
    );
}

#[test]
fn project_onto_line_defaults_to_the_midpoint_and_survives_nothing() {
    let map = shared_line_map();
    assert_eq!(
        project_onto_line(&map, 0, None),
        point(128, 128),
        "the segment midpoint",
    );
    assert_eq!(
        project_onto_line(&map, 9, None),
        ORIGIN,
        "a line that does not resolve has nothing to project onto",
    );
    let flat = Map {
        vertices: vec![vertex(7, 9), vertex(7, 9)],
        ..shared_line_map()
    };
    assert_eq!(
        project_onto_line(&flat, 0, Some(point(100, 100))),
        point(7, 9),
        "a zero-length line projects onto its own vertex",
    );
}

#[test]
fn door_segment_fits_the_exact_line_width() {
    let map = wide_wall_map();
    let ((start, finish), available) =
        door_segment(&map, 0, point(64, 0), 128_i32).expect("the whole wall");
    assert_eq!(start, point(0, 0), "to v1");
    assert_eq!(finish, point(128, 0), "to v2");
    assert_eq!(available, 128_i32, "the integer length");
    let small = door_segment(&map, 0, point(100, 0), 48_i32).expect("an off-centre door");
    assert_eq!(
        small.0,
        (point(76, 0), point(124, 0)),
        "48 units around 100"
    );
}

#[test]
fn door_segment_refuses_widths_past_the_wall_with_its_length() {
    let map = wide_wall_map();
    let error = door_segment(&map, 0, point(64, 0), 129_i32).expect_err("one unit too wide");
    assert_eq!(error.code(), "invalid_argument", "fit refusal");
    assert!(refuses(&error, "128"), "the message carries the length");
    let off_centre =
        door_segment(&map, 0, point(16, 0), 64_i32).expect_err("the chunk runs past v1");
    assert!(refuses(&off_centre, "128"), "and says so with the length");
}

#[test]
fn door_segment_refuses_bad_walls_centres_and_widths() {
    let map = wide_wall_map();
    let missing = door_segment(&map, 9, point(64, 0), 32_i32).expect_err("no linedef 9");
    assert_eq!(missing.code(), "not_found", "a missing line is not_found");
    assert!(
        refuses(&missing, "door linedef 9 does not exist"),
        "the role and the index are named"
    );
    let mut dangling = map.clone();
    dangling.linedefs.get_mut(0).expect("the line").v2 = 9;
    let broken = door_segment(&dangling, 0, point(64, 0), 32_i32).expect_err("vertex 9 gone");
    assert!(refuses(&broken, "vertex"), "the dangling vertex is named");
    let flat = Map {
        vertices: vec![vertex(0, 0), vertex(0, 0)],
        ..wide_wall_map()
    };
    let zero = door_segment(&flat, 0, point(0, 0), 32_i32).expect_err("nothing to span");
    assert!(refuses(&zero, "zero-length"), "the degenerate line");
    let off = door_segment(&map, 0, point(64, 20), 32_i32).expect_err("centre off the wall");
    assert!(refuses(&off, "project_onto_line"), "the fix is named");
    let none = door_segment(&map, 0, point(64, 0), 0_i32).expect_err("no such door");
    assert!(refuses(&none, "positive"), "the width rule");
}

#[test]
fn door_wall_composes_projection_and_fit() {
    let map = wide_wall_map();
    let error = door_wall(&map, 0, Some(point(200, 10)), 64_i32)
        .expect_err("clamped onto v2 there is no room");
    assert!(refuses(&error, "128"), "the fit refusal names the length");
    let centred = door_wall(&map, 0, None, 64_i32).expect("the midpoint fits");
    assert_eq!(centred.linedef, 0_usize, "the chosen line");
    assert_eq!(centred.point, point(64, 0), "the line midpoint");
    assert_eq!(
        centred.half_dir,
        point(32, 0),
        "half the door, along the wall"
    );
    assert_eq!(centred.available_width, 128_i32, "the whole wall");
    let off = door_wall(&map, 0, Some(point(100, 2)), 48_i32).expect("a projected door");
    assert_eq!(off.point, point(100, 0), "projected back onto the line");
    assert_eq!(off.half_dir, point(24, 0), "half of 48");
}
