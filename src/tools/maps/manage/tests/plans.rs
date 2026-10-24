//! Tests for the management machinery's pure half in [`super::super`]:
//! the format, namespace, name and plan validators, the `GL` marker plan,
//! the empty-block encoder, the copied block and the doomed positions.

use super::super::{
    GlPlan, copied_block, doomed_positions, empty_block, gl_marker, gl_plan, is_loadable_name,
    name_warning, plan_copy, plan_create,
};
use super::{
    base_session, carrying, code, copy_args, copy_plan, create_args, layer_map, marker, name,
};
use crate::session::Scope;
use crate::tools::maps::common::directory;
use crate::tools::maps::{parse_format, resolve_namespace};
use crate::wad::classify::{MapFormat, analyze};

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
fn gl_markers_fit_short_names_only() {
    assert_eq!(
        gl_marker(name("MAP07")).expect("five fit"),
        name("GL_MAP07")
    );
    assert_eq!(gl_marker(name("E1M1")).expect("four fit"), name("GL_E1M1"));
    assert_eq!(gl_marker(name("LONGNAME")), None, "GL_ plus eight is over");
    assert_eq!(
        gl_plan(name("GL_MAP01"), name("MAP09")),
        GlPlan::Rename(name("GL_MAP09"))
    );
    assert_eq!(gl_plan(name("GL_LEVEL"), name("MAP09")), GlPlan::Keep);
    assert_eq!(gl_plan(name("GL_MAP01"), name("LONGNAME")), GlPlan::Drop);
}

#[test]
fn empty_blocks_match_the_documented_member_sets() {
    let doom = empty_block(name("MAP05"), MapFormat::Doom, None).expect("doom writes");
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
    let hexen = empty_block(name("MAP05"), MapFormat::Hexen, None).expect("hexen writes");
    assert_eq!(
        analyze(&hexen).maps.first().expect("one block").format,
        MapFormat::Hexen,
        "the generated BEHAVIOR marks it Hexen",
    );
    let udmf = empty_block(name("MAP05"), MapFormat::Udmf, Some("doom")).expect("udmf writes");
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
fn a_copied_block_renames_header_and_gl_marker() {
    let session = base_session(layer_map());
    let dir = directory(&session, Scope::Merged).expect("the view reads");
    let block = dir
        .find_block(name("MAP01"))
        .expect("the layer map")
        .clone();
    let copies = copied_block(&dir, &block, name("MAP09"));
    let names: Vec<String> = copies.iter().map(|lump| lump.name.to_string()).collect();
    assert_eq!(
        names,
        vec!["MAP09", "THINGS", "LINEDEFS", "GL_MAP09", "GL_VERT"],
        "header and marker renamed, member and GL bytes kept",
    );
    let things = copies.get(1).expect("THINGS").data.as_ref().to_vec();
    assert_eq!(String::from_utf8(things).expect("text payload"), "things");
    let dropped = copied_block(&dir, &block, name("LONGNAME"));
    let dropped_names: Vec<String> = dropped.iter().map(|lump| lump.name.to_string()).collect();
    assert_eq!(
        dropped_names,
        vec!["LONGNAME", "THINGS", "LINEDEFS"],
        "the GL run goes when the marker would not fit",
    );
}

#[test]
fn a_copied_gl_level_marker_stays_gl_level() {
    let session = base_session(vec![
        marker("E1M1"),
        carrying("THINGS", "t"),
        marker("GL_LEVEL"),
        carrying("GL_VERT", "g"),
    ]);
    let dir = directory(&session, Scope::Merged).expect("the view reads");
    let block = dir.find_block(name("E1M1")).expect("the block").clone();
    let copies = copied_block(&dir, &block, name("E2M1"));
    let names: Vec<String> = copies.iter().map(|lump| lump.name.to_string()).collect();
    assert_eq!(
        names,
        vec!["E2M1", "THINGS", "GL_LEVEL", "GL_VERT"],
        "GL_LEVEL keeps its name and attaches to the copy above it",
    );
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
    assert_eq!(plan.warnings.len(), 1, "the plan carries the warning");
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
