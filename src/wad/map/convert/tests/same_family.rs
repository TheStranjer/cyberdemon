//! Same-family rewrites: the mechanical tail of [`convert_map`] on a map
//! that keeps its flag family. A `Doom` map round trips through `UDMF`
//! without loss, an unnamed `UDMF` target takes its family's default
//! namespace, a `UDMF` map dropping to binary loses every `extra` field and
//! counts what was a rounded original and loses its carried `UDMF` globals
//! and unknown blocks with a warning naming each (a namespace rewrite keeps
//! them), a binary `Hexen`
//! `Line_SetIdentification` becomes a `UDMF` line id and back, an id binary
//! `Hexen` cannot keep drops with a warning naming the line, and a value no
//! binary record stores is the writer's `limit_exceeded`.

use super::{
    assert_cross_family_keys_are_zero, binary, carried, counts, cycle, doom_udmf, hexen_map, udmf,
    udmf_default, zdoom_udmf,
};
use crate::error::CyberError;
use crate::wad::classify::MapFormat;
use crate::wad::map::common::fixtures::two_room_map;
use crate::wad::map::convert::{Conversion, KindCounts, convert_map, encodes};

/// One line, side, sector and vertex per element kind, each carrying the
/// fields a binary record has no room for.
const ODD_FIELDS: &str = concat!(
    "vertex { x = 64.5; y = 0; }",
    "thing { x = 0; y = 0.25; type = 5; comment = \"hi\"; }",
    "linedef { v1 = 0; v2 = 0; sidefront = 0; alpha = 0.5; }",
    "sidedef { sector = 0; offsetx = 1.5; scalex_top = 2; }",
    "sector { texturefloor = \"FLOOR1\"; textureceiling = \"CEIL1\";",
    " heightfloor = 8.5; lightfloor = 0.5; }",
);

#[test]
fn a_doom_map_round_trips_through_udmf_without_loss() {
    let original = two_room_map();
    let mut as_udmf = original.clone();
    let out = convert_map(&mut as_udmf, &udmf("doom")).expect("the Doom family converts");
    assert_eq!(out, Conversion::default(), "a Doom line needs no note");
    assert_eq!(as_udmf.format, MapFormat::Udmf, "the layout moved");
    assert_eq!(
        as_udmf.udmf_namespace.as_deref(),
        Some("doom"),
        "and declares its own namespace"
    );
    assert!(
        as_udmf.other.is_empty(),
        "the node lumps left: {:?}",
        carried(&as_udmf)
    );
    assert_eq!(
        as_udmf.linedefs, original.linedefs,
        "the lines are untouched"
    );
    assert_eq!(as_udmf.things, original.things, "and the things");
    assert_eq!(cycle(&as_udmf), as_udmf, "the UDMF block round trips");
    let mut back = cycle(&as_udmf);
    convert_map(&mut back, &binary("doom")).expect("and converts back");
    assert_eq!(back.format, MapFormat::Doom, "the layout came home");
    assert_eq!(back.udmf_namespace, None, "and the namespace went");
    assert_eq!(cycle(&back), original, "modulo the empty node markers");
}

#[test]
fn a_same_family_conversion_publishes_the_cross_family_keys_as_zero() {
    let mut as_udmf = two_room_map();
    let out = convert_map(&mut as_udmf, &udmf("doom")).expect("the Doom family converts");
    assert_cross_family_keys_are_zero(&out);
    let mut back = cycle(&as_udmf);
    let again = convert_map(&mut back, &binary("doom")).expect("and converts back");
    assert_cross_family_keys_are_zero(&again);
}

#[test]
fn an_unnamed_udmf_target_takes_the_source_family_default_namespace() {
    let mut from_doom = two_room_map();
    convert_map(&mut from_doom, &udmf_default()).expect("a Doom map converts");
    assert_eq!(
        from_doom.udmf_namespace.as_deref(),
        Some("doom"),
        "a Doom family source defaults to the doom namespace"
    );
    let mut from_hexen = hexen_map();
    convert_map(&mut from_hexen, &udmf_default()).expect("a Hexen map converts");
    assert_eq!(
        from_hexen.udmf_namespace.as_deref(),
        Some("zdoom"),
        "a Hexen family source defaults to zdoom"
    );
}

#[test]
fn a_udmf_doom_map_drops_its_extra_fields_and_counts_them() {
    let mut map = doom_udmf(ODD_FIELDS);
    let out = convert_map(&mut map, &binary("doom")).expect("the Doom family converts back");
    assert_eq!(
        out.dropped_fields,
        counts([2_usize, 1_usize, 2_usize, 2_usize, 1_usize]),
        "every unmodelled field counts, the preserved fraction texts included"
    );
    assert_eq!(
        out.fractional_coordinates,
        counts([1_usize, 0_usize, 1_usize, 1_usize, 1_usize]),
        "only a modelled number's text is a rounded original: `alpha` and \
         `lightfloor` are unmodelled fields that simply drop"
    );
    let thing = map.things.first().expect("one thing");
    assert_eq!(thing.y, 0_i32, "the model kept the rounded integer");
    assert!(thing.extra.is_empty(), "and its bag is empty now");
    assert!(
        map.vertices
            .first()
            .is_some_and(|vertex| vertex.extra.is_empty() && vertex.x == 65_i32),
        "64.5 was already stored as 65 on load"
    );
    assert!(
        out.warnings
            .iter()
            .any(|text| text.contains("BEHAVIOR") && text.contains("binary Doom")),
        "the carried BEHAVIOR cannot stay: {:?}",
        out.warnings
    );
    assert_eq!(
        carried(&map),
        Vec::<String>::new(),
        "no block lump survived"
    );
    assert_eq!(map.udmf_namespace, None, "the namespace declaration went");
    assert_eq!(
        cycle(&map).format,
        MapFormat::Doom,
        "and the block reads as Doom"
    );
}

#[test]
fn carried_globals_and_unknown_blocks_drop_into_binary_with_warnings() {
    let body = concat!(
        "ednum = 5;",
        "vertex { x = 0; y = 0; }",
        "sidedef { sector = 0; }",
        "sector { texturefloor = \"FLOOR1\"; textureceiling = \"CEIL1\"; }",
        "linedef { v1 = 0; v2 = 0; sidefront = 0; }",
        "vertex_slope { vertex = 0; }",
    );
    let mut map = doom_udmf(body);
    let out = convert_map(&mut map, &binary("doom")).expect("the Doom family converts back");
    assert!(
        out.warnings
            .iter()
            .any(|text| text.contains("ednum") && text.contains("dropped")),
        "the dropped global is named: {:?}",
        out.warnings
    );
    assert!(
        out.warnings
            .iter()
            .any(|text| text.contains("vertex_slope") && text.contains("dropped")),
        "the dropped block is named: {:?}",
        out.warnings
    );
    assert!(map.extra_globals.is_empty(), "the globals bag is emptied");
    assert!(map.extra_blocks.is_empty(), "the blocks bag is emptied");
    assert_eq!(
        cycle(&map).format,
        MapFormat::Doom,
        "the block reads as Doom"
    );
}

#[test]
fn a_udmf_namespace_rewrite_keeps_globals_and_unknown_blocks() {
    let body = concat!(
        "ednum = 5;",
        "vertex { x = 0; y = 0; }",
        "sidedef { sector = 0; }",
        "sector { texturefloor = \"FLOOR1\"; textureceiling = \"CEIL1\"; }",
        "linedef { v1 = 0; v2 = 0; sidefront = 0; }",
        "vertex_slope { vertex = 0; }",
    );
    let mut map = zdoom_udmf(body);
    let out = convert_map(&mut map, &udmf("eternity")).expect("the same family rewrites");
    assert_eq!(
        out,
        Conversion::default(),
        "a namespace rewrite notes nothing"
    );
    assert_eq!(map.extra_globals.len(), 1, "the global stayed");
    assert_eq!(map.extra_blocks.len(), 1, "and so did the unknown block");
    assert_eq!(cycle(&map), map, "the rewritten block round trips");
}

#[test]
fn a_hexen_line_identification_becomes_a_udmf_line_id_and_back() {
    let original = hexen_map();
    let mut as_udmf = original.clone();
    let out = convert_map(&mut as_udmf, &udmf("zdoom")).expect("the Hexen family converts");
    assert!(out.warnings.is_empty(), "a storable id needs no note");
    assert_eq!(
        out.dropped_fields,
        KindCounts::default(),
        "UDMF drops nothing"
    );
    let identified = as_udmf.linedefs.get(2).expect("the identified line");
    assert_eq!(identified.tag, 7_i32, "args[0] became the line id");
    assert_eq!(identified.special, 0_i32, "the special went");
    assert_eq!(identified.args, [0_i32; 5], "and its arg with it");
    assert_eq!(
        carried(&as_udmf),
        vec!["BEHAVIOR", "SCRIPTS"],
        "the script lumps ride along, the node lumps do not"
    );
    assert_eq!(cycle(&as_udmf), as_udmf, "the UDMF block round trips");
    let mut back = cycle(&as_udmf);
    let again = convert_map(&mut back, &binary("hexen")).expect("and converts back");
    assert!(
        again.warnings.is_empty(),
        "the id fits a byte and the line is plain"
    );
    let restored = back.linedefs.get(2).expect("the identified line");
    assert_eq!(restored.tag, 0_i32, "Hexen stores no line id");
    assert_eq!(
        restored.special, 121_i32,
        "it became Line_SetIdentification"
    );
    assert_eq!(restored.args.first().copied(), Some(7_i32), "in args[0]");
    assert_eq!(cycle(&back), original, "the Hexen block is what it was");
}

#[test]
fn an_unstorable_line_id_drops_with_a_warning() {
    let body = concat!(
        "vertex { x = 0; y = 0; } sidedef { sector = 0; }",
        "sector { texturefloor = \"FLOOR1\"; textureceiling = \"CEIL1\"; }",
        "linedef { v1 = 0; v2 = 0; sidefront = 0; id = 9; special = 10; }",
        "linedef { v1 = 0; v2 = 0; sidefront = 0; id = 300; }",
        "linedef { v1 = 0; v2 = 0; sidefront = 0; id = 4; }",
    );
    let mut map = zdoom_udmf(body);
    let out = convert_map(&mut map, &binary("hexen")).expect("the Hexen family converts");
    assert_eq!(out.warnings.len(), 2, "two ids had nowhere to go");
    let taken = out.warnings.first().expect("two warnings");
    assert!(
        taken.contains("linedef 0 line id 9") && taken.contains("already uses special 10"),
        "the busy line is named: {taken}"
    );
    let oversized = out.warnings.get(1).expect("two warnings");
    assert!(
        oversized.contains("linedef 1 line id 300") && oversized.contains("one byte"),
        "the oversized id is named: {oversized}"
    );
    let busy = map.linedefs.first().expect("the busy line");
    assert_eq!(busy.tag, 0_i32, "the id left the model either way");
    assert_eq!(busy.special, 10_i32, "and its own special stayed");
    let free = map.linedefs.get(2).expect("the plain line");
    assert_eq!(
        free.special, 121_i32,
        "the storable id became Line_SetIdentification"
    );
    assert_eq!(free.args.first().copied(), Some(4_i32), "in args[0]");
    encodes(&map).expect("what is left writes, and the Hexen writer adds BEHAVIOR");
}

#[test]
fn values_a_binary_hexen_record_cannot_store_are_limit_exceeded() {
    let body = concat!(
        "vertex { x = 0; y = 0; } sidedef { sector = 0; }",
        "sector { texturefloor = \"FLOOR1\"; textureceiling = \"CEIL1\"; }",
        "linedef { v1 = 0; v2 = 0; sidefront = 0; arg1 = 300; }",
    );
    let mut wide_arg = zdoom_udmf(body);
    convert_map(&mut wide_arg, &binary("hexen")).expect("the conversion itself is fine");
    let CyberError::LimitExceeded(message) = encodes(&wide_arg).expect_err("300 is no byte") else {
        panic!("expected LimitExceeded for a wide arg");
    };
    assert!(
        message.contains("linedef 0 args[1] 300 does not fit u8"),
        "the element and field are named: {message}"
    );
    let mut wide_special = zdoom_udmf(&body.replace("arg1 = 300", "special = 300"));
    convert_map(&mut wide_special, &binary("hexen")).expect("a wide special converts");
    let CyberError::LimitExceeded(special_message) =
        encodes(&wide_special).expect_err("300 is no byte")
    else {
        panic!("expected LimitExceeded for a wide special");
    };
    assert!(
        special_message.contains("linedef 0 special 300"),
        "the special is named: {special_message}"
    );
}
