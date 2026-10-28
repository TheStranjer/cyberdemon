//! The maths and expansion topic: how [`Operations::apply_point`] composes
//! scale, rotation and translation, where the default origin lands, and which
//! vertices and things [`expand_selection`] gathers out of a request.

use alloc::collections::BTreeSet;

use super::{grown, point, selection, shifted, transform, turned};
use crate::wad::map::common::fixtures::{spawn, two_room_map};
use crate::wad::map::transform::apply::{default_origin, rotated_angle};
use crate::wad::map::transform::{
    Operations, Point, Rotation, Scale, SelectionRequest, expand_selection,
};
use crate::wad::map::{Map, Sector};

/// The zero origin.
const ZERO: Point = point(0, 0);

#[test]
fn translate_shifts_points_and_ignores_the_origin() {
    let ops = shifted(30, -40);
    assert_eq!(
        ops.apply_point(point(10, 20), ZERO),
        Some(point(40, -20)),
        "plain shift"
    );
    assert_eq!(
        ops.apply_point(point(10, 20), point(7, 9)),
        Some(point(40, -20)),
        "a translation never needs a pivot: the origin cancels exactly"
    );
    assert_eq!(
        shifted(0, 0).apply_point(point(5, 5), point(-3, -3)),
        Some(point(5, 5)),
        "the zero shift keeps the point"
    );
}

#[test]
fn rotate_turns_points_counter_clockwise() {
    assert_eq!(
        turned(90).apply_point(point(100, 0), ZERO),
        Some(point(0, 100)),
        "a quarter turn about the origin"
    );
    assert_eq!(
        turned(45).apply_point(point(100, 0), ZERO),
        Some(point(71, 71)),
        "45 degrees rounds to nearest"
    );
    assert_eq!(
        turned(360).apply_point(point(123, 45), ZERO),
        Some(point(123, 45)),
        "a full turn is the identity"
    );
    assert_eq!(
        turned(90).apply_point(point(110, 100), point(10, 100)),
        Some(point(10, 200)),
        "rotation is about the origin, not the zero point"
    );
}

#[test]
fn scale_multiplies_the_origin_relative_offset() {
    let origin = point(10, 10);
    assert_eq!(
        grown(2, 1).apply_point(point(20, 30), origin),
        Some(point(30, 50)),
        "2/1 doubles the offset"
    );
    assert_eq!(
        grown(1, 2).apply_point(point(20, 30), origin),
        Some(point(15, 20)),
        "1/2 halves it"
    );
    assert_eq!(
        grown(1, 2).apply_point(point(21, 30), origin),
        Some(point(16, 20)),
        "an odd half offset rounds away from zero"
    );
    assert_eq!(
        grown(3, 2).apply_point(point(10, 10), origin),
        Some(point(10, 10)),
        "the origin itself never moves"
    );
}

#[test]
fn operations_apply_scale_then_rotate_then_translate() {
    let ops = Operations {
        scale: Some(Scale::new(2, 1).expect("2/1 is positive")),
        rotation: Some(Rotation::from_degrees(90)),
        translate: Some(point(10, 0)),
    };
    assert_eq!(
        ops.apply_point(point(2, 0), ZERO),
        Some(point(10, 4)),
        "(2,0) scales to (4,0), turns to (0,4), shifts to (10,4)"
    );
    assert!(!ops.is_empty(), "a set of three operations is not empty");
    assert!(Operations::default().is_empty(), "the empty set refuses");
}

#[test]
fn a_thing_angle_wraps_past_a_full_turn() {
    assert_eq!(rotated_angle(350, 20), 10_i32, "350 + 20 wraps to 10");
    assert_eq!(rotated_angle(180, -180), 0_i32, "a half turn back");
    assert_eq!(rotated_angle(0, -1), 359_i32, "one degree south of east");
    let mut map = Map {
        things: vec![spawn(1, 100, 100, 350)],
        ..Default::default()
    };
    let report = transform(&mut map, &selection(&[], &[0]), &turned(20)).expect("the thing turns");
    assert_eq!(map.things[0].angle, 10_i32, "the stored facing wraps");
    assert_eq!(
        (map.things[0].x, map.things[0].y),
        (100_i32, 100_i32),
        "a thing at the default origin keeps its position"
    );
    assert_eq!(report.things_moved, 1, "one thing moved");
}

#[test]
fn the_default_origin_is_the_selection_bbox_centre() {
    let map = two_room_map();
    assert_eq!(
        default_origin(&map, &selection(&[1, 2, 3, 4], &[1])),
        point(384, 128),
        "x 256..512 and y 0..256 centre on (384, 128)"
    );
    let report = transform(&mut map.clone(), &selection(&[2, 3], &[]), &grown(1, 1))
        .expect("a 1/1 scale just reports");
    assert_eq!(report.origin, point(512, 128), "the right wall's centre");
    assert_eq!(
        default_origin(&map, &selection(&[0], &[])),
        point(0, 0),
        "a single point spans nothing and centres on itself"
    );
}

#[test]
fn sector_selection_grabs_its_boundary_and_its_things() {
    let map = two_room_map();
    let back_room = expand_selection(
        &map,
        &SelectionRequest {
            sectors: vec![1],
            include_things_in_sectors: true,
            ..Default::default()
        },
    )
    .expect("sector 1 is there");
    assert_eq!(
        back_room.vertices,
        BTreeSet::from([1_usize, 2, 3, 4]),
        "every corner of the back room, the step line's too"
    );
    assert_eq!(
        back_room.things,
        BTreeSet::from([1_usize]),
        "the imp stands in it"
    );
    let walls_only = expand_selection(
        &map,
        &SelectionRequest {
            sectors: vec![1],
            ..Default::default()
        },
    )
    .expect("the same sector without things");
    assert!(walls_only.things.is_empty(), "the switch reaches off");
    let line = expand_selection(
        &map,
        &SelectionRequest {
            linedefs: vec![2],
            ..Default::default()
        },
    )
    .expect("the step line");
    assert_eq!(
        line.vertices,
        BTreeSet::from([1_usize, 4]),
        "a linedef drags its two endpoints"
    );
    assert!(line.things.is_empty(), "linedefs never drag things");
    let everything = expand_selection(
        &map,
        &SelectionRequest {
            all: true,
            ..Default::default()
        },
    )
    .expect("all");
    assert_eq!(everything.vertices.len(), 6, "every vertex");
    assert_eq!(everything.things.len(), 2, "every thing");
}

#[test]
fn a_thing_exactly_on_the_shared_line_follows_the_front_sector() {
    let mut map = two_room_map();
    map.things.push(spawn(3004, 256, 128, 0));
    let picker = |sectors: &[usize]| {
        expand_selection(
            &map,
            &SelectionRequest {
                sectors: sectors.to_vec(),
                include_things_in_sectors: true,
                ..Default::default()
            },
        )
        .expect("the sector is there")
    };
    let hall = picker(&[0]);
    assert!(
        hall.things.contains(&2_usize),
        "the step line's front is the hall: its indexed lookup hands \
         the line's middle thing to sector 0"
    );
    let raised = picker(&[1]);
    assert!(
        !raised.things.contains(&2_usize),
        "and the back sector does not claim it"
    );
}

#[test]
fn a_sector_the_index_bounds_nothing_for_selects_nothing() {
    let mut map = two_room_map();
    map.sectors.push(Sector::default());
    let bare = expand_selection(
        &map,
        &SelectionRequest {
            sectors: vec![2],
            include_things_in_sectors: true,
            ..Default::default()
        },
    );
    let error = bare.expect_err("an unbounded sector selects nothing to move");
    assert!(
        error.to_string().contains("nothing selected"),
        "the refusal instructs like before: {error}"
    );
}
