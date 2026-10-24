//! Removing a working patch: the `remove_patch` run the group's router
//! method delegates here, with the working scan that finds the lump's slot
//! and the commit that takes lump and `PNAMES` entry out as one undoable
//! edit.
//!
//! [`working_patch_slot`] answers the last [`is_removal_candidate`] of the
//! name in the working directory, [`remove_patch_run`] plans the `PNAMES`
//! removal over the merged view before editing so a `conflict` removes
//! nothing, and [`commit_removal`] deletes the guarded slot through
//! [`remove_lump_at`] and writes the renumbered list and texture lumps with
//! the shared [`crate::tools::textures`] machinery. The parameters and the
//! report shape stay in the group root, [`super`], together with the
//! candidate rule [`super::write::holds_global_patch`] the scan borrows.

use crate::error::CyberError;
use crate::session::{Scope, Session};
use crate::state::AppState;
use crate::tools::textures::{PnamesRewrite, plan_pnames_removal, write_pnames_rewrite};
use crate::wad::archive::{Lump, Wad};
use crate::wad::classify::{LumpInfo, LumpKind, Namespace, analyze};
use crate::wad::name::LumpName;

use super::{RemoveOutcome, RemovePatchParams, write::pnames_position};

/// Whether one working directory entry is a `remove_patch` candidate: a
/// non-marker lump of `name` inside a `patches` region, or - when the
/// merged `PNAMES` lists the name - one living outside the flats and
/// sprites regions, the same candidate rule [`super::write::holds_global_patch`]
/// reads for a `PNAMES`-named global.
fn is_removal_candidate(info: &LumpInfo, name: LumpName, listed: bool) -> bool {
    if info.name != name || info.kind == LumpKind::Marker {
        return false;
    }
    if info.namespace == Some(Namespace::Patches) {
        return true;
    }
    listed && !matches!(info.namespace, Some(Namespace::Flats | Namespace::Sprites))
}

/// The working archive's last `remove_patch` candidate index for `name`,
/// `None` when the working archive holds no such lump. `PNAMES` entries
/// without a lump answer `None` here; the caller decides whether they are
/// still removable through the [`plan_pnames_removal`] list.
///
/// # Errors
///
/// [`crate::tools::imports::current_pnames`]' [`CyberError::Format`] for a malformed `PNAMES` and
/// [`Session::lumps_in`]' read errors (none for the working scope).
pub(super) fn working_patch_slot(
    session: &Session,
    name: LumpName,
) -> Result<Option<usize>, CyberError> {
    let listed = pnames_position(session, name)?.is_some();
    let entries = session.lumps_in(Scope::Working)?;
    let lumps: Vec<Lump> = entries.iter().map(|entry| entry.lump.clone()).collect();
    let analysis = analyze(&lumps);
    let mut slot = None;
    for info in &analysis.lumps {
        if is_removal_candidate(info, name, listed) {
            slot = Some(info.index);
        }
    }
    Ok(slot)
}

/// The refusal for a name the working archive does not hold (and that the
/// `PNAMES` plan did not claim either): removal reaches working lumps
/// only, so a layer patch must first gain a working copy through
/// `replace_patch`.
pub(super) fn working_removal_missing(name: LumpName) -> CyberError {
    CyberError::NotFound(format!(
        "the working archive holds no patch named {name}: only working lumps can be removed - \
         an IWAD or resource-layer patch must first be overridden by replace_patch to gain the \
         working copy (or be left alone), and remove_from_pnames additionally needs the name \
         listed in PNAMES"
    ))
}

/// Deletes the working lump at `index`, guarding the pre-edit scan's
/// answer: any other lump standing there means the directory moved under
/// the scan and nothing is committed.
///
/// # Errors
///
/// [`CyberError::Internal`] when `index` holds no lump named `name`.
pub(super) fn remove_lump_at(
    wad: &mut Wad,
    name: LumpName,
    index: usize,
) -> Result<(), CyberError> {
    let held = wad.lumps.get(index).is_some_and(|lump| lump.name == name);
    if !held {
        return Err(CyberError::Internal(format!(
            "no lump named {name} at index {index} of the working archive"
        )));
    }
    // One `Vec::remove`: the region markers stay, so the `patches` region
    // never opens.
    let _removed = wad.lumps.remove(index);
    Ok(())
}

/// The synchronous half of a `remove_patch`: one undoable edit deleting the
/// working lump at the index the pre-edit scan found (when it found one)
/// and applying the planned [`PnamesRewrite`] - `PNAMES` plus every texture
/// lump the renumbering changed - so lump and list always leave together.
///
/// # Errors
///
/// [`remove_lump_at`]' and [`write_pnames_rewrite`]'
/// [`CyberError::Internal`] for a directory that moved under the scan;
/// a refusal leaves the working archive untouched.
fn commit_removal(
    session: &mut Session,
    name: LumpName,
    index: Option<usize>,
    rewrite: Option<PnamesRewrite>,
) -> Result<RemoveOutcome, CyberError> {
    let removed_from_pnames = rewrite.is_some();
    let textures_changed = rewrite
        .as_ref()
        .map_or(0_usize, |plan| plan.textures_changed);
    let label = format!("remove_patch {name}");
    session.edit(&label, move |wad| {
        let removed_lump = match index {
            Some(at) => {
                remove_lump_at(wad, name, at)?;
                true
            }
            None => false,
        };
        if let Some(plan) = rewrite.as_ref() {
            write_pnames_rewrite(wad, plan)?;
        }
        Ok(RemoveOutcome {
            name,
            removed_lump,
            removed_from_pnames,
            textures_changed,
        })
    })
}

/// `remove_patch`' work: scan the working archive, plan the `PNAMES`
/// removal over the merged view before editing (a `conflict` there removes
/// nothing), refuse a name neither half holds, then commit lump and list
/// as the one edit. Nothing here touches the network, so the run is
/// synchronous behind the `async` tool entry point.
///
/// # Errors
///
/// [`LumpName::parse`]' [`CyberError::InvalidArgument`] for a bad name,
/// [`plan_pnames_removal`]' [`CyberError::Conflict`] unless `force`,
/// [`working_removal_missing`]' [`CyberError::NotFound`],
/// [`crate::tools::imports::current_pnames`]' [`CyberError::Format`] and
/// [`commit_removal`]' errors.
pub(super) fn remove_patch_run(
    state: &AppState,
    args: &RemovePatchParams,
) -> Result<RemoveOutcome, CyberError> {
    let name = LumpName::parse(&args.name)?;
    let index = state.with_session(|session| working_patch_slot(session, name))?;
    let rewrite = if args.remove_from_pnames.unwrap_or(false) {
        let force = args.force.unwrap_or(false);
        let plan = state.with_session(move |session| {
            plan_pnames_removal(session, &[name.to_string()], force)
        })?;
        (!plan.removed.is_empty()).then_some(plan)
    } else {
        None
    };
    if index.is_none() && rewrite.is_none() {
        return Err(working_removal_missing(name));
    }
    state.with_session(move |session| commit_removal(session, name, index, rewrite))
}
