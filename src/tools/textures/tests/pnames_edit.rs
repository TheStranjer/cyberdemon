//! `edit_pnames` committed: [`edit_pnames_run`]'s `add` and `remove` arms
//! landing in the working archive as one undoable step and what they report,
//! and the reusable [`plan_pnames_removal`]/[`write_pnames_rewrite`] pair the
//! patch tools fold into their own edit.

use crate::session::Session;
use crate::tools::textures::pnames::Operation;
use crate::tools::textures::pnames_rewrite::{
    edit_pnames_run, plan_pnames_removal, write_pnames_rewrite,
};
use crate::wad::archive::Wad;
use crate::wad::textures::{parse_pnames, parse_texture_lump};

use super::{code, name, run, session, session_with_spare, set_of};

#[test]
fn the_add_edit_lands_in_the_working_archive_and_undoes_as_one_step() {
    let mut session = session();
    let iwad_before = session.layers()[0].wad.lumps.len();
    let add = ["NEWP1".to_owned(), "WALLP1".to_owned()];
    let report = edit_pnames_run(&mut session, Operation::Add(&add), false).expect("runs");
    assert_eq!(report.pnames, 3, "one new entry");
    assert_eq!(report.added, vec!["NEWP1".to_owned()], "the new name");
    assert_eq!(report.skipped, vec!["WALLP1".to_owned()], "already present");
    assert_eq!(report.textures_changed, 0, "appends never shift indices");
    let working: Vec<String> = session
        .working()
        .lumps
        .iter()
        .map(|lump| lump.name.to_string())
        .collect();
    assert_eq!(
        working,
        vec!["PNAMES".to_owned()],
        "the override was appended"
    );
    assert_eq!(
        session.undo_labels(),
        vec!["edit_pnames add"],
        "one undoable edit"
    );
    assert!(session.is_dirty(), "and unsaved work");
    assert_eq!(
        set_of(&session).pnames,
        vec!["WALLP1".to_owned(), "STEPP1".to_owned(), "NEWP1".to_owned()],
        "the working PNAMES won the merge"
    );
    session.undo().expect("undo the whole edit");
    assert_eq!(set_of(&session).pnames.len(), 2, "back to the IWAD list");
    assert!(
        session.working().lumps.is_empty(),
        "and no working copy at all"
    );
    assert_eq!(
        session.layers()[0].wad.lumps.len(),
        iwad_before,
        "the IWAD layer never changed"
    );
}

#[test]
fn an_add_of_only_present_names_changes_nothing_and_records_no_history() {
    let mut session = session();
    let add = ["wallp1".to_owned()];
    let report = edit_pnames_run(&mut session, Operation::Add(&add), false).expect("runs");
    assert_eq!(report.pnames, 2, "the count stands");
    assert_eq!(
        report.skipped,
        vec!["WALLP1".to_owned()],
        "reported skipped"
    );
    assert!(session.undo_labels().is_empty(), "no edit recorded");
    assert!(!session.is_dirty(), "the session stayed clean");
}

#[test]
fn an_add_without_any_pnames_creates_the_lump() {
    let mut bare = Session::default();
    let add = ["FIRST".to_owned()];
    let report = edit_pnames_run(&mut bare, Operation::Add(&add), false).expect("creation");
    assert_eq!(report.pnames, 1, "the list started fresh");
    assert_eq!(
        bare.working().lumps.len(),
        1,
        "PNAMES landed in the working archive"
    );
    assert_eq!(
        bare.working().lumps.first().expect("one lump").name,
        name("PNAMES"),
        "named PNAMES"
    );
}

#[test]
fn edit_pnames_remove_refuses_referenced_names_until_forced() {
    let remove = ["WALLP1".to_owned()];
    let error = run(Operation::Remove(&remove), false).expect_err("WALL1 refers to it");
    assert_eq!(code(&error), "conflict", "conflict code");
    let message = error.to_string();
    assert!(
        message.contains("WALL1 (TEXTURE1)") && message.contains("force=true"),
        "the message lists the affected texture and the escape: {message}"
    );
    assert!(
        !message.contains("STEP1"),
        "STEP1 only renumbers, it loses no reference: {message}"
    );
}

#[test]
fn forced_remove_prunes_renumbers_and_writes_one_edit() {
    let mut session = session();
    let remove = ["WALLP1".to_owned()];
    let report =
        edit_pnames_run(&mut session, Operation::Remove(&remove), true).expect("the force runs");
    assert_eq!(report.pnames, 1, "only STEPP1 is left");
    assert_eq!(
        report.removed,
        vec!["WALLP1".to_owned()],
        "reported removed"
    );
    assert_eq!(
        report.textures_changed, 2,
        "both textures shifted or pruned"
    );
    assert_eq!(
        session.undo_labels(),
        vec!["edit_pnames remove"],
        "one labelled edit for PNAMES and TEXTURE1 together"
    );
    let working: Vec<String> = session
        .working()
        .lumps
        .iter()
        .map(|lump| lump.name.to_string())
        .collect();
    assert_eq!(working, vec!["PNAMES", "TEXTURE1"], "both overrides exist");
    let loaded = set_of(&session);
    assert_eq!(
        loaded.pnames,
        vec!["STEPP1".to_owned()],
        "the shortened list"
    );
    let (wall, _) = loaded.find("WALL1").expect("still there");
    assert_eq!(wall.patches.len(), 1, "WALL1 kept only its renumbered ref");
    assert_eq!(
        wall.patches.first().expect("one ref").patch,
        0,
        "index 1 became 0"
    );
}

#[test]
fn a_remove_of_absent_names_skips_without_writing() {
    let mut session = session();
    let remove = ["NOPE".to_owned()];
    let report =
        edit_pnames_run(&mut session, Operation::Remove(&remove), false).expect("nothing to do");
    assert_eq!(report.skipped, vec!["NOPE".to_owned()], "reported skipped");
    assert_eq!(report.pnames, 2, "the count stands");
    assert!(session.undo_labels().is_empty(), "a no-op records nothing");
}

#[test]
fn a_planned_removal_of_an_unreferenced_name_rewrites_no_textures() {
    let session = session_with_spare();
    let names = ["EXTRAP".to_owned()];
    let rewrite = plan_pnames_removal(&session, &names, false).expect("nothing refers to it");
    assert_eq!(
        rewrite.removed,
        vec!["EXTRAP".to_owned()],
        "reported removed"
    );
    assert!(rewrite.skipped.is_empty(), "the name was listed");
    assert_eq!(rewrite.textures_changed, 0, "no reference shifted or fell");
    assert!(
        rewrite.texture1.is_none() && rewrite.texture2.is_none(),
        "no texture bytes"
    );
    assert_eq!(
        rewrite.names,
        vec!["WALLP1".to_owned(), "STEPP1".to_owned()],
        "the shortened list"
    );
    assert_eq!(
        parse_pnames(&rewrite.pnames_bytes).expect("a well-formed lump"),
        rewrite.names,
        "the bytes carry the new list"
    );
}

#[test]
fn a_planned_removal_conflicts_on_referenced_names_until_forced() {
    let session = session();
    let names = ["WALLP1".to_owned()];
    let error = plan_pnames_removal(&session, &names, false).expect_err("WALL1 refers to it");
    assert_eq!(code(&error), "conflict", "conflict code");
    assert!(
        error.to_string().contains("WALL1 (TEXTURE1)"),
        "the message labels the offending lump: {error}"
    );
    let rewrite = plan_pnames_removal(&session, &names, true).expect("the force plans");
    assert_eq!(
        rewrite.textures_changed, 2,
        "WALL1 pruned, STEP1 renumbered"
    );
    assert!(rewrite.texture2.is_none(), "the fixture has no TEXTURE2");
    let bytes = rewrite.texture1.as_ref().expect("TEXTURE1 was rewritten");
    let lump = parse_texture_lump(bytes, 1).expect("the bytes decode");
    let wall = lump
        .textures
        .iter()
        .find(|candidate| candidate.name == "WALL1")
        .expect("WALL1 stands");
    assert_eq!(
        wall.patches.iter().map(|r| r.patch).collect::<Vec<u16>>(),
        vec![0_u16],
        "the vanished refs fell, index 1 became 0"
    );
}

#[test]
fn a_planned_removal_skips_absent_names_and_validates_its_list() {
    let session = session();
    let absent = ["NOPE".to_owned(), "GONE".to_owned()];
    let rewrite = plan_pnames_removal(&session, &absent, false).expect("nothing to remove");
    assert_eq!(
        rewrite.skipped,
        vec!["NOPE".to_owned(), "GONE".to_owned()],
        "both reported skipped"
    );
    assert!(rewrite.removed.is_empty(), "nothing removed");
    assert_eq!(rewrite.textures_changed, 0, "nothing renumbered");
    assert!(
        rewrite.texture1.is_none() && rewrite.texture2.is_none(),
        "no bytes"
    );
    let empty: Vec<String> = Vec::new();
    let empty_error = plan_pnames_removal(&session, &empty, false).expect_err("empty list");
    assert_eq!(code(&empty_error), "invalid_argument", "empty remove code");
    let bad = ["TOO LONG".to_owned()];
    let bad_error = plan_pnames_removal(&session, &bad, false).expect_err("bad name");
    assert_eq!(code(&bad_error), "invalid_argument", "bad name code");
}

#[test]
fn write_pnames_rewrite_writes_pnames_and_the_changed_texture_lumps() {
    let session = session();
    let names = ["WALLP1".to_owned()];
    let rewrite = plan_pnames_removal(&session, &names, true).expect("the force plans");
    let mut wad = Wad::default();
    write_pnames_rewrite(&mut wad, &rewrite).expect("the write runs");
    let stored: Vec<String> = wad.lumps.iter().map(|lump| lump.name.to_string()).collect();
    assert_eq!(
        stored,
        vec!["PNAMES", "TEXTURE1"],
        "both overrides appended"
    );
    assert_eq!(
        wad.lumps.first().expect("PNAMES").data.to_vec(),
        rewrite.pnames_bytes,
        "the planned bytes landed"
    );
}
