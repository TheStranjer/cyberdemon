//! The `edit_linedef` topic and the removals: the request validation, the
//! computed line, the edit step with its side edits and refusals, and
//! `remove_linedef` with its cascade.

use super::{code, edit_args, fixture, removal, removal_error, side, solid_line};
use crate::tools::edit_linedefs::edit::{
    EditPlan, edit_in_place, edited_line, has_change, validate_edit,
};
use crate::tools::edit_linedefs::side_edit::side_edited;
use crate::tools::edit_linedefs::{check_binary_offsets, check_line};
use crate::wad::classify::MapFormat;
use crate::wad::map::FlagFamily;

#[test]
fn edit_validation_names_every_conflict_and_nothing() {
    let good =
        validate_edit(&edit_args(r#"{"index": 0, "special": 1}"#)).expect("one field is a change");
    assert_eq!(good, vec![0_usize], "the target");
    let refusals = [
        r#"{"index": 0, "flags": 3, "set_flags": ["blocking"]}"#,
        r#"{"index": 0, "flags": 3, "clear_flags": ["blocking"]}"#,
        r#"{"index": 0, "front": null}"#,
        r#"{"index": 0, "back": null, "back_side": {"middle": "X"}}"#,
        r#"{"index": 0}"#,
        r#"{"index": []}"#,
        r#"{"index": 0, "flip": false}"#,
        r#"{"index": 0, "front_side": {}}"#,
    ];
    for json in refusals {
        let error = validate_edit(&edit_args(json)).expect_err("the request must refuse");
        assert_eq!(code(&error), "invalid_argument", "refusal code for {json}");
    }
    assert!(
        has_change(&edit_args(r#"{"index": 0, "back": null}"#)),
        "back: null is a change"
    );
    assert!(
        has_change(&edit_args(r#"{"index": 0, "flip": true}"#)),
        "flip: true is a change"
    );
}

#[test]
fn a_flip_swaps_ends_and_sides_together() {
    let map = fixture(MapFormat::Doom);
    let args = edit_args(r#"{"index": 2, "flip": true}"#);
    let plan = EditPlan::new(&args, vec![2_usize], &map).expect("a flagless plan");
    let line = map.linedefs.get(2).expect("the step line");
    let flipped = edited_line(line, 2, &plan).expect("the flip computes");
    assert_eq!((flipped.v1, flipped.v2), (1_usize, 4), "the ends swapped");
    assert_eq!(flipped.front, Some(3), "the old back is the front now");
    assert_eq!(flipped.back, Some(2), "the old front trails");
    assert_eq!(flipped.flags, line.flags, "flags stand");
}

#[test]
fn setting_and_dropping_the_back_adjusts_the_two_sided_bits() {
    let map = fixture(MapFormat::Doom);
    let opening = edit_args(r#"{"index": 0, "back": 3}"#);
    let plan = EditPlan::new(&opening, vec![0_usize], &map).expect("a flagless plan");
    let opened = edited_line(map.linedefs.first().expect("line 0"), 0, &plan).expect("opens");
    assert_eq!(opened.flags, 4_u32, "blocking clears, twosided sets");
    assert_eq!(opened.back, Some(3), "and the side lands");
    let closing = edit_args(r#"{"index": 2, "back": null}"#);
    let shut = EditPlan::new(&closing, vec![2_usize], &map).expect("a flagless plan");
    let closed = edited_line(map.linedefs.get(2).expect("the step"), 2, &shut).expect("closes");
    assert_eq!(closed.flags, 1_u32, "twosided clears, blocking sets");
    assert_eq!(closed.back, None, "and the side is gone");
    let held = edit_args(r#"{"index": 2, "back": null, "flags": 256}"#);
    let explicit = EditPlan::new(&held, vec![2_usize], &map).expect("an explicit plan");
    let kept = edited_line(map.linedefs.get(2).expect("the step"), 2, &explicit).expect("holds");
    assert_eq!(kept.flags, 256_u32, "given flags stand untouched");
    assert_eq!(kept.back, None, "and the side still goes");
}

#[test]
fn an_edit_replaces_fields_or_refuses_a_collapsed_line() {
    let map = fixture(MapFormat::Doom);
    let args =
        edit_args(r#"{"index": 0, "flags": ["blocking", "mapped"], "special": 10, "tag": 1}"#);
    let plan = EditPlan::new(&args, vec![0_usize], &map).expect("a plain plan");
    let edited = edited_line(map.linedefs.first().expect("line 0"), 0, &plan).expect("edits");
    assert_eq!(edited.flags, 257_u32, "the names fold to 1 | 256");
    assert_eq!(
        (edited.special, edited.tag),
        (10_i32, 1_i32),
        "the fields land"
    );
    assert_eq!(edited.v1, 0, "unasked references stand");
    let collapsed = edit_args(r#"{"index": 0, "v2": 0}"#);
    let flat = EditPlan::new(&collapsed, vec![0_usize], &map).expect("a plan");
    let error =
        edited_line(map.linedefs.first().expect("line 0"), 0, &flat).expect_err("0 to 0 collapses");
    assert_eq!(code(&error), "invalid_argument", "zero-length code");
    assert!(
        error.to_string().contains("linedef 0 zero-length"),
        "the message names the line: {error}"
    );
}

#[test]
fn editing_touches_only_the_selection_and_applies_side_fields() {
    let mut map = fixture(MapFormat::Doom);
    let args = edit_args(
        r#"{"index": [2, 0], "special": 12, "front_side": {"x_offset": 8, "lower": "step1"}}"#,
    );
    let plan = EditPlan::new(&args, vec![0_usize, 2], &map).expect("a plain plan");
    let rows = edit_in_place(&mut map, &plan).expect("both lines edit");
    assert_eq!(
        rows.iter().map(|row| row.index).collect::<Vec<_>>(),
        vec![0_usize, 2],
        "ascending rows"
    );
    assert_eq!(rows[0].detail.special, 12_i32, "the special landed");
    assert_eq!(rows[0].detail.front, Some(0), "front 0");
    assert_eq!(rows[1].detail.front, Some(2), "front 2");
    assert_eq!(
        map.sidedefs.first().expect("side 0").x_offset,
        8_i32,
        "line 0's front shifted"
    );
    assert_eq!(
        (
            map.sidedefs.get(2).expect("side 2").x_offset,
            map.sidedefs.get(2).expect("side 2").lower.as_str()
        ),
        (8_i32, "STEP1"),
        "and line 2's front dressed"
    );
    assert_eq!(
        map.sidedefs.get(1).expect("side 1").x_offset,
        0_i32,
        "the unselected side stands"
    );
    assert_eq!(
        map.linedefs.get(1).expect("line 1").special,
        0_i32,
        "and the unselected line"
    );
}

#[test]
fn a_side_edit_names_a_missing_side_and_bad_references_are_not_found() {
    let mut map = fixture(MapFormat::Doom);
    let args = edit_args(r#"{"index": 0, "back_side": {"middle": "X"}}"#);
    let plan = EditPlan::new(&args, vec![0_usize], &map).expect("a plain plan");
    let error = edit_in_place(&mut map, &plan).expect_err("line 0 has no back");
    assert_eq!(code(&error), "invalid_argument", "missing side code");
    assert!(
        error.to_string().contains("linedef 0 has no back side"),
        "the message names the line and side: {error}"
    );
    let bad_line = edit_args(r#"{"index": 9, "special": 1}"#);
    let gone = EditPlan::new(&bad_line, vec![9_usize], &map).expect("a plan");
    assert_eq!(
        code(&edit_in_place(&mut map, &gone).expect_err("linedef 9 is not there")),
        "not_found",
        "a missing linedef is not_found"
    );
    let bad_vertex = edit_args(r#"{"index": 0, "v2": 99}"#);
    let vertexless = EditPlan::new(&bad_vertex, vec![0_usize], &map).expect("a plan");
    assert_eq!(
        code(&edit_in_place(&mut map, &vertexless).expect_err("vertex 99 is not there")),
        "not_found",
        "a missing vertex is not_found"
    );
    let bad_side = edit_args(r#"{"index": 0, "back": 99}"#);
    let sidless = EditPlan::new(&bad_side, vec![0_usize], &map).expect("a plan");
    assert_eq!(
        code(&edit_in_place(&mut map, &sidless).expect_err("sidedef 99 is not there")),
        "not_found",
        "a missing sidedef is not_found"
    );
    assert_eq!(map, fixture(MapFormat::Doom), "no refusal changed anything");
}

#[test]
fn side_edits_keep_the_sector_and_offsets_stay_in_range() {
    let current = side("UP1", "MID1", "LOW1", 4);
    let args = edit_args(r#"{"index": 0, "front_side": {"middle": "NEW1"}}"#);
    let edit = args.front_side.as_ref().expect("the parsed edit");
    let edited = side_edited(&current, edit, MapFormat::Doom).expect("the name computes");
    assert_eq!(edited.middle, "NEW1", "the asked field changed");
    assert_eq!(edited.upper, "UP1", "the unasked ones stand");
    assert_eq!(edited.lower, "LOW1", "on both remaining fields");
    assert_eq!(edited.sector, 4, "the sector is never touched");
    assert!(
        check_line(MapFormat::Doom, FlagFamily::Doom, &solid_line(0, 1, 0)).is_ok(),
        "a plain line passes the family and range checks"
    );
    let shifted = edit_args(r#"{"index": 0, "front_side": {"y_offset": 40000}}"#);
    let wide = shifted.front_side.as_ref().expect("the parsed edit");
    let moved = side_edited(&current, wide, MapFormat::Udmf).expect("UDMF-style storage");
    assert_eq!(moved.y_offset, 40_000_i32, "the edit itself builds");
    assert_eq!(
        code(&check_binary_offsets(MapFormat::Hexen, &moved).expect_err("binary i16 cap")),
        "invalid_argument",
        "and the binary range check refuses it"
    );
}

#[test]
fn removal_prunes_orphans_with_the_cascade_by_default() {
    let mut map = fixture(MapFormat::Doom);
    let report = removal(r#"{"index": [5, 6]}"#, &mut map);
    assert_eq!(report.linedefs, vec![5_usize, 6], "the selected lines");
    assert_eq!(
        report.sidedefs,
        vec![6_usize, 7],
        "their sides went orphaned"
    );
    assert_eq!(
        report.vertices,
        vec![2_usize],
        "and so did their shared point"
    );
    assert!(report.sectors.is_empty(), "sectors never go");
    assert_eq!(map.linedefs.len(), 5, "five lines survive");
    assert_eq!(map.vertices.len(), 5, "the vertex count dropped");
    assert_eq!(map.sidedefs.len(), 6, "the orphaned sides dropped");
}

#[test]
fn removal_pruning_can_be_switched_off() {
    let mut map = fixture(MapFormat::Doom);
    let report = removal(
        r#"{"index": [5, 6], "prune_sidedefs": false, "prune_vertices": false}"#,
        &mut map,
    );
    assert_eq!(report.linedefs, vec![5_usize, 6], "the selected lines went");
    assert!(report.sidedefs.is_empty(), "the sides stay orphaned");
    assert!(report.vertices.is_empty(), "the vertex stays too");
    assert_eq!(map.vertices.len(), 6, "nothing was pruned");
    assert_eq!(map.sidedefs.len(), 8, "in either block");
}

#[test]
fn removing_a_missing_index_reports_not_found() {
    let mut map = fixture(MapFormat::Doom);
    let error = removal_error(r#"{"index": 40}"#, &mut map);
    assert_eq!(code(&error), "not_found", "out of range code");
    assert_eq!(map, fixture(MapFormat::Doom), "nothing removed");
}
