//! The embedded table and its parsing: the shipped rows' count and
//! identity, the hand-picked rows read back field by field, the lookups by
//! `Doom` and `ZDoom` number, how the activation words combine, and every
//! header and column the parser refuses.

use super::check_row;
use crate::wad::map::xlat::table::{DOOM_TO_ZDOOM_TSV, parse_xlat_table};
use crate::wad::map::xlat::{XlatFlags, XlatRow, rows_for_doom, rows_for_zspecial, xlat_rows};

/// The header line of a valid translation table.
const HEADER: &str = "doom\tactivation\tzspecial\tname\targs\n";

#[test]
fn the_embedded_table_parses_with_141_rows() {
    let rows = xlat_rows().expect("the embedded table must parse");
    assert_eq!(rows.len(), 141, "the shipped table's row count");
    assert_eq!(
        parse_xlat_table(DOOM_TO_ZDOOM_TSV).expect("parsing the text again works"),
        rows.to_vec(),
        "the static holds the same rows as a fresh parse"
    );
}

#[test]
fn row_one_is_the_dr_door_translation() {
    check_row(&XlatRow {
        doom: 1,
        activation: XlatFlags(2_u32 | 0x10_u32 | 0x20_u32),
        zspecial: 12,
        name: "Door_Raise".to_owned(),
        args: [0_i32, 16_i32, 150_i32, 0_i32, 0_i32],
        arg_count: 4,
        tag_slot: Some(3),
    });
}

#[test]
fn row_12_is_a_lone_tag_argument() {
    check_row(&XlatRow {
        doom: 12,
        activation: XlatFlags::WALK,
        zspecial: 234,
        name: "Light_MaxNeighbor".to_owned(),
        args: [0_i32, 0_i32, 0_i32, 0_i32, 0_i32],
        arg_count: 1,
        tag_slot: Some(0),
    });
}

#[test]
fn row_26_carries_five_arguments_with_the_tag_last() {
    check_row(&XlatRow {
        doom: 26,
        activation: XlatFlags(2_u32 | 0x10_u32),
        zspecial: 13,
        name: "Door_LockedRaise".to_owned(),
        args: [0_i32, 16_i32, 150_i32, 130_i32, 0_i32],
        arg_count: 5,
        tag_slot: Some(4),
    });
}

#[test]
fn row_48_has_no_activation() {
    check_row(&XlatRow {
        doom: 48,
        activation: XlatFlags(0_u32),
        zspecial: 100,
        name: "Scroll_Texture_Left".to_owned(),
        args: [64_i32, 0_i32, 0_i32, 0_i32, 0_i32],
        arg_count: 1,
        tag_slot: None,
    });
    assert!(
        rows_for_doom(48)
            .expect("the embedded table is valid")
            .first()
            .expect("the row exists")
            .activation
            .is_empty(),
        "0 means no activation"
    );
}

#[test]
fn row_97_puts_the_tag_in_the_second_slot() {
    check_row(&XlatRow {
        doom: 97,
        activation: XlatFlags(1_u32 | 0x10_u32 | 0x20_u32),
        zspecial: 70,
        name: "Teleport".to_owned(),
        args: [0_i32, 0_i32, 0_i32, 0_i32, 0_i32],
        arg_count: 2,
        tag_slot: Some(1),
    });
}

#[test]
fn row_100_translates_to_the_crushing_stairs() {
    check_row(&XlatRow {
        doom: 100,
        activation: XlatFlags::WALK,
        zspecial: 273,
        name: "Stairs_BuildUpDoomCrush".to_owned(),
        args: [0_i32, 32_i32, 16_i32, 0_i32, 0_i32],
        arg_count: 5,
        tag_slot: Some(0),
    });
}

#[test]
fn row_125_is_monster_walk_only() {
    check_row(&XlatRow {
        doom: 125,
        activation: XlatFlags::MONWALK,
        zspecial: 70,
        name: "Teleport".to_owned(),
        args: [0_i32, 0_i32, 0_i32, 0_i32, 0_i32],
        arg_count: 2,
        tag_slot: Some(1),
    });
}

#[test]
fn rows_for_doom_of_an_unknown_special_are_empty() {
    assert!(
        rows_for_doom(0)
            .expect("the embedded table is valid")
            .is_empty(),
        "the table has no vanilla special 0"
    );
    let one = rows_for_doom(1).expect("the embedded table is valid");
    assert_eq!(one.len(), 1, "special 1 has exactly one row");
}

#[test]
fn rows_for_zspecial_collects_every_source_row() {
    let rows = rows_for_zspecial(206).expect("the embedded table is valid");
    let doom: Vec<i32> = rows.iter().map(|found| found.doom).collect();
    assert_eq!(
        doom,
        vec![
            10_i32, 21_i32, 62_i32, 88_i32, 120_i32, 121_i32, 122_i32, 123_i32
        ],
        "the eight plat rows translate to Plat_DownWaitUpStayLip"
    );
}

#[test]
fn activation_flags_combine_and_test() {
    let flags = XlatFlags::parse("WALK|MONST|REP").expect("three known words");
    assert!(flags.contains(XlatFlags::WALK), "WALK is set");
    assert!(flags.contains(XlatFlags::REP), "REP is set");
    assert!(!flags.contains(XlatFlags::USE), "USE is not set");
    assert!(!flags.is_empty(), "three bits are not empty");
    assert!(
        XlatFlags::parse("0").expect("0 is none").is_empty(),
        "0 is empty"
    );
    let error = XlatFlags::parse("WALK|FLY").expect_err("FLY is no flag");
    assert!(error.contains("FLY"), "the unknown word is named: {error}");
}

#[test]
fn a_wrong_header_line_is_rejected() {
    let error = parse_xlat_table("doom\tactivation\tzspecial\tname\n")
        .expect_err("a four-column header is invalid");
    assert!(error.contains("header"), "the header is named: {error}");
    let empty = parse_xlat_table("").expect_err("no text is invalid");
    assert!(empty.contains("empty"), "the message says so: {empty}");
}

#[test]
fn a_row_with_four_fields_names_the_line() {
    let text = format!("{HEADER}1\tUSE\t12\tDoor_Raise\n");
    let error = parse_xlat_table(&text).expect_err("a four-field row is invalid");
    assert!(error.contains("line 2"), "the line is named: {error}");
}

#[test]
fn a_non_numeric_doom_names_the_line_and_column() {
    let text = format!("{HEADER}x\tUSE\t12\tDoor_Raise\t0\n");
    let error = parse_xlat_table(&text).expect_err("a non-numeric doom is invalid");
    assert!(error.contains("line 2"), "the line is named: {error}");
    assert!(error.contains("\"doom\""), "the column is named: {error}");
}

#[test]
fn an_unknown_activation_word_names_the_line_and_word() {
    let text = format!("{HEADER}1\tFLY\t12\tDoor_Raise\t0\n");
    let error = parse_xlat_table(&text).expect_err("FLY is no activation");
    assert!(error.contains("line 2"), "the line is named: {error}");
    assert!(error.contains("FLY"), "the word is named: {error}");
}

#[test]
fn two_tag_entries_are_refused() {
    let text = format!("{HEADER}1\tUSE\t13\tDoor_LockedRaise\ttag,16,tag\n");
    let error = parse_xlat_table(&text).expect_err("only one tag is allowed");
    assert!(error.contains("line 2"), "the line is named: {error}");
    assert!(error.contains("tag"), "the entry is named: {error}");
}

#[test]
fn six_args_are_refused() {
    let text = format!("{HEADER}1\tUSE\t13\tDoor_LockedRaise\t1,2,3,4,5,6\n");
    let error = parse_xlat_table(&text).expect_err("five slots is the cap");
    assert!(error.contains("line 2"), "the line is named: {error}");
    assert!(error.contains("args"), "the column is named: {error}");
}

#[test]
fn a_negative_arg_is_refused() {
    let text = format!("{HEADER}1\tUSE\t12\tDoor_Raise\t0,-1,150\n");
    let error = parse_xlat_table(&text).expect_err("args are non-negative");
    assert!(error.contains("line 2"), "the line is named: {error}");
}
