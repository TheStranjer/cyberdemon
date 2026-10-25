//! The linedef stages' round trips: a translated row restoring its own
//! special and tag from either source shape, the rows that split on their
//! activation bits, a binary line id folding back to the trigger tag, a
//! `UDMF` id keeping its field, the arg-only rows that come back with tag 0
//! and the specials that match no row in either direction.

use super::{ZERO_ARGS, line_map, only_line};
use crate::wad::map::model::{Linedef, Map};
use crate::wad::map::xlat::{Untranslated, linedefs_from_hexen, linedefs_to_hexen};

#[test]
fn the_row_1_line_round_trips_through_both_source_shapes() {
    let original = Linedef {
        flags: 0x121_u32,
        special: 1_i32,
        tag: 5_i32,
        ..Default::default()
    };
    for byte_limited in [false, true] {
        let mut map = Map {
            linedefs: vec![original.clone()],
            ..Default::default()
        };
        let there = linedefs_to_hexen(&mut map, byte_limited).expect("the table parses");
        let hexen = only_line(&map);
        assert_eq!(hexen.special, 12_i32, "Door_Raise");
        assert_eq!(
            hexen.args,
            [0_i32, 16_i32, 150_i32, 5_i32, 0_i32],
            "the template with the tag in slot 3"
        );
        assert_eq!(hexen.tag, 0_i32, "the tag moved to the args");
        assert_eq!(
            hexen.flags, 0x2721_u32,
            "Doom bits plus the use field, repeat and monster bits"
        );
        assert_eq!(there.translated, 1, "the row translated");
        assert!(
            there.untranslated.is_empty() && there.warnings.is_empty(),
            "the forward trip is lossless: {there:?}"
        );
        let back = linedefs_from_hexen(&mut map, !byte_limited).expect("the table parses");
        assert_eq!(back.translated, 1, "and read back");
        assert!(
            back.untranslated.is_empty() && back.warnings.is_empty(),
            "the reverse trip is lossless: {back:?}"
        );
        assert_eq!(
            map.linedefs[0], original,
            "there and back with byte_limited={byte_limited}"
        );
    }
}

#[test]
fn the_door_lockedraise_line_finds_row_26_by_slot_3() {
    let mut map = line_map(&[(26_i32, 9_i32, ZERO_ARGS, 0x100_u32)]);
    let there = linedefs_to_hexen(&mut map, true).expect("the table parses");
    let hexen = only_line(&map);
    assert_eq!(hexen.special, 13_i32, "Door_LockedRaise");
    assert_eq!(
        hexen.args,
        [0_i32, 16_i32, 150_i32, 130_i32, 9_i32],
        "the locked-kind 130 sits in slot 3"
    );
    assert_eq!(
        hexen.flags, 0x700_u32,
        "mapped plus the use field and repeat, no monster"
    );
    assert_eq!(there.translated, 1, "row 26 translated");
    let back = linedefs_from_hexen(&mut map, false).expect("the table parses");
    let doom = only_line(&map);
    assert_eq!(doom.special, 26_i32, "slot-3 130 separates row 26");
    assert_eq!(doom.tag, 9_i32, "read from the tag slot 4");
    assert_eq!(doom.flags, 0x100_u32, "the trigger bits cleared");
    assert_eq!(doom.args, ZERO_ARGS, "the matched line zeroes its args");
    assert_eq!(back.translated, 1, "exactly one row matched");
    assert!(
        back.untranslated.is_empty() && back.warnings.is_empty(),
        "lossless: {back:?}"
    );
}

#[test]
fn an_arg_only_scroller_restores_with_tag_zero() {
    let mut map = line_map(&[(48_i32, 0_i32, ZERO_ARGS, 0_u32)]);
    let there = linedefs_to_hexen(&mut map, true).expect("the table parses");
    let hexen = only_line(&map);
    assert_eq!(hexen.special, 100_i32, "Scroll_Texture_Left");
    assert_eq!(
        hexen.args,
        [64_i32, 0_i32, 0_i32, 0_i32, 0_i32],
        "the speed template, no tag slot"
    );
    assert_eq!(
        hexen.flags, 0_u32,
        "no activation: the cross field stays clear"
    );
    assert_eq!(there.translated, 1, "row 48 translated");
    let mut handbuilt = line_map(&[(100_i32, 0_i32, hexen.args, 0_u32)]);
    let back = linedefs_from_hexen(&mut handbuilt, true).expect("the table parses");
    let doom = only_line(&handbuilt);
    assert_eq!(
        doom.special, 48_i32,
        "zdoom 100 with the speed reads as Doom 48"
    );
    assert_eq!(doom.tag, 0_i32, "a row without a tag slot gives tag 0");
    assert_eq!(doom.args, ZERO_ARGS, "the matched line zeroes its args");
    assert_eq!(back.translated, 1, "the scroller matched");
    assert!(
        back.untranslated.is_empty() && back.warnings.is_empty(),
        "lossless: {back:?}"
    );
}

#[test]
fn a_binary_line_id_folds_back_to_the_trigger_tag() {
    let mut map = line_map(&[(0_i32, 7_i32, ZERO_ARGS, 0_u32)]);
    let there = linedefs_to_hexen(&mut map, true).expect("the table parses");
    let hexen = only_line(&map);
    assert_eq!(
        hexen.special, 121_i32,
        "Line_SetIdentification carries the id"
    );
    assert_eq!(
        hexen.args,
        [7_i32, 0_i32, 0_i32, 0_i32, 0_i32],
        "in args[0]"
    );
    assert_eq!(hexen.tag, 0_i32, "binary Hexen models never tag lines");
    assert_eq!(there.translated, 0, "a plain line translates nothing");
    assert!(there.warnings.is_empty(), "an id in a byte never warns");
    let back = linedefs_from_hexen(&mut map, false).expect("the table parses");
    let doom = only_line(&map);
    assert_eq!(doom.special, 0_i32, "the id special folded away");
    assert_eq!(doom.tag, 7_i32, "the id became the trigger tag again");
    assert_eq!(doom.args, ZERO_ARGS, "args[0] was zeroed by the fold");
    assert_eq!(back.translated, 0, "nothing needed the table");
    assert!(
        back.untranslated.is_empty() && back.warnings.is_empty(),
        "the fold is lossless: {back:?}"
    );
}

#[test]
fn a_udmf_line_keeps_its_id_in_the_tag_field() {
    let mut map = line_map(&[(0_i32, 7_i32, ZERO_ARGS, 0_u32)]);
    let there = linedefs_to_hexen(&mut map, false).expect("the table parses");
    assert_eq!(
        only_line(&map).special,
        0_i32,
        "UDMF writes no Line_SetIdentification"
    );
    assert_eq!(only_line(&map).tag, 7_i32, "the tag is the line id here");
    assert_eq!(there.translated, 0, "a plain line translates nothing");
    let back = linedefs_from_hexen(&mut map, true).expect("the table parses");
    assert_eq!(
        only_line(&map).tag,
        7_i32,
        "rule 1: a plain line's id stays as the trigger tag"
    );
    assert_eq!(back.translated, 0, "nothing needed the table");
    assert!(
        back.untranslated.is_empty() && back.warnings.is_empty(),
        "the plain tag is no loss: {back:?}"
    );
}

#[test]
fn the_teleport_rows_split_on_activation_bits() {
    let cases = [
        (39_i32, 0x2000_u32),
        (97_i32, 0x2200),
        (125_i32, 0x0800),
        (126_i32, 0x0A00),
    ];
    for (source, trigger) in cases {
        let mut map = line_map(&[(source, 7_i32, ZERO_ARGS, 0_u32)]);
        let there = linedefs_to_hexen(&mut map, false).expect("the table parses");
        let hexen = only_line(&map);
        assert_eq!(hexen.special, 70_i32, "all four share Teleport");
        assert_eq!(
            hexen.args,
            [0_i32, 7_i32, 0_i32, 0_i32, 0_i32],
            "the tag rides in slot 1"
        );
        assert_eq!(
            hexen.flags, trigger,
            "the field and bits row {source} asks for"
        );
        assert_eq!(there.translated, 1, "Doom {source} translated");
        let back = linedefs_from_hexen(&mut map, true).expect("the table parses");
        let doom = only_line(&map);
        assert_eq!(doom.special, source, "and matched back to itself");
        assert_eq!(doom.tag, 7_i32, "read from the tag slot");
        assert_eq!(back.translated, 1, "one row matched for {source}");
        assert!(
            back.untranslated.is_empty() && back.warnings.is_empty(),
            "unambiguous for {source}: {back:?}"
        );
    }
}

#[test]
fn an_unknown_zspecial_clears_and_is_reported() {
    let mut map = line_map(&[(
        560_i32,
        3_i32,
        [1_i32, 2_i32, 3_i32, 4_i32, 5_i32],
        0x201_u32,
    )]);
    let back = linedefs_from_hexen(&mut map, false).expect("the table parses");
    let doom = only_line(&map);
    assert_eq!(doom.special, 0_i32, "no row, no special");
    assert_eq!(
        doom.flags, 0x1_u32,
        "the repeat bit is consumed, blocking stays"
    );
    assert_eq!(
        doom.tag, 3_i32,
        "a still-nonzero tag is the Doom trigger tag"
    );
    assert_eq!(back.translated, 0, "nothing translated");
    assert_eq!(
        back.untranslated,
        vec![Untranslated {
            linedef: 0,
            special: 560
        }],
        "the line and its old special are recorded"
    );
    assert_eq!(back.warnings.len(), 1, "one grouped warning");
    assert_eq!(
        back.warnings[0],
        "1 linedefs had ZDoom specials with no Doom translation (indices 0): they were set \
         to 0"
    );
}

#[test]
fn activation_fields_the_table_never_writes_are_unmatched() {
    for field in [4_u32, 5] {
        let flags = field.checked_shl(10).unwrap_or(0_u32);
        let mut map = line_map(&[(12_i32, 0_i32, [0_i32, 16_i32, 150_i32, 5_i32, 0_i32], flags)]);
        let back = linedefs_from_hexen(&mut map, true).expect("the table parses");
        let doom = only_line(&map);
        assert_eq!(doom.special, 0_i32, "field {field} matches no row");
        assert_eq!(
            back.untranslated,
            vec![Untranslated {
                linedef: 0,
                special: 12
            }],
            "the field-{field} line is reported"
        );
        assert_eq!(back.translated, 0, "nothing translated");
        assert_eq!(
            doom.flags & 0x1C00,
            0_u32,
            "the activation field bits cleared too"
        );
    }
}

#[test]
fn a_busy_udmf_id_drops_with_a_warning_and_the_line_still_translates() {
    let mut map = line_map(&[(243_i32, 4_i32, ZERO_ARGS, 0x400_u32)]);
    let back = linedefs_from_hexen(&mut map, true).expect("the table parses");
    let doom = only_line(&map);
    assert_eq!(
        doom.special, 11_i32,
        "zdoom 243 with the use field is row 11"
    );
    assert_eq!(
        doom.tag, 0_i32,
        "the row has no tag slot, so the freed id does not return"
    );
    assert_eq!(back.translated, 1, "the special itself translated");
    assert!(back.untranslated.is_empty(), "a match is no miss");
    assert_eq!(
        back.warnings.len(),
        1,
        "one busy-id warning, no grouped miss"
    );
    assert!(
        back.warnings[0].starts_with("linedef 0 line id 4 was dropped")
            && back.warnings[0].contains("already uses special 243"),
        "the line, its id and its special are named: {}",
        back.warnings[0]
    );
}

#[test]
fn an_unmatched_udmf_line_keeps_its_id_as_the_doom_trigger_tag() {
    let mut map = line_map(&[(10_i32, 4_i32, ZERO_ARGS, 0_u32)]);
    let back = linedefs_from_hexen(&mut map, true).expect("the table parses");
    assert_eq!(only_line(&map).tag, 4_i32, "no row took the tag field");
    assert_eq!(
        only_line(&map).special,
        0_i32,
        "and the door row needs 16 in slot 1"
    );
    assert_eq!(
        back.untranslated,
        vec![Untranslated {
            linedef: 0,
            special: 10
        }],
        "the special is reported"
    );
    assert_eq!(back.warnings.len(), 1, "only the grouped miss warns");
    assert!(
        back.warnings[0].starts_with("1 linedefs had ZDoom specials"),
        "{}",
        back.warnings[0]
    );
}

#[test]
fn a_set_identification_from_a_udmf_source_matches_no_row() {
    // UDMF has no Line_SetIdentification: a zdoom line hand-written with
    // special 121 is just a special the table carries no row for.
    let mut map = line_map(&[(121_i32, 7_i32, ZERO_ARGS, 0_u32)]);
    let back = linedefs_from_hexen(&mut map, true).expect("the table parses");
    let doom = only_line(&map);
    assert_eq!(doom.special, 0_i32, "the unmatched path clears the special");
    assert_eq!(doom.tag, 7_i32, "its id stands on as the Doom tag");
    assert_eq!(
        back.untranslated,
        vec![Untranslated {
            linedef: 0,
            special: 121
        }],
        "121 lands among the unmatched"
    );
    assert_eq!(back.warnings.len(), 1, "only the grouped miss warns");
}
