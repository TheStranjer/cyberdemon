//! `edit_dehacked`'s write: one patch lump replaced wholesale or line by
//! line, committed as exactly one undoable edit.
//!
//! [`plan`] parses the arguments into a [`Draft`] outside any lock,
//! [`resolve`] finishes it (the `source` mode's fetch, read with
//! [`super::decode_patch`] so every fetched byte survives), and [`edit_run`]
//! computes the new text - [`apply_ops`] over [`current_text`] for the line
//! modes, the given text wholesale for the others - counts the change with
//! the minimal line diff [`super::line_diff`] and commits the
//! [`super::encode_patch`]ed bytes through one
//! [`crate::session::Session::edit`] labelled [`super::UNDO_LABEL`], in
//! place when the working archive already holds the lump and as an appended
//! override otherwise ([`write_lump`], which the two script tools reuse).
//! The read half lives in [`mod@super::get_dehacked`] and the router in
//! [`super`].

use alloc::sync::Arc;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::defs::dehacked_edit;
use crate::error::CyberError;
use crate::session::Session;
use crate::state::AppState;
use crate::wad::archive::{Lump, Wad};
use crate::wad::name::LumpName;

use super::line_diff::line_diff;
use super::{UNDO_LABEL, decode_patch, encode_patch};

/// Arguments for [`Cyberdemon::edit_dehacked`]: exactly one of the four
/// modes, named by the field that is present.
#[derive(Debug, Deserialize, JsonSchema)]
pub(super) struct EditDehackedParams {
    /// Patch lump to edit: 1 to 8 printable `ASCII` characters, matched
    /// case-insensitively. Defaults to `"DEHACKED"`.
    #[serde(default)]
    pub(super) name: Option<String>,
    /// Replace the whole lump text with this, stored byte-preserving
    /// (`Latin-1` one-byte characters, wider ones as `UTF-8`). Exactly one
    /// mode.
    #[serde(default)]
    pub(super) text: Option<String>,
    /// A `.deh`/`.bex` file (absolute path, `file://` URI, `http(s)://`
    /// URL or uploaded asset URL) whose bytes replace the lump text
    /// verbatim. Exactly one mode.
    #[serde(default)]
    pub(super) source: Option<String>,
    /// Line updates to apply in order. Exactly one mode.
    #[serde(default)]
    pub(super) set: Option<Vec<SetEntry>>,
    /// Line removals to apply in order. Exactly one mode.
    #[serde(default)]
    pub(super) remove: Option<Vec<RemoveEntry>>,
}

/// One `set` entry: a key of a named block or BEX section.
#[derive(Debug, Deserialize, JsonSchema)]
pub(super) struct SetEntry {
    /// The block or section, e.g. `"Thing 12"`, `"Misc"` or `"[STRINGS]"`
    /// (case-insensitive; the first matching block wins).
    pub(super) block: String,
    /// The key to update or insert (case-insensitive match; the written
    /// line uses this spelling).
    pub(super) key: String,
    /// The new value; must not contain newlines. A character up to
    /// `U+00FF` is stored as its one `Latin-1` byte.
    pub(super) value: String,
}

/// One `remove` entry: a key of a named block, or the whole block.
#[derive(Debug, Deserialize, JsonSchema)]
pub(super) struct RemoveEntry {
    /// The block or section to take lines from, same spelling as in `set`.
    pub(super) block: String,
    /// The key line to remove; omit it to remove the whole block, header
    /// through its last line.
    #[serde(default)]
    pub(super) key: Option<String>,
}

/// Structured result of [`Cyberdemon::edit_dehacked`].
#[derive(Serialize)]
pub(super) struct EditReport {
    /// The lump that was written.
    pub(super) name: LumpName,
    /// Total lines of the resulting text.
    pub(super) lines: usize,
    /// Lines the resulting text gained, from the minimal line diff against
    /// the old text.
    pub(super) added: usize,
    /// Lines the resulting text lost, from the same diff.
    pub(super) removed: usize,
    /// Lines the same diff pairs off as rewritten in place.
    pub(super) changed: usize,
    /// Whether the lump was created in the working archive (it was absent,
    /// or held only by a read-only layer that was copied-on-write).
    pub(super) created: bool,
}

/// The parsed mode of one `edit_dehacked` call.
#[derive(Debug)]
pub(super) enum Draft {
    /// The new full text is already in hand.
    Replace(String),
    /// Fetch this source and use its bytes as the new full text.
    Fetch(String),
    /// Apply these line operations to the current text.
    Ops(Vec<Op>),
}

/// One validated line operation.
#[derive(Clone, Debug)]
pub(super) enum Op {
    /// Update or insert one key, creating the block when missing.
    Set {
        /// The target block or section spec.
        block: String,
        /// The key to set.
        key: String,
        /// The value to write.
        value: String,
    },
    /// Remove one key, or the whole block when `key` is `None`.
    Remove {
        /// The target block or section spec.
        block: String,
        /// The key to remove, `None` for the whole block.
        key: Option<String>,
    },
}

impl Op {
    /// Whether this operation removes lines (it cannot create a patch).
    const fn is_remove(&self) -> bool {
        matches!(self, Self::Remove { .. })
    }

    /// Runs the operation over a patch text, answering the new text.
    ///
    /// # Errors
    ///
    /// [`dehacked_edit::set_patch`] and [`dehacked_edit::remove_patch`]
    /// errors pass through.
    fn apply(&self, text: &str) -> Result<String, CyberError> {
        match self.clone() {
            Self::Set { block, key, value } => {
                dehacked_edit::set_patch(text, &block, &key, &value).map(|(next, _)| next)
            }
            Self::Remove { block, key } => {
                dehacked_edit::remove_patch(text, &block, key.as_deref()).map(|(next, _)| next)
            }
        }
    }
}

/// Validate [`Cyberdemon::edit_dehacked`]'s arguments before any fetching.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] unless exactly one mode is present, and
/// for an empty `set`/`remove` list, a refused block spec, key or value.
pub(super) fn plan(args: &EditDehackedParams) -> Result<Draft, CyberError> {
    let modes = usize::from(args.text.is_some())
        .saturating_add(usize::from(args.source.is_some()))
        .saturating_add(usize::from(args.set.is_some()))
        .saturating_add(usize::from(args.remove.is_some()));
    if modes != 1 {
        return Err(CyberError::InvalidArgument(
            "edit_dehacked takes exactly one mode: pass one of text, source, set and remove"
                .to_owned(),
        ));
    }
    if let Some(body) = args.text.as_ref() {
        return Ok(Draft::Replace(body.clone()));
    }
    if let Some(raw) = args.source.as_ref() {
        return Ok(Draft::Fetch(raw.clone()));
    }
    if let Some(entries) = args.set.as_ref() {
        return Ok(Draft::Ops(plan_set(entries)?));
    }
    args.remove.as_ref().map_or_else(
        || {
            Err(CyberError::Internal(
                "edit_dehacked planned no mode".to_owned(),
            ))
        },
        |entries| Ok(Draft::Ops(plan_remove(entries)?)),
    )
}

/// The validated `set` operations.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] for an empty list or a refused block,
/// key or value.
fn plan_set(entries: &[SetEntry]) -> Result<Vec<Op>, CyberError> {
    if entries.is_empty() {
        return Err(CyberError::InvalidArgument(
            "set needs at least one entry: pass block, key and value objects".to_owned(),
        ));
    }
    let mut ops: Vec<Op> = Vec::new();
    for entry in entries {
        dehacked_edit::parse_spec(&entry.block)?;
        dehacked_edit::check_key(&entry.key)?;
        dehacked_edit::check_value(&entry.value)?;
        ops.push(Op::Set {
            block: entry.block.clone(),
            key: entry.key.clone(),
            value: entry.value.clone(),
        });
    }
    Ok(ops)
}

/// The validated `remove` operations.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] for an empty list, a refused block spec
/// or an empty key.
fn plan_remove(entries: &[RemoveEntry]) -> Result<Vec<Op>, CyberError> {
    if entries.is_empty() {
        return Err(CyberError::InvalidArgument(
            "remove needs at least one entry: pass block (and optionally key) objects".to_owned(),
        ));
    }
    let mut ops: Vec<Op> = Vec::new();
    for entry in entries {
        dehacked_edit::parse_spec(&entry.block)?;
        if entry
            .key
            .as_deref()
            .is_some_and(|key| key.trim().is_empty())
        {
            return Err(CyberError::InvalidArgument(
                "remove key is empty: pass a key or omit it to drop the whole block".to_owned(),
            ));
        }
        ops.push(Op::Remove {
            block: entry.block.clone(),
            key: entry.key.clone(),
        });
    }
    Ok(ops)
}

/// Finish a draft outside the session lock: fetch the `source` mode's file
/// and read its bytes with the `Latin-1` codec, so a legacy patch keeps
/// every byte.
///
/// # Errors
///
/// [`AppState::fetch_source`]'s errors pass through.
pub(super) async fn resolve(state: &AppState, draft: Draft) -> Result<Body, CyberError> {
    match draft {
        Draft::Replace(body) => Ok(Body::Replace(body)),
        Draft::Fetch(raw) => {
            let fetched = state.fetch_source(&raw).await?;
            Ok(Body::Replace(decode_patch(&fetched.bytes)))
        }
        Draft::Ops(ops) => Ok(Body::Ops(ops)),
    }
}

/// The resolved edit: a whole new text, or line operations on the current.
#[derive(Debug)]
pub(super) enum Body {
    /// Replace the lump with this text.
    Replace(String),
    /// Apply these operations to the current text.
    Ops(Vec<Op>),
}

/// The text the winning copy of `name` holds, the working one first, read
/// with the `Latin-1` patch codec.
fn current_text(session: &Session, name: LumpName, at: Option<usize>) -> Option<String> {
    if let Some(index) = at {
        let lump = session.working().lumps.get(index)?;
        return Some(decode_patch(&lump.data));
    }
    let entry = session
        .merged()
        .into_iter()
        .rev()
        .find(|found| found.lump.name == name)?;
    Some(decode_patch(&entry.lump.data))
}

/// The result text of the operations mode, over the current text or the
/// canonical header when the lump is absent everywhere.
///
/// # Errors
///
/// [`CyberError::NotFound`] when `remove` runs against no lump at all, and
/// [`dehacked_edit::set_patch`]/[`dehacked_edit::remove_patch`]'s errors
/// for missing blocks and keys; nothing changes when an operation fails.
pub(super) fn apply_ops(
    base: Option<&str>,
    ops: &[Op],
    name: LumpName,
) -> Result<String, CyberError> {
    let mut text = match base {
        Some(found) => found.to_owned(),
        None if ops.iter().any(Op::is_remove) => {
            return Err(CyberError::NotFound(format!(
                "lump {name} is in none of the loaded archives: remove has nothing to take \
                 from; get_dehacked lists what exists and edit_dehacked set creates a patch"
            )));
        }
        None => dehacked_edit::NEW_PATCH_HEADER.to_owned(),
    };
    for op in ops {
        text = op.apply(&text)?;
    }
    Ok(text)
}

/// Overwrite the working copy at `at` with `data`, or append the override.
pub(super) fn write_lump(wad: &mut Wad, name: LumpName, at: Option<usize>, data: &[u8]) {
    if let Some(index) = at
        && let Some(lump) = wad.lumps.get_mut(index)
    {
        lump.data = Arc::from(data);
        return;
    }
    wad.lumps.push(Lump::new(name, data.to_vec()));
}

/// `edit_dehacked`'s synchronous half: compute the new text, count the
/// minimal line diff against the old one, then commit exactly one undoable
/// write of the `Latin-1`-encoded bytes.
///
/// # Errors
///
/// [`apply_ops`]' refusals (the session does not change), and
/// [`crate::session::Session::edit`]'s error pass-through.
pub(super) fn edit_run(
    session: &mut Session,
    name: LumpName,
    body: Body,
) -> Result<EditReport, CyberError> {
    let at = session.working().find_last(name);
    let base = current_text(session, name, at);
    let text = match body {
        Body::Replace(next) => next,
        Body::Ops(ops) => apply_ops(base.as_deref(), &ops, name)?,
    };
    let diff = line_diff(base.as_deref().unwrap_or_default(), &text);
    let created = at.is_none();
    let lines = text.lines().count();
    let data = encode_patch(&text);
    session.edit(UNDO_LABEL, |wad| {
        write_lump(wad, name, at, &data);
        Ok(())
    })?;
    Ok(EditReport {
        name,
        lines,
        added: diff.added,
        removed: diff.removed,
        changed: diff.changed,
        created,
    })
}
