//! The append and move topics: reuse, the coordinate ranges the formats
//! allow, the request validation, where a vertex lands and the linedefs a
//! move must not collapse.

use serde_json::Value;

use super::{add_args, code, empty_session, fixture, move_args, name, solid_line};
use crate::tools::edit_vertices::moves::{
    collapse_refusal, made_zero_length, move_in_place, moved_to, new_coord, same_point, stored,
    validate_move,
};
use crate::tools::edit_vertices::{
    COORD_MAX, COORD_MIN, Moves, add_in_place, check_binary_coords, current_map, find_vertex,
    fits_coord, reuses, untouched_fragment,
};
use crate::wad::classify::MapFormat;
use crate::wad::map::{Linedef, Vertex};

#[test]
fn reuse_is_the_default_and_finds_an_exact_point() {
    let plain = add_args(r#"{"x": 8, "y": -8}"#);
    assert!(reuses(&plain), "reuse_existing defaults to true");
    let fresh = add_args(r#"{"x": 8, "y": -8, "reuse_existing": false}"#);
    assert!(!reuses(&fresh), "and can be switched off");
    let map = fixture(MapFormat::Doom);
    assert_eq!(
        find_vertex(&map, 256_i32, 256_i32),
        Some(4_usize),
        "the fixture's back-left corner"
    );
    assert_eq!(
        find_vertex(&map, 0_i32, 0_i32),
        Some(0_usize),
        "the first match wins"
    );
    assert_eq!(
        find_vertex(&map, 256_i32, 255_i32),
        None,
        "one unit off is another point"
    );
}

#[test]
fn an_added_vertex_appends_and_reports_its_row() {
    let mut map = fixture(MapFormat::Doom);
    let args = add_args(r#"{"x": -16, "y": 128}"#);
    let row = add_in_place(&mut map, &args).expect("a plain point appends");
    assert_eq!((row.index, row.x, row.y), (6_usize, -16_i32, 128_i32));
    assert_eq!(map.vertices.len(), 7, "the list grew by one");
    let last = map.vertices.last().expect("the appended vertex");
    assert!(last.extra.is_empty(), "and it carries no format extras");
}

#[test]
fn binary_maps_cap_coordinates_and_udmf_does_not() {
    assert!(
        fits_coord("x", COORD_MIN).is_ok() && fits_coord("y", COORD_MAX).is_ok(),
        "the i16 edges fit"
    );
    let over = COORD_MAX.checked_add(1).expect("the edge plus one");
    let error = fits_coord("x", over).expect_err("above i16 refuses");
    assert_eq!(code(&error), "invalid_argument", "range code");
    assert!(
        error.to_string().contains(
            "x 32768 is out of range for the binary map format: the field holds -32768..=32767"
        ),
        "the message names the field and the range: {error}"
    );
    assert!(
        check_binary_coords(MapFormat::Hexen, 0_i32, COORD_MIN).is_ok(),
        "the Hexen record is i16 too"
    );
    assert!(
        check_binary_coords(MapFormat::Doom, over, 0_i32).is_err(),
        "x above i16 refuses on binary Doom"
    );
    assert!(
        check_binary_coords(MapFormat::Udmf, i32::MAX, i32::MIN).is_ok(),
        "UDMF stores full integers"
    );
    let mut wide = fixture(MapFormat::Udmf);
    let args = add_args(r#"{"x": 40000, "y": -40000}"#);
    let row = add_in_place(&mut wide, &args).expect("UDMF takes the wide point");
    assert_eq!(row.x, 40_000_i32, "and stores it");
    let mut binary = fixture(MapFormat::Doom);
    let refusal = add_in_place(&mut binary, &args).expect_err("the same point refuses");
    assert_eq!(code(&refusal), "invalid_argument", "binary code");
    assert_eq!(binary.vertices.len(), 6, "and nothing appended");
}

#[test]
fn a_reused_point_reports_an_untouched_fragment() {
    let fragment = untouched_fragment(name("MAP01"));
    assert_eq!(fragment["map"], Value::String("MAP01".to_owned()), "named");
    assert_eq!(
        fragment["nodes_invalidated"],
        Value::Bool(false),
        "nothing was invalidated"
    );
    assert_eq!(
        fragment["copied_to_working"],
        Value::Bool(false),
        "nothing was copied"
    );
}

#[test]
fn current_map_falls_back_to_the_merged_view() {
    let session = empty_session();
    let error = current_map(&session, name("MAP01")).expect_err("no map anywhere");
    assert_eq!(code(&error), "not_found", "the merged refusal stands");
    assert!(
        error.to_string().contains("merged"),
        "the message names the scope it checked: {error}"
    );
}

#[test]
fn move_validation_reads_absolute_relative_and_lists() {
    let single = validate_move(&move_args(r#"{"index": 3, "x": 8, "y": 16}"#))
        .expect("absolute on one index");
    assert_eq!(single, vec![3_usize], "the target");
    let list = validate_move(&move_args(r#"{"index": [4, 1, 4], "dx": 8, "dy": -8}"#))
        .expect("a list with deltas");
    assert_eq!(list, vec![1_usize, 4], "sorted and deduplicated");
    let refusals = [
        r#"{"index": 0, "x": 1, "dx": 2}"#,
        r#"{"index": 0, "y": 1, "dy": 2}"#,
        r#"{"index": 0, "x": 1, "dy": 2}"#,
        r#"{"index": [0, 1], "x": 4}"#,
        r#"{"index": [0, 1], "y": 4}"#,
        r#"{"index": 0}"#,
        r#"{"index": []}"#,
    ];
    for json in refusals {
        let error = validate_move(&move_args(json)).expect_err("the request must refuse");
        assert_eq!(code(&error), "invalid_argument", "refusal code for {json}");
    }
    let nothing = validate_move(&move_args(r#"{"index": 0}"#))
        .expect_err("nothing to move")
        .to_string();
    assert!(
        nothing.contains("x and/or y") && nothing.contains("dx and/or dy"),
        "the message spells out both forms: {nothing}"
    );
}

#[test]
fn moved_points_shift_or_replace_and_stay_in_range() {
    let vertex = Vertex {
        x: 100_i32,
        y: -100_i32,
        ..Default::default()
    };
    let absolute = move_args(r#"{"index": 0, "x": -5}"#);
    assert_eq!(
        moved_to(&absolute, MapFormat::Doom, &vertex).expect("x moves, y stands"),
        (-5_i32, -100_i32),
        "an axis left out stays"
    );
    let relative = move_args(r#"{"index": 0, "dx": 8, "dy": 8}"#);
    assert_eq!(
        moved_to(&relative, MapFormat::Doom, &vertex).expect("both axes shift"),
        (108_i32, -92_i32),
        "the block moves"
    );
    let far = move_args(r#"{"index": 0, "dx": 40000}"#);
    let error = moved_to(&far, MapFormat::Doom, &vertex).expect_err("above i16 refuses");
    assert_eq!(code(&error), "invalid_argument", "the binary cap applies");
    assert!(
        moved_to(&far, MapFormat::Udmf, &vertex).is_ok(),
        "UDMF has no cap"
    );
    assert_eq!(
        new_coord("x", COORD_MAX, Some(0_i32), None).expect("absolute wins"),
        0_i32,
        "a replacement ignores the old edge"
    );
    let overflow = new_coord("y", i32::MAX, None, Some(1_i32)).expect_err("above i32 refuses");
    assert_eq!(
        code(&overflow),
        "invalid_argument",
        "an overflowing delta refuses"
    );
    assert!(
        overflow
            .to_string()
            .contains("y 2147483647 plus 1 leaves the range of map coordinates"),
        "the message names the axis: {overflow}"
    );
}

#[test]
fn a_move_rewrites_the_targets_and_reports_them() {
    let mut map = fixture(MapFormat::Doom);
    let args = move_args(r#"{"index": [0, 1], "dy": 32}"#);
    let rows = move_in_place(&mut map, &args, &[0_usize, 1])
        .expect("the shared wall's bottom corners rise");
    assert_eq!(
        rows.iter()
            .map(|row| (row.index, row.x, row.y))
            .collect::<Vec<_>>(),
        vec![(0_usize, 0_i32, 32_i32), (1_usize, 256_i32, 32_i32)],
        "ascending, at the new points"
    );
    assert_eq!(
        (map.vertices[2].x, map.vertices[2].y),
        (512_i32, 0_i32),
        "an unselected vertex stands"
    );
}

#[test]
fn a_move_refuses_a_missing_index_and_changes_nothing() {
    let mut map = fixture(MapFormat::Doom);
    let before = map.clone();
    let error = move_in_place(&mut map, &move_args(r#"{"index": 0, "dx": 8}"#), &[9_usize])
        .expect_err("the map holds six vertices");
    assert_eq!(code(&error), "not_found", "out of range code");
    assert!(
        error
            .to_string()
            .contains("vertex 9 does not exist (the map has 6 vertices, valid indices are 0..=5)"),
        "the message counts the vertices: {error}"
    );
    assert_eq!(map, before, "and the map stands");
}

#[test]
fn a_move_onto_another_endpoint_is_a_conflict_naming_the_line() {
    let mut map = fixture(MapFormat::Doom);
    let args = move_args(r#"{"index": 5, "x": 0, "y": 0}"#);
    let error = move_in_place(&mut map, &args, &[5_usize]).expect_err("line 0 collapses");
    assert_eq!(code(&error), "conflict", "zero-length code");
    assert!(
        error
            .to_string()
            .contains("the move makes linedef(s) 0 zero-length")
            && error.to_string().contains("other endpoint"),
        "the message names the line and the fix: {error}"
    );
    assert_eq!(map, fixture(MapFormat::Doom), "nothing moved");
    let refusal = collapse_refusal(&[3_usize, 6]).to_string();
    assert!(
        refusal.contains("linedef(s) 3, 6"),
        "several lines are listed: {refusal}"
    );
}

#[test]
fn a_group_move_collapsing_two_lines_names_them_both() {
    let mut map = fixture(MapFormat::Doom);
    let args = move_args(r#"{"index": [1, 3], "dx": -256}"#);
    let error = move_in_place(&mut map, &args, &[1_usize, 3]).expect_err("lines 3 and 4 go");
    assert_eq!(code(&error), "conflict", "zero-length code");
    assert!(
        error.to_string().contains("linedef(s) 3, 4"),
        "both collapsed lines are named: {error}"
    );
    assert_eq!(map, fixture(MapFormat::Doom), "nothing moved");
}

#[test]
fn a_move_onto_a_shared_point_without_a_line_spanning_it_is_fine() {
    let mut map = fixture(MapFormat::Doom);
    let args = move_args(r#"{"index": 5, "x": 512, "y": 0}"#);
    let rows = move_in_place(&mut map, &args, &[5_usize]).expect("no single line spans 5 and 2");
    assert_eq!(rows.len(), 1, "the far corner moved");
    assert_eq!(
        (map.vertices[5].x, map.vertices[5].y),
        (512_i32, 0_i32),
        "onto vertex 2's point"
    );
    assert_eq!(
        (map.vertices[2].x, map.vertices[2].y),
        (512_i32, 0_i32),
        "which stands where it was"
    );
}

#[test]
fn zero_length_detection_reads_the_map_not_the_request() {
    let mut map = fixture(MapFormat::Doom);
    map.linedefs.push(Linedef {
        v1: 5,
        v2: 99,
        ..Default::default()
    });
    map.linedefs.push(solid_line(4, 4, 0));
    let moved = Moves::from([(5_usize, (0_i32, 0_i32))]);
    let spanning = map.linedefs.first().expect("line 0 (0,0 to 0,256)");
    assert!(
        made_zero_length(&map, &moved, spanning),
        "line 0 collapses when vertex 5 lands on (0, 0)"
    );
    let dangling = map.linedefs.get(7).expect("the dangling line");
    assert!(
        !made_zero_length(&map, &moved, dangling),
        "a dangling reference is not a collapsed line"
    );
    let self_spanning = map.linedefs.get(8).expect("the self-referencing line");
    assert!(
        !made_zero_length(&map, &moved, self_spanning),
        "a line that is already zero-length stays outside the refusal"
    );
    assert!(
        same_point(Some((1_i32, 2_i32)), Some((1_i32, 2_i32))),
        "equal points match"
    );
    assert!(!same_point(None, None), "two unknowns are not one point");
    assert_eq!(
        stored(&map, 0_usize),
        Some((0_i32, 0_i32)),
        "the stored point reads back"
    );
}
