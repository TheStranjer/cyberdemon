//! The engine topic: whole transforms over the fixture map and every refusal
//! that has to leave it untouched, from an out-of-range index and a runaway
//! coordinate to a collapsing scale and a hand-built selection.

use alloc::collections::BTreeMap;

use super::{code, grown, point, selection, shifted, transform, turned, vertex_at};
use crate::wad::classify::MapFormat;
use crate::wad::map::Map;
use crate::wad::map::common::fixtures::two_room_map;
use crate::wad::map::transform::apply::{check_collapse, coincident_pairs, rotated_angle};
use crate::wad::map::transform::{
    Point, Rotation, Scale, SelectionRequest, apply_transform, expand_selection,
};

#[test]
fn bad_indices_and_empty_expansions_are_refused() {
    let map = two_room_map();
    let cases = [
        (
            SelectionRequest {
                vertices: vec![9],
                ..Default::default()
            },
            "vertex 9 does not exist (the map has 6 vertices, valid indices are 0..=5)",
        ),
        (
            SelectionRequest {
                linedefs: vec![7],
                ..Default::default()
            },
            "linedef 7 does not exist (the map has 7 linedefs, valid indices are 0..=6)",
        ),
        (
            SelectionRequest {
                sectors: vec![2],
                ..Default::default()
            },
            "sector 2 does not exist (the map has 2 sectors, valid indices are 0..=1)",
        ),
        (
            SelectionRequest {
                things: vec![2],
                ..Default::default()
            },
            "thing 2 does not exist (the map has 2 things, valid indices are 0..=1)",
        ),
    ];
    for (picked, phrasing) in cases {
        let error = expand_selection(&map, &picked).expect_err("the named index is past the block");
        assert_eq!(code(&error), "not_found", "out of range code");
        assert_eq!(error.to_string(), phrasing, "the documented phrasing");
    }
    let empty =
        expand_selection(&map, &SelectionRequest::default()).expect_err("nothing is selected");
    assert_eq!(code(&empty), "invalid_argument", "empty expansion code");
    let nothing_here = Map::default();
    let bare = expand_selection(
        &nothing_here,
        &SelectionRequest {
            all: true,
            ..Default::default()
        },
    )
    .expect_err("an empty map selects nothing even with all");
    assert_eq!(code(&bare), "invalid_argument", "still empty");
}

#[test]
fn the_engine_moves_a_whole_selection_and_reports_it() {
    let mut map = two_room_map();
    let report = transform(&mut map, &selection(&[1, 2, 3, 4], &[1]), &shifted(0, 64))
        .expect("the back room slides up");
    assert_eq!(report.origin, point(384, 128), "the selection centre");
    assert_eq!(report.vertices_moved, 4, "four corners");
    assert_eq!(report.things_moved, 1, "the imp");
    assert_eq!(report.vertices_selected, vec![1, 2, 3, 4], "ascending");
    assert_eq!(report.things_selected, vec![1], "the moved thing");
    assert!(report.coincident_vertices.is_empty(), "nothing meets");
    assert_eq!(
        vertex_at(&map, 1),
        (256_i32, 64_i32),
        "the shared corner moved"
    );
    assert_eq!(
        vertex_at(&map, 3),
        (512_i32, 320_i32),
        "the top corner moved"
    );
    assert_eq!(vertex_at(&map, 0), (0_i32, 0_i32), "the hall corner stayed");
    assert_eq!(
        (map.things[1].x, map.things[1].y),
        (384_i32, 192_i32),
        "the imp rode along"
    );
    assert_eq!(map.things[0].x, 64_i32, "the player stands still");
}

#[test]
fn a_half_turn_maps_the_fixture_onto_itself() {
    let mut map = two_room_map();
    let all = selection(&[0, 1, 2, 3, 4, 5], &[0, 1]);
    let report = apply_transform(&mut map, &all, &turned(180), Some(point(256, 128)))
        .expect("the whole map turns");
    assert_eq!(report.vertices_moved, 6, "everything moved");
    assert_eq!(
        vertex_at(&map, 0),
        (512_i32, 256_i32),
        "the hall corner is the far one"
    );
    assert_eq!(
        (map.things[0].x, map.things[0].angle),
        (448_i32, 180_i32),
        "the player moved and turned to face the new way"
    );
    let back = apply_transform(&mut map, &all, &turned(-180), Some(point(256, 128)))
        .expect("and back again");
    assert!(back.coincident_vertices.is_empty(), "nothing stacked");
    assert_eq!(map, two_room_map(), "a half turn twice is the identity");
}

#[test]
fn coordinates_the_format_cannot_store_refuse_intact() {
    let before = two_room_map();
    let mut doom = two_room_map();
    let error = transform(&mut doom, &selection(&[0, 2], &[]), &grown(200, 1))
        .expect_err("x -50944 leaves the i16 range");
    assert_eq!(code(&error), "invalid_argument", "out of range code");
    let message = error.to_string();
    assert!(
        message.contains("i16") && message.contains("convert_map_format"),
        "the error names the limit and the fix: {message}"
    );
    assert_eq!(doom, before, "nothing moved");
    let mut udmf = two_room_map();
    udmf.format = MapFormat::Udmf;
    let report = transform(&mut udmf, &selection(&[0, 2], &[]), &grown(200, 1))
        .expect("UDMF stores 32-bit coordinates");
    assert_eq!(
        report.vertices_moved, 2,
        "the bottom wall grew out both ways"
    );
    assert_eq!(
        vertex_at(&udmf, 0),
        (-50_944_i32, 0_i32),
        "far left, on the floor line"
    );
    let mut runaway = two_room_map();
    let huge = transform(
        &mut runaway,
        &selection(&[0, 2], &[]),
        &grown(0x4000_0000, 1),
    )
    .expect_err("past even the model's i32");
    assert_eq!(code(&huge), "invalid_argument", "model overflow code");
    assert_eq!(runaway, before, "still nothing moved");
}

#[test]
fn a_collapsing_scale_is_a_conflict_naming_the_line() {
    let before = two_room_map();
    let mut map = two_room_map();
    let error = transform(
        &mut map,
        &selection(&[0, 1, 2, 3, 4, 5], &[0, 1]),
        &grown(1, 1_000),
    )
    .expect_err("1/1000 stacks the map onto its centre");
    assert_eq!(code(&error), "conflict", "a zero-length line conflicts");
    let message = error.to_string();
    assert!(
        message.contains("linedef") && message.contains("zero-length"),
        "the error names the line: {message}"
    );
    assert_eq!(map, before, "the map stands");
}

#[test]
fn vertices_stacking_on_unmoved_vertices_are_reported() {
    let mut map = two_room_map();
    let report = transform(&mut map, &selection(&[5], &[]), &shifted(256, -256))
        .expect("vertex 5 may land on vertex 1");
    assert_eq!(
        report.coincident_vertices,
        vec![[5_usize, 1_usize]],
        "the pair is [moved, unmoved]"
    );
    assert_eq!(map.vertices.len(), 6, "nothing merged");
    assert_eq!(
        vertex_at(&map, 5),
        (256_i32, 0_i32),
        "both sit at the corner"
    );
}

#[test]
fn a_hand_built_selection_past_the_blocks_is_refused() {
    let mut map = two_room_map();
    let error =
        transform(&mut map, &selection(&[99], &[]), &shifted(1, 0)).expect_err("no vertex 99");
    assert_eq!(
        code(&error),
        "invalid_argument",
        "hand-built selections refuse"
    );
    let empty = transform(&mut map, &selection(&[], &[]), &shifted(1, 0))
        .expect_err("the engine never moves nothing");
    assert_eq!(code(&empty), "invalid_argument", "empty selections refuse");
}

#[test]
fn scale_refuses_non_positive_fractions() {
    let zero = Scale::new(0, 2).expect_err("zero grows nothing");
    assert_eq!(code(&zero), "invalid_argument", "zero numerator code");
    assert!(Scale::new(-1, 2).is_err(), "a negative factor refuses");
    assert!(Scale::new(2, -1).is_err(), "a negative denominator refuses");
    assert_eq!(
        Scale::new(3, 2).expect("3/2 is a factor"),
        Scale { num: 3, den: 2 },
        "the parts widen to i64"
    );
}

#[test]
fn millidegree_rotations_round_thing_facings() {
    let exact = Rotation::from_millidegrees(45_000);
    assert_eq!(exact.degrees, 45, "45.000 degrees");
    assert_eq!(exact.angle, Rotation::from_degrees(45).angle, "same angle");
    let rounded = Rotation::from_millidegrees(45_500);
    assert_eq!(rounded.degrees, 46, "45.5 rounds away from zero");
    assert_eq!(
        rotated_angle(10, rounded.degrees),
        56_i32,
        "facings take the rounded degree"
    );
}

#[test]
fn dangling_endpoints_neither_collapse_nor_hide_a_pair() {
    let mut map = two_room_map();
    map.linedefs
        .last_mut()
        .expect("the fixture has linedefs")
        .v2 = 999;
    let moved: BTreeMap<usize, Point> = BTreeMap::from([(2_usize, point(512, 64))]);
    check_collapse(&map, &moved).expect("a dangling endpoint cannot collide");
    let pairs = coincident_pairs(
        &map,
        &BTreeMap::from([(0_usize, point(512, 0)), (1_usize, point(256, 0))]),
    );
    assert_eq!(
        pairs,
        vec![[0_usize, 2_usize]],
        "vertex 5's corner is the only unmoved meeting point"
    );
}
