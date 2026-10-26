//! Writing patch bytes into the working archive: the `add_patch` and
//! `replace_patch` runs the group's router methods delegate here, with the
//! merged-view scans that decide whether a name already stands and the
//! commits that place the staged bytes as one undoable edit.
//!
//! [`located_patch`] reads the merged view the way `list_patches` does and
//! answers how a replacement must land; [`add_patch_run`] stages the bytes
//! on the shared pipeline of [`crate::tools::imports::image_pipeline`]
//! outside any lock, then runs [`land_add_patch`] - one session lock in
//! which the working `patches`-region twin is refused, the `PNAMES` entry
//! is planned from the list as it stands and patch and (when it grew) list
//! commit together as one edit; [`replace_patch_run`] re-locates the target
//! inside its single lock too, then commits the staged bytes over it and
//! never writes `PNAMES`. The staging, the report shapes and the
//! parameters stay in the group root, [`super`].

use crate::error::CyberError;
use crate::session::{Scope, Session};
use crate::state::AppState;
use crate::tools::imports::image_pipeline::parse_import;
use crate::tools::imports::{PnamesPlan, current_pnames, pnames_plan, put_lump, region_write};
use crate::wad::archive::Lump;
use crate::wad::classify::{LumpKind, Namespace, analyze};
use crate::wad::lumps;
use crate::wad::name::LumpName;

use super::{
    AddPatchParams, PatchOutcome, ReplaceOutcome, ReplacePatchParams, Staged, resolve_staged,
};

/// Whether the working archive's `patches` region already holds `name`.
///
/// # Errors
///
/// [`Session::lumps_in`]' errors (none for the working scope).
pub(super) fn holds_patch(session: &Session, name: LumpName) -> Result<bool, CyberError> {
    let entries = session.lumps_in(Scope::Working)?;
    let lumps: Vec<Lump> = entries.iter().map(|entry| entry.lump.clone()).collect();
    let holds = analyze(&lumps)
        .lumps
        .iter()
        .any(|info| info.name == name && info.namespace == Some(Namespace::Patches));
    Ok(holds)
}

/// The duplicate refusal: `put_lump`' own hint says `replace=true`, which
/// this tool has no parameter for, so the message points at
/// `replace_patch` instead.
///
/// # Errors
///
/// [`CyberError::AlreadyExists`] for a working `patches`-region twin, and
/// [`Session::lumps_in`]' read errors.
pub(super) fn refuse_working_twin(session: &Session, name: LumpName) -> Result<(), CyberError> {
    if holds_patch(session, name)? {
        return Err(CyberError::AlreadyExists(format!(
            "lump {name} already exists in the patches region of the working archive: use \
             replace_patch to overwrite it in place, or add the picture under another name"
        )));
    }
    Ok(())
}

/// The name's unchanged entry index in the merged `PNAMES` list, `None`
/// when no `PNAMES` answers or does not list it.
///
/// # Errors
///
/// [`current_pnames`]' [`CyberError::Format`] for a malformed lump.
pub(super) fn pnames_position(
    session: &Session,
    name: LumpName,
) -> Result<Option<usize>, CyberError> {
    let text = name.to_string();
    Ok(current_pnames(session)?
        .iter()
        .position(|entry| entry.eq_ignore_ascii_case(&text)))
}

/// Whether the merged view holds a non-marker lump named `name` outside
/// the flats and sprites regions: `list_patches`' candidate rule for a
/// `PNAMES`-named global living outside the `patches` region. (A
/// `patches`-region occurrence also matches here, which is harmless: this
/// only ever decides the fallback after the region lookup came up empty.)
pub(super) fn holds_global_patch(session: &Session, name: LumpName) -> bool {
    let view = session.merged();
    let lumps: Vec<Lump> = view.iter().map(|entry| entry.lump.clone()).collect();
    analyze(&lumps).lumps.iter().any(|info| {
        info.name == name
            && info.kind != LumpKind::Marker
            && !matches!(info.namespace, Some(Namespace::Flats | Namespace::Sprites))
    })
}

/// What the merged view stands on for a `replace_patch` target, read
/// before the fetch and again inside the land: the name must stand as a
/// patch the way `list_patches` reads one - inside a `patches` region, or
/// as a `PNAMES`-named global - and the answer carries how the write must
/// land (overwrite in place, or override a lower layer) with the name's
/// unchanged `PNAMES` index.
///
/// # Errors
///
/// [`current_pnames`]' [`CyberError::Format`] for a malformed `PNAMES` and
/// [`CyberError::NotFound`] naming `add_patch` for a name no layer holds
/// as a patch.
pub(super) fn located_patch(
    session: &Session,
    name: LumpName,
) -> Result<(bool, Option<usize>), CyberError> {
    let pnames_index = pnames_position(session, name)?;
    let in_region = session.find(name, Some(Namespace::Patches)).is_some();
    let stands = in_region || (pnames_index.is_some() && holds_global_patch(session, name));
    if !stands {
        return Err(CyberError::NotFound(format!(
            "no patch named {name} in the merged view: pass a name list_patches shows, or \
             store a new one with add_patch"
        )));
    }
    holds_patch(session, name).map(|in_working| (in_working, pnames_index))
}

/// The synchronous half of an `add_patch`: one undoable edit placing the
/// staged lump in the working `patches` region and, when the effective
/// `PNAMES` lacked the name, writing the grown list as a working
/// copy-on-write - the same shape as the import tools' commit step. The
/// `put_lump` call carries `replace: false` as the final twin guard behind
/// [`refuse_working_twin`].
///
/// # Errors
///
/// [`put_lump`]' placement and `already_exists` errors and
/// [`crate::wad::archive::Wad::put_global`]' stale-index [`CyberError::Internal`].
fn commit_patch(
    session: &mut Session,
    staged: Staged,
    plan: Option<PnamesPlan>,
) -> Result<PatchOutcome, CyberError> {
    let label = format!("add_patch {}", staged.name);
    session.edit(&label, move |wad| {
        let write = region_write(staged.name, &staged.data, Namespace::Patches, false);
        put_lump(wad, &write)?;
        if let Some(list) = plan.as_ref().and_then(|entry| entry.bytes.as_ref()) {
            wad.put_global(lumps::PNAMES, list.clone())?;
        }
        Ok(PatchOutcome {
            name: staged.name,
            converted: staged.converted,
            width: staged.width,
            height: staged.height,
            left_offset: staged.left_offset,
            top_offset: staged.top_offset,
            pnames_index: plan.map(|entry| entry.index),
        })
    })
}

/// `add_patch`' work: validate and refuse a working twin before any fetch,
/// stage the bytes outside every lock, then run [`land_add_patch`] as one
/// session step.
///
/// # Errors
///
/// [`parse_import`]' argument errors, [`refuse_working_twin`]'
/// [`CyberError::AlreadyExists`] from either the pre-fetch check or the
/// land, whatever [`resolve_staged`] reports for the source and picture,
/// and [`pnames_plan`]' and [`commit_patch`]' errors; a refusal leaves the
/// working archive untouched.
pub(super) async fn add_patch_run(
    state: &AppState,
    args: &AddPatchParams,
) -> Result<PatchOutcome, CyberError> {
    let import = parse_import(&args.common())?;
    let Some(name) = import.name else {
        return Err(CyberError::Internal(
            "add_patch always carries its lump name".to_owned(),
        ));
    };
    state.with_session(|session| refuse_working_twin(session, name))?;
    let staged = resolve_staged(state, name, &import).await?;
    let add_to_pnames = args.add_to_pnames;
    state.with_session(move |session| land_add_patch(session, staged, add_to_pnames))
}

/// The whole session half of an `add_patch` under one lock: refuse the
/// twin the pre-fetch scan could not see coming, plan the `PNAMES` entry
/// from the effective list as it stands right now, and commit patch and
/// list together - no other tool's commit can grow the list between the
/// plan and the write and have its entry overwritten.
///
/// # Errors
///
/// [`refuse_working_twin`]' [`CyberError::AlreadyExists`],
/// [`pnames_plan`]' read and write refusals and [`commit_patch`]' errors.
pub(super) fn land_add_patch(
    session: &mut Session,
    staged: Staged,
    add_to_pnames: Option<bool>,
) -> Result<PatchOutcome, CyberError> {
    refuse_working_twin(session, staged.name)?;
    let plan = pnames_plan(session, staged.name, add_to_pnames)?;
    commit_patch(session, staged, plan)
}

/// The synchronous half of a `replace_patch`: one undoable edit writing the
/// staged bytes over the named patch. `in_working` is the decision made
/// before the edit from the read-only working scan: `true` overwrites the
/// working `patches`-region lump in place at its own index, `false` lets
/// [`put_lump`] insert the working override (creating the region's markers
/// on the working archive when it has none). `PNAMES` is never written.
///
/// # Errors
///
/// [`put_lump`]' placement and stale-index errors - the `already_exists`
/// half is unreachable here because the scan chose `replace` to fit the
/// directory.
fn commit_replacement(
    session: &mut Session,
    staged: Staged,
    pnames_index: Option<usize>,
    in_working: bool,
) -> Result<ReplaceOutcome, CyberError> {
    let label = format!("replace_patch {}", staged.name);
    session.edit(&label, move |wad| {
        let write = region_write(staged.name, &staged.data, Namespace::Patches, in_working);
        let lump_index = put_lump(wad, &write)?;
        Ok(ReplaceOutcome {
            patch: PatchOutcome {
                name: staged.name,
                converted: staged.converted,
                width: staged.width,
                height: staged.height,
                left_offset: staged.left_offset,
                top_offset: staged.top_offset,
                pnames_index,
            },
            created_override: !in_working,
            lump_index,
        })
    })
}

/// `replace_patch`' work: validate, refuse a name no layer holds as a
/// patch before any fetch, stage the bytes outside every lock, then
/// re-locate and commit through [`land_replacement`] as one session step.
///
/// # Errors
///
/// [`parse_import`]' argument errors, [`located_patch`]'
/// [`CyberError::NotFound`] and `format_error` from either the pre-fetch
/// scan or the land, whatever [`resolve_staged`] reports for the source and
/// picture, and [`commit_replacement`]' errors; a refusal leaves the
/// working archive untouched.
pub(super) async fn replace_patch_run(
    state: &AppState,
    args: &ReplacePatchParams,
) -> Result<ReplaceOutcome, CyberError> {
    let import = parse_import(&args.common())?;
    let Some(name) = import.name else {
        return Err(CyberError::Internal(
            "replace_patch always carries its lump name".to_owned(),
        ));
    };
    state.with_session(|session| located_patch(session, name).map(|_| ()))?;
    let staged = resolve_staged(state, name, &import).await?;
    state.with_session(move |session| land_replacement(session, staged))
}

/// The whole session half of a `replace_patch` under one lock:
/// [`located_patch`] re-reads how the write must land (in-place overwrite
/// or working override, and the name's unchanged `PNAMES` index) from the
/// session as it stands right now, and [`commit_replacement`] writes what
/// that decision calls for - a concurrent `add_patch` or `remove_patch`
/// cannot move the directory between the scan and the edit.
///
/// # Errors
///
/// [`located_patch`]' refusals and [`commit_replacement`]' errors.
pub(super) fn land_replacement(
    session: &mut Session,
    staged: Staged,
) -> Result<ReplaceOutcome, CyberError> {
    let (in_working, pnames_index) = located_patch(session, staged.name)?;
    commit_replacement(session, staged, pnames_index, in_working)
}
