//! The redundancy sweeps and the plan bookkeeping: the operation names
//! and their safe default set, the concatenated-name table, lumps and
//! whole map blocks identical to the base, empty marker pairs emptied
//! from the inside, region aliases, shadowed globals and the copies that
//! are none of those, the canonical request order and the empty request.

use super::{dropped, flat_map, lump, marker, name, planned, rows};
use crate::named::Named as _;
use crate::wad::archive::Lump;
use crate::wad::optimize::redundant::is_concatenated;
use crate::wad::optimize::{CONCATENATED_NAMES, Operation};

#[test]
fn operation_names_round_trip_and_the_default_set_is_the_safe_four() {
    assert_eq!(Operation::ALL.len(), 6, "six sweeps exist");
    for operation in Operation::ALL.iter().copied() {
        assert_eq!(
            Operation::parse(operation.as_str()),
            Some(operation),
            "exact parse of {}",
            operation.as_str()
        );
        assert_eq!(
            Operation::parse(&operation.as_str().to_ascii_uppercase()),
            Some(operation),
            "any case parses"
        );
    }
    assert_eq!(
        Operation::parse("unused_everything"),
        None,
        "unknown names do not parse"
    );
    assert_eq!(
        Operation::SAFE,
        [
            Operation::IdenticalOverrides,
            Operation::EmptyNamespaces,
            Operation::ShadowedDuplicates,
            Operation::UnusedPnames,
        ],
        "the documented safe set"
    );
    let joined = Operation::SAFE
        .iter()
        .map(|operation| operation.as_str())
        .collect::<Vec<&str>>()
        .join(", ");
    assert_eq!(
        joined, "identical_overrides, empty_namespaces, shadowed_duplicates, unused_pnames",
        "the join reads as documented"
    );
}

#[test]
fn every_concatenated_name_is_a_valid_lump_name_and_recognised() {
    assert_eq!(CONCATENATED_NAMES.len(), 25, "the documented list");
    for text in CONCATENATED_NAMES {
        let parsed = name(text);
        assert!(
            is_concatenated(parsed),
            "{text} must be recognised after name normalisation"
        );
    }
    assert!(
        !is_concatenated(name("PLAYPAL")),
        "plain globals can be pruned"
    );
}

#[test]
fn an_identical_global_override_is_planned_and_a_changed_one_is_not() {
    let base = vec![lump("PLAYPAL", b"same"), lump("COLORMAP", b"base")];
    let working = vec![lump("PLAYPAL", b"same"), lump("COLORMAP", b"edited")];
    let outcome = planned(&working, &base, &Operation::SAFE);
    assert_eq!(
        rows(&outcome),
        vec![(Operation::IdenticalOverrides, "PLAYPAL".to_owned())],
        "only the byte-identical copy goes"
    );
    assert_eq!(
        dropped(&outcome, Operation::IdenticalOverrides),
        vec![0],
        "at its working index"
    );
    assert_eq!(outcome.bytes_saved(), 4, "the dropped payload");
    assert_eq!(outcome.lumps_removed(), 1, "one entry");
}

#[test]
fn namespace_and_map_membership_key_the_identical_override_match() {
    let base = vec![
        marker("F_START"),
        lump("FLOOR_1", b"flat"),
        marker("F_END"),
        lump("PLAYPAL", b"pp"),
    ];
    let working = vec![
        lump("FLOOR_1", b"flat"),
        marker("F_START"),
        lump("PLAYPAL", b"pp"),
        marker("S_END"),
    ];
    let outcome = planned(&working, &base, &Operation::SAFE);
    assert!(
        rows(&outcome).is_empty(),
        "a loose flat never pairs with the region's copy, and a region PLAYPAL never pairs \
             with the base's global one: {:?}",
        rows(&outcome)
    );
}

#[test]
fn an_identical_map_block_drops_whole_and_a_changed_one_stays() {
    let base = flat_map("MAP01", "FLOOR1", "FLOOR1");
    let mut working = flat_map("MAP01", "FLOOR1", "FLOOR1");
    working.extend(flat_map("MAP02", "FLOOR1", "FLOOR1"));
    let outcome = planned(&working, &base, &Operation::SAFE);
    assert_eq!(
        rows(&outcome),
        vec![(Operation::IdenticalOverrides, "MAP01".to_owned())],
        "only the byte-identical block is a unit worth removing"
    );
    let action = outcome.actions.first().expect("one row");
    assert_eq!(action.index, 0, "the block reports at its header");
    let expected_bytes: usize = base.iter().skip(1).map(Lump::size).sum();
    assert_eq!(
        action.bytes, expected_bytes,
        "every member's bytes are freed"
    );
    assert_eq!(
        outcome.lumps_removed(),
        base.len(),
        "the whole block counts toward the removal"
    );
}

#[test]
fn empty_marker_pairs_go_including_the_ones_emptied_from_inside() {
    let working = vec![
        marker("F_START"),
        marker("F_END"),
        marker("S_START"),
        lump("TROOA1", b"sprite"),
        marker("S_END"),
        marker("FF_START"),
        marker("F1_START"),
        marker("F1_END"),
        marker("F_END"),
        marker("F_START"),
        marker("S_END"),
    ];
    let outcome = planned(&working, &[], &Operation::SAFE);
    let mut empty = dropped(&outcome, Operation::EmptyNamespaces);
    empty.sort_unstable();
    assert_eq!(
        empty,
        vec![0, 1, 5, 6, 7, 8],
        "the bare F_START/F_END and the nested FF pair go; the non-empty sprites region \
             and the mismatched F_START/S_END stay"
    );
    assert!(
        outcome.actions.iter().all(|action| action.bytes == 0),
        "markers free no payload"
    );
}

#[test]
fn region_alias_pairs_pair_and_mismatched_ones_do_not() {
    let working = vec![
        marker("SS_START"),
        marker("F_END"),
        marker("A_START"),
        marker("A_END"),
    ];
    let outcome = planned(&working, &[], &[Operation::EmptyNamespaces]);
    assert_eq!(
        dropped(&outcome, Operation::EmptyNamespaces),
        vec![2, 3],
        "SS_START names sprites while F_END names flats, so the first pair is two different \
             regions and never an empty one; A_START/A_END is"
    );
}

#[test]
fn shadowed_duplicates_keep_the_last_copy_and_concatenated_names_whole() {
    let working = vec![
        lump("NOTE", b"one"),
        lump("NOTE", b"two"),
        lump("NOTE", b"three"),
        lump("DECORATE", b"a"),
        lump("DECORATE", b"b"),
    ];
    let outcome = planned(&working, &[], &Operation::SAFE);
    assert_eq!(
        dropped(&outcome, Operation::ShadowedDuplicates),
        vec![0, 1],
        "only the last NOTE survives the sweep"
    );
    assert_eq!(
        outcome.kept_concatenated,
        vec![name("DECORATE")],
        "the concatenated pair stays whole and is reported"
    );
}

#[test]
fn a_single_copy_is_no_shadow_and_region_copies_are_not_globals() {
    let working = vec![
        lump("NOTE", b"only"),
        marker("P_START"),
        lump("ART", b"a"),
        lump("ART", b"b"),
        marker("P_END"),
    ];
    let outcome = planned(&working, &[], &Operation::SAFE);
    assert!(
        rows(&outcome).is_empty(),
        "one copy shadows nothing and duplicate patches are not globals: {:?}",
        rows(&outcome)
    );
}
