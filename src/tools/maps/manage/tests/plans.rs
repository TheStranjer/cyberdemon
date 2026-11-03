//! Tests for the management machinery's pure half in [`super::super`]:
//! the format, namespace, name and plan validators, the `GL` run copies,
//! the empty-block encoder, the copied block and the doomed positions.

use super::super::{
    CreateStart, copied_block, doomed_positions, empty_block, gl_copies, is_loadable_name,
    name_warning, plan_copy, plan_create,
};
use super::{
    base_session, carrying, code, copy_args, copy_plan, create_args, layer_map, marker, name,
    start_at, started_args,
};
use crate::session::Scope;
use crate::tools::edit_things::default_bits;
use crate::tools::maps::common::{Directory, directory};
use crate::tools::maps::{parse_format, resolve_namespace};
use crate::wad::archive::Lump;
use crate::wad::classify::{MapFormat, analyze};
use crate::wad::map::consts::{COORD_MAX, PLAYER1_START};
use crate::wad::map::{FlagFamily, Thing, read_map};

#[test]
fn parse_format_defaults_to_doom_and_lists_the_choices() {
    assert_eq!(parse_format(None).expect("absent"), MapFormat::Doom);
    assert_eq!(parse_format(Some("Hexen")).expect("case"), MapFormat::Hexen);
    assert_eq!(parse_format(Some("udmf")).expect("lower"), MapFormat::Udmf);
    let error = parse_format(Some("zdoom")).expect_err("not a format");
    assert_eq!(code(&error), "invalid_argument", "bad format code");
    assert!(
        error.to_string().contains("doom") && error.to_string().contains("udmf"),
        "the message lists the choices: {error}"
    );
}

#[test]
fn namespaces_default_validate_and_refuse_binaries() {
    assert_eq!(
        resolve_namespace(None, MapFormat::Udmf).expect("default"),
        Some("zdoom".to_owned()),
        "UDMF without a namespace gets the default",
    );
    assert_eq!(
        resolve_namespace(Some("GLBDOOM"), MapFormat::Udmf).expect("lowered"),
        Some("glbdoom".to_owned()),
        "names are stored lowercase",
    );
    assert_eq!(
        resolve_namespace(None, MapFormat::Doom).expect("binary default"),
        None,
        "binary maps declare no namespace",
    );
    let clash = resolve_namespace(Some("zdoom"), MapFormat::Hexen).expect_err("binary ns");
    assert_eq!(code(&clash), "invalid_argument", "binary namespace code");
    assert!(
        clash.to_string().contains("UDMF"),
        "the message names the format: {clash}"
    );
    let bad = resolve_namespace(Some("not a ns"), MapFormat::Udmf).expect_err("bad name");
    assert_eq!(code(&bad), "invalid_argument", "bad namespace code");
}

#[test]
fn loadable_names_follow_the_two_engine_forms() {
    for text in ["MAP01", "map09", "MAP99", "E1M1", "E9M9"] {
        assert!(is_loadable_name(name(text)), "{text} is loadable");
    }
    for text in ["E12M3", "MAP0A", "HOUR01", "SLOTPAD", "E1N1"] {
        assert!(!is_loadable_name(name(text)), "{text} is not");
    }
}

#[test]
fn odd_names_warn_with_the_engine_rule() {
    assert!(name_warning(name("MAP07")).is_empty(), "MAPxx is quiet");
    assert!(name_warning(name("E2M7")).is_empty(), "ExMy is quiet");
    let warnings = name_warning(name("SLOTPAD"));
    assert_eq!(warnings.len(), 1, "the odd name warns once");
    assert!(
        warnings[0].contains("SLOTPAD")
            && warnings[0].contains("ExMy")
            && warnings[0].contains("MAPINFO"),
        "the warning names the map and the fix: {}",
        warnings[0],
    );
}

#[test]
fn gl_copies_rename_the_head_and_warn_when_the_marker_is_gone() {
    let session = base_session(layer_map());
    let dir = directory(&session, Scope::Merged).expect("the view reads");
    let mut warnings = Vec::new();
    let copies = gl_copies(&dir, (4, 6), name("MAP09"), &mut warnings);
    let names: Vec<String> = copies.iter().map(|lump| lump.name.to_string()).collect();
    assert_eq!(
        names,
        vec!["GL_MAP09", "GL_VERT"],
        "the head follows the name"
    );
    assert!(warnings.is_empty(), "nothing was dropped: {warnings:?}");
    let long = gl_copies(&dir, (4, 6), name("LONGMAP"), &mut warnings);
    let long_names: Vec<String> = long.iter().map(|lump| lump.name.to_string()).collect();
    assert_eq!(
        long_names,
        vec!["GL_LEVEL", "GL_VERT"],
        "the shared fallback"
    );
    assert!(
        warnings.is_empty(),
        "a fallback is not a drop: {warnings:?}"
    );
    let stale = Directory {
        entries: Vec::new(),
        lumps: Vec::new(),
        analysis: analyze(&[]),
    };
    let dropped = gl_copies(&stale, (2, 5), name("LONGMAP"), &mut warnings);
    assert!(dropped.is_empty(), "there was nothing to copy");
    assert_eq!(warnings.len(), 1, "the drop is reported: {warnings:?}");
    assert!(
        warnings[0].contains("2..5") && warnings[0].contains("build_nodes"),
        "the warning names the run and the fix: {}",
        warnings[0],
    );
}

#[test]
fn empty_blocks_match_the_documented_member_sets() {
    let doom = empty_block(name("MAP05"), MapFormat::Doom, None, None).expect("doom writes");
    let doom_names: Vec<String> = doom.iter().map(|lump| lump.name.to_string()).collect();
    assert_eq!(
        doom_names,
        vec![
            "MAP05", "THINGS", "LINEDEFS", "SIDEDEFS", "VERTEXES", "SEGS", "SSECTORS", "NODES",
            "SECTORS", "REJECT", "BLOCKMAP",
        ],
        "header plus the ten vanilla members",
    );
    assert!(
        doom.iter().all(|lump| lump.size() == 0),
        "every member is empty"
    );
    let hexen = empty_block(name("MAP05"), MapFormat::Hexen, None, None).expect("hexen writes");
    assert_eq!(
        analyze(&hexen).maps.first().expect("one block").format,
        MapFormat::Hexen,
        "the generated BEHAVIOR marks it Hexen",
    );
    let udmf =
        empty_block(name("MAP05"), MapFormat::Udmf, Some("doom"), None).expect("udmf writes");
    let udmf_names: Vec<String> = udmf.iter().map(|lump| lump.name.to_string()).collect();
    assert_eq!(
        udmf_names,
        vec!["MAP05", "TEXTMAP", "ENDMAP"],
        "the text block"
    );
    assert_eq!(
        analyze(&udmf).maps.first().expect("one block").format,
        MapFormat::Udmf
    );
    let body = udmf.get(1).expect("TEXTMAP").data.as_ref().to_vec();
    assert_eq!(
        String::from_utf8(body).expect("UDMF is text").trim(),
        "namespace = \"doom\";",
        "the TEXTMAP declares only the namespace",
    );
}

#[test]
fn a_fresh_plan_without_a_start_says_nobody_can_spawn() {
    let bare = plan_create(&create_args("MAP05", None, None)).expect("valid plan");
    assert_eq!(bare.start, None, "no start was asked for");
    let warning = bare
        .warnings
        .iter()
        .find(|text| text.contains("player 1 start"))
        .expect("an empty block says its player 1 start is missing");
    assert!(
        warning.contains("type 1") && warning.contains("add_thing") && warning.contains("spawn"),
        "the notice names the number, the tool and what is wrong: {warning}"
    );
    let started = plan_create(&started_args("MAP05", None, start_at(64, -128, None)))
        .expect("a start is a valid plan");
    assert_eq!(
        started.start,
        Some(CreateStart {
            x: 64_i32,
            y: -128_i32,
            angle: 0_i32,
        }),
        "the point travels, facing east",
    );
    assert!(
        started.warnings.is_empty(),
        "a loadable name with a start deserves no notice: {:?}",
        started.warnings,
    );
}

#[test]
fn a_start_facing_wraps_and_binary_coordinates_are_checked() {
    let spun = plan_create(&started_args(
        "MAP05",
        None,
        start_at(0_i32, 0_i32, Some(370_i32)),
    ))
    .expect("a facing past a full turn wraps");
    assert_eq!(
        spun.start.map(|spawn| spawn.angle),
        Some(10_i32),
        "370 is 10"
    );
    let back = plan_create(&started_args(
        "MAP05",
        None,
        start_at(0_i32, 0_i32, Some(-90_i32)),
    ))
    .expect("a negative facing wraps");
    assert_eq!(
        back.start.map(|spawn| spawn.angle),
        Some(270_i32),
        "-90 is 270"
    );
    let over = COORD_MAX.saturating_add(1);
    let error = plan_create(&started_args("MAP05", None, start_at(over, 0_i32, None)))
        .expect_err("a binary THINGS x column is i16");
    assert_eq!(code(&error), "invalid_argument", "refused before any write");
    assert!(
        error.to_string().contains('x'),
        "the message names the field: {error}"
    );
    let wide = plan_create(&started_args(
        "MAP05",
        Some("udmf"),
        start_at(over, 0_i32, None),
    ))
    .expect("UDMF stores full integers");
    assert_eq!(
        wide.start.map(|spawn| spawn.x),
        Some(over),
        "and keeps them"
    );
}

/// The one thing of the block `lumps` holds, decoded back out.
fn sole_thing(lumps: &[Lump]) -> Thing {
    let block = analyze(lumps).maps.first().expect("one block").clone();
    let map = read_map(lumps, &block).expect("the block reads back");
    assert_eq!(map.things.len(), 1, "exactly the one start");
    map.things.first().expect("the start is there").clone()
}

#[test]
fn empty_blocks_stand_the_asked_start_in_things() {
    let plan = plan_create(&started_args(
        "MAP05",
        None,
        start_at(64, -128, Some(450_i32)),
    ))
    .expect("a start is a valid plan");
    let doom = empty_block(name("MAP05"), MapFormat::Doom, None, plan.start).expect("doom writes");
    let spawn = sole_thing(&doom);
    assert_eq!(spawn.kind, PLAYER1_START, "thing type 1");
    assert_eq!(
        (spawn.x, spawn.y, spawn.angle),
        (64_i32, -128_i32, 90_i32),
        "the point and the wrapped facing land",
    );
    assert_eq!(
        spawn.flags,
        default_bits(FlagFamily::Doom),
        "the start spawns at every skill, the way add_thing defaults",
    );
    let hexen = empty_block(name("MAP05"), MapFormat::Hexen, None, plan.start).expect("hexen");
    assert_eq!(
        sole_thing(&hexen).flags,
        default_bits(FlagFamily::Hexen),
        "a Hexen block gets the Hexen family's presence bits",
    );
    let udmf =
        empty_block(name("MAP05"), MapFormat::Udmf, Some("doom"), plan.start).expect("udmf writes");
    let text = sole_thing(&udmf);
    assert_eq!(
        (text.kind, text.x, text.y, text.angle),
        (PLAYER1_START, 64_i32, -128_i32, 90_i32),
        "the TEXTMAP carries the start too",
    );
}

#[test]
fn a_copied_block_renames_header_and_gl_marker() {
    let session = base_session(layer_map());
    let dir = directory(&session, Scope::Merged).expect("the view reads");
    let block = dir
        .find_block(name("MAP01"))
        .expect("the layer map")
        .clone();
    let mut warnings = Vec::new();
    let copies = copied_block(&dir, &block, name("MAP09"), &mut warnings);
    let names: Vec<String> = copies.iter().map(|lump| lump.name.to_string()).collect();
    assert_eq!(
        names,
        vec!["MAP09", "THINGS", "LINEDEFS", "GL_MAP09", "GL_VERT"],
        "header and marker renamed, member and GL bytes kept",
    );
    let things = copies.get(1).expect("THINGS").data.as_ref().to_vec();
    assert_eq!(String::from_utf8(things).expect("text payload"), "things");
    let long = copied_block(&dir, &block, name("LONGNAME"), &mut warnings);
    let long_names: Vec<String> = long.iter().map(|lump| lump.name.to_string()).collect();
    assert_eq!(
        long_names,
        vec!["LONGNAME", "THINGS", "LINEDEFS", "GL_LEVEL", "GL_VERT"],
        "the run comes along under the shared fallback marker",
    );
    assert!(warnings.is_empty(), "nothing was dropped: {warnings:?}");
}

#[test]
fn a_copied_gl_level_marker_follows_the_target_name() {
    let session = base_session(vec![
        marker("E1M1"),
        carrying("THINGS", "t"),
        marker("GL_LEVEL"),
        carrying("GL_VERT", "g"),
    ]);
    let dir = directory(&session, Scope::Merged).expect("the view reads");
    let block = dir.find_block(name("E1M1")).expect("the block").clone();
    let mut warnings = Vec::new();
    let copies = copied_block(&dir, &block, name("E2M1"), &mut warnings);
    let names: Vec<String> = copies.iter().map(|lump| lump.name.to_string()).collect();
    assert_eq!(
        names,
        vec!["E2M1", "THINGS", "GL_E2M1", "GL_VERT"],
        "a fitting name is embedded, so the copy attaches by name",
    );
    let long = copied_block(&dir, &block, name("E2M12345"), &mut warnings);
    let long_names: Vec<String> = long.iter().map(|lump| lump.name.to_string()).collect();
    assert_eq!(
        long_names,
        vec!["E2M12345", "THINGS", "GL_LEVEL", "GL_VERT"],
        "GL_LEVEL stays GL_LEVEL and attaches to the copy above it",
    );
    assert!(warnings.is_empty(), "nothing was dropped: {warnings:?}");
}

#[test]
fn doomed_positions_covers_every_same_named_block_and_gl_run() {
    let session = base_session(vec![
        marker("MAP01"),
        carrying("THINGS", "a"),
        marker("GL_MAP01"),
        carrying("GL_VERT", "g"),
        marker("MAP01"),
        carrying("THINGS", "b"),
        marker("MAP02"),
        carrying("THINGS", "c"),
    ]);
    let dir = directory(&session, Scope::Layer(0)).expect("layer 0 reads");
    assert_eq!(
        doomed_positions(&dir.analysis, name("MAP01")),
        vec![0_usize, 1, 2, 3, 4, 5],
        "both MAP01 blocks and the attached GL run",
    );
    assert_eq!(
        doomed_positions(&dir.analysis, name("MAP02")),
        vec![6_usize, 7]
    );
    assert!(
        doomed_positions(&dir.analysis, name("MAP09")).is_empty(),
        "an unknown map dooms nothing",
    );
}

#[test]
fn plans_validate_names_and_scopes() {
    let bad = plan_create(&create_args("", None, None)).expect_err("empty name");
    assert_eq!(code(&bad), "invalid_argument", "empty name code");
    let ns = plan_create(&create_args("MAP05", Some("doom"), Some("zdoom")))
        .expect_err("namespace on a binary map");
    assert_eq!(code(&ns), "invalid_argument", "namespace clash code");
    let plan = plan_create(&create_args("SLOTPAD", None, None)).expect("odd name");
    assert_eq!(
        plan.warnings.len(),
        2,
        "the odd name and the missing start: {:?}",
        plan.warnings,
    );
    assert!(
        plan.warnings[0].contains("SLOTPAD") && plan.warnings[1].contains("player 1 start"),
        "the name first, then the spawn: {:?}",
        plan.warnings,
    );
    assert_eq!(plan.namespace, None, "binary default");
    let scope =
        plan_copy(&copy_args("MAP01", "MAP02", Some("nope"), false)).expect_err("bad scope");
    assert_eq!(code(&scope), "invalid_argument", "bad scope code");
    let copy = copy_plan("map01", "map02", "working", true);
    assert_eq!(
        (copy.from, copy.to, copy.scope),
        (name("MAP01"), name("MAP02"), Scope::Working),
        "names upper, scope parsed, flag carried",
    );
}
