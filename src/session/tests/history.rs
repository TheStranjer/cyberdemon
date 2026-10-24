//! The working archive's history: the label and dirty state one edit
//! leaves behind, what a failed edit does not leave, walking back and forth
//! over undo and redo, the branch an edit after an undo drops, the cap on
//! the stack, and the depth marks a batch rolls back or squashes.

use std::path::PathBuf;

use super::{add_marker, append_marker, loaded, lump, name, pwad};
use crate::error::CyberError;
use crate::session::history::HISTORY_LIMIT;
use crate::wad::archive::Wad;

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
    session.replace_working(pwad(vec![lump("PLAYPAL")]), Some(PathBuf::from("out.wad")));
    assert!(!session.is_dirty(), "a replaced archive starts clean");
    assert!(session.undo_labels().is_empty(), "and cannot be undone");
    add_marker(&mut session, "MARK").expect("an edit");
    assert!(session.is_dirty(), "now there is unsaved work");
    session.mark_saved(PathBuf::from("out.wad"));
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
    session.replace_working(pwad(vec![lump("S_START")]), Some(PathBuf::from("new.wad")));
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
    session.replace_working(Wad::default(), None);
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
fn undo_depth_marks_a_batch_and_rollback_restores_the_pre_batch_archive() {
    let mut session = loaded();
    add_marker(&mut session, "KEEP").expect("an edit before the batch");
    let depth = session.undo_depth();
    assert_eq!(depth, 1, "one entry stands before the batch");
    add_marker(&mut session, "B1").expect("batch step 1");
    add_marker(&mut session, "B2").expect("batch step 2");
    assert_eq!(session.undo_depth(), 3, "the batch recorded two more");
    assert_eq!(
        session.rollback_to(depth),
        2,
        "both batch entries roll back at once"
    );
    assert_eq!(session.undo_depth(), depth, "the mark holds");
    assert_eq!(
        session.working().len(),
        1,
        "only the pre-batch lump is left"
    );
    assert!(
        session.redo_labels().is_empty(),
        "the rolled-back steps cannot be redone"
    );
}

#[test]
fn squash_since_makes_a_batch_one_undo_step_and_noops_on_an_empty_suffix() {
    let mut session = loaded();
    add_marker(&mut session, "KEEP").expect("an edit before the batch");
    let depth = session.undo_depth();
    add_marker(&mut session, "B1").expect("batch step 1");
    add_marker(&mut session, "B2").expect("batch step 2");
    assert!(
        session.squash_since(depth, "batch: two steps"),
        "two entries stand above the mark"
    );
    assert_eq!(session.undo_depth(), 2, "the two batch entries became one");
    assert_eq!(
        session.undo().expect("one undo steps back over the batch"),
        "batch: two steps",
        "the squashed label is what undo reports"
    );
    assert_eq!(
        session.working().len(),
        1,
        "both batch edits are gone at once"
    );
    let top = session.undo_depth();
    assert!(
        !session.squash_since(top, "nothing above"),
        "a squash above the newest entry records nothing"
    );
    assert_eq!(
        session.undo_labels(),
        vec!["add KEEP"],
        "a squash above the newest entry records nothing and changes nothing"
    );
}
