//! Cascade and reference-reporting tests: what [`dependents`] lists, how far
//! [`expand_cascade`] grows a seeded removal, and how the expanded sets come
//! out of [`apply_removal`].

use alloc::collections::BTreeSet;

use super::{ends_of, sector_removal, sidedef_removal, vertex_removal};
use crate::wad::map::Linedef;
use crate::wad::map::common::fixtures::two_room_map;
use crate::wad::map::remove::{CascadeOptions, Removal, apply_removal, dependents, expand_cascade};

#[test]
fn removing_a_thing_renumbers_the_things_only() {
    let fixture = two_room_map();
    let mut untouched = fixture.clone();
    let empty =
        apply_removal(&mut untouched, &Removal::default()).expect("an empty removal is a no-op");
    assert!(
        empty.things.is_empty() && untouched == fixture,
        "nothing is reported and nothing moves"
    );
    let mut map = fixture.clone();
    let report = apply_removal(
        &mut map,
        &Removal {
            things: BTreeSet::from([0]),
            ..Default::default()
        },
    )
    .expect("the player start goes");
    assert_eq!(report.things, vec![0], "the removed original index");
    assert!(
        report.vertices.is_empty()
            && report.linedefs.is_empty()
            && report.sidedefs.is_empty()
            && report.sectors.is_empty()
            && report.flipped_linedefs.is_empty()
            && report.one_sided_linedefs.is_empty(),
        "nothing else is reported: {report:?}"
    );
    assert_eq!(map.things.len(), 1, "the imp survives");
    assert_eq!(map.things[0].kind, 3_001_i32, "and renumbered to index 0");
    assert_eq!(map.vertices, fixture.vertices, "vertices untouched");
    assert_eq!(map.linedefs, fixture.linedefs, "linedefs untouched");
    assert_eq!(map.sidedefs, fixture.sidedefs, "sidedefs untouched");
    assert_eq!(map.sectors, fixture.sectors, "sectors untouched");
}

#[test]
fn dependents_lists_every_reference_into_the_removal_set() {
    let map = two_room_map();
    let shared_line = dependents(&map, &vertex_removal(4));
    assert_eq!(
        shared_line.linedefs_using_vertices.get(&4),
        Some(&vec![1_usize, 2, 4]),
        "vertex 4 is used by the three lines meeting there"
    );
    assert!(
        shared_line.linedefs_using_sidedefs.is_empty()
            && shared_line.sidedefs_using_sectors.is_empty(),
        "only the vertex relation is populated"
    );
    assert!(!shared_line.is_clear(), "the removal has dependents");
    assert!(
        dependents(&map, &Removal::default()).is_clear(),
        "an empty removal depends on nothing"
    );
    let sides = dependents(
        &map,
        &Removal {
            sidedefs: BTreeSet::from([3]),
            sectors: BTreeSet::from([1]),
            ..Default::default()
        },
    );
    assert_eq!(
        sides.linedefs_using_sidedefs.get(&3),
        Some(&vec![2_usize]),
        "the shared line carries the removed back side"
    );
    assert_eq!(
        sides.sidedefs_using_sectors.get(&1),
        Some(&vec![3_usize, 5, 6, 7]),
        "four sides frame sector 1"
    );
    assert!(sides.linedefs_using_vertices.is_empty(), "no vertices");
}

#[test]
fn dependents_lists_a_self_referencing_line_once() {
    let mut map = two_room_map();
    map.linedefs.push(Linedef {
        v1: 4,
        v2: 4,
        front: Some(2),
        back: Some(2),
        ..Default::default()
    });
    let found = dependents(
        &map,
        &Removal {
            vertices: BTreeSet::from([4]),
            sidedefs: BTreeSet::from([2]),
            ..Default::default()
        },
    );
    assert_eq!(
        found.linedefs_using_vertices.get(&4),
        Some(&vec![1_usize, 2, 4, 7]),
        "a zero-length line lists its shared endpoint once"
    );
    assert_eq!(
        found.linedefs_using_sidedefs.get(&2),
        Some(&vec![2_usize, 7]),
        "a line with the same sidedef on both sides lists itself once"
    );
}

#[test]
fn cascading_a_vertex_removal_prunes_its_lines_and_their_sides() {
    let fixture = two_room_map();
    let removal = expand_cascade(&fixture, &vertex_removal(2), CascadeOptions::default());
    assert_eq!(
        removal.linedefs,
        BTreeSet::from([5_usize, 6]),
        "both back-room lines on vertex 2 go"
    );
    assert_eq!(
        removal.sidedefs,
        BTreeSet::from([6_usize, 7]),
        "the sides only those lines had are pruned"
    );
    assert_eq!(
        removal.vertices,
        BTreeSet::from([2_usize]),
        "only the seeded vertex goes: the hall's corners stay used"
    );
    assert!(
        removal.things.is_empty() && removal.sectors.is_empty(),
        "the cascade never adds things or sectors"
    );
    let mut map = fixture.clone();
    let report = apply_removal(&mut map, &removal).expect("the expanded removal applies");
    assert_eq!(report.linedefs, vec![5, 6], "sorted originals reported");
    assert_eq!(report.sidedefs, vec![6, 7], "sorted originals reported");
    assert_eq!(
        map.vertices.len(),
        5,
        "vertex 2 dropped, later ones shifted"
    );
    assert_eq!(map.linedefs.len(), 5, "five linedefs survive");
    assert_eq!(map.sidedefs.len(), 6, "six sidedefs survive");
    assert_eq!(
        map.sidedefs[2].sector, 0,
        "the step side still fronts sector 0"
    );
    for (index, line) in map.linedefs.iter().enumerate() {
        assert_eq!(
            ends_of(&map, line),
            ends_of(&fixture, &fixture.linedefs[index]),
            "surviving linedef {index} keeps its coordinates"
        );
    }
}

#[test]
fn cascading_a_sector_removal_closes_the_shared_line() {
    let fixture = two_room_map();
    let removal = expand_cascade(&fixture, &sector_removal(1), CascadeOptions::default());
    assert_eq!(
        removal.sidedefs,
        BTreeSet::from([3_usize, 5, 6, 7]),
        "every side of sector 1 goes"
    );
    assert_eq!(
        removal.linedefs,
        BTreeSet::from([4_usize, 5, 6]),
        "the lines that only those sides held go"
    );
    assert_eq!(
        removal.vertices,
        BTreeSet::from([2_usize, 3]),
        "the back-room corners go orphaned"
    );
    let mut map = fixture;
    let report = apply_removal(&mut map, &removal).expect("the expanded removal applies");
    assert_eq!(report.sectors, vec![1], "sorted originals reported");
    assert_eq!(report.vertices, vec![2, 3], "sorted originals reported");
    assert_eq!(report.one_sided_linedefs, vec![2], "the shared line closed");
    assert!(report.flipped_linedefs.is_empty(), "nothing had to flip");
    let shared = &map.linedefs[2];
    assert_eq!(shared.flags, 1, "two-sided bit cleared, blocking set");
    assert_eq!((shared.v1, shared.v2), (2, 1), "renumbered ends");
    assert_eq!(
        ends_of(&map, shared),
        (256_i32, 256_i32, 256_i32, 0_i32),
        "and same coordinates"
    );
    assert!(shared.back.is_none(), "the sector 1 side is gone");
    let front = &map.sidedefs[shared.front.expect("a front side")];
    assert_eq!(
        (front.middle.as_str(), front.lower.as_str()),
        ("STEP1", "-"),
        "the step texture moved to the middle so the wall shows"
    );
    assert_eq!(map.sidedefs.len(), 4, "only the hall's sides survive");
    assert_eq!(map.sectors.len(), 1, "the hall survives");
    assert_eq!(front.sector, 0, "and the step side still fronts it");
}

#[test]
fn cascading_a_front_side_removal_flips_the_line() {
    let fixture = two_room_map();
    let removal = expand_cascade(&fixture, &sidedef_removal(2), CascadeOptions::default());
    assert_eq!(
        removal,
        sidedef_removal(2),
        "the line keeps its back side, so nothing cascades"
    );
    let mut map = fixture;
    let report = apply_removal(&mut map, &removal).expect("the removal applies");
    assert_eq!(report.flipped_linedefs, vec![2], "the shared line flipped");
    assert_eq!(
        report.one_sided_linedefs,
        vec![2],
        "and it lost its back with the flip"
    );
    assert_eq!(report.sidedefs, vec![2], "the removed original index");
    let shared = &map.linedefs[2];
    assert_eq!((shared.v1, shared.v2), (1, 4), "the ends swapped");
    assert_eq!(shared.front, Some(2), "sector 1's side became the front");
    assert!(shared.back.is_none(), "there is no back any more");
    assert_eq!(shared.flags, 1, "two-sided bit cleared, blocking set");
    let front = &map.sidedefs[2];
    assert_eq!(front.sector, 1, "the raised room's side faces out now");
}

#[test]
fn cascade_drops_a_line_that_lost_both_sides() {
    let fixture = two_room_map();
    let removal = expand_cascade(
        &fixture,
        &Removal {
            sidedefs: BTreeSet::from([2_usize, 3]),
            ..Default::default()
        },
        CascadeOptions::default(),
    );
    assert_eq!(
        removal.linedefs,
        BTreeSet::from([2_usize]),
        "a line with no side left dies with them"
    );
    assert_eq!(
        removal.sidedefs,
        BTreeSet::from([2_usize, 3]),
        "the seed stays"
    );
    assert!(
        removal.vertices.is_empty(),
        "both ends are still used by surviving lines"
    );
}

#[test]
fn applying_a_side_only_removal_leaves_an_unsided_line() {
    let fixture = two_room_map();
    let mut map = fixture;
    let report = apply_removal(
        &mut map,
        &Removal {
            sidedefs: BTreeSet::from([2_usize, 3]),
            ..Default::default()
        },
    )
    .expect("sidedef references drop to None instead of failing");
    assert_eq!(
        report.one_sided_linedefs,
        vec![2],
        "the line lost its back and was normalised"
    );
    assert!(
        report.flipped_linedefs.is_empty(),
        "there was nothing to flip"
    );
    let shared = &map.linedefs[2];
    assert!(
        shared.front.is_none() && shared.back.is_none(),
        "both sides dropped"
    );
    assert_eq!(shared.flags, 1, "one-sided and blocking");
}
