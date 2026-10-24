//! `remove_lump`: what a plan accepts, which copies each name takes, how the
//! region and occurrence filters narrow them and what an aborted removal
//! leaves in the working archive.

use serde_json::json;

use super::{
    carrying, code, loaded, marker, name, names, pwad, remove_args, remove_plan, seed, twin_plan,
};
use crate::tools::lumps::edit::remove::{
    allowed_occurrence, apply_remove, matched_positions, plan_remove, remove_label, remove_lump,
};
use crate::tools::lumps::edit::replace::not_working_message;
use crate::wad::classify::{Namespace, analyze};

#[test]
fn remove_plans_refuse_an_empty_list_bad_names_and_shared_occurrence() {
    let empty = plan_remove(&remove_args(&[], None, None, false)).expect_err("no names");
    assert_eq!(code(&empty), "invalid_argument", "empty list code");
    assert!(
        empty.to_string().contains("at least one"),
        "the message states the rule: {empty}"
    );
    let bad =
        plan_remove(&remove_args(&["OK", "TOO LONG"], None, None, false)).expect_err("one bad");
    assert_eq!(code(&bad), "invalid_argument", "bad name code");
    let shared = remove_args(&["A", "B"], None, Some(1), false);
    let error = plan_remove(&shared).expect_err("occurrence with two names");
    assert_eq!(code(&error), "invalid_argument", "occurrence conflict code");
    assert!(
        error.to_string().contains("occurrence"),
        "the message names the field: {error}"
    );
    let region = remove_args(&["A"], Some("nope"), None, false);
    assert_eq!(
        code(&plan_remove(&region).expect_err("not a region")),
        "invalid_argument",
        "bad namespace code"
    );
    let once = plan_remove(&remove_args(&["a", "B"], Some("flats"), None, true)).expect("valid");
    assert_eq!(once.names, vec![name("A"), name("B")], "names uppercase");
    assert_eq!(once.namespace, Some(Namespace::Flats), "the region carries");
    assert!(once.all, "and the all flag");
    assert_eq!(once.occurrence, None, "no occurrence was given");
}

#[test]
fn allowed_occurrence_only_ever_accompanies_one_name() {
    assert_eq!(
        allowed_occurrence(Some(3), 1).expect("one name"),
        Some(3),
        "kept"
    );
    assert_eq!(
        allowed_occurrence(None, 4).expect("unset"),
        None,
        "absent is fine with any count"
    );
    let error = allowed_occurrence(Some(1), 2).expect_err("two names");
    assert_eq!(code(&error), "invalid_argument", "conflict code");
}

#[test]
fn removing_takes_the_last_copy_unless_asked_for_more() {
    let mut wad = pwad(vec![
        carrying("TWIN", "one"),
        carrying("OTHER", "x"),
        carrying("TWIN", "two"),
    ]);
    let removed = apply_remove(&mut wad, &remove_plan(&["TWIN"], false)).expect("one goes");
    assert_eq!(
        removed
            .iter()
            .map(|entry| format!("{}:{}", entry.name, entry.index))
            .collect::<Vec<String>>(),
        vec!["TWIN:2"],
        "the last match, reported at its old index"
    );
    assert_eq!(names(&wad), vec!["TWIN", "OTHER"], "and the first stays");
    let again = apply_remove(&mut wad, &remove_plan(&["TWIN"], true)).expect("the last goes");
    assert_eq!(again.len(), 1, "one position was left to remove");
    assert_eq!(names(&wad), vec!["OTHER"], "both twins are gone");
}

#[test]
fn removing_several_names_reports_directory_order_and_labels_the_edit() {
    let mut wad = pwad(vec![
        carrying("B", "b"),
        carrying("A", "a"),
        marker("F_START"),
        carrying("C", "c"),
        marker("F_END"),
    ]);
    let plan = remove_plan(&["C", "A"], false);
    let removed = apply_remove(&mut wad, &plan).expect("both names exist");
    assert_eq!(
        removed
            .iter()
            .map(|entry| format!("{}:{}", entry.name, entry.index))
            .collect::<Vec<String>>(),
        vec!["A:1", "C:3"],
        "reported in directory order with pre-removal indices"
    );
    assert_eq!(names(&wad), vec!["B", "F_START", "F_END"], "two went");
    assert_eq!(
        remove_label(&plan.names),
        "remove_lump C, A",
        "the label keeps the caller's order"
    );
}

#[test]
fn a_missing_name_aborts_a_multi_name_removal_entirely() {
    let mut wad = pwad(vec![carrying("A", "a"), carrying("B", "b")]);
    let before = names(&wad);
    let error =
        apply_remove(&mut wad, &remove_plan(&["A", "GHOST"], false)).expect_err("GHOST is absent");
    assert_eq!(code(&error), "not_found", "missing name code");
    assert!(
        error.to_string().contains("read-only"),
        "the message explains the layers: {error}"
    );
    assert!(
        error.to_string().contains("replace_lump"),
        "the message names the way to override: {error}"
    );
    assert_eq!(names(&wad), before, "nothing was removed");
}

#[test]
fn removals_honour_the_region_and_the_selected_occurrence() {
    let mut wad = pwad(vec![
        carrying("FLATA", "loose"),
        marker("F_START"),
        carrying("FLATA", "region"),
        marker("F_END"),
    ]);
    let flats =
        plan_remove(&remove_args(&["FLATA"], Some("flats"), None, false)).expect("a valid plan");
    let removed = apply_remove(&mut wad, &flats).expect("the region copy goes");
    assert_eq!(
        removed
            .iter()
            .map(|entry| entry.index)
            .collect::<Vec<usize>>(),
        vec![2_usize],
        "only the region member matched"
    );
    assert_eq!(
        names(&wad),
        vec!["FLATA", "F_START", "F_END"],
        "the loose copy survives"
    );
    let twins = pwad(vec![
        carrying("TWIN", "one"),
        carrying("TWIN", "two"),
        carrying("TWIN", "three"),
    ]);
    let infos = analyze(&twins.lumps);
    assert_eq!(
        matched_positions(&infos.lumps, name("TWIN"), &twin_plan(Some(0))).expect("the first copy"),
        vec![0_usize],
        "occurrence 0 is the first match"
    );
    let past = matched_positions(&infos.lumps, name("TWIN"), &twin_plan(Some(3)))
        .expect_err("past the end");
    assert_eq!(code(&past), "not_found", "an out of range occurrence");
    assert!(
        past.to_string().contains("0 to 2"),
        "the message names the range: {past}"
    );
    assert_eq!(
        matched_positions(&infos.lumps, name("TWIN"), &twin_plan(None)).expect("the last copy"),
        vec![2_usize],
        "the default occurrence is the last match"
    );
    assert_eq!(
        matched_positions(&infos.lumps, name("ABSENT"), &twin_plan(None))
            .err()
            .map(|error| error.to_string()),
        Some(not_working_message(name("ABSENT"), None)),
        "the documented message for an absent name"
    );
}

#[test]
fn remove_lump_edits_the_working_archive_once_and_rolls_a_refusal_back() {
    let mut session = loaded();
    seed(
        &mut session,
        vec![carrying("PLAYPAL", "mine"), carrying("NOTE", "n")],
    );
    let report = remove_lump(&mut session, &remove_args(&["NOTE"], None, None, false))
        .expect("the working archive holds NOTE");
    assert_eq!(
        serde_json::to_value(&report).expect("serializable"),
        json!({ "removed": [{ "name": "NOTE", "index": 1_u64 }] }),
        "the documented result shape"
    );
    assert_eq!(names(session.working()), vec!["PLAYPAL"], "one lump went");
    assert_eq!(
        session.undo_labels(),
        vec!["remove_lump NOTE", "seed"],
        "the undo label on top of the seeded lumps"
    );
    let error = remove_lump(&mut session, &remove_args(&["FLATA"], None, None, false))
        .expect_err("FLATA is only in the layer");
    assert_eq!(code(&error), "not_found", "layer lump code");
    assert_eq!(
        names(session.working()),
        vec!["PLAYPAL"],
        "nothing else went"
    );
    assert_eq!(
        session.undo_labels(),
        vec!["remove_lump NOTE", "seed"],
        "and no history for the refusal"
    );
}
