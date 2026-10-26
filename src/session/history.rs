//! Undo and redo history of the session's working archive.
//!
//! [`History`] keeps the states an editing operation left behind, the
//! revision the working archive currently holds and the revision it was last
//! saved at. Revisions come from a counter that is never rewound, so a state
//! can be reached again only by stepping back over history, never by a new
//! edit reusing a number. Dirty is then simply "the current revision is not
//! the saved one": undoing back to the saved state makes a session clean
//! again, and editing after an undo makes it dirty.
//!
//! A batch is one open scope: [`History::begin_batch`] hands out a
//! [`BatchToken`] and, while the scope stands, [`History::record`] folds
//! every edit into the single entry [`History::end_batch`] leaves on the
//! stack (commit) or drops (rollback), so the invariant "a batch is
//! exactly one history entry" is structural rather than a post-hoc squash.
//! While the scope is open the history walkers ([`History::undo`] /
//! [`History::redo`]) refuse, and so does
//! [`Session`](crate::session) for the whole-archive and saved-state swaps
//! a fold cannot carry.

use core::mem;

use crate::error::CyberError;
use crate::wad::archive::Wad;

/// How many undo entries a session keeps before the oldest is dropped.
pub const HISTORY_LIMIT: usize = 100;

/// The handle to one open [`History::begin_batch`] scope, presented back to
/// [`History::end_batch`]. Opaque: a session runs one batch at a time and
/// only the holder of the open batch's token may close it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BatchToken(u64);

/// How [`History::end_batch`] closes an open batch scope.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BatchOutcome {
    /// Keep the batch's edits as the one labelled undo entry.
    Commit,
    /// Restore the exact pre-batch working archive and revision and keep no
    /// entry: the batch is not redoable.
    Rollback,
}

/// The bookkeeping of the one batch scope a [`History`] may hold open.
#[derive(Clone, Debug, PartialEq, Eq)]
struct OpenBatch {
    /// The handle that closes this scope.
    token: BatchToken,
    /// The label of the single entry a committed batch leaves behind.
    label: String,
    /// The working archive as the batch found it.
    before: Wad,
    /// The revision as the batch found it.
    revision: u64,
    /// Whether any edit has folded in since the scope opened.
    edited: bool,
}

/// The `conflict` refusal an operation takes because a batch scope owns the
/// session until it ends. `what` names the operation in the message.
pub(crate) fn batch_open_conflict(what: &str) -> CyberError {
    CyberError::Conflict(format!("{what} cannot run while a batch is open")).with_hint(
        "the open run_batch owns the session's history until it ends; run this after the batch",
    )
}

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
    /// The one open batch scope, folding every edit into one entry.
    batch: Option<OpenBatch>,
    /// The counter batch tokens come from; never reused.
    batch_counter: u64,
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
    ///
    /// While a batch scope is open no entry is pushed: the edit folds into
    /// the single entry [`History::end_batch`] will settle, whose label is
    /// the batch's own, not `label`. Every other bookkeeping — the dropped
    /// redo branch, the fresh revision — happens exactly as for a plain
    /// record.
    #[inline]
    pub fn record(&mut self, label: &str, working: &mut Wad, edited: Wad) {
        let previous = mem::replace(working, edited);
        match self.batch.as_mut() {
            Some(batch) => batch.edited = true,
            None => self.undo.push(Entry {
                label: label.to_owned(),
                wad: previous,
                revision: self.revision,
            }),
        }
        self.redo.clear();
        self.revision = self.next_revision();
        while self.undo.len() > HISTORY_LIMIT {
            self.undo.remove(0);
        }
    }

    /// Opens a batch scope: every [`History::record`] from here until
    /// [`History::end_batch`] folds into one entry labelled `label`, whose
    /// snapshot is the `working` archive as it stands now (which the scope
    /// takes) at the revision it stands at now.
    ///
    /// # Errors
    ///
    /// [`CyberError::Conflict`] when a batch is already open: a session
    /// runs one batch at a time and nesting would fold two owners' edits
    /// into one entry.
    #[inline]
    pub fn begin_batch(&mut self, label: &str, working: &Wad) -> Result<BatchToken, CyberError> {
        if self.batch.is_some() {
            return Err(batch_open_conflict("opening a second batch"));
        }
        self.batch_counter = self.batch_counter.saturating_add(1);
        let token = BatchToken(self.batch_counter);
        self.batch = Some(OpenBatch {
            token,
            label: label.to_owned(),
            before: working.clone(),
            revision: self.revision,
            edited: false,
        });
        Ok(token)
    }

    /// Whether a batch scope currently owns this history.
    #[must_use]
    #[inline]
    pub const fn batch_open(&self) -> bool {
        self.batch.is_some()
    }

    /// Closes the batch scope `token` opened, applying `outcome` to the
    /// `working` archive: a commit leaves (at most) the one entry labelled
    /// with the batch's label, a rollback restores the exact pre-batch
    /// archive and revision and leaves no entry. Returns whether the batch
    /// left an undo entry, so a scope whose steps recorded no edits is
    /// invisible in the history.
    ///
    /// # Errors
    ///
    /// [`CyberError::Conflict`] when no batch is open or `token` is not the
    /// open batch's handle; the scope (if any) stays open.
    #[inline]
    pub fn end_batch(
        &mut self,
        working: &mut Wad,
        token: BatchToken,
        outcome: BatchOutcome,
    ) -> Result<bool, CyberError> {
        let batch = self.take_batch(token)?;
        if !batch.edited {
            return Ok(false);
        }
        match outcome {
            BatchOutcome::Commit => {
                self.settle_batch(batch);
                Ok(true)
            }
            BatchOutcome::Rollback => {
                *working = batch.before;
                self.revision = batch.revision;
                Ok(false)
            }
        }
    }

    /// Leaves the open batch's single entry on the undo stack: the folded
    /// edits undo (and redo) as one step under the batch's label.
    fn settle_batch(&mut self, batch: OpenBatch) {
        self.undo.push(Entry {
            label: batch.label,
            wad: batch.before,
            revision: batch.revision,
        });
        while self.undo.len() > HISTORY_LIMIT {
            self.undo.remove(0);
        }
    }

    /// Takes the open batch out, refusing when `token` does not name it.
    /// A refusal puts the scope back: only its own holder may close it.
    fn take_batch(&mut self, token: BatchToken) -> Result<OpenBatch, CyberError> {
        match self.batch.take() {
            Some(batch) if batch.token == token => Ok(batch),
            Some(batch) => {
                self.batch = Some(batch);
                Err(
                    CyberError::Conflict("this handle does not name the open batch".to_owned())
                        .with_hint("end the batch with the token its begin_batch returned"),
                )
            }
            None => Err(CyberError::Conflict("no batch is open".to_owned())
                .with_hint("begin_batch first; end_batch closes the scope it opened")),
        }
    }

    /// Restores the state before the most recent edit and returns its label.
    ///
    /// # Errors
    ///
    /// [`CyberError::NotFound`] with `"nothing to undo"` when the undo stack
    /// is empty, [`CyberError::Conflict`] while a batch scope is open.
    #[inline]
    pub fn undo(&mut self, working: &mut Wad) -> Result<String, CyberError> {
        if self.batch_open() {
            return Err(batch_open_conflict("undo"));
        }
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
    /// is empty, [`CyberError::Conflict`] while a batch scope is open.
    #[inline]
    pub fn redo(&mut self, working: &mut Wad) -> Result<String, CyberError> {
        if self.batch_open() {
            return Err(batch_open_conflict("redo"));
        }
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
    /// The batch tool reads this before opening its scope to refuse a batch
    /// whose steps would not fit under [`HISTORY_LIMIT`] at all; a folded
    /// batch itself only ever adds the one entry on commit.
    #[must_use]
    #[inline]
    pub fn undo_depth(&self) -> usize {
        self.undo.len()
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
    /// replaced wholesale, which is clean from the first moment. The
    /// session's guard keeps this out of reach while a batch scope is open.
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
        assert!(!history.batch_open(), "no batch scope owns it");
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
    fn rollback_is_replaced_by_the_batch_scope_which_keeps_older_redo_entries() {
        let mut working = wad("START");
        let mut history = History::default();
        edits(&mut history, &mut working, 3);
        history.undo(&mut working).expect("undo edit-3");
        assert_eq!(history.redo_labels(), vec!["edit-3"], "a branch to replay");
        let token = history
            .begin_batch("batch", &working)
            .expect("no batch is open yet");
        history.record("inner", &mut working, wad("BATCHED"));
        let rolled = history
            .end_batch(&mut working, token, super::BatchOutcome::Rollback)
            .expect("the token names the open batch");
        assert!(!rolled, "a rolled-back batch leaves no entry");
        assert_eq!(only(&working), "LUMP2", "the pre-batch state is back");
        assert_eq!(
            history.undo_labels(),
            vec!["edit-2", "edit-1"],
            "only the entries below the batch remain"
        );
        assert!(
            history.redo_labels().is_empty(),
            "the folded edit cleared the branch, and a rollback is not redoable"
        );
        assert!(
            history.is_dirty(),
            "rollback is not a new revision: the saved state was the one it left"
        );
    }

    #[test]
    fn a_committed_batch_folds_every_edit_into_one_undo_and_redo_step() {
        let mut working = wad("START");
        let mut history = History::default();
        edits(&mut history, &mut working, 2);
        history.mark_saved();
        let token = history
            .begin_batch("batch: three steps", &working)
            .expect("no batch is open yet");
        assert!(history.batch_open(), "the scope owns the history now");
        for step in 3_usize..=5_usize {
            let state = wad(&format!("LUMP{step}"));
            history.record(&format!("edit-{step}"), &mut working, state);
        }
        assert_eq!(
            history.undo_labels(),
            vec!["edit-2", "edit-1"],
            "mid-batch no inner entries stand on the stack"
        );
        let left = history
            .end_batch(&mut working, token, super::BatchOutcome::Commit)
            .expect("the token names the open batch");
        assert!(left, "the batch's edits stand as one entry");
        assert_eq!(
            history.undo_labels(),
            vec!["batch: three steps", "edit-2", "edit-1"],
            "three inner labels folded into the batch's one"
        );
        assert!(history.is_dirty(), "the folded stretch is unsaved work");
        let label = history.undo(&mut working).expect("undo the batch");
        assert_eq!(label, "batch: three steps", "the batch label is reported");
        assert_eq!(
            only(&working),
            "LUMP2",
            "the state before the first folded edit",
        );
        assert!(!history.is_dirty(), "one undo reached the saved revision");
        assert_eq!(
            history.redo_labels(),
            vec!["batch: three steps"],
            "the whole folded stretch redoes at once"
        );
        history.redo(&mut working).expect("redo the batch");
        assert_eq!(only(&working), "LUMP5", "and lands on the batch's result");
    }

    #[test]
    fn a_batch_that_recorded_nothing_leaves_no_entry_either_way() {
        let mut working = wad("START");
        let mut history = History::default();
        edits(&mut history, &mut working, 3);
        let quiet = history
            .begin_batch("empty batch", &working)
            .expect("no batch is open yet");
        let left = history
            .end_batch(&mut working, quiet, super::BatchOutcome::Commit)
            .expect("the token names the open batch");
        assert!(!left, "nothing recorded, nothing squashed");
        assert_eq!(
            history.undo_labels(),
            vec!["edit-3", "edit-2", "edit-1"],
            "the stack stands as it was"
        );
        let also_quiet = history
            .begin_batch("empty batch", &working)
            .expect("the scope closed and may reopen");
        let also_left = history
            .end_batch(&mut working, also_quiet, super::BatchOutcome::Rollback)
            .expect("the token names the open batch");
        assert!(!also_left, "a rollback with nothing to undo leaves nothing");
        assert_eq!(only(&working), "LUMP3", "and changes nothing");
    }

    #[test]
    fn the_batch_scope_refuses_a_second_begin_and_a_stale_end() {
        let mut working = wad("START");
        let mut history = History::default();
        let open = history
            .begin_batch("first", &working)
            .expect("a fresh history holds no batch");
        let nested = history
            .begin_batch("second", &working)
            .expect_err("a session runs one batch at a time");
        assert_eq!(nested.code(), "conflict", "nested begin code");
        assert!(
            nested.to_string().contains("batch is open"),
            "the message says why: {nested}"
        );
        assert!(
            history.batch_open(),
            "the refusal left the first batch open"
        );
        history
            .end_batch(&mut working, open, super::BatchOutcome::Commit)
            .expect("the holder's own token closes it");
        let later = history
            .begin_batch("second run", &working)
            .expect("a closed scope frees the history");
        let stale = history
            .end_batch(&mut working, open, super::BatchOutcome::Commit)
            .expect_err("a used token does not name the open batch");
        assert_eq!(stale.code(), "conflict", "stale token code");
        assert!(
            stale.to_string().contains("does not name"),
            "the message says why: {stale}"
        );
        assert!(history.batch_open(), "the refusal keeps the new scope open");
        let left = history
            .end_batch(&mut working, later, super::BatchOutcome::Commit)
            .expect("the current token closes it");
        assert!(!left, "nothing was recorded");
        let absent = history
            .end_batch(&mut working, later, super::BatchOutcome::Rollback)
            .expect_err("the scope is already closed");
        assert_eq!(absent.code(), "conflict", "no-batch end code");
        assert!(
            absent.to_string().contains("no batch is open"),
            "the message says why: {absent}"
        );
    }

    #[test]
    fn undo_and_redo_refuse_while_a_batch_is_open() {
        let mut working = wad("START");
        let mut history = History::default();
        edits(&mut history, &mut working, 2);
        let token = history
            .begin_batch("batch", &working)
            .expect("no batch is open yet");
        let undo = history
            .undo(&mut working)
            .expect_err("the batch owns the stack");
        assert_eq!(undo.code(), "conflict", "undo inside a batch code");
        assert!(
            undo.to_string().contains("undo"),
            "the message names the operation: {undo}"
        );
        let redo = history
            .redo(&mut working)
            .expect_err("and the same for redo");
        assert!(
            redo.to_string().contains("redo"),
            "the message names the operation: {redo}"
        );
        assert_eq!(only(&working), "LUMP2", "the refusals changed nothing");
        history
            .end_batch(&mut working, token, super::BatchOutcome::Commit)
            .expect("close the scope");
        assert_eq!(history.undo(&mut working).expect("now it walks"), "edit-2");
    }

    #[test]
    fn the_history_limit_holds_at_the_batch_scope() {
        let mut working = wad("START");
        let mut history = History::default();
        let total = HISTORY_LIMIT
            .checked_add(5)
            .expect("a count past the limit");
        edits(&mut history, &mut working, total);
        assert_eq!(history.undo_depth(), HISTORY_LIMIT, "the stack is capped");
        let token = history
            .begin_batch("capped batch", &working)
            .expect("no batch is open yet");
        history.record("inner", &mut working, wad("FINAL"));
        history
            .end_batch(&mut working, token, super::BatchOutcome::Commit)
            .expect("close the scope");
        assert_eq!(
            history.undo_depth(),
            HISTORY_LIMIT,
            "committing the folded entry holds the cap"
        );
        assert_eq!(
            history.undo_labels().first().copied(),
            Some("capped batch"),
            "the batch entry is the newest"
        );
        let undone = history.undo(&mut working).expect("undo the batch");
        assert_eq!(undone, "capped batch", "one step back over the fold");
        assert_eq!(
            only(&working),
            format!("LUMP{total}"),
            "the snapshot the batch took at its start",
        );
        let rolled = HISTORY_LIMIT
            .checked_sub(1)
            .expect("a depth inside the capped stack");
        assert_eq!(
            history.undo_depth(),
            rolled,
            "the batch entry went with the undo"
        );
    }
}
