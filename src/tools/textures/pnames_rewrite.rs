//! The committing half of `edit_pnames`: what one planned change writes and
//! the reusable `PNAMES` removal behind the patch tools.
//!
//! [`plan_pnames_removal`] folds a removal of names into a [`PnamesRewrite`]
//! (the shortened list's bytes plus every texture lump
//! [`rewrite_textures`] renumbered) and [`write_pnames_rewrite`] commits it,
//! so a caller that also deletes a patch lump takes both into ONE
//! [`crate::session::Session::edit`]. [`edit_pnames_run`] is the tool's own
//! path: it plans with [`plan_edit`], commits through [`put_global`] and
//! answers with [`report_of`]; [`pick_operation`] reads the one operation
//! the arguments carry.

use crate::error::CyberError;
use crate::session::Session;
use crate::wad::archive::Wad;
use crate::wad::name::LumpName;
use crate::wad::textures::{PatchMapping, remap_patches, write_pnames, write_texture_lump};

use super::pnames::{EditPnamesParams, EditReport, Operation, Plan, plan_edit};
use super::{PNAMES_RAW, TEXTURE1_RAW, TEXTURE2_RAW, TextureSet, put_global};

/// The re-encoded texture lumps of one edit.
#[derive(Debug, Default)]
struct PlannedTextures {
    /// New `TEXTURE1` bytes, when it changed.
    texture1: Option<Vec<u8>>,
    /// New `TEXTURE2` bytes, when it changed.
    texture2: Option<Vec<u8>>,
}

/// Re-encodes the set's texture lumps through `mapping`, counting the
/// changed textures.
///
/// # Errors
///
/// [`crate::wad::textures::write_texture_lump`]'s errors (a parsed lump
/// always re-encodes, so these are [`CyberError::Internal`]-class only).
fn rewrite_textures(
    set: &TextureSet,
    mapping: &PatchMapping,
) -> Result<(PlannedTextures, usize), CyberError> {
    let mut out = PlannedTextures::default();
    let mut changed = 0_usize;
    for (entry, first) in [
        (set.texture1.as_ref(), true),
        (set.texture2.as_ref(), false),
    ] {
        let Some(pair) = entry else {
            continue;
        };
        let mut edited = pair.0.clone();
        let count = remap_patches(&mut edited, mapping);
        if count == 0 {
            continue;
        }
        changed = changed.saturating_add(count);
        let bytes = write_texture_lump(&edited)?;
        if first {
            out.texture1 = Some(bytes);
        } else {
            out.texture2 = Some(bytes);
        }
    }
    Ok((out, changed))
}

/// The whole committed change of one `PNAMES` removal: the re-encoded
/// lumps ready to write and the removal's report lists.
///
/// Built by [`plan_pnames_removal`], applied by [`write_pnames_rewrite`];
/// a caller that also removes a patch lump folds both into ONE
/// [`crate::session::Session::edit`] so the removal and the
/// PNAMES/TEXTURE rewrite land together.
#[derive(Debug)]
pub(in crate::tools) struct PnamesRewrite {
    /// The new complete `PNAMES` list.
    pub(in crate::tools) names: Vec<String>,
    /// The re-encoded `PNAMES` lump bytes.
    pub(in crate::tools) pnames_bytes: Vec<u8>,
    /// New `TEXTURE1` bytes, when the renumbering changed it.
    pub(in crate::tools) texture1: Option<Vec<u8>>,
    /// New `TEXTURE2` bytes, when the renumbering changed it.
    pub(in crate::tools) texture2: Option<Vec<u8>>,
    /// Textures whose patch references were renumbered or pruned.
    pub(in crate::tools) textures_changed: usize,
    /// Names the removal deleted.
    pub(in crate::tools) removed: Vec<String>,
    /// Requested names that `PNAMES` did not hold.
    pub(in crate::tools) skipped: Vec<String>,
}

/// Plans the complete `PNAMES` removal of `names` over the merged view —
/// the shortened list, its bytes and the renumbered texture lumps —
/// without writing anything.
///
/// # Errors
///
/// [`plan_edit`]'s errors ([`CyberError::InvalidArgument`] for an empty or
/// unparsable name list, [`CyberError::LimitExceeded`] over the `u16`
/// index range, [`CyberError::Conflict`] listing the textures that still
/// refer to a removed name unless `force`), [`TextureSet::load`]'s
/// [`CyberError::Format`] and the codecs' [`CyberError::Internal`]-class
/// errors.
pub(in crate::tools) fn plan_pnames_removal(
    session: &Session,
    names: &[String],
    force: bool,
) -> Result<PnamesRewrite, CyberError> {
    let set = TextureSet::load(session)?;
    let textures = set.textures();
    let plan = plan_edit(&set.pnames, Operation::Remove(names), force, &textures)?;
    let pnames_bytes = write_pnames(&plan.names)?;
    let (rewritten, changed) = match plan.mapping.as_ref() {
        Some(mapping) => rewrite_textures(&set, mapping)?,
        None => (PlannedTextures::default(), 0_usize),
    };
    Ok(PnamesRewrite {
        names: plan.names,
        pnames_bytes,
        texture1: rewritten.texture1,
        texture2: rewritten.texture2,
        textures_changed: changed,
        removed: plan.removed,
        skipped: plan.skipped,
    })
}

/// Writes a planned removal into `wad`: `PNAMES` plus each texture lump
/// the renumbering changed, overwriting the last working copies or
/// appending them as overrides.
///
/// # Errors
///
/// [`put_global`]'s [`CyberError::Internal`] for a vanished lump slot.
pub(in crate::tools) fn write_pnames_rewrite(
    wad: &mut Wad,
    rewrite: &PnamesRewrite,
) -> Result<(), CyberError> {
    put_global(
        wad,
        LumpName::from_raw(PNAMES_RAW),
        rewrite.pnames_bytes.clone(),
    )?;
    if let Some(bytes) = rewrite.texture1.as_ref() {
        put_global(wad, LumpName::from_raw(TEXTURE1_RAW), bytes.clone())?;
    }
    if let Some(bytes) = rewrite.texture2.as_ref() {
        put_global(wad, LumpName::from_raw(TEXTURE2_RAW), bytes.clone())?;
    }
    Ok(())
}

/// `edit_pnames`' work: plan the change, then commit it as one edit.
///
/// A plan that changed nothing (all names skipped, no texture rewritten)
/// records no history and reports the counters as they stand.
///
/// # Errors
///
/// [`plan_edit`]'s errors, [`TextureSet::load`]'s [`CyberError::Format`],
/// the codecs' [`CyberError::InvalidArgument`] and
/// [`CyberError::LimitExceeded`].
pub(super) fn edit_pnames_run(
    session: &mut Session,
    operation: Operation<'_>,
    force: bool,
) -> Result<EditReport, CyberError> {
    if let Operation::Remove(list) = operation {
        return edit_pnames_remove_run(session, list, force);
    }
    let set = TextureSet::load(session)?;
    let textures = set.textures();
    let plan = plan_edit(&set.pnames, operation, force, &textures)?;
    let pnames_bytes = write_pnames(&plan.names)?;
    let (rewritten, changed) = match plan.mapping.as_ref() {
        Some(mapping) => rewrite_textures(&set, mapping)?,
        None => (PlannedTextures::default(), 0_usize),
    };
    let unchanged =
        plan.names == set.pnames && rewritten.texture1.is_none() && rewritten.texture2.is_none();
    if unchanged {
        return Ok(report_of(&plan, changed));
    }
    let label = format!("edit_pnames {}", operation.name());
    session.edit(&label, |wad| {
        put_global(wad, LumpName::from_raw(PNAMES_RAW), pnames_bytes.clone())?;
        if let Some(bytes) = rewritten.texture1.as_ref() {
            put_global(wad, LumpName::from_raw(TEXTURE1_RAW), bytes.clone())?;
        }
        if let Some(bytes) = rewritten.texture2.as_ref() {
            put_global(wad, LumpName::from_raw(TEXTURE2_RAW), bytes.clone())?;
        }
        Ok(())
    })?;
    Ok(report_of(&plan, changed))
}

/// `edit_pnames`' `remove` arm: [`plan_pnames_removal`] computes the
/// whole change, [`write_pnames_rewrite`] commits it as one edit. A
/// removal that changed nothing records no history; with nothing removed
/// the new list stands as it was, so that is exactly an empty `removed`
/// paired with no renumbered texture.
///
/// # Errors
///
/// [`plan_pnames_removal`]'s errors and [`write_pnames_rewrite`]'s.
fn edit_pnames_remove_run(
    session: &mut Session,
    list: &[String],
    force: bool,
) -> Result<EditReport, CyberError> {
    let rewrite = plan_pnames_removal(session, list, force)?;
    let unchanged = rewrite.removed.is_empty() && rewrite.textures_changed == 0;
    if !unchanged {
        session.edit("edit_pnames remove", |wad| {
            write_pnames_rewrite(wad, &rewrite)
        })?;
    }
    Ok(remove_report_of(&rewrite))
}

/// The report for a finished plan.
fn report_of(plan: &Plan, changed: usize) -> EditReport {
    EditReport {
        pnames: plan.names.len(),
        added: plan.added.clone(),
        skipped: plan.skipped.clone(),
        removed: plan.removed.clone(),
        renamed: plan.renamed.clone(),
        textures_changed: changed,
    }
}

/// The report for a committed [`PnamesRewrite`]: a removal adds nothing
/// and renames nothing.
fn remove_report_of(rewrite: &PnamesRewrite) -> EditReport {
    EditReport {
        pnames: rewrite.names.len(),
        added: Vec::new(),
        skipped: rewrite.skipped.clone(),
        removed: rewrite.removed.clone(),
        renamed: Vec::new(),
        textures_changed: rewrite.textures_changed,
    }
}

/// The single operation the arguments carry, refusing none and many.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] unless exactly one of `add`, `remove`,
/// `rename` and `replace_all` is present.
pub(super) fn pick_operation(args: &EditPnamesParams) -> Result<Operation<'_>, CyberError> {
    let mut given = [
        args.add.as_deref().map(Operation::Add),
        args.remove.as_deref().map(Operation::Remove),
        args.rename.as_deref().map(Operation::Rename),
        args.replace_all.as_deref().map(Operation::ReplaceAll),
    ]
    .into_iter()
    .flatten();
    let Some(operation) = given.next() else {
        return Err(no_operation());
    };
    if given.next().is_some() {
        return Err(no_operation());
    }
    Ok(operation)
}

/// The refusal naming the accepted operations.
fn no_operation() -> CyberError {
    CyberError::InvalidArgument(
        "edit_pnames takes exactly one operation: pass one of add, remove, rename or \
         replace_all"
            .to_owned(),
    )
}
