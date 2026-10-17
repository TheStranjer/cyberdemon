//! Undo and redo history of the session's working archive.
//!
//! [`History`] keeps the states an editing operation left behind, the
//! revision the working archive currently holds and the revision it was last
//! saved at. Revisions come from a counter that is never rewound, so a state
//! can be reached again only by stepping back over history, never by a new
//! edit reusing a number. Dirty is then simply "the current revision is not
//! the saved one": undoing back to the saved state makes a session clean
//! again, and editing after an undo makes it dirty.

use core::mem;

use crate::error::CyberError;
use crate::wad::archive::Wad;

/// How many undo entries a session keeps before the oldest is dropped.
pub const HISTORY_LIMIT: usize = 100;

/// One point on the undo or the redo stack.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Entry {
    /// The label of the edit on the far side of this entry: what undoing or
    /// redoing it reports.
    label: String,
    /// The working archive to restore.
    wad: Wad,
    /// The revision that goes with `wad`.
    revision: u64,
}

/// The undo and redo stacks plus the revision bookkeeping of one session.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct History {
    /// States to step back to, oldest first.
    undo: Vec<Entry>,
    /// States stepped back over, oldest first.
    redo: Vec<Entry>,
    /// The revision the working archive currently holds.
    revision: u64,
    /// The revision the working archive was last saved at.
    saved_revision: u64,
    /// The counter every fresh revision comes from; never rewound.
    counter: u64,
}

impl History {
    /// The next revision out of the monotonic counter.
    const fn next_revision(&mut self) -> u64 {
        self.counter = self.counter.saturating_add(1);
        self.counter
    }

    /// Records that `working` changed into `edited` under `label`.
    ///
    /// The state `working` held before the call becomes the new undo entry,
    /// so a later [`History::undo`] restores it, and the redo stack is
    /// dropped: a new edit after an undo starts a new branch. The oldest
    /// entry is dropped past [`HISTORY_LIMIT`] steps.
    #[inline]
    pub fn record(&mut self, label: &str, working: &mut Wad, edited: Wad) {
        let previous = mem::replace(working, edited);
        self.undo.push(Entry {
            label: label.to_owned(),
            wad: previous,
            revision: self.revision,
        });
        self.redo.clear();
        self.revision = self.next_revision();
        while self.undo.len() > HISTORY_LIMIT {
            self.undo.remove(0);
        }
    }

    /// Restores the state before the most recent edit and returns its label.
    ///
    /// # Errors
    ///
    /// [`CyberError::NotFound`] with `"nothing to undo"` when the undo stack
    /// is empty.
    #[inline]
    pub fn undo(&mut self, working: &mut Wad) -> Result<String, CyberError> {
        let entry = self
            .undo
            .pop()
            .ok_or_else(|| CyberError::NotFound("nothing to undo".to_owned()))?;
        let label = entry.label;
        let left = mem::replace(working, entry.wad);
        self.redo.push(Entry {
            label: label.clone(),
            wad: left,
            revision: self.revision,
        });
        self.revision = entry.revision;
        Ok(label)
    }

    /// Re-applies the most recently undone edit and returns its label.
    ///
    /// # Errors
    ///
    /// [`CyberError::NotFound`] with `"nothing to redo"` when the redo stack
    /// is empty.
    #[inline]
    pub fn redo(&mut self, working: &mut Wad) -> Result<String, CyberError> {
        let entry = self
            .redo
            .pop()
            .ok_or_else(|| CyberError::NotFound("nothing to redo".to_owned()))?;
        let label = entry.label;
        let left = mem::replace(working, entry.wad);
        self.undo.push(Entry {
            label: label.clone(),
            wad: left,
            revision: self.revision,
        });
        self.revision = entry.revision;
        Ok(label)
    }

    /// How many undo entries the stack currently holds.
    ///
    /// A batch runner captures this before its first step and later passes
    /// it back to [`History::rollback_to`] or [`History::squash_since`]; as
    /// long as no step drops entries at the [`HISTORY_LIMIT`] bottom, the
    /// entries above the mark are exactly the ones the batch recorded.
    #[must_use]
    #[inline]
    pub fn undo_depth(&self) -> usize {
        self.undo.len()
    }

    /// Undoes the newest entry into `working` without leaving a redo trail.
    ///
    /// Unlike [`History::undo`], the state being stepped away from is
    /// dropped rather than pushed onto redo: a rolled-back batch must not
    /// be replayable. Returns whether an entry was there to take.
    fn undo_one(&mut self, working: &mut Wad) -> bool {
        let Some(entry) = self.undo.pop() else {
            return false;
        };
        *working = entry.wad;
        self.revision = entry.revision;
        true
    }

    /// Steps back over every undo entry above `depth`, restoring the
    /// working archive, and discards those entries: the states the rollback
    /// steps through are never redoable. Redo entries made before the
    /// rollback stay on the redo stack, and a `depth` at or past the
    /// stack's length rolls nothing. Returns the number of entries undone.
    #[inline]
    pub fn rollback_to(&mut self, working: &mut Wad, depth: usize) -> usize {
        let mut undone = 0_usize;
        while self.undo.len() > depth {
            // The length test guarantees `undo_one` finds an entry and the
            // pop shrinks the stack, so the loop always advances.
            undone = undone.saturating_add(usize::from(self.undo_one(working)));
        }
        undone
    }

    /// Replaces every undo entry above `depth` with one entry whose
    /// snapshot is the state before the first of them and whose label is
    /// `label`: the whole stretch undoes (and redoes) in a single step. A
    /// no-op when nothing stands above `depth`, so a batch that recorded
    /// nothing stays invisible in the history. Returns whether an entry was
    /// replaced.
    #[inline]
    pub fn squash_since(&mut self, depth: usize, label: &str) -> bool {
        let Some(anchor) = self.undo.get(depth) else {
            return false;
        };
        let squashed = Entry {
            label: label.to_owned(),
            wad: anchor.wad.clone(),
            revision: anchor.revision,
        };
        self.undo.truncate(depth);
        self.undo.push(squashed);
        true
    }

    /// The labels of the recorded edits, newest first.
    #[must_use]
    #[inline]
    pub fn undo_labels(&self) -> Vec<&str> {
        labels(&self.undo)
    }

    /// The labels of the undone edits, newest first.
    #[must_use]
    #[inline]
    pub fn redo_labels(&self) -> Vec<&str> {
        labels(&self.redo)
    }

    /// Marks the working archive's current state as the saved one.
    #[inline]
    pub const fn mark_saved(&mut self) {
        self.saved_revision = self.revision;
    }

    /// Whether the working archive differs from the last saved state.
    #[must_use]
    #[inline]
    pub const fn is_dirty(&self) -> bool {
        self.revision != self.saved_revision
    }

    /// Drops both stacks and starts a fresh revision for a working archive
    /// replaced wholesale, which is clean from the first moment.
    #[inline]
    pub fn reset(&mut self) {
        self.undo.clear();
        self.redo.clear();
        self.revision = self.next_revision();
        self.saved_revision = self.revision;
    }
}

/// The labels of `stack`, newest first.
fn labels(stack: &[Entry]) -> Vec<&str> {
    stack
        .iter()
        .rev()
        .map(|entry| entry.label.as_str())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{Entry, HISTORY_LIMIT, History, labels};
    use crate::wad::archive::{Lump, Wad, WadKind};
    use crate::wad::name::LumpName;

    /// A valid parsed name for test data.
    fn name(text: &str) -> LumpName {
        LumpName::parse(text).expect("test names are valid")
    }

    /// A one-lump archive named after `text`, so states are distinguishable.
    fn wad(text: &str) -> Wad {
        Wad {
            kind: WadKind::Pwad,
            lumps: vec![Lump::marker(name(text))],
        }
    }

    /// Applies edits labelled `edit-1` to `edit-count`.
    fn edits(history: &mut History, working: &mut Wad, count: usize) {
        for step in 1..=count {
            let label = format!("edit-{step}");
            let state = wad(&format!("LUMP{step}"));
            history.record(label.as_str(), working, state);
        }
    }

    /// The lump name the archive holds, or `-` when empty.
    fn only(working: &Wad) -> String {
        working
            .lumps
            .first()
            .map_or_else(|| "-".to_owned(), |lump| lump.name.to_string())
    }

    #[test]
    fn a_fresh_history_is_clean_and_empty() {
        let history = History::default();
        assert!(!history.is_dirty(), "nothing has changed yet");
        assert!(history.undo_labels().is_empty(), "nothing to undo");
        assert!(history.redo_labels().is_empty(), "nothing to redo");
    }

    #[test]
    fn record_adopts_the_new_state_and_marks_dirty() {
        let mut working = wad("START");
        let mut history = History::default();
        history.record("added a floor", &mut working, wad("FLOOR"));
        assert_eq!(only(&working), "FLOOR", "the edit is committed");
        assert_eq!(
            history.undo_labels(),
            vec!["added a floor"],
            "the label is recorded"
        );
        assert!(history.is_dirty(), "an edit is unsaved work");
    }

    #[test]
    fn mark_saved_clears_dirty_and_record_sets_it_again() {
        let mut working = wad("START");
        let mut history = History::default();
        history.record("edit-1", &mut working, wad("ONE"));
        history.mark_saved();
        assert!(!history.is_dirty(), "saving makes the state clean");
        history.record("edit-2", &mut working, wad("TWO"));
        assert!(history.is_dirty(), "a new edit is unsaved again");
    }

    #[test]
    fn undo_and_redo_round_trip_the_state_and_label() {
        let mut working = wad("START");
        let mut history = History::default();
        history.record("edit-1", &mut working, wad("ONE"));
        history.record("edit-2", &mut working, wad("TWO"));
        assert_eq!(history.undo(&mut working).expect("one step"), "edit-2");
        assert_eq!(only(&working), "ONE", "the previous state is back");
        assert_eq!(history.undo(&mut working).expect("two steps"), "edit-1");
        assert_eq!(only(&working), "START", "back to the beginning");
        assert!(history.undo(&mut working).is_err(), "the stack is empty");
        assert_eq!(history.redo(&mut working).expect("replay one"), "edit-1");
        assert_eq!(only(&working), "ONE", "redo restored the edit");
        assert_eq!(history.redo(&mut working).expect("replay two"), "edit-2");
        assert_eq!(only(&working), "TWO", "the whole branch is back");
        assert!(history.redo(&mut working).is_err(), "nothing left to redo");
    }

    #[test]
    fn undoing_to_the_saved_state_is_clean_and_editing_again_is_not() {
        let mut working = wad("START");
        let mut history = History::default();
        history.record("edit-1", &mut working, wad("ONE"));
        history.mark_saved();
        history.record("edit-2", &mut working, wad("TWO"));
        assert!(history.is_dirty(), "edit-2 is unsaved");
        history.undo(&mut working).expect("back to the save");
        assert!(!history.is_dirty(), "undo reached the saved revision");
        history.record("edit-3", &mut working, wad("THREE"));
        assert!(history.is_dirty(), "a new revision is not the saved one");
        assert!(history.redo_labels().is_empty(), "the old branch is gone");
    }

    #[test]
    fn reset_clears_both_stacks_and_starts_clean() {
        let mut working = wad("START");
        let mut history = History::default();
        history.record("edit-1", &mut working, wad("ONE"));
        history.undo(&mut working).expect("one undo");
        history.reset();
        assert!(!history.is_dirty(), "a replaced archive is clean");
        assert!(history.undo_labels().is_empty(), "its past is gone");
        assert!(history.redo_labels().is_empty(), "and its future too");
        history
            .undo(&mut working)
            .expect_err("reset really emptied the stack");
    }

    #[test]
    fn the_undo_stack_keeps_the_newest_entries_and_drops_the_oldest() {
        let mut working = wad("START");
        let mut history = History::default();
        let total = HISTORY_LIMIT
            .checked_add(5)
            .expect("a test count past the limit");
        edits(&mut history, &mut working, total);
        let kept = history.undo_labels();
        assert_eq!(kept.len(), HISTORY_LIMIT, "the depth limit holds");
        let first = total.checked_sub(HISTORY_LIMIT).expect("some were dropped");
        let oldest = first.checked_add(1).expect("the oldest kept step");
        let newest = format!("edit-{total}");
        let oldest_label = format!("edit-{oldest}");
        assert_eq!(
            kept.first().copied().expect("a newest label"),
            newest.as_str()
        );
        assert_eq!(
            kept.last().copied().expect("an oldest kept label"),
            oldest_label.as_str(),
            "exactly the oldest entries were dropped"
        );
        assert!(!kept.contains(&"edit-1"), "the first edit is gone");
        for step in (oldest..=total).rev() {
            let label = format!("edit-{step}");
            let undone = history.undo(&mut working).expect("a step back");
            assert_eq!(undone, label, "undo replays the kept stack, newest first");
        }
        assert!(
            history.undo(&mut working).is_err(),
            "exactly the limit of steps"
        );
        assert_eq!(
            history.redo_labels().len(),
            HISTORY_LIMIT,
            "and the same number can be replayed"
        );
    }

    #[test]
    fn revisions_are_never_reused_across_branches() {
        let mut working = wad("START");
        let mut history = History::default();
        history.record("edit-1", &mut working, wad("ONE"));
        history.record("edit-2", &mut working, wad("TWO"));
        history.undo(&mut working).expect("back one");
        history.undo(&mut working).expect("back to the start");
        history.record("edit-3", &mut working, wad("THREE"));
        assert!(
            history.is_dirty(),
            "the fresh revision is not the first one"
        );
        history.undo(&mut working).expect("undo edit-3");
        assert!(!history.is_dirty(), "revision 0 is clean again");
    }

    #[test]
    fn labels_are_reported_newest_first() {
        let stack = vec![
            Entry {
                label: "old".to_owned(),
                wad: wad("A"),
                revision: 1,
            },
            Entry {
                label: "new".to_owned(),
                wad: wad("B"),
                revision: 2,
            },
        ];
        assert_eq!(labels(&stack), vec!["new", "old"], "newest first");
        assert!(
            labels(&Vec::new()).is_empty(),
            "an empty stack has no labels"
        );
    }

    #[test]
    fn the_history_limit_is_what_the_tools_document() {
        assert_eq!(HISTORY_LIMIT, 100, "a hundred undo steps");
    }

    #[test]
    fn undo_depth_counts_the_recorded_entries_only() {
        let mut working = wad("START");
        let mut history = History::default();
        assert_eq!(history.undo_depth(), 0, "a fresh stack is empty");
        edits(&mut history, &mut working, 3);
        assert_eq!(history.undo_depth(), 3, "three edits, three entries");
        history.undo(&mut working).expect("step back");
        assert_eq!(history.undo_depth(), 2, "an undone entry is not depth");
    }

    #[test]
    fn rollback_to_undoes_above_depth_and_discards_the_redo_it_makes() {
        let mut working = wad("START");
        let mut history = History::default();
        edits(&mut history, &mut working, 5);
        history.mark_saved();
        let undone = history.rollback_to(&mut working, 2);
        assert_eq!(undone, 3, "entries three, four and five rolled back");
        assert_eq!(only(&working), "LUMP2", "the state after edit-2 is back");
        assert_eq!(
            history.undo_labels(),
            vec!["edit-2", "edit-1"],
            "only the entries below the mark remain"
        );
        assert!(
            history.redo_labels().is_empty(),
            "a rollback is not redoable"
        );
        history
            .redo(&mut working)
            .expect_err("the rolled-back states cannot be replayed");
        assert!(
            history.is_dirty(),
            "rollback is not a new revision: the saved state was the one it left"
        );
    }

    #[test]
    fn rollback_to_keeps_older_redo_entries_and_rolls_nothing_past_the_stack() {
        let mut working = wad("START");
        let mut history = History::default();
        edits(&mut history, &mut working, 3);
        history.undo(&mut working).expect("undo edit-3");
        assert_eq!(history.redo_labels(), vec!["edit-3"], "a branch to replay");
        let undone = history.rollback_to(&mut working, 1);
        assert_eq!(undone, 1, "only edit-2's entry stood above the mark");
        assert_eq!(only(&working), "LUMP1", "the state after edit-1 is back");
        assert_eq!(
            history.redo_labels(),
            vec!["edit-3"],
            "the redo entry made before the rollback survives"
        );
        let past = history.undo_depth().checked_add(9).expect("a deep mark");
        assert_eq!(
            history.rollback_to(&mut working, past),
            0,
            "a depth past the stack rolls nothing"
        );
        assert_eq!(only(&working), "LUMP1", "and changes nothing");
    }

    #[test]
    fn squash_since_replaces_the_entries_above_depth_with_one() {
        let mut working = wad("START");
        let mut history = History::default();
        edits(&mut history, &mut working, 2);
        history.mark_saved();
        for step in 3_usize..=5_usize {
            let label = format!("edit-{step}");
            let state = wad(&format!("LUMP{step}"));
            history.record(label.as_str(), &mut working, state);
        }
        assert!(
            history.squash_since(2, "batch: edit-3, edit-4, edit-5"),
            "three entries stand above the mark"
        );
        assert_eq!(
            history.undo_labels(),
            vec!["batch: edit-3, edit-4, edit-5", "edit-2", "edit-1"],
            "three entries became one above the older two"
        );
        assert!(history.is_dirty(), "the squashed stretch is unsaved work");
        let label = history.undo(&mut working).expect("undo the squash");
        assert_eq!(label, "batch: edit-3, edit-4, edit-5", "the given label");
        assert_eq!(
            only(&working),
            "LUMP2",
            "the state before the first squashed edit",
        );
        assert!(!history.is_dirty(), "one undo reached the saved revision");
        assert_eq!(
            history.redo_labels(),
            vec!["batch: edit-3, edit-4, edit-5"],
            "the whole squashed stretch redoes at once"
        );
        history.redo(&mut working).expect("redo the squash");
        assert_eq!(
            only(&working),
            "LUMP5",
            "and lands back on the batch's result"
        );
    }

    #[test]
    fn squash_since_is_a_noop_when_nothing_stands_above_depth() {
        let mut working = wad("START");
        let mut history = History::default();
        assert!(
            !history.squash_since(0, "empty batch"),
            "nothing recorded, nothing squashed"
        );
        edits(&mut history, &mut working, 3);
        assert!(
            !history.squash_since(3, "no steps"),
            "a depth at the newest entry squashes nothing"
        );
        assert!(
            !history.squash_since(10, "past the stack"),
            "a depth past the stack squashes nothing"
        );
        assert_eq!(
            history.undo_labels(),
            vec!["edit-3", "edit-2", "edit-1"],
            "a no-op squash changes nothing"
        );
    }

    #[test]
    fn rollback_and_squash_hold_at_the_history_limit() {
        let mut working = wad("START");
        let mut history = History::default();
        let total = HISTORY_LIMIT
            .checked_add(5)
            .expect("a count past the limit");
        edits(&mut history, &mut working, total);
        assert_eq!(history.undo_depth(), HISTORY_LIMIT, "the stack is capped");
        let top = HISTORY_LIMIT
            .checked_sub(5)
            .expect("a depth inside the capped stack");
        let undone = history.rollback_to(&mut working, top);
        assert_eq!(undone, 5, "exactly the newest entries roll back");
        assert_eq!(history.undo_depth(), top, "the mark holds");
        assert!(
            history.squash_since(0, "the whole kept history"),
            "the capped stack has entries above the origin"
        );
        assert_eq!(history.undo_depth(), 1, "the capped stack squashes to one");
        let label = history
            .undo(&mut working)
            .expect("undo the squashed history");
        assert_eq!(label, "the whole kept history", "under the given label");
        assert_eq!(
            only(&working),
            "LUMP5",
            "the oldest snapshot the limit left behind",
        );
    }
}
