//! The split topic: where a cut lands, what it does to the sides and every
//! refusal it reports.
//!
//! [`midpoint`] (rounded through [`crate::fixed::midpoint`]) is the
//! arithmetic, [`split_linedef`] the whole pass: the midpoint and
//! off-centre cuts, the vertex reuse, the shifted copies, the one-sided
//! line and the `udmf` extra bag, against the off-line, endpoint,
//! zero-length, `i16`-range and over-long-piece refusals.

use crate::wad::classify::MapFormat;
use crate::wad::map::Map;
use crate::wad::map::common::fixtures::vertex;
use crate::wad::map::ops::{midpoint, split_linedef};

use super::{code, point, split_fixture};

#[test]
fn midpoint_is_the_rounded_average_of_both_axes() {
    assert_eq!(
        midpoint(point(0, 0), point(256, 256)),
        point(128, 128),
        "even"
    );
    assert_eq!(
        midpoint(point(-256, -1), point(0, 2)),
        point(-128, 1),
        "odd, negative"
    );
}

#[test]
fn split_horizontal_at_midpoint_cuts_the_line_and_shifts_the_sides() {
    let mut map = split_fixture(MapFormat::Doom);
    let result = split_linedef(&mut map, 0, None).expect("the cut lands");
    assert_eq!(result.vertex, 2, "a new vertex at the midpoint");
    assert_eq!(result.new_linedef, 1, "the second half is appended");
    assert_eq!(
        result.new_sidedefs,
        vec![2_usize, 3],
        "front copy, then back copy"
    );
    let first = map.linedefs.first().expect("the survivor");
    assert_eq!((first.v1, first.v2), (0_usize, 2), "v1 keeps the start");
    let second = map.linedefs.get(1).expect("the second half");
    assert_eq!(
        (second.v1, second.v2),
        (2_usize, 1),
        "it runs midpoint to old end"
    );
    assert_eq!(
        (second.flags, second.special, second.tag),
        (0x104_u32, 9_i32, 3_i32),
        "properties copy"
    );
    assert_eq!(second.front, Some(2), "and it fronts with its copy");
    assert_eq!(second.back, Some(3), "and backs with its copy");
    let mid = map.vertices.get(2).expect("the new vertex");
    assert_eq!((mid.x, mid.y), (128_i32, 0_i32), "at the midpoint");
    assert_eq!(
        map.sidedefs.first().expect("side 0").x_offset,
        10_i32,
        "the original front stays"
    );
    assert_eq!(
        map.sidedefs.get(2).expect("front copy").x_offset,
        138_i32,
        "shifted by 128"
    );
    assert_eq!(
        map.sidedefs.get(2).expect("front copy").middle,
        "WALL1",
        "and dressed alike"
    );
    assert_eq!(
        map.sidedefs.get(1).expect("back original").x_offset,
        133_i32,
        "5 + 128"
    );
    assert_eq!(
        map.sidedefs.get(3).expect("back copy").x_offset,
        5_i32,
        "the copy takes the old offset"
    );
    assert_eq!(
        map.sidedefs.get(3).expect("back copy").sector,
        1,
        "over the back's sector"
    );
}

#[test]
fn split_diagonal_at_midpoint_shifts_by_the_floored_hypot() {
    let mut map = split_fixture(MapFormat::Doom);
    map.vertices = vec![vertex(0, 0), vertex(256, 256)];
    let result = split_linedef(&mut map, 0, None).expect("the cut lands");
    let mid = map.vertices.get(result.vertex).expect("the midpoint");
    assert_eq!((mid.x, mid.y), (128_i32, 128_i32), "each axis halved");
    assert_eq!(
        map.sidedefs.get(2).expect("front copy").x_offset,
        10_i32 + 181_i32,
        "hypot(128,128) floors to 181, so the front shifts by 181"
    );
    assert_eq!(
        map.sidedefs.get(1).expect("back original").x_offset,
        5_i32 + 181_i32,
        "and the back shifts by the other 181-unit piece"
    );
    assert_eq!(
        map.sidedefs.get(3).expect("back copy").x_offset,
        5_i32,
        "the copy keeps the old offset at the untouched end"
    );
}

#[test]
fn split_at_a_given_point_uses_it_and_reuses_the_vertex_there() {
    let mut map = split_fixture(MapFormat::Doom);
    map.vertices.push(vertex(64, 0));
    let result = split_linedef(&mut map, 0, Some(point(64, 0))).expect("an off-centre cut");
    assert_eq!(
        result.vertex, 2,
        "the existing vertex at exactly that point"
    );
    assert_eq!(map.vertices.len(), 3, "and nothing new came in");
    assert_eq!(
        map.sidedefs.get(2).expect("front copy").x_offset,
        10_i32 + 64_i32,
        "shifted by the first piece only"
    );
    assert_eq!(
        map.sidedefs.get(1).expect("back original").x_offset,
        5_i32 + 192_i32,
        "the back shifts by the second piece"
    );
}

#[test]
fn split_tolerates_a_point_exactly_one_unit_off_the_line() {
    let mut map = split_fixture(MapFormat::Doom);
    let result = split_linedef(&mut map, 0, Some(point(128, 1))).expect("cross^2 == length^2");
    let mid = map.vertices.get(result.vertex).expect("the split vertex");
    assert_eq!(
        (mid.x, mid.y),
        (128_i32, 1_i32),
        "the point stands as given"
    );
}

#[test]
fn split_refuses_points_off_beyond_and_on_the_endpoints() {
    let refusals = [
        point(128, 5),
        point(0, 0),
        point(256, 0),
        point(300, 0),
        point(-4, 0),
    ];
    for given in refusals {
        let mut map = split_fixture(MapFormat::Doom);
        let error = split_linedef(&mut map, 0, Some(given)).expect_err("must refuse");
        assert_eq!(code(&error), "invalid_argument", "refusal for {given:?}");
        assert_eq!(
            map,
            split_fixture(MapFormat::Doom),
            "the refusal changed nothing"
        );
    }
}

#[test]
fn split_refuses_a_zero_length_line_and_its_rounded_onto_endpoint() {
    let mut flat = split_fixture(MapFormat::Doom);
    flat.vertices = vec![vertex(0, 0), vertex(0, 0)];
    let error = split_linedef(&mut flat, 0, None).expect_err("nothing to cut");
    assert_eq!(code(&error), "invalid_argument", "zero-length code");
    assert!(
        error.to_string().contains("zero-length"),
        "the message names the rule: {error}"
    );
    let mut tiny = split_fixture(MapFormat::Doom);
    tiny.vertices = vec![vertex(0, 0), vertex(1, 0)];
    let short = split_linedef(&mut tiny, 0, None).expect_err("the midpoint rounds onto v2");
    assert_eq!(code(&short), "invalid_argument", "one-unit line code");
}

#[test]
fn split_refuses_offsets_a_binary_map_cannot_store() {
    let mut map = split_fixture(MapFormat::Doom);
    map.sidedefs.get_mut(0).expect("the front").x_offset = 32_700_i32;
    let error = split_linedef(&mut map, 0, None).expect_err("32828 is over i16");
    assert_eq!(code(&error), "invalid_argument", "front offset code");
    assert!(
        error.to_string().contains("front"),
        "the message names the side: {error}"
    );
    let mut back_map = split_fixture(MapFormat::Doom);
    back_map.sidedefs.get_mut(0).expect("the front").x_offset = 0_i32;
    back_map.sidedefs.get_mut(1).expect("the back").x_offset = 32_700_i32;
    let back_error = split_linedef(&mut back_map, 0, None).expect_err("the back shifts too far");
    assert_eq!(code(&back_error), "invalid_argument", "back offset code");
    assert!(
        back_error.to_string().contains("back"),
        "the message names the side: {back_error}"
    );
    assert_eq!(back_map, split_fixture_back_only(), "nothing shifted");
}

/// The split fixture with the front unshifted but the back at 32700.
fn split_fixture_back_only() -> Map {
    let mut map = split_fixture(MapFormat::Doom);
    map.sidedefs.get_mut(0).expect("the front").x_offset = 0_i32;
    map.sidedefs.get_mut(1).expect("the back").x_offset = 32_700_i32;
    map
}

#[test]
fn split_wraps_offsets_past_i32_on_udmf_without_range_checks() {
    let mut map = split_fixture(MapFormat::Udmf);
    map.sidedefs.get_mut(0).expect("the front").x_offset = 0x7FFF_FFF8_i32;
    let result = split_linedef(&mut map, 0, None).expect("UDMF does not range-check offsets");
    let copy = map
        .sidedefs
        .get(result.new_sidedefs.first().copied().expect("a front copy"))
        .expect("the copy");
    assert_eq!(
        copy.x_offset,
        0x7FFF_FFF8_i32.wrapping_add(128_i32),
        "the offset wraps as the ticket prescribes"
    );
}

#[test]
fn split_refuses_a_point_a_binary_map_cannot_store() {
    let mut map = split_fixture(MapFormat::Doom);
    map.vertices = vec![vertex(40_000, 0), vertex(60_000, 0)];
    let error = split_linedef(&mut map, 0, None).expect_err("the midpoint is over i16");
    assert_eq!(code(&error), "invalid_argument", "coordinate code");
    assert!(
        error.to_string().contains("vertices hold"),
        "the message names the range: {error}"
    );
}

#[test]
fn split_refuses_pieces_too_long_to_shift_an_offset() {
    let mut map = split_fixture(MapFormat::Udmf);
    map.vertices = vec![vertex(i32::MIN, 0), vertex(i32::MAX, 0)];
    let error = split_linedef(&mut map, 0, Some(point(i32::MIN + 2, 0)))
        .expect_err("the second piece is past i32::MAX");
    assert_eq!(code(&error), "invalid_argument", "length code");
    assert!(
        error.to_string().contains("past what an x_offset"),
        "the message names the rule: {error}"
    );
}

#[test]
fn split_one_sided_lines_copy_only_the_front() {
    let mut map = split_fixture(MapFormat::Doom);
    map.linedefs.get_mut(0).expect("the line").back = None;
    map.sidedefs.pop();
    let result = split_linedef(&mut map, 0, None).expect("the cut lands");
    assert_eq!(result.new_sidedefs, vec![1_usize], "one copy for one side");
    let second = map.linedefs.get(1).expect("the second half");
    assert_eq!(second.front, Some(1), "it keeps a front");
    assert_eq!(second.back, None, "and no back");
}

#[test]
fn split_reports_missing_lines_vertices_and_side_references() {
    let mut map = split_fixture(MapFormat::Doom);
    let gone = split_linedef(&mut map, 9, None).expect_err("linedef 9 is not there");
    assert_eq!(code(&gone), "not_found", "missing line code");
    map.linedefs.get_mut(0).expect("the line").v2 = 99;
    let dangling = split_linedef(&mut map, 0, None).expect_err("vertex 99 is not there");
    assert_eq!(code(&dangling), "not_found", "missing vertex code");
    let mut sided = split_fixture(MapFormat::Doom);
    sided.linedefs.get_mut(0).expect("the line").back = Some(9);
    let sideless = split_linedef(&mut sided, 0, None).expect_err("sidedef 9 is not there");
    assert_eq!(code(&sideless), "not_found", "missing sidedef code");
    assert!(
        sideless.to_string().contains("back"),
        "the message names the side: {sideless}"
    );
}

#[test]
fn split_copies_the_udmf_extra_bag_onto_the_new_line() {
    let mut map = split_fixture(MapFormat::Udmf);
    map.linedefs
        .get_mut(0)
        .expect("the line")
        .extra
        .insert("autoflow".to_owned(), "1".to_owned());
    let result = split_linedef(&mut map, 0, None).expect("the cut lands");
    let second = map.linedefs.get(result.new_linedef).expect("the half");
    assert_eq!(
        second.extra.get("autoflow").map(String::as_str),
        Some("1"),
        "the extra bag rides along"
    );
}
