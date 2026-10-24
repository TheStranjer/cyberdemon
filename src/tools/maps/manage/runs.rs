//! The four management runs and their reports: the undoable steps behind
//! `create_map`, `copy_map`, `rename_map` and `remove_map`.
//!
//! [`create_run`] appends a freshly encoded empty block, [`copy_run`]
//! appends a renamed copy of a scope's block (with its `GL` run per
//! [`super::gl_copies`]), [`rename_run`] renames a working block in place
//! and [`remove_run`] drops a block and its `GL` run. Each is one
//! [`crate::session::Session::edit`] over the working archive; the active
//! map follows outside the history. The plans they consume, the block
//! builders and the `GL` helpers stay in the module root, [`super`].

use serde::Serialize;

use crate::error::CyberError;
use crate::session::Session;
use crate::tools::maps::common;
use crate::wad::classify::{MapFormat, analyze};
use crate::wad::name::LumpName;

use super::{
    CopyPlan, CreatePlan, append_block, apply_rename, copied_block, doomed_positions,
    drop_positions, empty_block,
};

/// Structured result of [`crate::server::Cyberdemon::create_map`].
#[derive(Debug, Serialize)]
pub(in crate::tools::maps) struct CreateReport {
    /// The map that was created.
    map: LumpName,
    /// The layout its block uses.
    format: MapFormat,
    /// The namespace a `UDMF` block declares; `null` for binary formats.
    namespace: Option<String>,
    /// Non-fatal notices: the odd-name warning.
    warnings: Vec<String>,
    /// The session's active map after the call.
    active_map: Option<LumpName>,
}

/// Runs one `create_map`: append the freshly encoded empty block, then
/// adopt the new map as the active selection when none was set.
///
/// # Errors
///
/// [`super::empty_block`]'s encoder refusals and [`CyberError::AlreadyExists`]
/// for a working block of that name without `replace`; the session does
/// not change when the edit fails.
pub(in crate::tools::maps) fn create_run(
    session: &mut Session,
    plan: CreatePlan,
) -> Result<CreateReport, CyberError> {
    let CreatePlan {
        name,
        format,
        namespace,
        replace,
        warnings,
    } = plan;
    let lumps = empty_block(name, format, namespace.as_deref())?;
    let label = format!("create_map {name}");
    session.edit(&label, move |wad| append_block(wad, name, replace, lumps))?;
    if session.active_map().is_none() {
        session.set_active_map(Some(name));
    }
    Ok(CreateReport {
        map: name,
        format,
        namespace,
        warnings,
        active_map: session.active_map(),
    })
}

/// Structured result of [`crate::server::Cyberdemon::copy_map`].
#[derive(Debug, Serialize)]
pub(in crate::tools::maps) struct CopyReport {
    /// The map that was read.
    from: LumpName,
    /// The name the copy holds in the working archive.
    to: LumpName,
    /// Lumps appended (header and members, the kept GL run included).
    lumps: usize,
}

/// Runs one `copy_map`: resolve the source block in its scope and append
/// the renamed copy as one undoable edit.
///
/// # Errors
///
/// [`CyberError::NotFound`] when `scope` holds no block named `from`,
/// [`CyberError::AlreadyExists`] when the working archive holds `to`
/// without `replace`; a failed call changes nothing.
pub(in crate::tools::maps) fn copy_run(
    session: &mut Session,
    plan: &CopyPlan,
) -> Result<CopyReport, CyberError> {
    let dir = common::directory(session, plan.scope)?;
    let block = dir
        .find_block(plan.from)
        .ok_or_else(|| common::missing_map(plan.from, plan.scope))?;
    let copies = copied_block(&dir, block, plan.to);
    let count = copies.len();
    let label = format!("copy_map {} -> {}", plan.from, plan.to);
    let to = plan.to;
    let replace = plan.replace;
    session.edit(&label, move |wad| {
        let existing = doomed_positions(&analyze(&wad.lumps), to);
        if !existing.is_empty() && !replace {
            return Err(CyberError::AlreadyExists(format!(
                "the working archive already holds a map block named {to}: pass replace true to \
                 discard it first, or copy to another name"
            )));
        }
        drop_positions(wad, &existing);
        wad.lumps.extend(copies);
        Ok(())
    })?;
    Ok(CopyReport {
        from: plan.from,
        to: plan.to,
        lumps: count,
    })
}

/// Structured result of [`crate::server::Cyberdemon::rename_map`].
#[derive(Debug, Serialize)]
pub(in crate::tools::maps) struct RenameReport {
    /// The name the map had.
    pub(super) from: LumpName,
    /// The name it has now.
    pub(super) to: LumpName,
}

/// Runs one `rename_map`: rename the working block in place, then move the
/// active selection when it pointed at the old name.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] for `from` equal to `to`, plus
/// [`super::apply_rename`]'s refusals; a failed call changes nothing.
pub(in crate::tools::maps) fn rename_run(
    session: &mut Session,
    from: LumpName,
    to: LumpName,
) -> Result<RenameReport, CyberError> {
    if from == to {
        return Err(CyberError::InvalidArgument(format!(
            "rename_map names the same map twice ({from}): pass a different to, or no rename is \
             needed"
        )));
    }
    let label = format!("rename_map {from} -> {to}");
    session.edit(&label, |wad| apply_rename(wad, from, to))?;
    if session.active_map() == Some(from) {
        session.set_active_map(Some(to));
    }
    Ok(RenameReport { from, to })
}

/// Structured result of [`crate::server::Cyberdemon::remove_map`].
#[derive(Debug, Serialize)]
pub(in crate::tools::maps) struct RemoveReport {
    /// The map that was removed.
    map: LumpName,
    /// Lumps removed (block and `GL` run together).
    lumps_removed: usize,
}

/// Runs one `remove_map`: drop the block and its `GL` run, then clear the
/// active selection when it pointed at the removed map.
///
/// # Errors
///
/// [`CyberError::NotFound`] when the working archive holds no such block;
/// a failed call changes nothing.
pub(in crate::tools::maps) fn remove_run(
    session: &mut Session,
    name: LumpName,
) -> Result<RemoveReport, CyberError> {
    let label = format!("remove_map {name}");
    let removed = session.edit(&label, |wad| {
        let doomed = doomed_positions(&analyze(&wad.lumps), name);
        if doomed.is_empty() {
            return Err(CyberError::NotFound(format!(
                "no map block named {name} in the working archive: the IWAD and resource layers \
                 are read-only and never lose their maps, so list_maps shows what is editable"
            )));
        }
        Ok(drop_positions(wad, &doomed))
    })?;
    if session.active_map() == Some(name) {
        session.set_active_map(None);
    }
    Ok(RemoveReport {
        map: name,
        lumps_removed: removed,
    })
}
