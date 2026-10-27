//! The room topic: painting a sector over the fixture map and reading what
//! the rebuild leaves behind.
//!
//! [`normalize_ring`] and [`midpoint`] are the two arithmetic helpers the
//! probes run on; the rest draw a floating room in empty space, a nub
//! attached mid-wall, a room over two sectors and a room over a platform,
//! reading the four new one-sided walls, the split wall and its opened
//! middle, the dropped painted-over pieces, the sector `keep_interior`
//! spares and the one a plain paint buries.

use crate::geom::ring::normalize_ring;
use crate::wad::map::common::fixtures::two_room_map;
use crate::wad::map::geometry::point_in_sector;
use crate::wad::map::overlay::apply_overlay;
use crate::wad::map::overlay::geometry::midpoint;

use super::{assert_clean, empty_map, line_between, rect, room_request, side_of};

#[test]
fn ring_normalization_and_midpoints_follow_the_engine() {
    let closed = vec![(0, 0), (10, 0), (10, 10), (0, 0)];
    assert_eq!(normalize_ring(&closed).len(), 3, "the closing repeat drops");
    let doubled = vec![(0, 0), (0, 0), (10, 0), (10, 10)];
    assert_eq!(normalize_ring(&doubled).len(), 3, "adjacent repeats drop");
    assert_eq!(midpoint((0, 0), (10, 4)), (5, 2), "exact halves");
    assert_eq!(
        midpoint((-3, 0), (0, 3)),
        (-2, 2),
        "odd sums round away from zero"
    );
}

#[test]
fn a_floating_room_gets_four_one_sided_walls() {
    let mut map = empty_map();
    let result = apply_overlay(&mut map, &room_request(&rect(0, 0, 256, 256))).expect("draws");
    assert_eq!(result.sector, Some(0), "the appended sector");
    assert_eq!(result.linedefs_created.len(), 4, "four new walls");
    assert!(result.removed_linedefs.is_empty(), "nothing to remove");
    assert_eq!(
        result.vertices_created,
        vec![0_usize, 1, 2, 3],
        "four corners"
    );
    assert_eq!(result.sidedefs_created.len(), 4, "one front side per wall");
    for line in &map.linedefs {
        assert_eq!(line.flags, 1, "every new wall blocks");
        assert!(line.back.is_none(), "every wall is one-sided");
    }
    for side in &map.sidedefs {
        assert_eq!(side.sector, 0, "fronts face the new sector");
        assert_eq!(side.middle, "W1", "the front wears the wall texture");
        assert_eq!(side.upper, "-", "and nothing above or below");
    }
    assert_clean(&map);
}

#[test]
fn a_room_attached_mid_wall_splits_the_wall_and_opens_the_middle() {
    let mut map = two_room_map();
    map.sidedefs.get_mut(1).expect("hall north side").x_offset = 10_i32;
    let result = apply_overlay(&mut map, &room_request(&rect(64, 256, 192, 384)))
        .expect("the nub room draws");
    assert_eq!(result.sector, Some(2), "a third sector");
    assert_clean(&map);
    let low = line_between(&map, (0, 256), (64, 256)).expect("wall piece before the nub");
    let shared = line_between(&map, (64, 256), (192, 256)).expect("shared piece");
    let high = line_between(&map, (192, 256), (256, 256)).expect("wall piece after");
    assert_eq!(map.linedefs.get(low).expect("line").flags, 1, "one-sided");
    assert_eq!(map.linedefs.get(high).expect("line").flags, 1, "one-sided");
    assert_eq!(
        map.linedefs.get(shared).expect("line").flags,
        4,
        "two-sided"
    );
    let hall_side = side_of(&map, shared, true);
    assert_eq!(hall_side.sector, 0, "still faces the hall");
    assert_eq!(hall_side.middle, "-", "the middle moved onto the faces");
    assert_eq!(hall_side.upper, "WALL1");
    assert_eq!(
        hall_side.x_offset, 74_i32,
        "10 plus the 64 units up to the nub"
    );
    let back = side_of(&map, shared, false);
    assert_eq!(back.sector, 2, "the new room");
    assert_eq!(back.middle, "-", "same heights, bare faces");
    assert_eq!(back.upper, "-");
    assert_eq!(back.lower, "-");
    assert!(result.removed_linedefs.is_empty(), "no line vanished");
    assert_eq!(
        result.linedef_map.get(1),
        Some(&Some(low)),
        "the old wall maps to its first piece"
    );
}

#[test]
fn a_room_over_two_sectors_drops_the_painted_over_parts() {
    let mut map = two_room_map();
    let result = apply_overlay(&mut map, &room_request(&rect(128, 64, 384, 320)))
        .expect("the overlap draws");
    assert_eq!(result.sector, Some(2), "a third sector");
    assert_clean(&map);
    assert!(
        line_between(&map, (256, 64), (256, 256)).is_none(),
        "the step line's painted-over middle is gone"
    );
    assert!(
        line_between(&map, (256, 0), (256, 64)).is_some(),
        "and the step line's south part stays"
    );
    assert!(
        line_between(&map, (0, 256), (128, 256)).is_some(),
        "the hall north wall keeps its west part"
    );
    assert!(
        !result.notes.iter().any(|note| note.contains("sector 0")),
        "the hall still has a boundary: {:?}",
        result.notes
    );
    let sides = map.sidedefs.iter().filter(|side| side.sector == 2).count();
    assert!(sides >= 4, "the new room is bounded all around");
}

#[test]
fn keep_interior_leaves_an_inner_room_alone_and_repaints_the_rest() {
    let mut map = two_room_map();
    apply_overlay(&mut map, &room_request(&rect(64, 64, 192, 192))).expect("the platform");
    let before = map.sectors.len();
    let mut request = room_request(&rect(32, 32, 224, 224));
    request.keep_interior = true;
    let result = apply_overlay(&mut map, &request).expect("the bigger room draws");
    assert_clean(&map);
    assert_eq!(result.sector, Some(before), "the new sector sits last");
    assert_eq!(
        result.empty_sectors,
        Vec::<usize>::new(),
        "the platform keeps its area: {:?}",
        result.notes
    );
    let platform = before.saturating_sub(1);
    let walls = map
        .sidedefs
        .iter()
        .filter(|side| side.sector == platform)
        .count();
    assert_eq!(walls, 4, "the platform's four sides stand");
    assert_eq!(
        point_in_sector(&map, 128, 128),
        Some(2),
        "the pillar centre still belongs to the platform"
    );
}

#[test]
fn overwriting_the_interior_empties_the_buried_sector_and_notes_it() {
    let mut map = two_room_map();
    apply_overlay(&mut map, &room_request(&rect(64, 64, 192, 192))).expect("the platform");
    let result = apply_overlay(&mut map, &room_request(&rect(32, 32, 224, 224)))
        .expect("the bigger room buries it");
    assert_clean(&map);
    assert_eq!(result.empty_sectors, vec![2_usize], "the platform sector");
    assert!(
        result.notes.iter().any(|note| note.contains("sector 2")),
        "the note names it: {:?}",
        result.notes
    );
    assert_eq!(map.sectors.len(), 4, "the empty sector stays in the map");
    assert_eq!(point_in_sector(&map, 128, 128), Some(3), "now the big room");
}
