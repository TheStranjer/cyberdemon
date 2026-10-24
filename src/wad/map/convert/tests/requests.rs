//! What a request may ask for: every cross-family target converts instead
//! of refusing, a request naming the layout and namespace the map is
//! already in is an `invalid_argument` that changes nothing, a namespace
//! change inside one family rewrites only the declaration, and [`Target`]
//! validates its format word and namespace shape. The last check reads the
//! documented [`FlagFamily`](crate::wad::map::flags::FlagFamily) of each
//! layout, the wire spelling of [`format_name`] and the exact set of node
//! lumps [`is_node_lump`] names.

use super::{binary, carried, hexen_map, refusal, udmf, udmf_default, udmf_map};
use crate::wad::classify::MapFormat;
use crate::wad::map::common::fixtures::two_room_map;
use crate::wad::map::convert::layout::family_of;
use crate::wad::map::convert::{Conversion, Target, convert_map, format_name, is_node_lump};
use crate::wad::map::flags::FlagFamily;

#[test]
fn cross_family_requests_refuse_nothing() {
    let cases = [
        (two_room_map(), binary("hexen")),
        (two_room_map(), udmf("zdoom")),
        (hexen_map(), binary("doom")),
        (udmf_map("zdoom"), binary("doom")),
        (hexen_map(), udmf("heretic")),
    ];
    for (map, target) in cases {
        let mut moved = map.clone();
        let before = map.clone();
        let out = convert_map(&mut moved, &target)
            .unwrap_or_else(|error| panic!("{target:?} now converts: {error}"));
        assert_ne!(moved, before, "{target:?} changes the map");
        assert_eq!(out.thing_flags_changed, 2, "every fixture thing moves");
    }
}

#[test]
fn a_request_naming_the_layout_in_hand_is_invalid_argument() {
    let same_binary = refusal(&two_room_map(), &binary("doom"));
    assert_eq!(
        same_binary.0, "invalid_argument",
        "doom to doom changes nothing"
    );
    assert!(
        same_binary
            .1
            .contains("MAP01 already is a doom map: nothing to convert"),
        "the map and its layout are named: {}",
        same_binary.1
    );
    let same_hexen = refusal(&hexen_map(), &binary("hexen"));
    assert_eq!(
        same_hexen.0, "invalid_argument",
        "hexen to hexen changes nothing"
    );
    let same_udmf = refusal(&udmf_map("zdoom"), &udmf_default());
    assert_eq!(
        same_udmf.0, "invalid_argument",
        "the family default names the namespace in hand"
    );
    assert!(
        same_udmf.1.contains("udmf (zdoom)"),
        "the namespace is part of the label: {}",
        same_udmf.1
    );
    let spelled = refusal(&udmf_map("zdoom"), &udmf("ZDoom"));
    assert_eq!(
        spelled.0, "invalid_argument",
        "namespaces match case-insensitively"
    );
    let mut moved = udmf_map("heretic");
    convert_map(&mut moved, &udmf_default()).expect("heretic to the family default converts");
    assert_eq!(
        moved.udmf_namespace.as_deref(),
        Some("doom"),
        "a Doom family source rewrites the odd namespace to the family default"
    );
}

#[test]
fn a_refused_conversion_changes_nothing() {
    let mut refused = two_room_map();
    let before = refused.clone();
    let error = convert_map(&mut refused, &binary("doom")).expect_err("the layout in hand");
    assert_eq!(error.code(), "invalid_argument", "same-layout code");
    assert_eq!(refused, before, "a refused conversion changes nothing");
}

#[test]
fn a_namespace_change_inside_one_family_converts_the_declaration_only() {
    let original = udmf_map("zdoom");
    let mut renamed = original.clone();
    let out = convert_map(&mut renamed, &udmf("gzdoom")).expect("zdoom to gzdoom converts");
    assert_eq!(
        out,
        Conversion::default(),
        "UDMF keeps every field it wrote"
    );
    assert_eq!(
        renamed.udmf_namespace.as_deref(),
        Some("gzdoom"),
        "the namespace moved"
    );
    assert_eq!(renamed.format, MapFormat::Udmf, "and the layout stayed");
    assert_eq!(renamed.things, original.things, "the elements stand");
    assert_eq!(renamed.linedefs, original.linedefs, "all of them");
    assert_eq!(
        carried(&renamed),
        vec!["BEHAVIOR"],
        "only the node lumps left"
    );
}

#[test]
fn targets_validate_their_words_and_namespace_shape() {
    assert_eq!(binary("Doom").format, MapFormat::Doom, "any case");
    assert_eq!(
        binary("hexen").namespace,
        None,
        "a binary target declares nothing"
    );
    let target = udmf("GZDoom");
    assert_eq!(target.format, MapFormat::Udmf, "udmf");
    assert_eq!(
        target.namespace.as_deref(),
        Some("gzdoom"),
        "and lowercases"
    );
    for bad in ["zmap", "", "doom2"] {
        let error = Target::parse(bad, None).expect_err("not a format");
        assert_eq!(error.code(), "invalid_argument", "unknown format code");
        assert!(
            error.to_string().contains("\"udmf\""),
            "the message lists the choices: {error}"
        );
    }
    let on_binary = Target::parse("hexen", Some("zdoom")).expect_err("a binary format has none");
    assert_eq!(
        on_binary.code(),
        "invalid_argument",
        "a namespace on a binary target is refused"
    );
    assert!(
        on_binary.to_string().contains("only applies to UDMF"),
        "the fix is named: {on_binary}"
    );
    for bad in ["", "has space", &"x".repeat(33)] {
        let error = Target::parse("udmf", Some(bad)).expect_err("not a namespace");
        assert_eq!(error.code(), "invalid_argument", "bad namespace code");
        assert!(
            error.to_string().contains("32 ASCII"),
            "the shape is named: {error}"
        );
    }
}

#[test]
fn families_names_and_node_lumps_are_the_documented_ones() {
    assert_eq!(
        family_of(MapFormat::Doom, Some("zdoom")),
        FlagFamily::Doom,
        "a namespace means nothing to a binary target"
    );
    assert_eq!(
        family_of(MapFormat::Hexen, None),
        FlagFamily::Hexen,
        "binary Hexen"
    );
    assert_eq!(
        family_of(MapFormat::Udmf, Some("heretic")),
        FlagFamily::Doom,
        "heretic is a Doom family namespace"
    );
    assert_eq!(
        family_of(MapFormat::Udmf, None),
        FlagFamily::Hexen,
        "an unset namespace writes as zdoom"
    );
    assert_eq!(format_name(MapFormat::Udmf), "udmf", "the wire spelling");
    for lump_name in ["SEGS", "SSECTORS", "NODES", "REJECT", "BLOCKMAP", "ZNODES"] {
        assert!(
            is_node_lump(lump_name.as_bytes()),
            "{lump_name} is node data"
        );
    }
    for lump_name in ["THINGS", "BEHAVIOR", "SCRIPTS", "TEXTMAP", "ENDMAP"] {
        assert!(!is_node_lump(lump_name.as_bytes()), "{lump_name} is not");
    }
}
