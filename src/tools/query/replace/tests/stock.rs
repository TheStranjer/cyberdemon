//! Checking existence: the merged stock's namespaces and the empty
//! stock's sky, `require_target` around the clearing dash, `parse_terms`
//! over the pure passes' parsers, `replace_thing_type`'s
//! doomednum-or-name side and the folded `run_replacement` reports.

use serde_json::{from_value, json};

use crate::session::Session;
use crate::tools::query::replace::{
    Stock, ThingRef, parse_terms, require_defined_thing, require_target, resolve_thing_ref,
    run_replacement, thing_definitions,
};
use crate::wad::map::bulk::{Face, Surface, replace_textures};

use super::{all_maps, base_session, blocks, replacement, stocked_session};

#[test]
fn stock_answers_from_the_merged_namespaces_and_refuses_the_rest() {
    let session = stocked_session();
    let stock = Stock::load(&session).expect("the merged view reads");
    assert!(stock.has_flat("FLOOR4"), "the flats namespace");
    assert!(stock.has_flat("F_SKY1"), "F_SKY1 is always available");
    assert!(stock.has_wall("WALL1"), "the TX_ lump names a wall");
    assert!(!stock.has_wall("TX_WALL1"), "not under its lump name");
    assert!(!stock.has_flat("NUKAGE1"), "unknown flats stay unknown");
    stock.require_wall("WALL1").expect("WALL1 exists");
    stock.require_flat("FLOOR4").expect("FLOOR4 exists");
    let missing_wall = stock.require_wall("BRICK1").expect_err("no such composite");
    assert_eq!(missing_wall.code(), "not_found", "missing wall code");
    assert!(
        missing_wall.to_string().contains("list_textures"),
        "the fix is named: {missing_wall}"
    );
    let missing_flat = stock.require_flat("GRASS1").expect_err("no such flat");
    assert_eq!(missing_flat.code(), "not_found", "missing flat code");
    let message = missing_flat.to_string();
    assert!(
        message.contains("list_flats") && message.contains("import_image_as_flat"),
        "both fixes are named: {message}"
    );
}

#[test]
fn an_empty_stock_still_offers_the_sky() {
    let session = Session::default();
    let stock = Stock::load(&session).expect("an empty view reads");
    assert!(stock.walls.is_empty(), "no textures exist yet");
    assert!(stock.has_flat("F_SKY1"), "F_SKY1 is always available");
}

#[test]
fn parse_terms_folds_the_list_and_refuses_a_stray_name() {
    let faces_given = vec!["upper".to_owned(), "MIDDLE".to_owned()];
    let faces = parse_terms(Some(&faces_given), Face::parse).expect("two face names");
    assert_eq!(faces, vec![Face::Upper, Face::Middle], "parsed in order");
    let surfaces_given = vec!["ceiling".to_owned()];
    let surfaces = parse_terms(Some(&surfaces_given), Surface::parse).expect("one surface");
    assert_eq!(surfaces, vec![Surface::Ceiling], "case folds");
    assert!(
        parse_terms(None, Face::parse)
            .expect("absent is the tools' default")
            .is_empty(),
        "an absent list stays empty for the pure pass to widen"
    );
    let bad = parse_terms(Some(&faces_given), Surface::parse).expect_err("faces are not surfaces");
    assert_eq!(bad.code(), "invalid_argument", "stray name code");
    let message = bad.to_string();
    assert!(
        message.contains("floor") && message.contains("ceiling"),
        "the refusal names the accepted values: {message}"
    );
}

#[test]
fn require_target_uppercases_and_checks_the_stock_around_the_clearing_dash() {
    let session = stocked_session();
    assert_eq!(
        require_target(&session, "wall1", true, Stock::require_wall).expect("the TX_ wall"),
        "WALL1",
        "a name the stock holds comes back uppercased"
    );
    assert_eq!(
        require_target(&session, "-", true, Stock::require_wall).expect("clearing skips the stock"),
        "-",
        "\"-\" is the clearing target a wall replacement accepts"
    );
    assert_eq!(
        require_target(&session, "floor4", false, Stock::require_flat).expect("a flat"),
        "FLOOR4",
        "flats check the flats namespace"
    );
    let flat_dash = require_target(&session, "-", false, Stock::require_flat)
        .expect_err("no sector may wear a dash");
    assert_eq!(flat_dash.code(), "not_found", "a flat \"-\" must exist");
    let missing =
        require_target(&session, "NOSUCH", true, Stock::require_wall).expect_err("no such wall");
    assert_eq!(missing.code(), "not_found", "missing target code");
    assert!(
        missing.to_string().contains("list_textures"),
        "the fix is named: {missing}"
    );
    let spaced = require_target(&session, "bad name", true, Stock::require_wall)
        .expect_err("not a name at all");
    assert_eq!(spaced.code(), "invalid_argument", "unvalidatable code");
}

#[test]
fn thing_ref_reads_a_doomednum_or_a_name_from_the_wire() {
    let numbered: ThingRef = from_value(json!(3001_i64)).expect("a doomednum");
    assert!(
        matches!(numbered, ThingRef::Number(3001_u32)),
        "a number binds Number: {numbered:?}"
    );
    let named: ThingRef = from_value(json!("imp")).expect("a name");
    assert!(
        matches!(&named, ThingRef::Name(text) if text == "imp"),
        "a string binds Name: {named:?}"
    );
}

#[test]
fn resolve_thing_ref_passes_numbers_and_resolves_names() {
    let session = Session::default();
    assert_eq!(
        resolve_thing_ref(&session, &ThingRef::Number(3001_u32)).expect("a doomednum"),
        3001_i32,
        "numbers pass through without touching the definitions"
    );
    assert_eq!(
        resolve_thing_ref(&session, &ThingRef::Name("imp".to_owned())).expect("the exact name"),
        3001_i32,
        "the exact name Imp wins over the Dead Imp substring"
    );
    assert_eq!(
        resolve_thing_ref(&session, &ThingRef::Name("demon".to_owned()))
            .expect("the exact name beats its substrings"),
        3002_i32,
        "Demon names 3002, not the CyberDemon (16) or Cacodemon it is a substring of"
    );
    let huge = resolve_thing_ref(&session, &ThingRef::Number(u32::MAX))
        .expect_err("above the signed range");
    assert_eq!(huge.code(), "invalid_argument", "range refusal code");
    assert!(
        huge.to_string().contains("32-bit"),
        "the message names the range: {huge}"
    );
}

#[test]
fn resolve_thing_ref_refuses_unknown_and_ambiguous_names() {
    let session = Session::default();
    let unknown = resolve_thing_ref(&session, &ThingRef::Name("not-a-thing".to_owned()))
        .expect_err("nothing carries that name");
    assert_eq!(unknown.code(), "invalid_argument", "unknown name code");
    assert!(
        unknown.to_string().contains("get_thing_types"),
        "the fix is named: {unknown}"
    );
    let ambiguous = resolve_thing_ref(&session, &ThingRef::Name("emon".to_owned()))
        .expect_err("several types share that substring");
    assert_eq!(ambiguous.code(), "invalid_argument", "ambiguous name code");
    let message = ambiguous.to_string();
    assert!(
        message.contains("Demon")
            && message.contains("Cacodemon")
            && message.contains("CyberDemon"),
        "the candidate names are listed: {message}"
    );
}

#[test]
fn require_defined_thing_checks_the_session_tables_and_names_the_escape() {
    let definitions = thing_definitions(&Session::default()).expect("the default tables read");
    require_defined_thing(&definitions, 3002_i32).expect("the Demon is defined");
    require_defined_thing(&definitions, 1_i32).expect("the player start is defined");
    let missing = require_defined_thing(&definitions, 99999_i32).expect_err("nothing here");
    assert_eq!(missing.code(), "invalid_argument", "undefined target code");
    let message = missing.to_string();
    assert!(
        message.contains("get_thing_types") && message.contains("allow_unknown"),
        "the reference and the escape hatch are both named: {message}"
    );
    let negative = require_defined_thing(&definitions, -1_i32).expect_err("no type is negative");
    assert_eq!(negative.code(), "invalid_argument", "negative target code");
}

#[test]
fn run_replacement_previews_the_folded_counts_without_writing() {
    let mut session = base_session(blocks(&["MAP01", "MAP02"]));
    let selector = all_maps();
    let preview = run_replacement(&mut session, &replacement(&selector, true), |map| {
        replace_textures(map, "WALL*", "STEP1", &[])
    })
    .expect("the preview runs");
    let shown = serde_json::to_value(&preview).expect("the report serializes");
    assert_eq!(
        shown["maps"],
        json!({"MAP01": 1_u64, "MAP02": 1_u64}),
        "one WALL64 middle per map"
    );
    assert_eq!(
        shown["total"],
        json!(2_u64),
        "the sum of the per-map counts"
    );
    assert_eq!(shown["dry_run"], json!(true), "the flag travels");
    assert!(
        shown["copied_maps"].as_array().expect("a list").is_empty(),
        "a preview copies nothing"
    );
    assert!(
        session.undo_labels().is_empty(),
        "and records no history entry"
    );
}

#[test]
fn run_replacement_writes_one_labeled_step_and_reports_the_copies() {
    let mut session = base_session(blocks(&["MAP01", "MAP02"]));
    let selector = all_maps();
    let done = run_replacement(&mut session, &replacement(&selector, false), |map| {
        replace_textures(map, "WALL*", "STEP1", &[])
    })
    .expect("the step runs");
    assert_eq!(done.total, 2_usize, "both middles moved");
    assert_eq!(
        done.copied_maps,
        vec!["MAP01".to_owned(), "MAP02".to_owned()],
        "both maps came from the layer alone"
    );
    assert_eq!(
        session.undo_labels(),
        vec!["replace_texture WALL* -> STEP1"],
        "one step for two maps"
    );
}

#[test]
fn run_replacement_reports_a_glob_that_matches_nothing() {
    let mut session = base_session(blocks(&["MAP01", "MAP02"]));
    let selector = all_maps();
    let zero = run_replacement(&mut session, &replacement(&selector, false), |map| {
        replace_textures(map, "NOPE*", "STEP1", &[])
    })
    .expect("a miss is a normal run");
    assert_eq!(zero.total, 0_usize, "nothing matched");
    assert_eq!(
        serde_json::to_value(&zero).expect("serializable")["maps"],
        json!({"MAP01": 0_u64, "MAP02": 0_u64}),
        "every selected map still holds an entry"
    );
    assert_eq!(
        session.undo_labels().len(),
        1_usize,
        "the no-change step is recorded like any other"
    );
}
