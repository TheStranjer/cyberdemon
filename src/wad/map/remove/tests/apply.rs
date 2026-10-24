//! Apply tests: the pruning knobs of [`CascadeOptions`], the texture move
//! one-sided normalisation makes, and the refusals an unexpanded or
//! out-of-range removal answers with.

use alloc::collections::BTreeSet;

use super::{linedef_removal, sector_removal, sidedef_removal, vertex_removal};
use crate::wad::map::common::fixtures::{solid_line, two_room_map};
use crate::wad::map::remove::{CascadeOptions, Removal, apply_removal, expand_cascade};

#[test]
fn cascade_options_keep_orphans_when_pruning_is_off() {
    let defaults = CascadeOptions::default();
    assert!(
        defaults.prune_sidedefs && defaults.prune_vertices,
        "pruning is on by default"
    );
    let fixture = two_room_map();
    let shared_sides = expand_cascade(
        &fixture,
        &linedef_removal(2),
        CascadeOptions {
            prune_sidedefs: false,
            ..defaults
        },
    );
    assert_eq!(
        shared_sides,
        linedef_removal(2),
        "the shared line's sides stay orphaned"
    );
    let unpruned = expand_cascade(
        &fixture,
        &Removal {
            linedefs: BTreeSet::from([5_usize, 6]),
            ..Default::default()
        },
        CascadeOptions {
            prune_vertices: false,
            ..defaults
        },
    );
    assert_eq!(
        unpruned.sidedefs,
        BTreeSet::from([6_usize, 7]),
        "side pruning still runs"
    );
    assert!(unpruned.vertices.is_empty(), "the orphaned vertex stays");
    let mut map = fixture;
    apply_removal(&mut map, &unpruned).expect("the unpruned removal still applies");
    assert_eq!(map.vertices.len(), 6, "vertex 2 survives unreferenced");
    let used: BTreeSet<usize> = map
        .linedefs
        .iter()
        .flat_map(|line| [line.v1, line.v2])
        .collect();
    assert!(!used.contains(&2), "no surviving line uses vertex 2");
}

#[test]
fn cascade_prunes_only_sides_and_vertices_no_survivor_needs() {
    let mut fixture = two_room_map();
    fixture.linedefs.push(solid_line(0, 5, 0));
    let removal = expand_cascade(&fixture, &linedef_removal(0), CascadeOptions::default());
    assert_eq!(
        removal,
        linedef_removal(0),
        "the twin keeps side 0 and both endpoints in use"
    );
}

#[test]
fn one_sided_normalisation_moves_only_an_invisible_texture() {
    let fixture = two_room_map();
    let removal = expand_cascade(&fixture, &sector_removal(1), CascadeOptions::default());
    let mut upper_only = fixture.clone();
    let side = &mut upper_only.sidedefs[2];
    side.upper = "TOP1".to_owned();
    side.lower = "-".to_owned();
    apply_removal(&mut upper_only, &removal).expect("the expanded removal applies");
    assert_eq!(
        (
            upper_only.sidedefs[2].middle.as_str(),
            upper_only.sidedefs[2].upper.as_str()
        ),
        ("TOP1", "-"),
        "a lone upper texture moves to the middle"
    );
    let mut middle_set = fixture;
    middle_set.sidedefs[2].middle = "DOOR1".to_owned();
    apply_removal(&mut middle_set, &removal).expect("the expanded removal applies");
    assert_eq!(
        (
            middle_set.sidedefs[2].middle.as_str(),
            middle_set.sidedefs[2].lower.as_str()
        ),
        ("DOOR1", "STEP1"),
        "a side that already shows a middle keeps its layout"
    );
}

#[test]
fn applying_a_removal_with_live_references_fails_untouched() {
    let fixture = two_room_map();
    let mut map = fixture.clone();
    let vertex_error = apply_removal(&mut map, &vertex_removal(4))
        .expect_err("surviving lines still reference vertex 4");
    assert_eq!(
        vertex_error.code(),
        "internal",
        "the dangling line is caught"
    );
    assert!(
        vertex_error.to_string().contains("vertex 4"),
        "the message names the target: {vertex_error}"
    );
    let sector_error = apply_removal(&mut map, &sector_removal(1))
        .expect_err("surviving sides still reference sector 1");
    assert_eq!(
        sector_error.code(),
        "internal",
        "the dangling side is caught"
    );
    assert_eq!(map, fixture, "the map is untouched after refusals");
}

#[test]
fn applying_a_removal_over_an_out_of_range_reference_fails() {
    let mut map = two_room_map();
    map.linedefs[0].v2 = 99;
    let mut scratch = map.clone();
    let error = apply_removal(&mut scratch, &Removal::default())
        .expect_err("the pre-broken reference is caught");
    assert_eq!(
        error.code(),
        "internal",
        "a missing target is an internal error"
    );
    assert_eq!(scratch, map, "the map is untouched");
}

#[test]
fn removing_an_index_the_map_does_not_have_reports_not_found() {
    let fixture = two_room_map();
    let cases = [
        ("vertex", vertex_removal(6)),
        ("linedef", linedef_removal(7)),
        ("sidedef", sidedef_removal(8)),
        ("sector", sector_removal(2)),
        (
            "thing",
            Removal {
                things: BTreeSet::from([2]),
                ..Default::default()
            },
        ),
    ];
    let mut map = fixture.clone();
    for (kind, removal) in cases {
        let error =
            apply_removal(&mut map, &removal).expect_err("the index is past the end of the block");
        assert_eq!(error.code(), "not_found", "{kind} index is missing");
        assert!(
            error.to_string().contains(kind),
            "the message names {kind}: {error}"
        );
    }
    assert_eq!(map, fixture, "the map is untouched");
}
