//! What the linedef stages report: the grouped warnings for dropped
//! `Hexen`-only blocking bits and for specials no row covers, the tie
//! [`best_match`] gives to the lower source special, the twenty-index cap on
//! a grouped warning, how `passuse` rides the `extra` bag through both
//! stages, and the actions and arguments a binary `Hexen` byte vetoes.

use super::{LineSpec, ZERO_ARGS, line_map, only_line};
use crate::wad::map::model::Linedef;
use crate::wad::map::xlat::lines_from::best_match;
use crate::wad::map::xlat::lines_to::wide_field;
use crate::wad::map::xlat::{
    Untranslated, XlatFlags, XlatRow, linedefs_from_hexen, linedefs_to_hexen, rows_for_doom,
};

#[test]
fn hexen_only_blocking_bits_drop_with_one_grouped_warning() {
    let mut map = line_map(&[
        (0_i32, 0_i32, ZERO_ARGS, 0x8001_u32),
        (0_i32, 0_i32, ZERO_ARGS, 0x4100),
        (0_i32, 0_i32, ZERO_ARGS, 0x1),
    ]);
    let back = linedefs_from_hexen(&mut map, true).expect("the table parses");
    let flags: Vec<u32> = map.linedefs.iter().map(|line| line.flags).collect();
    assert_eq!(
        flags,
        vec![0x1_u32, 0x100, 0x1],
        "the shared Doom bits kept"
    );
    assert_eq!(back.translated, 0, "plain lines translate nothing");
    assert_eq!(back.warnings.len(), 1, "one grouped warning");
    assert_eq!(
        back.warnings[0],
        "2 linedefs had Hexen-only blocking flags with no Doom equivalent (indices 0, 1): \
         they were dropped"
    );
}

#[test]
fn best_match_gives_a_tie_to_the_lower_source_special() {
    let row_a = XlatRow {
        doom: 5_i32,
        activation: XlatFlags::WALK,
        zspecial: 999_i32,
        name: "Tie_First".to_owned(),
        args: ZERO_ARGS,
        arg_count: 2,
        tag_slot: Some(1),
    };
    let row_b = XlatRow {
        doom: 2_i32,
        ..row_a.clone()
    };
    let line = Linedef {
        special: 999_i32,
        args: [0_i32, 9_i32, 0_i32, 0_i32, 0_i32],
        ..Default::default()
    };
    let rows = [&row_a, &row_b];
    let matched = best_match(&rows, &line);
    assert_eq!(
        matched.map(|(index, _)| index),
        Some(1_usize),
        "the later, lower-doom row wins the tie"
    );
    assert_eq!(
        matched.map(|(_, tag)| tag),
        Some(9_i32),
        "and the tag comes from its own slot"
    );
    assert_eq!(best_match(&[], &line), None, "no rows, no match");
}

#[test]
fn linedef_group_warnings_list_at_most_twenty_indices() {
    let lines: Vec<LineSpec> = (0..25_usize)
        .map(|_| (560_i32, 0_i32, ZERO_ARGS, 0_u32))
        .collect();
    let mut map = line_map(&lines);
    let back = linedefs_from_hexen(&mut map, true).expect("the table parses");
    assert_eq!(back.untranslated.len(), 25, "every line is recorded");
    assert_eq!(back.warnings.len(), 1, "still one warning");
    let warning = back.warnings.first().expect("one warning");
    assert!(
        warning.starts_with("25 linedefs") && warning.contains("19, and 5 more"),
        "twenty are listed, the total is stated: {warning}"
    );
    assert!(
        !warning.contains(", 20,"),
        "index 20 is not listed: {warning}"
    );
}

#[test]
fn passuse_rides_in_the_extra_bag_through_both_stages() {
    let mut map = line_map(&[(0_i32, 0_i32, ZERO_ARGS, 0x201_u32)]);
    let there = linedefs_to_hexen(&mut map, true).expect("the table parses");
    let hexen = only_line(&map);
    assert_eq!(
        hexen.flags, 0x1_u32,
        "bit 512 is repeatspecial in this family"
    );
    assert_eq!(
        hexen.extra.get("passuse").map(String::as_str),
        Some("true"),
        "the key rode over"
    );
    assert_eq!(there.translated, 0, "a plain line translates nothing");
    let back = linedefs_from_hexen(&mut map, false).expect("the table parses");
    let doom = only_line(&map);
    assert_eq!(doom.flags, 0x1_u32, "no Hexen bits to consume");
    assert_eq!(
        doom.extra.get("passuse").map(String::as_str),
        Some("true"),
        "the reverse stage never touches extra"
    );
    assert_eq!(back.translated, 0, "and nothing to translate back");
    assert!(
        there.warnings.is_empty() && back.warnings.is_empty(),
        "carrying passuse warns nothing"
    );
}

#[test]
fn an_unknown_doom_special_clears_and_keeps_its_tag_as_a_line_id() {
    let mut map = line_map(&[(9999_i32, 3_i32, ZERO_ARGS, 0_u32)]);
    let there = linedefs_to_hexen(&mut map, true).expect("the table parses");
    let hexen = only_line(&map);
    assert_eq!(hexen.special, 121_i32, "the plain id carrier takes over");
    assert_eq!(
        hexen.args,
        [3_i32, 0_i32, 0_i32, 0_i32, 0_i32],
        "the trigger tag rides on as the id"
    );
    assert_eq!(hexen.tag, 0_i32, "binary Hexen models never tag lines");
    assert_eq!(there.translated, 0, "the special found no row");
    assert_eq!(
        there.untranslated,
        vec![Untranslated {
            linedef: 0,
            special: 9999
        }],
        "the old Doom special is recorded"
    );
    assert_eq!(there.warnings.len(), 1, "one grouped warning");
    assert!(
        there.warnings[0]
            .starts_with("1 linedefs had Doom specials with no ZDoom translation (indices 0)"),
        "{}",
        there.warnings[0]
    );
    let back = linedefs_from_hexen(&mut map, false).expect("the table parses");
    assert_eq!(only_line(&map).special, 0_i32, "the id carrier folded away");
    assert_eq!(
        only_line(&map).tag,
        3_i32,
        "and the id reads back as the trigger tag"
    );
    assert_eq!(back.translated, 0, "nothing to match");
    assert!(
        back.untranslated.is_empty() && back.warnings.is_empty(),
        "the miss was already reported: {back:?}"
    );
}

#[test]
fn generalized_unknown_and_negative_specials_share_one_grouped_warning() {
    let mut map = line_map(&[
        (0x2F80_i32, 2_i32, ZERO_ARGS, 0_u32),
        (999_i32, 3_i32, ZERO_ARGS, 1_u32),
        (-5_i32, 4_i32, ZERO_ARGS, 0_u32),
        (0_i32, 0_i32, ZERO_ARGS, 2_u32),
    ]);
    let there = linedefs_to_hexen(&mut map, true).expect("the table parses");
    assert_eq!(there.translated, 0, "no line reached the rule 5 rewrite");
    assert_eq!(
        there.untranslated,
        vec![
            Untranslated {
                linedef: 0,
                special: 0x2F80,
            },
            Untranslated {
                linedef: 1,
                special: 999,
            },
            Untranslated {
                linedef: 2,
                special: -5,
            },
        ],
        "the generalized, the table-unknown and the negative special each lose their line"
    );
    assert_eq!(
        there.warnings.len(),
        1,
        "the three losses share one warning"
    );
    assert_eq!(
        there.warnings[0],
        "3 linedefs had Doom specials with no ZDoom translation (indices 0, 1, 2): they \
         were set to 0",
        "the grouped warning names every index and the total"
    );
    let carried: Vec<(i32, i32, i32)> = map
        .linedefs
        .iter()
        .map(|line| {
            (
                line.special,
                line.tag,
                line.args.first().copied().unwrap_or(0_i32),
            )
        })
        .collect();
    assert_eq!(
        carried,
        vec![
            (121_i32, 0_i32, 2_i32),
            (121_i32, 0_i32, 3_i32),
            (121_i32, 0_i32, 4_i32),
            (0_i32, 0_i32, 0_i32)
        ],
        "every cleared tag rides on as a Line_SetIdentification id, and the plain line \
         stands untouched"
    );
}

#[test]
fn byte_limited_refuses_actions_the_hexen_bytes_cannot_hold() {
    let mut narrow = line_map(&[(100_i32, 5_i32, ZERO_ARGS, 0_u32)]);
    let there = linedefs_to_hexen(&mut narrow, true).expect("the table parses");
    assert_eq!(
        there.translated, 0,
        "Stairs_BuildUpDoomCrush needs action 273"
    );
    assert_eq!(
        there.untranslated,
        vec![Untranslated {
            linedef: 0,
            special: 100
        }],
        "the vetoed line is reported"
    );
    assert_eq!(
        only_line(&narrow).special,
        121_i32,
        "the line fell back to a plain id carrier"
    );
    assert_eq!(
        only_line(&narrow).args,
        [5_i32, 0_i32, 0_i32, 0_i32, 0_i32],
        "its tag rides on as the id"
    );
    assert_eq!(there.warnings.len(), 1, "the vetoed line warns once");
    let veto = there.warnings.first().expect("one warning");
    assert!(
        veto.contains("linedef 0 special 100 was set to 0")
            && veto.contains("Stairs_BuildUpDoomCrush")
            && veto.contains("ZDoom action 273")
            && veto.contains("its action number")
            && veto.contains("convert to a UDMF Hexen-family map"),
        "the warning names the line, the action and the fix: {veto}"
    );
    let door = rows_for_doom(26).expect("the table parses")[0];
    assert!(
        wide_field(door, 9_i32).is_none(),
        "row 26's template fits the bytes with a narrow tag"
    );
    assert_eq!(
        wide_field(door, 300_i32).as_deref(),
        Some("its tag 300 in arg4"),
        "the wide tag landing in its slot 4 vetoes the line"
    );
    assert_eq!(
        wide_field(door, -1_i32).as_deref(),
        Some("its tag -1 in arg4"),
        "a negative tag never fits a u8 slot"
    );
    let stairs = rows_for_doom(100).expect("the table parses")[0];
    assert_eq!(
        wide_field(stairs, 5_i32).as_deref(),
        Some("its action number"),
        "zspecial 273 is the veto here"
    );
}
