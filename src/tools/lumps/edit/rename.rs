//! `rename_lump`: renaming one working-archive lump in place.
//!
//! [`rename_lump`] selects the lump with the shared [`super::EditSelector`]
//! machinery, [`apply_rename`] refuses an unasked-for same-name twin in the
//! region that lump stands in unless `allow_duplicate` says otherwise, and
//! [`rename_in_place`] writes the new name over the directory entry, keeping
//! its position and its bytes. Only the working archive changes: a lower
//! layer's copy of the old name keeps showing through the merged view.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::replace::not_working_message;
use super::{edit_selector, region_text, working_match};
use crate::error::CyberError;
use crate::session::Session;
use crate::wad::archive::Wad;
use crate::wad::classify::{LumpInfo, Namespace, analyze};
use crate::wad::name::LumpName;

/// Arguments for [`Cyberdemon::rename_lump`](crate::server::Cyberdemon).
#[derive(Debug, Deserialize, JsonSchema)]
pub(in crate::tools::lumps) struct RenameLumpParams {
    /// Lump name to rename, from the working archive only: 1 to 8 printable
    /// `ASCII` characters, matched case-insensitively.
    pub(super) name: String,
    /// The new name: 1 to 8 printable `ASCII` characters, uppercased.
    pub(super) new_name: String,
    /// Keep only matches inside this namespace region (`sprites`, `flats`,
    /// ...).
    #[serde(default)]
    pub(super) namespace: Option<String>,
    /// 0-based index among the working archive's same-name matches; the
    /// default is the last one.
    #[serde(default)]
    pub(super) occurrence: Option<usize>,
    /// Rename even when the target region already holds a lump with
    /// `new_name` (default `false`).
    #[serde(default)]
    pub(super) allow_duplicate: bool,
}

/// Structured result of [`Cyberdemon::rename_lump`](crate::server::Cyberdemon).
#[derive(Debug, Serialize)]
pub(in crate::tools::lumps) struct RenameReport {
    /// The name the lump had.
    old_name: LumpName,
    /// The name it has now.
    new_name: LumpName,
    /// Its position in the working archive.
    index: usize,
}

/// Renames the lump at `index`, refusing an unasked-for same-name twin in the
/// region that lump sits in.
///
/// # Errors
///
/// [`CyberError::Internal`] for a stale index, [`CyberError::AlreadyExists`]
/// when another lump already carries `to` in the same region and
/// `allow_duplicate` is `false`. Nothing changes in either case.
pub(super) fn apply_rename(
    wad: &mut Wad,
    index: usize,
    to: LumpName,
    allow_duplicate: bool,
) -> Result<(), CyberError> {
    let analysis = analyze(&wad.lumps);
    let Some(info) = analysis.lumps.get(index) else {
        return Err(CyberError::Internal(format!(
            "no lump at index {index} of the working archive: it was expected to be renamed to \
             {to}"
        )));
    };
    let namespace = info.namespace;
    if !allow_duplicate && holds_name(&analysis.lumps, index, to, namespace) {
        return Err(CyberError::AlreadyExists(format!(
            "a lump named {to} already stands in {}: pass allow_duplicate true to keep both \
             copies, or rename to another name",
            region_text(namespace)
        )));
    }
    rename_in_place(wad, index, to)
}

/// Whether any lump but `index` already carries `name` inside `namespace`.
fn holds_name(
    infos: &[LumpInfo],
    index: usize,
    name: LumpName,
    namespace: Option<Namespace>,
) -> bool {
    infos
        .iter()
        .any(|other| other.index != index && other.name == name && other.namespace == namespace)
}

/// Writes `to` over the name of the lump at `index`.
///
/// # Errors
///
/// [`CyberError::Internal`] when the directory holds no lump there.
pub(super) fn rename_in_place(wad: &mut Wad, index: usize, to: LumpName) -> Result<(), CyberError> {
    let Some(lump) = wad.lumps.get_mut(index) else {
        return Err(CyberError::Internal(format!(
            "the working archive holds no lump at index {index} to rename to {to}"
        )));
    };
    lump.name = to;
    Ok(())
}

/// Runs `rename_lump`: one undoable rename inside the working archive.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] for a name or `new_name` that does not
/// parse, [`CyberError::NotFound`] when the working archive holds no match,
/// [`CyberError::AlreadyExists`] for a twin without `allow_duplicate`.
pub(in crate::tools::lumps) fn rename_lump(
    session: &mut Session,
    args: &RenameLumpParams,
) -> Result<RenameReport, CyberError> {
    let selector = edit_selector(&args.name, args.namespace.as_deref(), args.occurrence)?;
    let to = LumpName::parse(&args.new_name)?;
    let index = working_match(session, &selector)?.ok_or_else(|| {
        CyberError::NotFound(not_working_message(selector.name, selector.namespace))
    })?;
    let label = format!("rename_lump {} -> {to}", selector.name);
    session.edit(&label, |wad| {
        apply_rename(wad, index, to, args.allow_duplicate)
    })?;
    Ok(RenameReport {
        old_name: selector.name,
        new_name: to,
        index,
    })
}
