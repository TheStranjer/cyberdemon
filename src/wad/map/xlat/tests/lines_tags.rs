//! Tags and line ids through the linedef stages: the veto warning naming
//! a wide template argument, the `UDMF` trip the byte limit leaves alone, the
//! tag with no `tag` slot riding on as a line id and dropping for a binary
//! target, the wide and busy ids, the reverse pass mixing every branch, and
//! [`trip_table_row`] run over every row of the shipped table.

use super::{ZERO_ARGS, line_map, only_line, trip_table_row};
use crate::wad::map::xlat::lines_to::{too_wide_note, wide_field};
use crate::wad::map::xlat::{
    Untranslated, XlatFlags, XlatRow, linedefs_from_hexen, linedefs_to_hexen, xlat_rows,
};

#[test]
fn a_wide_template_argument_is_named_by_its_own_veto_warning() {
    let row = XlatRow {
        doom: 200_i32,
        activation: XlatFlags(0_u32),
        zspecial: 50_i32,
        name: "Wide_Action".to_owned(),
        args: [0_i32, 300_i32, 0_i32, 0_i32, 0_i32],
        arg_count: 2_usize,
        tag_slot: None,
    };
    assert_eq!(
        wide_field(&row, 7_i32).as_deref(),
        Some("its arg1 300"),
        "a template value over a byte is the veto, and a row without a tag slot never \
         reads the line's tag"
    );
    let note = too_wide_note(4_usize, 200_i32, &row, "its arg1 300");
    assert!(
        note.contains("linedef 4 special 200 was set to 0")
            && note.contains("Wide_Action")
            && note.contains("ZDoom action 50")
            && note.contains("its arg1 300 is more than a binary Hexen field can store"),
        "{note}"
    );
}

#[test]
fn a_stairs_line_trips_udmf_untouched_by_the_byte_limit() {
    let mut map = line_map(&[(100_i32, 5_i32, ZERO_ARGS, 0_u32)]);
    let there = linedefs_to_hexen(&mut map, false).expect("the table parses");
    assert_eq!(there.translated, 1, "UDMF args are not byte-limited");
    assert_eq!(
        only_line(&map).special,
        273_i32,
        "the stairs action rides on"
    );
    assert_eq!(
        only_line(&map).args,
        [5_i32, 32_i32, 16_i32, 0_i32, 0_i32],
        "the tag took slot 0"
    );
    let back = linedefs_from_hexen(&mut map, true).expect("the table parses");
    assert_eq!(back.translated, 1, "and zspecial 273 reads back in UDMF");
    assert_eq!(
        only_line(&map).special,
        100_i32,
        "row 100, not the USE twin 127"
    );
    assert_eq!(only_line(&map).tag, 5_i32, "the tag returned from slot 0");
    assert_eq!(
        only_line(&map).args,
        ZERO_ARGS,
        "args zeroed on the way back"
    );
    assert!(
        back.warnings.is_empty(),
        "the UDMF trip was lossless: {back:?}"
    );
}

#[test]
fn a_tag_with_no_slot_rides_the_udmf_id_and_warns_on_the_way_back() {
    let mut map = line_map(&[(11_i32, 4_i32, ZERO_ARGS, 0x100_u32)]);
    let there = linedefs_to_hexen(&mut map, false).expect("the table parses");
    let hexen = only_line(&map);
    assert_eq!(hexen.special, 243_i32, "Exit_Normal has no tag argument");
    assert_eq!(hexen.tag, 4_i32, "the trigger tag rides on as the line id");
    assert_eq!(there.translated, 1, "row 11 translated");
    assert!(there.warnings.is_empty(), "UDMF loses nothing yet");
    let back = linedefs_from_hexen(&mut map, true).expect("the table parses");
    let doom = only_line(&map);
    assert_eq!(
        doom.special, 11_i32,
        "zdoom 243 with the use field is row 11"
    );
    assert_eq!(doom.tag, 0_i32, "the id had nowhere to end up");
    assert_eq!(
        doom.flags, 0x100_u32,
        "the use field bit was consumed, mapped kept"
    );
    assert_eq!(back.translated, 1, "the special itself translated");
    assert_eq!(back.warnings.len(), 1, "the busy-id warning names the loss");
    assert!(
        back.warnings[0].contains("linedef 0 line id 4"),
        "{}",
        back.warnings[0]
    );
}

#[test]
fn a_homeless_tag_drops_at_once_for_a_binary_target() {
    let mut map = line_map(&[(11_i32, 4_i32, ZERO_ARGS, 0_u32)]);
    let there = linedefs_to_hexen(&mut map, true).expect("the table parses");
    assert_eq!(there.translated, 1, "Exit_Normal itself fits the bytes");
    assert_eq!(
        there.warnings.len(),
        1,
        "binary Hexen drops the homeless id at once"
    );
    assert!(
        there.warnings[0].contains("linedef 0 line id 4 was dropped")
            && there.warnings[0].contains("already uses special 243"),
        "{}",
        there.warnings[0]
    );
    assert_eq!(only_line(&map).tag, 0_i32, "the dropped id left the model");
    let back = linedefs_from_hexen(&mut map, false).expect("the table parses");
    assert_eq!(
        only_line(&map).special,
        11_i32,
        "the line itself still came home"
    );
    assert_eq!(back.translated, 1, "and through the table");
    assert!(
        back.warnings.is_empty(),
        "the loss was already reported: {back:?}"
    );
}

#[test]
fn a_wide_homeless_tag_drops_with_the_byte_range_hint() {
    let mut map = line_map(&[(0_i32, 300_i32, ZERO_ARGS, 0_u32)]);
    let there = linedefs_to_hexen(&mut map, true).expect("the table parses");
    assert_eq!(there.translated, 0, "a plain line translates nothing");
    assert_eq!(there.warnings.len(), 1, "the id itself does not fit a byte");
    assert!(
        there.warnings[0].starts_with("linedef 0 line id 300 was dropped")
            && there.warnings[0].contains("one byte"),
        "{}",
        there.warnings[0]
    );
    let doom = only_line(&map);
    assert_eq!(doom.tag, 0_i32, "the doomed id left with the warning");
    assert_eq!(doom.special, 0_i32, "and no id carrier was written");
}

#[test]
fn a_wide_tag_on_a_translated_line_drops_with_the_busy_warning() {
    let mut map = line_map(&[(48_i32, 300_i32, ZERO_ARGS, 0_u32)]);
    let there = linedefs_to_hexen(&mut map, true).expect("the table parses");
    assert_eq!(there.translated, 1, "zspecial 100 and arg 64 fit the bytes");
    assert_eq!(
        only_line(&map).special,
        100_i32,
        "the line translated anyway"
    );
    assert_eq!(
        only_line(&map).tag,
        0_i32,
        "the homeless id left with the warning"
    );
    assert_eq!(
        there.warnings.len(),
        1,
        "one drop, the used special being the reason"
    );
    assert!(
        there.warnings[0].starts_with("linedef 0 line id 300 was dropped")
            && there.warnings[0].contains("already uses special 100"),
        "{}",
        there.warnings[0]
    );
}

#[test]
fn the_reverse_pass_mixes_every_branch() {
    let mut map = line_map(&[
        (
            12_i32,
            0_i32,
            [0_i32, 16_i32, 150_i32, 5_i32, 0_i32],
            0x2600_u32,
        ),
        (0_i32, 2_i32, ZERO_ARGS, 0_u32),
        (560_i32, 0_i32, ZERO_ARGS, 0x4000),
    ]);
    let back = linedefs_from_hexen(&mut map, true).expect("the table parses");
    assert_eq!(back.translated, 1, "line 0 came back through the table");
    assert_eq!(map.linedefs[0].special, 1_i32, "row 1 matched");
    assert_eq!(map.linedefs[0].tag, 5_i32, "its tag read from slot 3");
    assert_eq!(
        map.linedefs[1].tag, 2_i32,
        "the plain id stayed the trigger tag"
    );
    assert_eq!(
        map.linedefs[1].special, 0_i32,
        "rule 1: nothing to translate"
    );
    assert_eq!(
        map.linedefs[2].special, 0_i32,
        "the unmatched special cleared"
    );
    assert_eq!(map.linedefs[2].flags, 0_u32, "blockplayers has no Doom bit");
    assert_eq!(
        back.untranslated,
        vec![Untranslated {
            linedef: 2,
            special: 560
        }],
        "only the miss is recorded"
    );
    assert_eq!(
        back.warnings.len(),
        2,
        "the grouped miss and the grouped flag drop"
    );
    assert!(
        back.warnings[0].contains("(indices 2)"),
        "{}",
        back.warnings[0]
    );
    assert!(
        back.warnings[1].contains("(indices 2)"),
        "{}",
        back.warnings[1]
    );
}

#[test]
fn every_table_row_round_trips_through_udmf() {
    let rows = xlat_rows().expect("the embedded table parses");
    for row in rows {
        trip_table_row(row);
    }
    let tagless = rows.iter().filter(|row| row.tag_slot.is_none()).count();
    assert_eq!(tagless, 6_usize, "the six arg-only rows have no tag slot");
}

#[test]
fn the_crushing_stairs_row_drops_its_wide_action_at_the_byte_limit() {
    // Going in: binary Hexen has no byte for zspecial 273.
    let mut entering = line_map(&[(100_i32, 13_i32, ZERO_ARGS, 0_u32)]);
    let there = linedefs_to_hexen(&mut entering, true).expect("the table parses");
    assert_eq!(there.translated, 0, "row 100's action does not fit a byte");
    assert_eq!(
        there.untranslated,
        vec![Untranslated {
            linedef: 0,
            special: 100
        }],
        "the vetoed Doom special is reported"
    );
    assert_eq!(there.warnings.len(), 1, "one veto warning, no grouped miss");
    assert!(
        there
            .warnings
            .first()
            .is_some_and(|note| note.contains("special 100 was set to 0")),
        "the line is told its own action is the problem: {:?}",
        there.warnings
    );
    let hexen = only_line(&entering);
    assert_eq!(hexen.special, 121_i32, "the cleared line carries its id");
    assert_eq!(
        hexen.args,
        [13_i32, 0_i32, 0_i32, 0_i32, 0_i32],
        "the trigger tag rode on as the line id"
    );
    // Coming out: a line still carrying zdoom 273 that fits no row.
    let mut leaving = line_map(&[(273_i32, 13_i32, [1_i32, 2_i32, 3_i32, 4_i32, 5_i32], 0_u32)]);
    let back = linedefs_from_hexen(&mut leaving, false).expect("the table parses");
    assert_eq!(back.translated, 0, "neither row 100 nor row 127 accepts it");
    assert_eq!(
        back.untranslated,
        vec![Untranslated {
            linedef: 0,
            special: 273
        }],
        "273 is reported leaving as well"
    );
    assert_eq!(back.warnings.len(), 1, "one grouped miss");
    let doom = only_line(&leaving);
    assert_eq!(doom.special, 0_i32, "the special ends 0");
    assert_eq!(doom.tag, 13_i32, "its tag stands as the trigger tag");
}
