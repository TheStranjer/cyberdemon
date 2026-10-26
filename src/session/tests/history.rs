//! The working archive's history: the label and dirty state one edit
//! leaves behind, what a failed edit does not leave, walking back and forth
//! over undo and redo, the branch an edit after an undo drops, the cap on
//! the stack, and the batch scope that folds nested edits into one entry,
//! rolls back whole, and refuses the session changes a fold cannot carry.

use std::path::PathBuf;

use super::{add_marker, append_marker, layer, loaded, lump, marker, name, pwad};
use crate::error::CyberError;
use crate::session::history::{BatchOutcome, BatchToken, HISTORY_LIMIT};
use crate::session::{LayerRole, Session};
use crate::wad::archive::Wad;

/// Opens a scope labelled `batch` on `session`, panicking if one is open.
fn begin(session: &mut Session) -> BatchToken {
    session.begin_batch("batch").expect("no batch is open yet")
}

/// Closes `token` on `session` with `outcome`, reporting whether the batch
/// left an undo entry; panics on a refusal.
fn end(session: &mut Session, token: BatchToken, outcome: BatchOutcome) -> bool {
    session
        .end_batch(token, outcome)
        .expect("the token names the open batch")
}

#[test]
fn edit_records_its_label_and_marks_the_session_dirty() {
    let mut session = loaded();
    let index = add_marker(&mut session, "F_START").expect("the edit succeeds");
    assert_eq!(index, 0, "the closure's value comes back");
    assert_eq!(session.undo_labels(), vec!["add F_START"], "label recorded");
    assert!(session.is_dirty(), "an edit is unsaved work");
    assert_eq!(session.working().len(), 1, "the working archive changed");
    assert_eq!(
        session
            .layers()
            .iter()
            .map(|item| item.wad.len())
            .collect::<Vec<_>>(),
        vec![8, 4],
        "layers are never edited"
    );
}

#[test]
fn a_failing_edit_changes_nothing_and_is_not_recorded() {
    let mut session = loaded();
    add_marker(&mut session, "A_START").expect("the first edit lands");
    let error = session
        .edit("broken", |_| {
            Err::<(), CyberError>(CyberError::InvalidArgument("nope".to_owned()))
        })
        .expect_err("the second edit fails");
    assert_eq!(error.code(), "invalid_argument", "the error passes through");
    assert_eq!(session.working().len(), 1, "no partial edit landed");
    assert_eq!(
        session.undo_labels(),
        vec!["add A_START"],
        "the failed edit left no history"
    );
    assert!(session.is_dirty(), "only for the one edit that worked");
}

#[test]
fn undo_and_redo_walk_the_working_archive() {
    let mut session = loaded();
    add_marker(&mut session, "A_START").expect("first edit");
    add_marker(&mut session, "A_END").expect("second edit");
    assert_eq!(session.working().len(), 2, "both landed");
    assert_eq!(
        session.undo().expect("undo"),
        "add A_END",
        "newest label first"
    );
    assert_eq!(session.working().len(), 1, "the last edit is gone");
    assert_eq!(session.undo_labels(), vec!["add A_START"], "one step left");
    assert_eq!(session.redo_labels(), vec!["add A_END"], "one step back");
    assert_eq!(session.redo().expect("redo"), "add A_END", "replayed label");
    assert_eq!(session.working().len(), 2, "the edit is back");
    assert!(session.redo().is_err(), "nothing further to replay");
}

#[test]
fn undoing_back_to_the_saved_state_makes_the_session_clean() {
    let mut session = loaded();
    session
        .replace_working(pwad(vec![lump("PLAYPAL")]), Some(PathBuf::from("out.wad")))
        .expect("no batch is open");
    assert!(!session.is_dirty(), "a replaced archive starts clean");
    assert!(session.undo_labels().is_empty(), "and cannot be undone");
    add_marker(&mut session, "MARK").expect("an edit");
    assert!(session.is_dirty(), "now there is unsaved work");
    session
        .mark_saved(PathBuf::from("out.wad"))
        .expect("no batch is open");
    assert!(!session.is_dirty(), "saving cleared it");
    assert_eq!(
        session
            .working_path()
            .expect("a path")
            .display()
            .to_string(),
        "out.wad",
        "and recorded where"
    );
    add_marker(&mut session, "MORE").expect("another edit");
    assert!(session.is_dirty(), "unsaved again");
    session.undo().expect("undo it");
    assert!(!session.is_dirty(), "back to the saved revision");
}

#[test]
fn editing_after_an_undo_drops_the_redo_branch() {
    let mut session = loaded();
    add_marker(&mut session, "ONE").expect("first");
    add_marker(&mut session, "TWO").expect("second");
    session.undo().expect("back to one");
    assert_eq!(session.redo_labels(), vec!["add TWO"], "a branch to replay");
    add_marker(&mut session, "THREE").expect("a new branch");
    assert!(session.redo_labels().is_empty(), "the old branch is gone");
    assert_eq!(
        session.undo_labels(),
        vec!["add THREE", "add ONE"],
        "the history continues from the undone state, with a fresh revision"
    );
    assert_eq!(session.working().len(), 2, "ONE and THREE, not TWO");
}

#[test]
fn replace_working_forgets_history_and_remember_the_new_path() {
    let mut session = loaded();
    add_marker(&mut session, "ONE").expect("an edit");
    session
        .replace_working(pwad(vec![lump("S_START")]), Some(PathBuf::from("new.wad")))
        .expect("no batch is open");
    assert!(
        session.undo_labels().is_empty(),
        "no history across archives"
    );
    assert!(!session.is_dirty(), "the new archive is clean");
    assert_eq!(
        session
            .working_path()
            .expect("the new path")
            .display()
            .to_string(),
        "new.wad",
        "the path came with it"
    );
    session
        .replace_working(Wad::default(), None)
        .expect("still no batch");
    assert!(
        session.working_path().is_none(),
        "an unsaved archive has no path"
    );
    assert!(session.undo().is_err(), "and nothing to undo");
}

#[test]
fn undo_and_redo_on_an_empty_history_are_not_found() {
    let mut session = loaded();
    let undo = session.undo().expect_err("nothing was edited");
    assert_eq!(undo.code(), "not_found", "empty undo code");
    assert_eq!(undo.to_string(), "nothing to undo", "empty undo message");
    let redo = session.redo().expect_err("nothing was undone");
    assert_eq!(redo.code(), "not_found", "empty redo code");
    assert_eq!(redo.to_string(), "nothing to redo", "empty redo message");
}

#[test]
fn history_keeps_only_the_newest_edits() {
    let mut session = loaded();
    let steps = HISTORY_LIMIT
        .checked_add(5)
        .expect("a count past the limit");
    for step in 0..steps {
        let label = format!("edit-{step}");
        session
            .edit(label.as_str(), append_marker)
            .expect("appends never fail");
    }
    let labels = session.undo_labels();
    assert_eq!(labels.len(), HISTORY_LIMIT, "the undo stack is capped");
    assert!(!labels.contains(&"edit-0"), "the oldest edit was dropped");
    let newest = steps.checked_sub(1).expect("a newest label");
    let newest_label = format!("edit-{newest}");
    assert_eq!(
        labels.first().copied().expect("a newest label"),
        newest_label.as_str(),
        "and the newest is on top"
    );
}

#[test]
fn undo_only_touches_the_working_archive() {
    let mut session = loaded();
    session.set_active_map(Some(name("MAP01")));
    add_marker(&mut session, "ONE").expect("an edit");
    session.undo().expect("undo it");
    assert_eq!(
        session.active_map(),
        Some(name("MAP01")),
        "the selection is not part of the history"
    );
    assert_eq!(session.layers().len(), 2, "the layers never changed");
}

#[test]
fn a_batch_folds_every_nested_edit_into_one_entry() {
    let mut session = loaded();
    add_marker(&mut session, "KEEP").expect("an edit before the batch");
    let token = begin(&mut session);
    assert!(session.batch_open(), "the scope owns the session");
    add_marker(&mut session, "B1").expect("batch step 1");
    add_marker(&mut session, "B2").expect("batch step 2");
    add_marker(&mut session, "B3").expect("batch step 3");
    assert!(
        end(&mut session, token, BatchOutcome::Commit),
        "the batch recorded edits"
    );
    assert_eq!(
        session.undo_labels(),
        vec!["batch", "add KEEP"],
        "three inner labels folded into the batch's one"
    );
    assert_eq!(
        session.working().len(),
        4,
        "KEEP plus the three batch lumps"
    );
    assert_eq!(
        session.undo().expect("one undo steps back over the batch"),
        "batch",
        "the batch label is what undo reports"
    );
    assert_eq!(
        session.undo_labels(),
        vec!["add KEEP"],
        "and the pre-batch entry stands below"
    );
    assert_eq!(session.working().len(), 1, "all three batch edits at once");
    session.redo().expect("redo the batch");
    assert_eq!(
        session.working().len(),
        4,
        "the whole fold replays as one step"
    );
}

#[test]
fn a_batch_rollback_restores_the_exact_pre_batch_archive_and_revision() {
    let mut session = loaded();
    add_marker(&mut session, "KEEP").expect("an edit before the batch");
    session
        .mark_saved(PathBuf::from("out.wad"))
        .expect("no batch is open");
    assert!(!session.is_dirty(), "the session is clean at the mark");
    let token = begin(&mut session);
    add_marker(&mut session, "B1").expect("batch step 1");
    add_marker(&mut session, "B2").expect("batch step 2");
    assert!(session.is_dirty(), "mid-batch the folded edits are unsaved");
    assert!(
        !end(&mut session, token, BatchOutcome::Rollback),
        "a rollback leaves no entry"
    );
    assert!(!session.batch_open(), "the scope closed");
    assert_eq!(
        session.undo_labels(),
        vec!["add KEEP"],
        "only the KEEP entry"
    );
    assert_eq!(
        session.working().len(),
        1,
        "only the pre-batch lump is left"
    );
    assert_eq!(
        session.working().lumps.first().expect("the kept lump").name,
        name("KEEP"),
        "the exact pre-batch archive is back"
    );
    assert!(
        !session.is_dirty(),
        "the exact pre-batch revision is back, and it was the saved one"
    );
    assert!(
        session.redo_labels().is_empty(),
        "the rolled-back steps cannot be redone"
    );
    session.undo().expect("the pre-batch undo is untouched");
    assert_eq!(session.undo_labels(), Vec::<&str>::new(), "and walked back");
}

#[test]
fn a_batch_whose_edits_all_fail_records_no_entry() {
    let mut session = loaded();
    add_marker(&mut session, "KEEP").expect("an edit before the batch");
    let token = begin(&mut session);
    let error = session
        .edit("broken", |_| {
            Err::<(), CyberError>(CyberError::InvalidArgument("nope".to_owned()))
        })
        .expect_err("the inner edit fails");
    assert_eq!(error.code(), "invalid_argument", "the error passes through");
    assert!(
        !end(&mut session, token, BatchOutcome::Commit),
        "nothing recorded, no entry"
    );
    assert_eq!(
        session.undo_labels(),
        vec!["add KEEP"],
        "the history shows only the pre-batch edit"
    );
    assert_eq!(
        session.working().len(),
        1,
        "and the archive changed nothing"
    );
    assert!(session.is_dirty(), "the KEEP edit is still unsaved work");
}

#[test]
fn undo_and_redo_refuse_while_a_batch_is_open() {
    let mut session = loaded();
    add_marker(&mut session, "KEEP").expect("an edit before the batch");
    let token = begin(&mut session);
    let undo = session.undo().expect_err("the batch owns the stack");
    assert_eq!(undo.code(), "conflict", "undo inside a batch code");
    assert!(
        undo.to_string().contains("undo"),
        "the message names the operation: {undo}"
    );
    let redo = session.redo().expect_err("and the same for redo");
    assert!(
        redo.to_string().contains("redo"),
        "the message names the operation: {redo}"
    );
    assert_eq!(session.undo_depth(), 1, "the stack stands");
    assert!(
        !end(&mut session, token, BatchOutcome::Rollback),
        "close it"
    );
    assert_eq!(
        session.undo().expect("now undo walks"),
        "add KEEP",
        "the refusal did not consume the entry"
    );
}

#[test]
fn replace_working_is_refused_while_a_batch_is_open() {
    let mut session = loaded();
    add_marker(&mut session, "KEEP").expect("an edit before the batch");
    let token = begin(&mut session);
    let error = session
        .replace_working(pwad(vec![lump("OTHER")]), None)
        .expect_err("a fold cannot carry an archive swap");
    assert_eq!(
        error.code(),
        "conflict",
        "replace_working inside a batch code"
    );
    assert!(
        error.to_string().contains("working archive"),
        "the message names the operation: {error}"
    );
    assert_eq!(session.working().len(), 1, "the archive did not move");
    assert_eq!(
        session.undo_labels(),
        vec!["add KEEP"],
        "nor did the history"
    );
    end(&mut session, token, BatchOutcome::Rollback);
    session
        .replace_working(pwad(vec![lump("OTHER")]), None)
        .expect("after the batch the swap runs");
    assert_eq!(session.working().len(), 1, "the fresh archive stands");
    assert!(session.undo_labels().is_empty(), "with its history cleared");
}

#[test]
fn the_layer_and_saved_state_swaps_refuse_while_a_batch_is_open() {
    let mut session = loaded();
    let token = begin(&mut session);
    let iwad = session
        .set_iwad(layer(LayerRole::Iwad, "other", vec![marker("PLAYPAL")]))
        .expect_err("a fold cannot carry a base swap");
    assert_eq!(iwad.code(), "conflict", "set_iwad inside a batch code");
    let resource = session
        .add_resource(layer(LayerRole::Resource, "extra", vec![marker("S_START")]))
        .expect_err("nor a layer stack");
    assert_eq!(
        resource.code(),
        "conflict",
        "add_resource inside a batch code"
    );
    let saved = session
        .mark_saved(PathBuf::from("out.wad"))
        .expect_err("nor a save mark");
    assert_eq!(saved.code(), "conflict", "mark_saved inside a batch code");
    assert_eq!(session.layers().len(), 2, "the stack stands as it was");
    assert!(
        session.working_path().is_none(),
        "no path was recorded while refused"
    );
    end(&mut session, token, BatchOutcome::Commit);
    session
        .add_resource(layer(LayerRole::Resource, "extra", vec![marker("S_START")]))
        .expect("after the batch the stack runs");
    assert_eq!(session.layers().len(), 3, "and it stacked");
}

#[test]
fn a_second_batch_is_refused_and_a_stale_token_cannot_close_the_open_one() {
    let mut session = loaded();
    let token = begin(&mut session);
    let nested = session
        .begin_batch("second")
        .expect_err("a session runs one batch at a time");
    assert_eq!(nested.code(), "conflict", "nested begin code");
    assert!(
        nested.to_string().contains("batch is open"),
        "the message says why: {nested}"
    );
    assert!(session.batch_open(), "the refusal keeps the scope open");
    end(&mut session, token, BatchOutcome::Commit);
    let later = session
        .begin_batch("second run")
        .expect("a closed scope frees the session");
    let stale = session
        .end_batch(token, BatchOutcome::Commit)
        .expect_err("a used token does not name the open batch");
    assert_eq!(stale.code(), "conflict", "stale token code");
    assert!(
        stale.to_string().contains("does not name"),
        "the message says why: {stale}"
    );
    assert!(session.batch_open(), "the refusal keeps the new scope open");
    assert!(
        !end(&mut session, later, BatchOutcome::Commit),
        "nothing ran"
    );
}

#[test]
fn ending_without_an_open_batch_is_refused() {
    let mut session = loaded();
    let token = begin(&mut session);
    end(&mut session, token, BatchOutcome::Commit);
    let error = session
        .end_batch(token, BatchOutcome::Rollback)
        .expect_err("the scope is already closed");
    assert_eq!(error.code(), "conflict", "no-batch end code");
    assert!(
        error.to_string().contains("no batch is open"),
        "the message says why: {error}"
    );
}
