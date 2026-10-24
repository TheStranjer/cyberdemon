//! The carving topic: solid rock over the fixture map, and the rows a
//! paint leaves behind.
//!
//! The three carvings read the one-sided walls a pillar and a trench leave
//! and the texture they inherit from the room they cut; the rest measure
//! the preserved offsets of the walls a room splits, the vertices it
//! reuses, the two non-default outside rules on a room's new edges, the
//! refusals that must leave the map untouched and the void over an empty
//! map that builds no rows at all.

use crate::wad::map::common::fixtures::two_room_map;
use crate::wad::map::geometry::point_in_sector;
use crate::wad::map::overlay::{Outside, apply_overlay};

use super::{
    assert_clean, code, empty_map, line_between, rect, room_request, side_of, void_request,
};

#[test]
fn carving_a_pillar_gives_the_room_one_sided_walls_in_its_own_texture() {
    let mut map = two_room_map();
    let result =
        apply_overlay(&mut map, &void_request(&rect(96, 96, 160, 160))).expect("the pillar carves");
    assert_eq!(result.sector, None, "void paint appends no sector");
    assert_clean(&map);
    assert_eq!(result.linedefs_created.len(), 4, "four new walls");
    for index in &result.linedefs_created {
        let line = map.linedefs.get(*index).expect("a new wall");
        assert_eq!(line.flags, 1, "blocking, one-sided");
        assert!(line.back.is_none(), "no back side");
        let side = side_of(&map, *index, true);
        assert_eq!(side.sector, 0, "faces the hall");
        assert_eq!(side.middle, "WALL1", "the hall's own wall texture");
    }
    assert!(
        point_in_sector(&map, 128, 128).is_none(),
        "the pillar centre is solid now"
    );
    assert!(
        result.removed_linedefs.is_empty(),
        "nothing was painted away"
    );
}

#[test]
fn carving_with_only_dash_walls_falls_back_to_the_request_texture() {
    let mut map = two_room_map();
    for row in &mut map.sidedefs {
        row.middle = "-".to_owned();
    }
    let result =
        apply_overlay(&mut map, &void_request(&rect(96, 96, 160, 160))).expect("the pillar carves");
    let index = *result.linedefs_created.first().expect("four walls");
    let side = side_of(&map, index, true);
    assert_eq!(side.middle, "W1", "the request's wall texture stands in");
}

#[test]
fn carving_across_two_rooms_separates_them_with_one_sided_walls() {
    let mut map = two_room_map();
    let result = apply_overlay(&mut map, &void_request(&rect(192, 64, 320, 192)))
        .expect("the trench carves");
    assert_clean(&map);
    assert!(
        line_between(&map, (256, 64), (256, 192)).is_none(),
        "the shared step line's carved middle is gone"
    );
    assert!(
        result.linedefs_created.iter().all(|index| {
            map.linedefs
                .get(*index)
                .is_some_and(|line| line.flags == 1 && line.back.is_none())
        }),
        "every new wall is one-sided"
    );
    let west = line_between(&map, (192, 64), (192, 192)).expect("the hall-facing wall");
    let side = side_of(&map, west, true);
    assert_eq!(side.sector, 0, "the hall walls its new edge");
    assert_eq!(side.middle, "WALL1", "with its own texture");
    assert!(result.empty_sectors.is_empty(), "both rooms survive");
}

#[test]
fn split_walls_keep_their_texture_offsets() {
    let mut map = two_room_map();
    map.sidedefs.get_mut(1).expect("hall north side").x_offset = 32_i32;
    apply_overlay(&mut map, &room_request(&rect(64, 256, 192, 384))).expect("the nub");
    let high = line_between(&map, (192, 256), (256, 256)).expect("east wall piece");
    let side = side_of(&map, high, true);
    assert_eq!(side.x_offset, 224_i32, "32 plus the 192 units before it");
    let shared = line_between(&map, (64, 256), (192, 256)).expect("shared piece");
    assert_eq!(side_of(&map, shared, true).x_offset, 96_i32, "32 plus 64");
    let low = line_between(&map, (0, 256), (64, 256)).expect("west wall piece");
    assert_eq!(
        side_of(&map, low, true).x_offset,
        32_i32,
        "the anchor stands"
    );
}

#[test]
fn reused_vertices_keep_their_indices_and_records() {
    let mut map = two_room_map();
    let result = apply_overlay(&mut map, &room_request(&rect(0, 256, 256, 512)))
        .expect("the room above the hall draws");
    assert_eq!(result.vertices_created, vec![6_usize, 7], "two new corners");
    assert_eq!(map.vertices.len(), 8, "six reused, two added");
    let shared = line_between(&map, (0, 256), (256, 256)).expect("the shared wall");
    let line = map.linedefs.get(shared).expect("line");
    assert_eq!(
        (line.v1, line.v2),
        (5_usize, 4_usize),
        "the reused vertices"
    );
    assert_clean(&map);
}

#[test]
fn a_solid_outside_keeps_new_edges_one_sided_but_the_shared_wall_open() {
    let mut map = two_room_map();
    let mut request = room_request(&rect(64, 256, 192, 384));
    request.outside = Outside::Solid;
    apply_overlay(&mut map, &request).expect("the nub draws");
    assert_clean(&map);
    let shared = line_between(&map, (64, 256), (192, 256)).expect("shared piece");
    assert_eq!(
        map.linedefs.get(shared).expect("line").flags,
        4,
        "the existing wall still opens"
    );
    let west = line_between(&map, (64, 256), (64, 384)).expect("a new edge");
    let line = map.linedefs.get(west).expect("new line");
    assert_eq!(line.flags, 1, "the new edge stays a single line");
    assert!(line.back.is_none(), "no side into the void");
}

#[test]
fn a_faced_outside_turns_new_edges_two_sided_against_the_named_sector() {
    let mut map = two_room_map();
    let mut request = room_request(&rect(1024, 0, 1280, 256));
    request.outside = Outside::Face(0);
    apply_overlay(&mut map, &request).expect("the far room draws against the hall");
    assert_clean(&map);
    let west = line_between(&map, (1024, 0), (1024, 256)).expect("a new edge");
    let line = map.linedefs.get(west).expect("new line");
    assert_eq!(line.flags, 4, "two-sided against the named sector");
    assert_eq!(side_of(&map, west, false).sector, 0, "the hall is faced");
    assert_eq!(side_of(&map, west, true).sector, 2, "and the new room");
}

#[test]
fn invalid_polygons_and_unknown_outsiders_are_refused_untouched() {
    let mut map = two_room_map();
    let before = map.clone();
    let bow = vec![(0_i64, 0_i64), (256, 256), (256, 0), (0, 256)];
    let error = apply_overlay(&mut map, &room_request(&bow)).expect_err("crosses itself");
    assert_eq!(code(&error), "invalid_argument", "self-crossing code");
    let flat = rect(0, 0, 128, 0);
    assert_eq!(
        code(&apply_overlay(&mut map, &room_request(&flat)).expect_err("zero area refuses")),
        "invalid_argument",
        "flat code"
    );
    let mut request = room_request(&rect(0, 0, 64, 64));
    request.outside = Outside::Face(99);
    let missing = apply_overlay(&mut map, &request).expect_err("sector 99 is not there");
    assert_eq!(code(&missing), "not_found", "unknown face code");
    assert!(
        missing.to_string().contains("map has 2 sectors"),
        "the message counts the sectors: {missing}"
    );
    assert_eq!(map, before, "the refusals painted nothing");
}

#[test]
fn a_void_paint_over_nothing_changes_nothing_but_the_rows_rebuild() {
    let mut map = empty_map();
    let result = apply_overlay(&mut map, &void_request(&rect(0, 0, 128, 128))).expect("carves");
    assert_eq!(result.sector, None, "no sector");
    assert!(map.linedefs.is_empty(), "void over void leaves no walls");
    assert!(map.vertices.is_empty(), "no piece, no vertex");
}
