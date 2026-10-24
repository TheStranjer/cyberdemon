//! `replace_lump`: how the selector and the payload fields parse, which
//! working copy a match picks, where an override lands in the directory and
//! what a refusal leaves behind.

use serde_json::json;

use super::{
    carrying, code, loaded, marker, name, names, pwad, replace_args, report_of_flat, seed,
    selector_for, with_payloads,
};
use crate::session::Scope;
use crate::state::AppState;
use crate::tools::lumps::edit::replace::{
    NewData, Replacement, Target, fetch_data, overwrite, pick_new_data, run_replace,
};
use crate::tools::lumps::edit::{edit_selector, in_region, region_text, working_match};
use crate::tools::lumps::rows::build_rows;
use crate::wad::archive::Wad;
use crate::wad::classify::Namespace;

#[test]
fn selectors_parse_names_and_regions_or_refuse() {
    let plain = edit_selector("flat", Some("flats"), Some(2)).expect("valid");
    assert_eq!(plain.name, name("FLAT"), "the name uppercases");
    assert_eq!(plain.namespace, Some(Namespace::Flats), "the region parses");
    assert_eq!(plain.occurrence, Some(2), "the occurrence carries");
    let loose = selector_for("NOTE");
    assert_eq!(loose.namespace, None, "no region means any match");
    assert_eq!(loose.occurrence, None, "no occurrence means the last one");
    let long = edit_selector("TOO LONG FOR WAD", None, None).expect_err("over eight bytes");
    assert_eq!(code(&long), "invalid_argument", "bad name code");
    let region = edit_selector("NOTE", Some("nope"), None).expect_err("not a region");
    assert_eq!(code(&region), "invalid_argument", "bad namespace code");
}

#[test]
fn a_selector_accepts_only_its_own_name_and_region() {
    let rows = build_rows(&loaded(), Scope::Merged).expect("the view lists");
    let flat = rows
        .iter()
        .find(|row| row.name == name("FLATA"))
        .expect("the base flat");
    let palette = rows
        .iter()
        .find(|row| row.name == name("PLAYPAL"))
        .expect("the base palette");
    assert!(
        edit_selector("flata", Some("flats"), None)
            .expect("valid")
            .accepts(flat),
        "the region copy matches its own selector"
    );
    assert!(
        !selector_for("PLAYPAL").accepts(flat),
        "another name never matches"
    );
    assert!(
        !edit_selector("FLATA", Some("sprites"), None)
            .expect("valid")
            .accepts(flat),
        "the wrong region does not match"
    );
    assert!(
        selector_for("PLAYPAL").accepts(palette),
        "a loose lump matches with no region filter"
    );
}

#[test]
fn region_filters_match_the_same_region_only() {
    let flats = Some(Namespace::Flats);
    assert!(in_region(None, None), "no filter matches a loose lump");
    assert!(in_region(flats, None), "no filter matches a region member");
    assert!(in_region(flats, flats), "the same region matches");
    assert!(
        !in_region(None, flats),
        "a loose lump is not in the flats region"
    );
    assert!(
        !in_region(flats, Some(Namespace::Sprites)),
        "a flat is no sprite"
    );
    assert_eq!(
        region_text(flats),
        "the flats region of the working archive",
        "the region text names the region"
    );
    assert_eq!(
        region_text(None),
        "the working archive",
        "a loose lump's region text"
    );
}

#[test]
fn replace_needs_exactly_one_payload_field() {
    assert!(
        matches!(pick_new_data(&replace_args(Some("t"), None)).expect("text"), NewData::Text(body) if body == "t"),
        "text alone"
    );
    assert!(
        matches!(
            pick_new_data(&replace_args(None, Some("4D55"))).expect("hex"),
            NewData::Hex(_)
        ),
        "hex alone"
    );
    let mut sourced = replace_args(None, None);
    sourced.source = Some("/x/y.bin".to_owned());
    assert!(
        matches!(pick_new_data(&sourced).expect("source"), NewData::Source(raw) if raw == "/x/y.bin"),
        "source alone"
    );
    let none = pick_new_data(&replace_args(None, None)).expect_err("no payload");
    assert_eq!(code(&none), "invalid_argument", "no payload code");
    assert!(
        none.to_string().contains("exactly one"),
        "the message states the rule: {none}"
    );
    let both = pick_new_data(&replace_args(Some("t"), Some("4D55"))).expect_err("two fields");
    assert_eq!(code(&both), "invalid_argument", "two payload fields code");
    assert!(
        both.to_string().contains("exactly one"),
        "the message states the rule: {both}"
    );
    let mut all_three = replace_args(Some("t"), Some("4D55"));
    all_three.source = Some("/x/y.bin".to_owned());
    assert_eq!(
        code(&pick_new_data(&all_three).expect_err("three fields")),
        "invalid_argument",
        "three payload fields code"
    );
}

#[tokio::test]
async fn payload_bytes_come_from_text_or_hex_and_report_bad_hex() {
    let state = AppState::default();
    assert_eq!(
        fetch_data(&state, NewData::Text("hi".to_owned()))
            .await
            .expect("text is taken as-is"),
        vec![b'h', b'i'],
        "the text's bytes"
    );
    assert_eq!(
        fetch_data(&state, NewData::Hex("4D 55".to_owned()))
            .await
            .expect("hex decodes"),
        vec![0x4D_u8, 0x55],
        "spaces ignored"
    );
    let error = fetch_data(&state, NewData::Hex("4Z".to_owned()))
        .await
        .expect_err("not hex");
    assert_eq!(code(&error), "invalid_argument", "bad hex code");
    assert!(
        error.to_string().starts_with("hex "),
        "the message names the field: {error}"
    );
}

#[test]
fn a_working_match_is_the_last_one_unless_occurrence_says_otherwise() {
    let mut session = loaded();
    seed(
        &mut session,
        vec![carrying("TWIN", "first"), carrying("TWIN", "second")],
    );
    assert_eq!(
        working_match(&session, &selector_for("TWIN")).expect("matches exist"),
        Some(1),
        "the default is the last copy"
    );
    let first = edit_selector("TWIN", None, Some(0)).expect("valid");
    assert_eq!(
        working_match(&session, &first).expect("occurrence 0 exists"),
        Some(0),
        "an explicit occurrence picks by directory order"
    );
    let past = edit_selector("TWIN", None, Some(2)).expect("valid");
    let error = working_match(&session, &past).expect_err("only two copies");
    assert_eq!(code(&error), "not_found", "past the end code");
    assert!(
        error.to_string().contains("only 2 lumps named TWIN"),
        "the message counts the copies: {error}"
    );
    assert_eq!(
        working_match(&session, &selector_for("ABSENT")).expect("no matches"),
        None,
        "an unknown name is no match at all"
    );
}

#[test]
fn targets_know_when_they_grow_the_directory_and_when_they_copy() {
    let in_place = Target::Working { index: 3 };
    let copied = Target::Map { map: name("MAP01") };
    let region = Target::Region {
        namespace: Namespace::Flats,
    };
    assert!(
        !in_place.creates_an_override(),
        "an overwrite is no override"
    );
    assert!(!in_place.needs_merged_view(), "nothing to copy");
    assert!(
        copied.creates_an_override(),
        "a copied block is an override"
    );
    assert!(copied.needs_merged_view(), "the block comes from the view");
    assert!(
        region.creates_an_override(),
        "a region insert is an override"
    );
    assert!(!region.needs_merged_view(), "but no block copy");
    assert!(
        Target::Append.creates_an_override(),
        "an append is an override"
    );
    assert!(!Target::Append.needs_merged_view(), "and no block copy");
}

#[test]
fn replacing_a_working_lump_overwrites_it_in_place() {
    let mut session = loaded();
    seed(&mut session, vec![carrying("NOTE", "old")]);
    let report = run_replace(&mut session, &selector_for("NOTE"), b"new").expect("replaced");
    assert_eq!(
        serde_json::to_value(&report).expect("serializable"),
        json!({ "name": "NOTE", "index": 0_u64, "size": 3_u64, "created_override": false }),
        "the documented result shape"
    );
    assert_eq!(
        with_payloads(session.working()),
        vec!["NOTE(new)"],
        "same place, new bytes"
    );
    assert_eq!(
        session.undo_labels(),
        vec!["replace_lump NOTE", "seed"],
        "the undo label on top of the seeded lump"
    );
}

#[test]
fn replacing_a_layer_flat_creates_an_override_inside_the_region() {
    let mut session = loaded();
    let report = run_replace(&mut session, &selector_for("FLATA"), b"mine").expect("override");
    assert!(report.created_override, "the flat was brought in");
    assert_eq!(report.index, 1, "between the created region markers");
    assert_eq!(
        names(session.working()),
        vec!["F_START", "FLATA", "F_END"],
        "a flats region was created in the working archive"
    );
    assert_eq!(
        report_of_flat(&session),
        "mine",
        "the override won the merged view"
    );
    assert_eq!(
        session.layers().first().expect("the base").wad.len(),
        9,
        "the layer is untouched"
    );
    assert_eq!(
        session.undo_labels(),
        vec!["replace_lump FLATA"],
        "one undoable edit"
    );
}

#[test]
fn an_existing_region_receives_the_override_at_its_end() {
    let mut session = loaded();
    seed(
        &mut session,
        vec![
            marker("F_START"),
            carrying("FLATB", "mine"),
            marker("F_END"),
        ],
    );
    let report = run_replace(&mut session, &selector_for("FLATA"), b"new")
        .expect("the override joins the region");
    assert_eq!(report.index, 2, "just before the working archive's F_END");
    assert_eq!(
        names(session.working()),
        vec!["F_START", "FLATB", "FLATA", "F_END"],
        "the existing region grew instead of a second one appearing"
    );
}

#[test]
fn replacing_iwad_map_data_copies_the_whole_block_first() {
    let mut session = loaded();
    let report =
        run_replace(&mut session, &selector_for("THINGS"), b"mine").expect("the block was copied");
    assert!(report.created_override, "the block came from the layer");
    assert_eq!(report.index, 1, "the copied THINGS");
    assert_eq!(report.size, 4, "the new bytes");
    assert_eq!(
        names(session.working()),
        vec!["MAP01", "THINGS", "SECTORS", "GL_MAP01", "GL_VERT"],
        "the whole MAP01 block including its GL run, in view order"
    );
    assert_eq!(
        with_payloads(session.working()).get(1).cloned(),
        Some("THINGS(mine)".to_owned()),
        "the copied member carries the new bytes"
    );
    assert_eq!(
        session.undo_labels(),
        vec!["replace_lump THINGS"],
        "copy and write are one undoable edit"
    );
}

#[test]
fn replacing_a_loose_layer_global_appends_the_override() {
    let mut session = loaded();
    let report = run_replace(&mut session, &selector_for("PLAYPAL"), b"new")
        .expect("the global was overridden");
    assert!(report.created_override, "an override arrived");
    assert_eq!(report.index, 0, "appended to the empty working archive");
    assert_eq!(names(session.working()), vec!["PLAYPAL"], "one loose lump");
    assert_eq!(
        session
            .find(name("PLAYPAL"), None)
            .expect("the view still has one")
            .source,
        crate::session::Source::Working,
        "the working copy wins the merged view"
    );
}

#[test]
fn replacing_a_name_no_archive_holds_is_not_found() {
    let mut session = loaded();
    let error =
        run_replace(&mut session, &selector_for("NOPE"), b"x").expect_err("nothing is named NOPE");
    assert_eq!(code(&error), "not_found", "missing lump code");
    assert!(
        error.to_string().contains("add_lump"),
        "the message names the tool that creates one: {error}"
    );
    assert!(session.working().is_empty(), "nothing was created");
    assert!(session.undo_labels().is_empty(), "and nothing recorded");
    assert!(!session.is_dirty(), "so the session is still clean");
}

#[test]
fn a_namespace_selector_only_overrides_that_regions_copy() {
    let mut session = loaded();
    let in_sprites = edit_selector("FLATA", Some("sprites"), None).expect("valid");
    let error = run_replace(&mut session, &in_sprites, b"x").expect_err("no sprite FLATA");
    assert_eq!(code(&error), "not_found", "wrong region");
    assert!(
        error.to_string().contains("in the sprites region"),
        "the message names the region: {error}"
    );
    assert!(session.working().is_empty(), "nothing was created");
}

#[test]
fn overwriting_a_stale_index_reports_internal() {
    let mut wad = pwad(vec![carrying("NOTE", "old")]);
    let error = overwrite(&mut wad, 9, name("NOTE"), b"new").expect_err("only one lump");
    assert_eq!(code(&error), "internal", "stale index code");
    assert!(
        error.to_string().contains("index 9"),
        "the message names the index: {error}"
    );
    assert_eq!(with_payloads(&wad), vec!["NOTE(old)"], "nothing changed");
    assert_eq!(
        overwrite(&mut wad, 0, name("NOTE"), b"new").expect("a real index"),
        0,
        "and the same call works when the lump is there"
    );
    assert_eq!(with_payloads(&wad), vec!["NOTE(new)"], "written in place");
}

#[test]
fn a_copied_block_without_the_named_lump_is_not_found() {
    let merged = loaded().merged();
    let mut wad = Wad::default();
    let write = Replacement {
        name: name("SIDEDEFS"),
        data: b"x".as_slice(),
        target: Target::Map { map: name("MAP01") },
        merged: &merged,
    };
    let error = write.apply(&mut wad).expect_err("MAP01 has no SIDEDEFS");
    assert_eq!(code(&error), "not_found", "missing member code");
    assert!(
        error.to_string().contains("the copied MAP01 block"),
        "the message says what disagrees: {error}"
    );
    assert_eq!(
        names(&wad),
        vec!["MAP01", "THINGS", "SECTORS", "GL_MAP01", "GL_VERT"],
        "the copy happened before the lookup failed, and the caller rolls it back"
    );
    assert!(
        run_replace(&mut loaded(), &selector_for("THINGS"), b"x")
            .is_ok_and(|report| report.created_override),
        "the same write through a session succeeds"
    );
}
