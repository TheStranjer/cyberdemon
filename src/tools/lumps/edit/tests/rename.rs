//! `rename_lump`: the working rename and its undo label, the same-name clash
//! `allow_duplicate` answers, another region's twin, the stale index and
//! both names' validation.

use serde_json::json;

use super::{carrying, code, loaded, marker, name, names, pwad, rename_args, seed, with_payloads};
use crate::tools::lumps::edit::rename::{
    RenameLumpParams, apply_rename, rename_in_place, rename_lump,
};

#[test]
fn renaming_moves_a_working_lump_and_labels_the_edit() {
    let mut session = loaded();
    seed(&mut session, vec![carrying("OLD", "x")]);
    let report =
        rename_lump(&mut session, &rename_args("old", "NEW", false)).expect("OLD is working");
    assert_eq!(
        serde_json::to_value(&report).expect("serializable"),
        json!({ "old_name": "OLD", "new_name": "NEW", "index": 0_u64 }),
        "the documented result shape"
    );
    assert_eq!(names(session.working()), vec!["NEW"], "renamed in place");
    assert_eq!(
        session.undo_labels(),
        vec!["rename_lump OLD -> NEW", "seed"],
        "the undo label on top of the seeded lump"
    );
    let error = rename_lump(&mut session, &rename_args("FLATA", "WHATEVER", false))
        .expect_err("FLATA is only in the layer");
    assert_eq!(code(&error), "not_found", "layer lump code");
    assert!(
        error.to_string().contains("cannot be removed or renamed"),
        "the message explains the layers: {error}"
    );
}

#[test]
fn renaming_onto_a_taken_name_needs_allow_duplicate() {
    let mut wad = pwad(vec![
        carrying("A", "a"),
        carrying("TAKEN", "t"),
        carrying("C", "c"),
    ]);
    let clash =
        apply_rename(&mut wad, 0, name("TAKEN"), false).expect_err("TAKEN already stands there");
    assert_eq!(code(&clash), "already_exists", "twin code");
    assert!(
        clash.to_string().contains("allow_duplicate"),
        "the message names the fix: {clash}"
    );
    assert_eq!(
        names(&wad),
        vec!["A", "TAKEN", "C"],
        "the refusal changes nothing"
    );
    apply_rename(&mut wad, 0, name("TAKEN"), true).expect("the twin is allowed");
    assert_eq!(
        names(&wad),
        vec!["TAKEN", "TAKEN", "C"],
        "both copies stand"
    );
    let stale = apply_rename(&mut wad, 42, name("OTHER"), false).expect_err("no lump at 42");
    assert_eq!(code(&stale), "internal", "stale index code");
}

#[test]
fn a_rename_onto_another_regions_name_is_no_clash() {
    let mut wad = pwad(vec![
        carrying("WALL1", "wall"),
        marker("F_START"),
        carrying("FLATA", "flat"),
        marker("F_END"),
    ]);
    apply_rename(&mut wad, 0, name("FLATA"), false).expect("a loose twin is no region clash");
    assert_eq!(
        names(&wad),
        vec!["FLATA", "F_START", "FLATA", "F_END"],
        "two FLATA copies, one loose and one in flats"
    );
    assert_eq!(
        with_payloads(&wad).first().cloned(),
        Some("FLATA(wall)".to_owned()),
        "the renamed lump kept its bytes"
    );
    let stale = apply_rename(&mut wad, 42, name("FLATA"), false).expect_err("no lump at 42");
    assert_eq!(code(&stale), "internal", "stale index code");
}

#[test]
fn renaming_in_place_reports_a_stale_index() {
    let mut wad = pwad(vec![carrying("A", "a")]);
    rename_in_place(&mut wad, 0, name("B")).expect("the only lump");
    assert_eq!(names(&wad), vec!["B"], "renamed");
    let error = rename_in_place(&mut wad, 7, name("C")).expect_err("nothing at 7");
    assert_eq!(code(&error), "internal", "stale index code");
    assert!(
        error.to_string().contains("index 7"),
        "the message names the index: {error}"
    );
    assert_eq!(names(&wad), vec!["B"], "the failed call changed nothing");
}

#[test]
fn rename_validates_both_names_before_editing() {
    let mut session = loaded();
    let error = rename_lump(&mut session, &rename_args("PLAYPAL", "TOO LONG", false))
        .expect_err("a bad new_name");
    assert_eq!(code(&error), "invalid_argument", "bad new name code");
    let bad_old =
        rename_lump(&mut session, &rename_args("TOO LONG", "OK", false)).expect_err("a bad name");
    assert_eq!(code(&bad_old), "invalid_argument", "bad name code");
    let region = RenameLumpParams {
        namespace: Some("nope".to_owned()),
        ..rename_args("PLAYPAL", "OK", false)
    };
    assert_eq!(
        code(&rename_lump(&mut session, &region).expect_err("not a region")),
        "invalid_argument",
        "bad namespace code"
    );
    assert!(session.undo_labels().is_empty(), "no edit was attempted");
    assert!(session.working().is_empty(), "and nothing changed");
}
