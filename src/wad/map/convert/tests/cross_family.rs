//! Cross-family steps: the [`crate::wad::map::xlat`] table in both
//! directions. Forward, a `Doom` door line rides a row into its generalized
//! `Hexen` shape, its `passuse` bit moves to the line's `extra` bag and the
//! trigger tag into the row's `tag` slot, and the trip home restores all of
//! it, spending only that extra key; a binary `Hexen` target needs no line
//! id carrier and a `UDMF` source stores its ids once, and the `Doom`
//! family's two `MBF21` block bits stay outside the shared mask on both
//! legs. Back, the thing bits
//! fold to their `Doom` twins, `Line_SetIdentification` comes back as a
//! trigger tag, a `UDMF` special no row carries is untranslated and the
//! listed misses stop at [`UNTRANSLATED_LIMIT`] with the rest counted.

use super::{
    binary, carried, cycle, doom_udmf, door_udmf_zdoom, hexen_map, udmf, udmf_map, zdoom_udmf,
};
use crate::error::CyberError;
use crate::wad::classify::MapFormat;
use crate::wad::map::common::fixtures::two_room_map;
use crate::wad::map::convert::{KindCounts, UNTRANSLATED_LIMIT, convert_map, encodes, format_name};
use crate::wad::map::flags::{DOOM_BLOCK_LAND_MONSTERS, DOOM_BLOCK_PLAYERS};
use crate::wad::map::xlat::Untranslated;

#[test]
fn a_doom_map_converts_forward_through_the_table_to_hexen() {
    let (map, out) = door_udmf_zdoom();
    assert_eq!(map.format, MapFormat::Udmf, "the layout moved");
    assert_eq!(
        map.udmf_namespace.as_deref(),
        Some("zdoom"),
        "and declares zdoom"
    );
    let door = map.linedefs.get(2).expect("the door line");
    assert_eq!(door.special, 12_i32, "Door_Raise");
    assert_eq!(door.args, [0_i32, 16_i32, 150_i32, 5_i32, 0_i32]);
    assert_eq!(door.tag, 0_i32, "the trigger tag moved into its slot");
    assert_eq!(
        door.flags, 0x2601_u32,
        "blocking plus the use field, repeat and monster bits; passuse left bit 512"
    );
    assert_eq!(
        door.extra.get("passuse").map(String::as_str),
        Some("true"),
        "the Doom passuse bit rides on as an extra key"
    );
    assert_eq!(out.translated_specials, 1, "the door line rode the table");
    assert!(out.untranslated.is_empty(), "row 1 translates");
    assert_eq!(out.thing_flags_changed, 2, "both things move to Hexen bits");
    assert_eq!(out.sector_types_changed, 0, "plain sectors stand");
    assert!(
        out.warnings.is_empty(),
        "UDMF loses nothing: {:?}",
        out.warnings
    );
}

#[test]
fn a_forward_converted_map_comes_home_and_drops_its_passuse_extra() {
    let (mut back, _there) = door_udmf_zdoom();
    let again = convert_map(&mut back, &binary("doom")).expect("and converts back");
    assert_eq!(
        again.translated_specials, 1,
        "row 1 came back through the table"
    );
    assert!(
        again.untranslated.is_empty(),
        "nothing failed going back: {:?}",
        again.warnings
    );
    assert_eq!(again.thing_flags_changed, 2, "the things come home too");
    let restored = back.linedefs.get(2).expect("the door line");
    assert_eq!(restored.special, 1_i32, "the Doom special is restored");
    assert_eq!(restored.tag, 5_i32, "with its trigger tag");
    assert_eq!(restored.args, [0_i32; 5], "and the generalized args gone");
    assert_eq!(
        restored.flags, 0x1_u32,
        "the trigger bits were consumed, blocking kept"
    );
    assert!(
        restored.extra.is_empty(),
        "the binary target drops the extra key"
    );
    assert_eq!(
        again.dropped_fields.linedefs, 1,
        "the forwarded passuse key counts once"
    );
    assert_eq!(
        format_name(back.format),
        "doom",
        "the line is home: only the passuse bit was spent on the trip"
    );
}

#[test]
fn a_forward_conversion_leaves_the_doom_mbf21_bits_behind() {
    let mut map = two_room_map();
    if let Some(step) = map.linedefs.get_mut(2) {
        step.flags = 0x101_u32 | DOOM_BLOCK_LAND_MONSTERS | DOOM_BLOCK_PLAYERS;
    }
    convert_map(&mut map, &udmf("zdoom")).expect("the forward direction converts");
    let step = map.linedefs.get(2).expect("the step line");
    assert_eq!(
        step.flags, 0x101_u32,
        "only the bits the families share cross over: the MBF21 pair sits \
         outside the shared mask, where Hexen reads repeat and monster bits"
    );
    let again = convert_map(&mut map, &udmf("doom")).expect("and comes home");
    let home = map.linedefs.get(2).expect("the step line");
    assert_eq!(
        home.flags, 0x101_u32,
        "with no MBF21 bit restored: {:?}",
        again.warnings
    );
}

#[test]
fn a_forward_conversion_to_binary_hexen_needs_no_line_id_carrier() {
    let mut map = two_room_map();
    if let Some(step) = map.linedefs.get_mut(2) {
        step.special = 1_i32;
        step.tag = 13_i32;
    }
    let out = convert_map(&mut map, &binary("hexen")).expect("the forward direction converts");
    assert_eq!(out.translated_specials, 1, "row 1's tag took its slot");
    assert!(out.untranslated.is_empty(), "row 1 fits the bytes");
    assert!(
        !out.warnings.iter().any(|note| note.contains("line id")),
        "the tag left no homeless id: {:?}",
        out.warnings
    );
    let door = map.linedefs.get(2).expect("the door line");
    assert_eq!(door.special, 12_i32, "Door_Raise in one byte");
    assert_eq!(door.args, [0_i32, 16_i32, 150_i32, 13_i32, 0_i32]);
    assert_eq!(door.tag, 0_i32, "binary Hexen models never tag lines");
    encodes(&map).expect("a binary Hexen record writes");
}

#[test]
fn a_udmf_doom_family_source_stores_its_line_ids_once() {
    let body = concat!(
        "vertex { x = 0; y = 0; } sidedef { sector = 0; }",
        "sector { texturefloor = \"FLOOR1\"; textureceiling = \"CEIL1\"; }",
        "linedef { v1 = 0; v2 = 0; sidefront = 0; id = 9; arg0 = 9; }",
    );
    let mut map = doom_udmf(body);
    let out = convert_map(&mut map, &binary("hexen")).expect("the forward direction converts");
    assert!(out.untranslated.is_empty(), "a plain line reports nothing");
    assert_eq!(out.translated_specials, 0, "and translates nothing");
    assert!(
        out.warnings.is_empty(),
        "keep_line_id stored the id once; store_line_ids found none left: {:?}",
        out.warnings
    );
    let line = map.linedefs.first().expect("the plain line");
    assert_eq!(line.special, 121_i32, "Line_SetIdentification carries it");
    assert_eq!(line.args.first().copied(), Some(9_i32), "in args[0]");
    assert_eq!(line.tag, 0_i32, "and the model tag emptied once");
    encodes(&map).expect("and the record writes");
}

#[test]
fn a_hexen_map_converts_back_to_a_binary_doom_layout() {
    let mut map = hexen_map();
    let out = convert_map(&mut map, &binary("doom")).expect("the reverse direction converts");
    assert_eq!(out.translated_specials, 0, "no line needed the table");
    assert!(out.untranslated.is_empty(), "nothing failed to translate");
    assert_eq!(
        out.thing_flags_changed, 2,
        "both things fold back to Doom bits"
    );
    assert_eq!(out.sector_types_changed, 0, "plain sectors stand");
    let portal = map.linedefs.get(2).expect("the identified line");
    assert_eq!(portal.special, 0_i32, "Line_SetIdentification folded away");
    assert_eq!(
        portal.tag, 7_i32,
        "its id came back as the Doom trigger tag"
    );
    assert_eq!(portal.args, [0_i32; 5], "and the id slot zeroed");
    let first = map.things.first().expect("the scripted thing");
    assert_eq!(first.tid, 0_i32, "the tid has no Doom home");
    assert_eq!(first.special, 0_i32, "nor does the thing special");
    assert_eq!(first.args, [0_i32; 5], "nor the thing args");
    assert_eq!(
        first.z, 128_i32,
        "the model keeps the height for a UDMF target"
    );
    assert_eq!(
        first.flags, 119_u32,
        "7 read as Hexen bits excludes every mode"
    );
    assert!(
        out.warnings
            .iter()
            .any(|text| text.contains("BEHAVIOR") && text.contains("binary Doom")),
        "the carried BEHAVIOR cannot stay: {:?}",
        out.warnings
    );
    assert_eq!(carried(&map), vec!["SCRIPTS"], "only SCRIPTS rode along");
    assert_eq!(map.format, MapFormat::Doom, "the layout moved");
    assert_eq!(map.udmf_namespace, None, "and no namespace claims Hexen");
    assert_eq!(
        cycle(&map).format,
        MapFormat::Doom,
        "the block reads as Doom"
    );
}

#[test]
fn a_zdoom_udmf_map_converts_back_to_binary_doom() {
    let mut map = udmf_map("zdoom");
    let out = convert_map(&mut map, &binary("doom")).expect("the reverse direction converts");
    assert_eq!(out.translated_specials, 0, "plain lines translate nothing");
    assert!(out.untranslated.is_empty(), "and report nothing");
    assert_eq!(
        out.thing_flags_changed, 2,
        "the flags 7 things exclude every mode"
    );
    assert_eq!(out.sector_types_changed, 0, "the two plain sectors stand");
    assert_eq!(
        out.dropped_fields,
        KindCounts::default(),
        "the fixture model carries no extras"
    );
    assert!(out.warnings.iter().any(|text| text.contains("BEHAVIOR")));
    assert!(
        !out.warnings.iter().any(|text| text.contains("line id")),
        "store_line_ids never fires for a Doom target: {:?}",
        out.warnings
    );
    assert_eq!(carried(&map), Vec::<String>::new(), "every block lump went");
    assert_eq!(map.format, MapFormat::Doom, "the layout moved");
    assert_eq!(
        cycle(&map).format,
        MapFormat::Doom,
        "and reads back as Doom"
    );
}

#[test]
fn a_hexen_map_converts_to_a_doom_family_udmf_namespace() {
    let mut map = hexen_map();
    let out = convert_map(&mut map, &udmf("heretic")).expect("UDMF keeps the scripts");
    assert_eq!(out.translated_specials, 0, "no line needed the table");
    let portal = map.linedefs.get(2).expect("the identified line");
    assert_eq!(portal.special, 0_i32, "the binary fold already ran");
    assert_eq!(
        portal.tag, 7_i32,
        "the id rides on as the Doom family line id"
    );
    assert_eq!(
        portal.args, [0_i32; 5],
        "and the 121 fold never fired again"
    );
    assert_eq!(map.format, MapFormat::Udmf, "the layout moved");
    assert_eq!(
        map.udmf_namespace.as_deref(),
        Some("heretic"),
        "best-effort spelling"
    );
    assert_eq!(
        carried(&map),
        vec!["BEHAVIOR", "SCRIPTS"],
        "the script lumps ride along, the node lumps do not"
    );
    let read_back = cycle(&map);
    assert_eq!(
        read_back.format,
        MapFormat::Udmf,
        "the block reads back as UDMF"
    );
    assert_eq!(
        read_back.udmf_namespace.as_deref(),
        Some("heretic"),
        "and keeps the namespace"
    );
    let folded = read_back.linedefs.get(2).expect("the identified line");
    assert_eq!(folded.special, 0_i32, "the line reads back plain");
    assert_eq!(
        folded.tag, 7_i32,
        "with its id where the Doom family keeps it"
    );
}

#[test]
fn a_set_identification_from_a_udmf_source_is_untranslated() {
    let body = concat!(
        "vertex { x = 0; y = 0; } sidedef { sector = 0; }",
        "sector { texturefloor = \"FLOOR1\"; textureceiling = \"CEIL1\"; }",
        "linedef { v1 = 0; v2 = 0; sidefront = 0; id = 7; special = 121; }",
    );
    let mut map = zdoom_udmf(body);
    let out = convert_map(&mut map, &binary("doom")).expect("121 has no row, not a refusal");
    assert_eq!(
        out.untranslated,
        vec![Untranslated {
            linedef: 0,
            special: 121
        }],
        "the literal 121 lands among the unmatched"
    );
    assert_eq!(out.translated_specials, 0, "and nothing translated");
    let line = map.linedefs.first().expect("the line");
    assert_eq!(line.special, 0_i32, "the special clears to 0");
    assert_eq!(line.tag, 7_i32, "the id is kept as the Doom trigger tag");
    assert!(
        out.warnings
            .iter()
            .any(|note| note.contains("no Doom translation")),
        "the grouped miss names it: {:?}",
        out.warnings
    );
    encodes(&map).expect("tag 7 fits a Doom i16 and writes");
}

#[test]
fn a_restored_tag_beyond_the_doom_field_is_the_writer_refusal() {
    let body = concat!(
        "vertex { x = 0; y = 0; } sidedef { sector = 0; }",
        "sector { texturefloor = \"FLOOR1\"; textureceiling = \"CEIL1\"; }",
        "linedef { v1 = 0; v2 = 0; sidefront = 0; id = 70000; special = 121; }",
    );
    let mut map = zdoom_udmf(body);
    let out = convert_map(&mut map, &binary("doom")).expect("the conversion does not clamp");
    assert_eq!(out.untranslated.first().expect("one miss").special, 121_i32);
    assert_eq!(map.linedefs.first().expect("the line").tag, 70000_i32);
    let CyberError::LimitExceeded(message) = encodes(&map).expect_err("70000 is no u16") else {
        panic!("expected LimitExceeded for a wide restored tag");
    };
    assert!(
        message.contains("linedef 0 tag 70000"),
        "the writer names the restored tag: {message}"
    );
}

#[test]
fn untranslated_lines_are_listed_up_to_the_limit_then_counted() {
    let mut body = concat!(
        "vertex { x = 0; y = 0; } sidedef { sector = 0; }",
        "sector { texturefloor = \"FLOOR1\"; textureceiling = \"CEIL1\"; }",
    )
    .to_owned();
    let total = UNTRANSLATED_LIMIT.saturating_add(5_usize);
    for _ in 0..total {
        body.push_str("linedef { v1 = 0; v2 = 0; sidefront = 0; special = 560; }");
    }
    let mut map = zdoom_udmf(&body);
    let out = convert_map(&mut map, &binary("doom")).expect("560 translates to nothing");
    assert_eq!(
        out.untranslated.len(),
        UNTRANSLATED_LIMIT,
        "the list is capped at the documented limit"
    );
    assert_eq!(
        out.untranslated.first().expect("one entry").special,
        560_i32,
        "the first lines are named"
    );
    assert!(
        out.warnings
            .iter()
            .any(|note| note.contains("5 further untranslated linedefs are not listed")),
        "the omitted count is warned: {:?}",
        out.warnings
    );
    assert_eq!(out.translated_specials, 0, "nothing made the trip");
}
