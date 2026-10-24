//! The `add_sector`, `edit_sector` and `remove_sector` topics: the vanilla
//! defaults, the flat, height and range checks, the request validation, the
//! append and edit steps, the planning pass and the removal with its
//! `conflict` refusal, cascade and pruning flags.

use alloc::collections::BTreeSet;

use super::{add_args, code, edit_args, remove_args, room, test_map};
use crate::tools::edit_sectors::edit::{
    add_in_place, cascade_options, dependent_refusal, edit_in_place, edited_sector, has_change,
    join, new_height, new_sector, planned_edits, remove_in_place, validate_edit,
};
use crate::tools::edit_sectors::{
    DEFAULT_CEILING_FLAT, DEFAULT_CEILING_HEIGHT, DEFAULT_FLOOR_FLAT, DEFAULT_LIGHT, FIELD_MAX,
    HEIGHT_MAX, HEIGHT_MIN, LIGHT_MAX, LIGHT_MIN, check_binary_ranges, check_height_order,
    check_light, edited_flat, flat_or_default, validate_flat,
};
use crate::wad::classify::MapFormat;
use crate::wad::map::Sector;

#[test]
fn added_sectors_take_the_vanilla_defaults() {
    let plain = new_sector(&add_args("{}"), MapFormat::Doom).expect("defaults build");
    assert_eq!(
        (plain.floor_height, plain.ceiling_height),
        (0_i32, DEFAULT_CEILING_HEIGHT),
        "the heights default to 0 and 128"
    );
    assert_eq!(plain.floor_flat, DEFAULT_FLOOR_FLAT, "the flat defaults");
    assert_eq!(plain.ceiling_flat, DEFAULT_CEILING_FLAT);
    assert_eq!(plain.light, DEFAULT_LIGHT, "and the light to 160");
    assert_eq!(
        (plain.special, plain.tag),
        (0_i32, 0_i32),
        "special and tag start at 0"
    );
    let named = add_args(
        r#"{"floor_height": -8, "ceiling_height": 200, "floor_flat": "floor1",
            "light": 255, "special": 11, "tag": 3000}"#,
    );
    let custom = new_sector(&named, MapFormat::Doom).expect("a full request builds");
    assert_eq!(custom.floor_height, -8_i32, "the given heights land");
    assert_eq!(custom.ceiling_height, 200_i32);
    assert_eq!(custom.floor_flat, "FLOOR1", "the name is uppercased");
    assert_eq!(custom.light, 255_i32, "the top of the vanilla range fits");
    assert_eq!((custom.special, custom.tag), (11_i32, 3_000_i32));
}

#[test]
fn flat_names_validate_uppercase_and_refuse_the_dash() {
    assert_eq!(
        flat_or_default(None, DEFAULT_FLOOR_FLAT, MapFormat::Doom).expect("absent is the fallback"),
        "FLOOR4_8"
    );
    let dashed = validate_flat("-", MapFormat::Doom).expect_err("a dash is no flat");
    assert_eq!(code(&dashed), "invalid_argument", "dash code");
    assert!(
        dashed.to_string().contains("FLOOR4_8"),
        "the message names a real flat: {dashed}"
    );
    assert_eq!(
        validate_flat("floor4_8", MapFormat::Doom).expect("a vanilla flat"),
        "FLOOR4_8",
        "names are uppercased"
    );
    assert!(
        validate_flat("123456789", MapFormat::Doom).is_err(),
        "nine characters refuse on binary"
    );
    assert!(
        validate_flat("123456789", MapFormat::Udmf).is_ok(),
        "UDMF takes 64"
    );
    assert_eq!(
        edited_flat("FLOOR1", None, MapFormat::Doom).expect("absent keeps the current"),
        "FLOOR1"
    );
    assert_eq!(
        edited_flat("FLOOR1", Some(&"ceil5".to_owned()), MapFormat::Doom).expect("a new flat"),
        "CEIL5",
        "replacement wins"
    );
}

#[test]
fn heights_shift_by_delta_and_absolute_wins() {
    assert_eq!(
        new_height("floor_height", 32_i32, Some(48_i32), None).expect("absolute"),
        48_i32
    );
    assert_eq!(
        new_height("ceiling_height", 128_i32, None, Some(-16_i32)).expect("delta shifts"),
        112_i32
    );
    assert_eq!(
        new_height("floor_height", 32_i32, None, None).expect("neither keeps"),
        32_i32
    );
    let overflow =
        new_height("floor_height", i32::MAX, None, Some(1_i32)).expect_err("above i32 refuses");
    assert_eq!(code(&overflow), "invalid_argument", "overflow code");
    assert!(
        overflow.to_string().contains("floor_height"),
        "the field is named: {overflow}"
    );
}

#[test]
fn ranges_light_and_height_order_are_checked() {
    let edges = room(HEIGHT_MIN, HEIGHT_MAX, LIGHT_MAX, FIELD_MAX);
    assert!(
        check_binary_ranges(MapFormat::Doom, &edges).is_ok(),
        "the i16/u16 edges fit"
    );
    let low = Sector {
        ceiling_height: HEIGHT_MAX.saturating_add(1),
        ..edges.clone()
    };
    let height_error = check_binary_ranges(MapFormat::Hexen, &low).expect_err("above i16 refuses");
    assert_eq!(code(&height_error), "invalid_argument", "height range code");
    let wide_tag = Sector {
        tag: FIELD_MAX.checked_add(1).expect("edge plus one"),
        ..edges
    };
    assert!(
        check_binary_ranges(MapFormat::Doom, &wide_tag).is_err(),
        "a tag above u16 refuses on binary"
    );
    assert!(
        check_binary_ranges(MapFormat::Udmf, &wide_tag).is_ok(),
        "UDMF stores full integers"
    );
    assert!(
        check_light(LIGHT_MIN).is_ok() && check_light(255_i32).is_ok(),
        "0 and 255 fit"
    );
    assert_eq!(
        code(&check_light(300_i32).expect_err("300 refuses")),
        "invalid_argument"
    );
    assert_eq!(
        code(&check_light(-1_i32).expect_err("negatives refuse")),
        "invalid_argument"
    );
    let closed = room(64, 64, 160, 0);
    assert!(
        check_height_order(&closed).is_ok(),
        "equal heights are a closed door"
    );
    let inverted = room(200, 128, 160, 0);
    let order_error = check_height_order(&inverted).expect_err("ceiling below floor refuses");
    assert_eq!(code(&order_error), "invalid_argument", "order code");
    assert!(
        order_error.to_string().contains("ceiling_height 128")
            && order_error.to_string().contains("floor_height 200"),
        "the message shows both heights: {order_error}"
    );
}

#[test]
fn edited_sectors_combine_unchanged_fields_with_the_request() {
    let current = room(32, 128, 192, 1);
    let args = edit_args(r#"{"index": 0, "floor_delta": 16, "light": 255}"#);
    let edited = edited_sector(&current, &args, MapFormat::Doom).expect("the edit computes");
    assert_eq!(edited.floor_height, 48_i32, "the delta raised the floor");
    assert_eq!(edited.ceiling_height, 128_i32, "the unasked height stands");
    assert_eq!(edited.light, 255_i32, "the asked light replaced");
    assert_eq!(edited.floor_flat, "FLOOR1", "and the flats survive");
    assert_eq!(edited.tag, 1_i32, "with the tag");
    let special = edit_args(r#"{"index": 0, "special": 0}"#);
    assert!(
        has_change(&special),
        "an explicit zero still counts as a change"
    );
}

#[test]
fn edit_validation_names_conflicts_and_nothings() {
    let sorted = validate_edit(&edit_args(r#"{"index": [3, 1], "floor_delta": 8}"#))
        .expect("a list with deltas is fine");
    assert_eq!(sorted, vec![1_usize, 3], "sorted and deduplicated");
    let refusals = [
        r#"{"index": 0, "floor_height": 8, "floor_delta": 8}"#,
        r#"{"index": 0, "ceiling_height": 8, "ceiling_delta": 8}"#,
        r#"{"index": 0}"#,
        r#"{"index": []}"#,
    ];
    for json in refusals {
        let error = validate_edit(&edit_args(json)).expect_err("the request must refuse");
        assert_eq!(code(&error), "invalid_argument", "refusal code for {json}");
    }
}

#[test]
fn adding_appends_only_after_every_check() {
    let mut map = test_map();
    let bad_flat = add_args(r#"{"floor_flat": "-"}"#);
    let flat_error = add_in_place(&mut map, &bad_flat).expect_err("the dash refuses");
    assert_eq!(code(&flat_error), "invalid_argument", "flat code");
    let bright = add_args(r#"{"light": 300}"#);
    assert_eq!(
        code(&add_in_place(&mut map, &bright).expect_err("light 300 refuses")),
        "invalid_argument",
        "light code"
    );
    let sunken = add_args(r#"{"floor_height": 200}"#);
    assert_eq!(
        code(&add_in_place(&mut map, &sunken).expect_err("ceiling below floor refuses")),
        "invalid_argument",
        "order code"
    );
    let wide = add_args(r#"{"tag": 70000}"#);
    assert_eq!(
        code(&add_in_place(&mut map, &wide).expect_err("a wide tag refuses on binary")),
        "invalid_argument",
        "range code"
    );
    assert_eq!(map.sectors.len(), 3, "none of the refusals appended");
    map.format = MapFormat::Udmf;
    let row = add_in_place(&mut map, &wide).expect("UDMF stores the wide tag");
    assert_eq!(row.index, 3, "after the existing three");
    assert_eq!(row.detail.tag, 70_000_i32, "and reports it");
    assert_eq!(map.sectors.len(), 4, "the list grew by one");
}

#[test]
fn editing_touches_only_the_selection() {
    let mut map = test_map();
    let past = edit_args(r#"{"index": 9, "light": 200}"#);
    let error = edit_in_place(&mut map, &past, &[9]).expect_err("sector 9 is not there");
    assert_eq!(code(&error), "not_found", "a missing index refuses");
    assert!(
        error.to_string().contains("map has 3 sectors"),
        "the message names the count: {error}"
    );
    let inverted = edit_args(r#"{"index": 0, "floor_height": 200}"#);
    assert_eq!(
        code(&edit_in_place(&mut map, &inverted, &[0]).expect_err("ceiling below floor refuses")),
        "invalid_argument",
        "order code"
    );
    assert_eq!(
        map.sectors[0].floor_height, 0_i32,
        "the refusal changed nothing"
    );
    let rows = edit_in_place(
        &mut map,
        &edit_args(r#"{"index": [0, 2], "light": 96}"#),
        &[0_usize, 2],
    )
    .expect("both sectors darken");
    assert_eq!(rows.len(), 2, "one row per selection");
    assert_eq!(rows[0].index, 0, "the rows come sorted");
    assert_eq!(map.sectors[0].light, 96_i32, "sector 0 changed");
    assert_eq!(map.sectors[2].light, 96_i32, "sector 2 changed");
    assert_eq!(
        map.sectors[1].light, 192_i32,
        "the unselected sector stands"
    );
}

#[test]
fn the_planning_pass_validates_the_whole_selection_first() {
    let map = test_map();
    let args = edit_args(r#"{"index": [0, 1], "floor_height": 200}"#);
    let error = planned_edits(&map, &args, &[0_usize, 1])
        .expect_err("sector 0's ceiling cannot go under its new floor");
    assert_eq!(code(&error), "invalid_argument", "order code");
    let light_refusal = edit_args(r#"{"index": [0, 1], "light": 300}"#);
    assert!(
        planned_edits(&map, &light_refusal, &[0_usize, 1]).is_err(),
        "the light range refuses before any store"
    );
}

#[test]
fn removal_refuses_used_sectors_and_cuts_orphans() {
    let mut map = test_map();
    let used = remove_in_place(&mut map, &remove_args(r#"{"index": 0}"#), &[0]);
    let error = used.expect_err("sidedefs 0/1 use sector 0");
    assert_eq!(code(&error), "conflict", "referenced without cascade");
    assert!(
        error
            .to_string()
            .contains("sector 0 is used by sidedef(s) 0, 1")
            && error.to_string().contains("cascade"),
        "the message names the users and the fix: {error}"
    );
    assert_eq!(map.sectors.len(), 3, "the conflict changed nothing");
    let past = remove_in_place(&mut map, &remove_args(r#"{"index": 9}"#), &[9]);
    assert_eq!(
        code(&past.expect_err("sector 9 is not there")),
        "not_found",
        "a missing index refuses"
    );
    let report = remove_in_place(&mut map, &remove_args(r#"{"index": 2}"#), &[2])
        .expect("the unreferenced sector goes without a cascade");
    assert_eq!(report.sectors, vec![2_usize], "the removed original index");
    assert!(
        report.sidedefs.is_empty() && report.linedefs.is_empty(),
        "nothing else"
    );
    assert_eq!(map.sectors.len(), 2, "and the list shrank");
}

#[test]
fn a_cascaded_removal_closes_the_line_and_prunes_the_room() {
    let mut map = test_map();
    let report = remove_in_place(
        &mut map,
        &remove_args(r#"{"index": 1, "cascade": true}"#),
        &[1],
    )
    .expect("the cascade lifts the conflict");
    assert_eq!(report.sectors, vec![1_usize], "the seeded sector goes");
    assert_eq!(report.sidedefs, vec![2_usize, 3], "both of its sides go");
    assert_eq!(
        report.linedefs,
        vec![2_usize],
        "the wall only they held goes"
    );
    assert_eq!(report.vertices, vec![3_usize], "and its orphaned corner");
    assert_eq!(
        report.one_sided_linedefs,
        vec![1_usize],
        "the shared line closed"
    );
    assert!(report.flipped_linedefs.is_empty(), "nothing had to flip");
    assert_eq!(map.sectors.len(), 2, "the hall and the spare survive");
    assert_eq!(map.sidedefs.len(), 2, "only the hall's sides survive");
    assert_eq!(map.linedefs.len(), 2, "two linedefs survive");
    assert_eq!(map.linedefs[1].back, None, "the shared line is one-sided");
    assert_eq!(map.linedefs[1].flags, 1, "two-sided cleared, blocking set");
}

#[test]
fn the_pruning_flags_reach_the_cascade() {
    let pruning = remove_args(r#"{"index": 1, "cascade": true}"#);
    assert!(
        cascade_options(&pruning).prune_sidedefs && cascade_options(&pruning).prune_vertices,
        "both prunings default on"
    );
    let let_alone = remove_args(
        r#"{"index": 1, "cascade": true, "prune_sidedefs": false, "prune_vertices": false}"#,
    );
    let options = cascade_options(&let_alone);
    assert!(
        !options.prune_sidedefs && !options.prune_vertices,
        "both switch off"
    );
    let mut map = test_map();
    let report = remove_in_place(&mut map, &let_alone, &[1]).expect("the unpruned cascade applies");
    assert_eq!(
        report.linedefs,
        vec![2_usize],
        "the sideless line still dies"
    );
    assert!(report.vertices.is_empty(), "the orphaned vertex stays");
    assert_eq!(map.vertices.len(), 4, "and the list kept its length");
    let used: BTreeSet<usize> = map
        .linedefs
        .iter()
        .flat_map(|line| [line.v1, line.v2])
        .collect();
    assert!(!used.contains(&3), "no surviving line uses vertex 3");
}

#[test]
fn the_conflict_message_joins_every_user() {
    let uses: alloc::collections::BTreeMap<usize, Vec<usize>> =
        [(0_usize, vec![0_usize, 1]), (1_usize, vec![2, 3])]
            .into_iter()
            .collect();
    let message = dependent_refusal(&uses).to_string();
    assert!(
        message.contains("sector 0 is used by sidedef(s) 0, 1")
            && message.contains("sector 1 is used by sidedef(s) 2, 3"),
        "both sectors are listed with their sides: {message}"
    );
    assert!(
        join(&[4_usize, 7]) == "4, 7",
        "the joiner spells the list out"
    );
}
