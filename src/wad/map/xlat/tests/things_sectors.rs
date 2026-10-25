//! The thing-flag and sector-type stages and the report shapes: the
//! presence bits both directions carry, the script fields they drop, the one
//! grouped warning for the `Hexen`-only bits, the Boom-style sector fields
//! through both folds with their clearing and index lists, the there-and-back
//! trips that lose nothing, and how the reports serialize.

use super::{flags_of, sector_map, specials_of, thing_map, trip_sector_base};
use crate::wad::map::model::Map;
use crate::wad::map::xlat::{
    FieldReport, LinesReport, Untranslated, linedefs_from_hexen, linedefs_to_hexen,
    sectors_from_hexen, sectors_to_hexen, things_from_hexen, things_to_hexen,
};

#[test]
fn things_to_hexen_translates_the_presence_bits() {
    let mut map = thing_map(&[0_u32, 15, 16, 128]);
    let report = things_to_hexen(&mut map);
    assert_eq!(
        flags_of(&map),
        vec![2016_u32, 2031, 1760, 10208],
        "flags 0 becomes single|coop|dm|all classes = 2016; the skill bits 1|2|4|8 stand; \
         notsingle 16 clears the 256 single bit; friend 128 arrives at 8192"
    );
    assert_eq!(
        report.changed, 4,
        "all four flag sets differ from their source"
    );
    assert!(report.warnings.is_empty(), "the forward trip drops nothing");
}

#[test]
fn things_to_hexen_leaves_a_translated_thing_uncounted() {
    // 8672 = 32|64|128|256|8192: the shape this stage writes for a
    // friend thing with single presence, and its fixed point. A value
    // like 2016 is NOT that shape: read as Doom bits its 32, 64 and
    // 128 exclude it from deathmatch and coop and mark it a friend,
    // so the second pass drops those presence bits and adds 8192.
    let mut map = thing_map(&[8672_u32, 2016]);
    let report = things_to_hexen(&mut map);
    assert_eq!(flags_of(&map), vec![8672_u32, 8672], "both settle on 8672");
    assert_eq!(report.changed, 1, "only the moving thing counts as changed");
}

#[test]
fn things_to_hexen_zeroes_the_script_fields() {
    let mut map = thing_map(&[8672_u32, 8672]);
    if let Some(first) = map.things.first_mut() {
        first.tid = 7_i32;
        first.special = 4_i32;
        first.args = [1_i32, 2_i32, 3_i32, 4_i32, 5_i32];
        first.x = 64_i32;
        first.y = -128_i32;
        first.z = 32_i32;
        first.angle = 90_i32;
        first.kind = 3001_i32;
    }
    let report = things_to_hexen(&mut map);
    let first = map.things.first().expect("two things");
    assert_eq!(first.tid, 0_i32, "the tid goes");
    assert_eq!(first.special, 0_i32, "the special goes");
    assert_eq!(first.args, [0_i32; 5], "the args go");
    assert_eq!(first.flags, 8672_u32, "and the translated shape stands");
    assert_eq!(
        (first.x, first.y, first.z, first.angle, first.kind),
        (64_i32, -128_i32, 32_i32, 90_i32, 3001_i32),
        "coordinates, height, facing and kind are untouched"
    );
    assert_eq!(report.changed, 1, "a nonzero script field alone counts");
}

#[test]
fn things_from_hexen_folds_the_hexen_bits_back() {
    let mut map = thing_map(&[10208_u32, 1984, 8192]);
    let report = things_from_hexen(&mut map);
    assert_eq!(
        flags_of(&map),
        vec![128_u32, 16, 240],
        "a fully translated friend reads back as Doom friend 128; the missing class1 bit \
         forces notsingle 16; a bare 8192 thing has no mode or class and is excluded from \
         all of them as well"
    );
    assert_eq!(report.changed, 3, "all three flag sets move");
    assert!(report.warnings.is_empty(), "nothing was dropped");
}

#[test]
fn things_from_hexen_warns_once_about_the_dropped_bits() {
    let mut map = thing_map(&[16_u32, 2064, 4096, 0x4000, 7, 120]);
    let report = things_from_hexen(&mut map);
    assert_eq!(report.warnings.len(), 1, "one grouped warning");
    assert_eq!(
        report.warnings.first().expect("one warning"),
        "5 things had Hexen-only flags with no Doom equivalent (dormant 3, translucent 1, \
         invisible 1, standing 1): they were dropped",
        "the totals and the nonzero per-bit parts, nothing else"
    );
    assert_eq!(report.changed, 5, "the 120 thing keeps its flags");
}

#[test]
fn things_from_hexen_zeroes_the_script_fields_without_counting() {
    let mut map = thing_map(&[120_u32]);
    if let Some(first) = map.things.first_mut() {
        first.tid = 9_i32;
        first.special = 2_i32;
        first.args = [5_i32; 5];
    }
    let report = things_from_hexen(&mut map);
    let first = map.things.first().expect("one thing");
    assert_eq!(first.tid, 0_i32, "the tid goes");
    assert_eq!(first.special, 0_i32, "the special goes");
    assert_eq!(first.args, [0_i32; 5], "the args go");
    assert_eq!(first.flags, 120_u32, "16|32|64|8 reads back as itself");
    assert_eq!(
        report.changed, 0,
        "the reverse counts flag differences only"
    );
    assert_eq!(report.warnings.len(), 1, "the dormant bit still warns");
}

#[test]
fn thing_flags_survive_a_there_and_back_trip() {
    let choices = [8_u32, 16_u32, 32_u32, 64_u32, 128_u32];
    let mut originals: Vec<u32> = vec![0_u32];
    for bit in choices {
        let grown: Vec<u32> = originals.iter().map(|sum| sum | bit).collect();
        originals.extend(grown);
    }
    assert_eq!(
        originals.len(),
        32,
        "every presence combination plus ambush and friend"
    );
    for original in originals {
        let mut map = thing_map(&[original]);
        let there = things_to_hexen(&mut map);
        let back = things_from_hexen(&mut map);
        assert_eq!(flags_of(&map), vec![original], "{original} comes home");
        assert!(
            there.warnings.is_empty() && back.warnings.is_empty(),
            "the trip warns nothing for {original}"
        );
    }
}

#[test]
fn sectors_to_hexen_moves_the_sector_fields() {
    let mut map = sector_map(&[0_i32, 1_i32, 9_i32, 33_i32, 999_i32]);
    let report = sectors_to_hexen(&mut map);
    assert_eq!(
        specials_of(&map),
        vec![0_i32, 65_i32, 1_024_i32, 321_i32, 8_007_i32],
        "plain stays plain, base 1 sits behind 64, base 9 becomes the secret bit, 33 moves \
         its damage field to 65|256, and 999 carries all five inputs at once"
    );
    assert_eq!(report.changed, 4, "only the plain sector stands");
    assert!(report.warnings.is_empty(), "in-range values warn nothing");
}

#[test]
fn sectors_to_hexen_clears_values_the_boom_bits_cannot_hold() {
    let mut map = sector_map(&[1_024_i32, -1_i32, 0_i32]);
    let report = sectors_to_hexen(&mut map);
    assert_eq!(
        specials_of(&map),
        vec![0_i32, 0_i32, 0_i32],
        "each odd value is cleared"
    );
    assert_eq!(report.changed, 2, "the plain sector did not move");
    assert_eq!(report.warnings.len(), 1, "one grouped warning");
    let warning = report.warnings.first().expect("one warning");
    assert!(
        warning.starts_with(
            "2 sectors had specials that are not Boom-style generalized \
                            values (indices 0, 1)"
        ) && warning.ends_with(": they were set to 0"),
        "the total opens and the fix closes it: {warning}"
    );
}

#[test]
fn sector_warnings_list_at_most_twenty_indices() {
    let specials = vec![2048_i32; 25];
    let mut map = sector_map(&specials);
    let report = sectors_to_hexen(&mut map);
    assert_eq!(report.warnings.len(), 1, "still one warning");
    assert_eq!(report.changed, 25, "all twenty-five cleared to 0");
    let warning = report.warnings.first().expect("one warning");
    assert!(
        warning.starts_with("25 sectors") && warning.contains("19, and 5 more"),
        "twenty are listed, the total is stated: {warning}"
    );
    assert!(
        !warning.contains(", 20,") && !warning.contains(" 20, and"),
        "index 20 is not listed: {warning}"
    );
}

#[test]
fn sectors_from_hexen_restores_the_boom_values() {
    let mut map = sector_map(&[0_i32, 65_i32, 81_i32, 1_345_i32]);
    let report = sectors_from_hexen(&mut map);
    assert_eq!(
        specials_of(&map),
        vec![0_i32, 1_i32, 17_i32, 161_i32],
        "0 stays 0, 65 decodes to base 1, 81 to base 17, and 65|256|1024 restores the \
         damage and secret fields of its Boom value 161"
    );
    assert_eq!(report.changed, 3, "the plain sector did not move");
    assert!(report.warnings.is_empty(), "translated values warn nothing");
}

#[test]
fn sectors_from_hexen_clears_types_that_are_not_translated() {
    let mut map = sector_map(&[1_024_i32, 200_i32, 10_i32, -3_i32]);
    let report = sectors_from_hexen(&mut map);
    assert_eq!(
        specials_of(&map),
        vec![0_i32, 0_i32, 0_i32, 0_i32],
        "none of them decodes"
    );
    assert_eq!(report.changed, 4, "all four moved to 0");
    assert_eq!(report.warnings.len(), 1, "one grouped warning");
    let warning = report.warnings.first().expect("one warning");
    assert!(
        warning.starts_with(
            "4 sectors had specials that are not translated Boom types \
                             (indices 0, 1, 2, 3)"
        ),
        "every index and the total are named: {warning}"
    );
}

#[test]
fn boom_sector_types_survive_a_there_and_back_trip() {
    // Type 9 is absent by design: it travels as a bare secret bit and
    // reads back as a native Hexen secret, not its Boom value.
    let bases = [
        1_i32, 2_i32, 3_i32, 4_i32, 5_i32, 6_i32, 7_i32, 8_i32, 10_i32, 11_i32, 12_i32, 13_i32,
        14_i32, 15_i32, 16_i32, 17_i32,
    ];
    let field_sets = [
        0_i32, 32_i32, 64_i32, 128_i32, 256_i32, 512_i32, 96_i32, 160_i32, 800_i32,
    ];
    for base in bases {
        trip_sector_base(base, &field_sets);
    }
}

#[test]
fn sector_translation_never_touches_the_tags() {
    let mut map = sector_map(&[1_i32, 1_024_i32]);
    for (index, area) in map.sectors.iter_mut().enumerate() {
        let offset = i32::try_from(index).unwrap_or(0_i32);
        area.tag = offset.saturating_add(40_i32);
    }
    let there = sectors_to_hexen(&mut map);
    let back = sectors_from_hexen(&mut map);
    assert_eq!(there.changed, 2, "the type moves, the odd value clears");
    assert_eq!(
        back.changed, 1,
        "the type comes back; the cleared sector stands"
    );
    assert_eq!(
        specials_of(&map),
        vec![1_i32, 0_i32],
        "the specials round trip"
    );
    let tags: Vec<i32> = map.sectors.iter().map(|area| area.tag).collect();
    assert_eq!(
        tags,
        vec![40_i32, 41_i32],
        "the tags stand through both stages"
    );
}

#[test]
fn empty_maps_translate_to_empty_reports() {
    let mut empty = Map::default();
    assert_eq!(things_to_hexen(&mut empty), FieldReport::default());
    assert_eq!(things_from_hexen(&mut empty), FieldReport::default());
    assert_eq!(sectors_to_hexen(&mut empty), FieldReport::default());
    assert_eq!(sectors_from_hexen(&mut empty), FieldReport::default());
    assert_eq!(
        linedefs_to_hexen(&mut empty, true).expect("the table parses"),
        LinesReport::default(),
        "a binary-target forward pass over no lines"
    );
    assert_eq!(
        linedefs_from_hexen(&mut empty, false).expect("the table parses"),
        LinesReport::default(),
        "a binary-source reverse pass over no lines"
    );
}

#[test]
fn the_field_report_serializes_its_two_fields() {
    let report = FieldReport {
        changed: 2,
        warnings: vec!["two".to_owned()],
    };
    let text = serde_json::to_string(&report).expect("FieldReport serializes");
    assert_eq!(
        text, r#"{"changed":2,"warnings":["two"]}"#,
        "the wire shape the wiring slice will publish"
    );
}

#[test]
fn the_lines_report_serializes_its_three_fields() {
    let report = LinesReport {
        translated: 1,
        untranslated: vec![Untranslated {
            linedef: 2,
            special: 560,
        }],
        warnings: vec!["gone".to_owned()],
    };
    let text = serde_json::to_string(&report).expect("LinesReport serializes");
    assert_eq!(
        text,
        r#"{"translated":1,"untranslated":[{"linedef":2,"special":560}],"warnings":["gone"]}"#,
        "the wire shape the wiring slice will publish"
    );
}

#[test]
fn untranslated_serializes_under_its_plain_field_names() {
    let lost = serde_json::to_value(Untranslated {
        linedef: 3,
        special: 999,
    })
    .expect("Untranslated serializes");
    assert_eq!(
        lost,
        serde_json::json!({"linedef": 3_i32, "special": 999_i32}),
        "plain field names, no serde renames"
    );
}

#[test]
fn the_default_lines_report_is_all_zero_and_empty() {
    let report = LinesReport::default();
    assert_eq!(report.translated, 0, "a fresh report translated nothing");
    assert!(report.untranslated.is_empty(), "and lost no lines");
    assert!(report.warnings.is_empty(), "and warns about nothing");
    assert_eq!(
        serde_json::to_value(&report).expect("LinesReport serializes"),
        serde_json::json!({"translated": 0_i32, "untranslated": [], "warnings": []}),
        "the empty wire shape"
    );
}
