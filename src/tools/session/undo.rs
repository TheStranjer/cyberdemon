//! The history side of the session group: `undo` steps back over the
//! working archive's edits, `redo` replays them.
//!
//! [`undo_run`] and [`redo_run`] drive the same walk in opposite directions:
//! [`walk_history`] applies [`HistoryStep`] (`Session::undo` or
//! `Session::redo`) up to [`parse_steps`] counted times, stopping quietly at
//! the bottom of a stack and failing only when not one step landed. After
//! the walk, [`clear_missing_active_map`] drops the active-map selection
//! when the stepped-over working archive no longer leaves that map in the
//! merged view ([`merged_holds_map`] asks [`crate::wad::classify`] over it);
//! layers cannot be undone, so a layered map always survives.
//!
//! Named for the tool, not for [`crate::session::history`], which is the
//! stack itself.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::error::CyberError;
use crate::session::Session;
use crate::session::history::HISTORY_LIMIT;
use crate::wad::classify::analyze;
use crate::wad::{Lump, LumpName};

/// Arguments for [`Cyberdemon::undo`].
#[derive(Debug, Deserialize, JsonSchema)]
pub(super) struct UndoParams {
    /// Number of edits to step back over, `1..=100`; default `1`. When
    /// fewer edits are on the undo stack, undo steps over what exists.
    #[serde(default)]
    pub(super) steps: Option<usize>,
}

/// Arguments for [`Cyberdemon::redo`].
#[derive(Debug, Deserialize, JsonSchema)]
pub(super) struct RedoParams {
    /// Number of undone edits to replay, `1..=100`; default `1`. When
    /// fewer edits are on the redo stack, redo replays what exists.
    #[serde(default)]
    pub(super) steps: Option<usize>,
}

/// Structured result of [`Cyberdemon::undo`].
#[derive(Debug, Serialize)]
pub(super) struct UndoReport {
    /// Labels of the edits stepped back over, most recent first.
    pub(super) undone: Vec<String>,
    /// Entries the undo stack still holds.
    pub(super) remaining_undo: usize,
    /// Entries on the redo stack, how far `redo` can replay.
    pub(super) redo_available: usize,
    /// Whether the working archive now differs from its last saved state.
    pub(super) dirty: bool,
    /// Whether the active map was cleared because the merged view no
    /// longer holds it.
    pub(super) active_map_cleared: bool,
}

/// Structured result of [`Cyberdemon::redo`].
#[derive(Debug, Serialize)]
pub(super) struct RedoReport {
    /// Labels of the edits replayed, in the order they were re-applied.
    pub(super) redone: Vec<String>,
    /// Entries the redo stack still holds.
    pub(super) remaining_redo: usize,
    /// Entries on the undo stack, how far `undo` can step back.
    pub(super) undo_available: usize,
    /// Whether the working archive now differs from its last saved state.
    pub(super) dirty: bool,
    /// Whether the active map was cleared because the merged view no
    /// longer holds it.
    pub(super) active_map_cleared: bool,
}

/// The `steps` argument [`Cyberdemon::undo`] and [`Cyberdemon::redo`]
/// accept: absent means `1`, and only counts up to [`HISTORY_LIMIT`] pass.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] naming the accepted range for `0` or a
/// count past the history limit.
pub(super) fn parse_steps(steps: Option<usize>) -> Result<usize, CyberError> {
    let Some(count) = steps else {
        return Ok(1_usize);
    };
    if count == 0 || count > HISTORY_LIMIT {
        return Err(CyberError::InvalidArgument(format!(
            "steps {count} is outside 1..={HISTORY_LIMIT}: pass the number of edits to step \
             through, or leave steps out for 1"
        )));
    }
    Ok(count)
}

/// One step of history: [`Session::undo`] or [`Session::redo`].
type HistoryStep = fn(&mut Session) -> Result<String, CyberError>;

/// Runs `step` (`Session::undo` or `Session::redo`) up to `count` times and
/// collects the labels it reports, in the order the steps ran. Reaching the
/// bottom of the stack is a normal stop; the refusal only becomes an error
/// when not a single step landed, so the caller reports `not_found` exactly
/// when there was nothing to undo/redo at all.
///
/// # Errors
///
/// Whatever `step` reports on its very first call; later refusals end the
/// walk and the collected labels are returned.
fn walk_history(
    session: &mut Session,
    count: usize,
    step: HistoryStep,
) -> Result<Vec<String>, CyberError> {
    let mut labels: Vec<String> = Vec::new();
    for _step in 0..count {
        match step(session) {
            Ok(label) => labels.push(label),
            Err(error) if labels.is_empty() => return Err(error),
            Err(_) => break,
        }
    }
    Ok(labels)
}

/// Whether the merged view still declares a map block named `map`.
pub(super) fn merged_holds_map(session: &Session, map: LumpName) -> bool {
    let view = session.merged();
    let lumps: Vec<Lump> = view.iter().map(|entry| entry.lump.clone()).collect();
    analyze(&lumps).maps.iter().any(|block| block.name == map)
}

/// Drops the active-map selection when the stepped-back working archive no
/// longer leaves its map in the merged view. Layers cannot be undone, so a
/// map a read-only layer provides always survives the check.
///
/// Reports whether the selection was cleared.
pub(super) fn clear_missing_active_map(session: &mut Session) -> bool {
    let Some(map) = session.active_map() else {
        return false;
    };
    if merged_holds_map(session, map) {
        return false;
    }
    session.set_active_map(None);
    true
}

/// `undo`'s work: step back over history, then report the stacks, the
/// dirty state and whether the active map had to go.
pub(super) fn undo_run(session: &mut Session, steps: usize) -> Result<UndoReport, CyberError> {
    let undone = walk_history(session, steps, Session::undo)?;
    let active_map_cleared = clear_missing_active_map(session);
    Ok(UndoReport {
        undone,
        remaining_undo: session.undo_labels().len(),
        redo_available: session.redo_labels().len(),
        dirty: session.is_dirty(),
        active_map_cleared,
    })
}

/// `redo`'s work: replay undone steps, then report the stacks, the dirty
/// state and whether the active map had to go.
pub(super) fn redo_run(session: &mut Session, steps: usize) -> Result<RedoReport, CyberError> {
    let redone = walk_history(session, steps, Session::redo)?;
    let active_map_cleared = clear_missing_active_map(session);
    Ok(RedoReport {
        redone,
        remaining_redo: session.redo_labels().len(),
        undo_available: session.undo_labels().len(),
        dirty: session.is_dirty(),
        active_map_cleared,
    })
}
