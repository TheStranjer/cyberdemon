//! The join topic: welding two linedefs into one.
//!
//! [`join_linedefs`] runs the whole pass: the round trip over a split, the
//! reversed pair whose survivor renumbers down, the head-to-tail,
//! collinearity, direction, sector, third-line, difference and `i16`-offset
//! refusals with the `force` that overrides the differences, and
//! [`find_vertex`], the exact-coordinate test the survivor's renumbering
//! rides on.

use crate::wad::classify::MapFormat;
use crate::wad::map::Sector;
use crate::wad::map::common::fixtures::vertex;
use crate::wad::map::ops::{OFFSET_MIN, find_vertex, join_linedefs, split_linedef};

use crate::wad::map::ops::join::require_matching_sectors;

use super::{code, join_fixture, line, point, side, split_fixture};

#[test]
fn join_of_the_two_halves_of_a_split_restores_the_map_exactly() {
    let original = split_fixture(MapFormat::Doom);
    let mut map = original.clone();
    let split = split_linedef(&mut map, 0, None).expect("the cut lands");
    let joined = join_linedefs(&mut map, 0, split.new_linedef, false).expect("the halves weld");
    assert_eq!(joined.linedef, 0, "the survivor stays first");
    assert_eq!(
        joined.removed.linedefs,
        vec![1_usize],
        "the second half went"
    );
    assert_eq!(
        joined.removed.sidedefs,
        vec![2_usize, 3],
        "its copies went with it"
    );
    assert_eq!(
        joined.removed.vertices,
        vec![2_usize],
        "and so did the cut point"
    );
    assert_eq!(map, original, "endpoints, sides and every offset are back");
}

#[test]
fn join_of_a_reversed_pair_survives_first_and_shifts_its_front() {
    let mut map = join_fixture(MapFormat::Doom);
    let joined = join_linedefs(&mut map, 1, 0, false).expect("the tail survives the weld");
    assert_eq!(joined.linedef, 0, "the survivor renumbered down");
    let survivor = map.linedefs.first().expect("one line left");
    assert_eq!(
        (survivor.v1, survivor.v2),
        (0_usize, 1),
        "it spans 0,0 to 256,0"
    );
    assert_eq!(
        map.sidedefs.len(),
        1,
        "the removed head's orphaned side went"
    );
    assert_eq!(
        map.sidedefs.first().expect("the survivor's front").x_offset,
        40_i32 - 128_i32,
        "its front re-anchors 128 units back"
    );
}

#[test]
fn join_keeps_the_shared_vertex_when_a_third_line_uses_it() {
    let mut map = join_fixture(MapFormat::Doom);
    map.linedefs.push(line(1, 2, Some(0), None));
    let error = join_linedefs(&mut map, 0, 1, false).expect_err("vertex 1 has a third user");
    assert_eq!(code(&error), "conflict", "third line code");
    assert!(
        error.to_string().contains("linedef 2"),
        "the message names the blocker: {error}"
    );
}

#[test]
fn join_refuses_pairs_that_meet_nowhere_backwards_or_off_line() {
    let mut apart = join_fixture(MapFormat::Doom);
    apart.vertices.push(vertex(512, 0));
    apart.linedefs.get_mut(1).expect("the tail").v1 = 3;
    apart.linedefs.get_mut(1).expect("the tail").v2 = 2;
    let nowhere = join_linedefs(&mut apart, 0, 1, false).expect_err("no head-to-tail meet");
    assert_eq!(code(&nowhere), "invalid_argument", "no shared vertex");
    assert!(
        nowhere.to_string().contains("head to tail"),
        "the message names the rule: {nowhere}"
    );

    let mut both_ends = join_fixture(MapFormat::Doom);
    both_ends.linedefs.get_mut(1).expect("the tail").v2 = 0;
    let folded = join_linedefs(&mut both_ends, 0, 1, false).expect_err("both ends shared");
    assert_eq!(code(&folded), "invalid_argument", "both-endpoints refusal");
    assert!(
        folded.to_string().contains("both endpoints"),
        "the message names the rule: {folded}"
    );

    let mut angle = join_fixture(MapFormat::Doom);
    angle.vertices.get_mut(2).expect("the far vertex").y = 128_i32;
    let crooked = join_linedefs(&mut angle, 0, 1, false).expect_err("a corner, not a run");
    assert_eq!(code(&crooked), "invalid_argument", "collinearity refusal");
    assert!(
        crooked.to_string().contains("collinear"),
        "the message names the rule: {crooked}"
    );

    let mut backwards = join_fixture(MapFormat::Doom);
    backwards.vertices = vec![vertex(0, 0), vertex(128, 0), vertex(64, 0)];
    let against = join_linedefs(&mut backwards, 0, 1, false).expect_err("the tail runs back");
    assert_eq!(code(&against), "invalid_argument", "direction refusal");
    assert!(
        against.to_string().contains("opposite"),
        "the message names the rule and the fix: {against}"
    );
}

#[test]
fn join_refuses_sides_that_open_onto_different_sectors() {
    let mut map = join_fixture(MapFormat::Doom);
    map.sidedefs.get_mut(1).expect("the tail's front").sector = 1;
    map.sectors.push(Sector::default());
    let error = join_linedefs(&mut map, 0, 1, false).expect_err("front sectors disagree");
    assert_eq!(code(&error), "invalid_argument", "front sector code");
    assert!(
        error.to_string().contains("front sides"),
        "the message names the side: {error}"
    );
    let mut closed = join_fixture(MapFormat::Doom);
    closed.linedefs.get_mut(1).expect("the tail").back = Some(2);
    closed.sidedefs.push(side(0, "-", 0));
    let back = join_linedefs(&mut closed, 0, 1, false).expect_err("one line is two-sided");
    assert_eq!(code(&back), "invalid_argument", "back side code");
    assert!(
        back.to_string().contains("back sides"),
        "the message names the side: {back}"
    );
}

#[test]
fn join_reads_dangling_sector_references_as_the_broken_indices_they_are() {
    // The ops pass compares sides through `Map::stored_side_sector`,
    // unchecked on purpose: two different out-of-range sector indices
    // still disagree, and the same out-of-range index on both lines
    // still matches. Reading both as nothing instead would let them
    // agree and fail deeper down as the removal engine's `Internal`.
    let mut map = join_fixture(MapFormat::Doom);
    map.sidedefs.get_mut(0).expect("the head's front").sector = 98;
    map.sidedefs.get_mut(1).expect("the tail's front").sector = 99;
    let error = join_linedefs(&mut map, 0, 1, false)
        .expect_err("two dangling fronts are different sectors");
    assert_eq!(code(&error), "invalid_argument", "front sector code");
    assert!(
        error.to_string().contains("front sides"),
        "the message names the side: {error}"
    );
    map.sidedefs.get_mut(1).expect("the tail's front").sector = 98;
    let first = map.linedefs.first().expect("the head line");
    let second = map.linedefs.get(1).expect("the tail line");
    require_matching_sectors(&map, first, second)
        .expect("the same dangling index on both fronts matches");
}

#[test]
fn join_conflicts_naming_every_difference_and_force_lets_the_first_win() {
    let mut map = join_fixture(MapFormat::Doom);
    map.linedefs.get_mut(1).expect("the tail").flags = 4_u32;
    map.linedefs.get_mut(1).expect("the tail").special = 7_i32;
    map.sidedefs.get_mut(1).expect("the tail's front").middle = "BRICK".to_owned();
    let error = join_linedefs(&mut map, 0, 1, false).expect_err("the pair differs");
    assert_eq!(code(&error), "conflict", "differences code");
    let message = error.to_string();
    assert!(
        message.contains("flags")
            && message.contains("special")
            && message.contains("front textures"),
        "the message lists every difference: {message}"
    );
    assert!(
        message.contains("force: true"),
        "and names the fix: {message}"
    );
    let joined = join_linedefs(&mut map, 0, 1, true).expect("force welds them");
    assert_eq!(joined.linedef, 0_usize, "the first linedef survived");
    let survivor = map.linedefs.first().expect("one line");
    assert_eq!(survivor.flags, 1_u32, "its flags won");
    assert_eq!(survivor.special, 0_i32, "as did its special");
    assert_eq!(
        map.sidedefs.first().expect("its front"),
        &side(0, "WALL1", 0),
        "and its textures"
    );
}

#[test]
fn force_join_welds_a_split_pair_losing_only_the_second_tags_value() {
    let mut map = split_fixture(MapFormat::Doom);
    let split = split_linedef(&mut map, 0, None).expect("the cut lands");
    map.linedefs
        .get_mut(split.new_linedef)
        .expect("the half")
        .tag = 9_i32;
    let joined = join_linedefs(&mut map, 0, split.new_linedef, true).expect("force over the tag");
    assert_eq!(
        joined.removed.linedefs,
        vec![split.new_linedef],
        "the half went"
    );
    assert_eq!(
        map,
        split_fixture(MapFormat::Doom),
        "the tag conflict cost the second half's tag"
    );
}

#[test]
fn join_refuses_offsets_a_binary_map_cannot_store() {
    let mut map = join_fixture(MapFormat::Doom);
    map.linedefs.get_mut(0).expect("the head").back = Some(2);
    map.linedefs.get_mut(1).expect("the tail").back = Some(3);
    map.sidedefs.push(side(OFFSET_MIN, "-", 0));
    map.sidedefs.push(side(0, "-", 0));
    let error = join_linedefs(&mut map, 0, 1, false).expect_err("-32768 - 128 is out");
    assert_eq!(code(&error), "invalid_argument", "joined offset code");
    assert!(
        error.to_string().contains("joined side"),
        "the message names the shift: {error}"
    );
}

#[test]
fn join_reports_missing_lines_and_vertices_as_not_found() {
    let mut map = join_fixture(MapFormat::Doom);
    let error = join_linedefs(&mut map, 0, 9, false).expect_err("linedef 9 is not there");
    assert_eq!(code(&error), "not_found", "missing line code");
    map.linedefs.get_mut(1).expect("the tail").v2 = 99;
    let gone = join_linedefs(&mut map, 0, 1, false).expect_err("vertex 99 is not there");
    assert_eq!(code(&gone), "not_found", "missing vertex code");
}

#[test]
fn join_of_one_sided_and_missing_side_pairs_passes_quietly() {
    let mut map = join_fixture(MapFormat::Doom);
    let joined = join_linedefs(&mut map, 0, 1, false).expect("heads: no backs to disagree");
    assert_eq!(joined.linedef, 0, "the head survived unchanged in place");
    assert_eq!(map.linedefs.len(), 1, "one line covers the run");
}

#[test]
fn find_vertex_finds_only_an_exact_coordinate_match() {
    let map = split_fixture(MapFormat::Doom);
    assert_eq!(find_vertex(&map, point(256, 0)), Some(1), "an exact hit");
    assert_eq!(find_vertex(&map, point(256, 1)), None, "one off is a miss");
}
