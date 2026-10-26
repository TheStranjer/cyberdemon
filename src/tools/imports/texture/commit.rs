//! Landing one `create_texture_from_image` call: [`plan_create`] runs the
//! collision rules ([`check_patch_clash`] and the texture-name refusal)
//! over a [`Session`] and grows the `PNAMES` list and the target
//! `TEXTURE1`/`TEXTURE2` record in memory as a [`Plan`]; [`land_create`]
//! runs plan and write under the tool's single session lock -
//! [`commit_create`] places patch, `PNAMES` and definition as ONE
//! [`crate::session::Session::edit`] labelled `create_texture NAME` and
//! [`apply_role`] points a theme wall role at the result (session state,
//! outside that history) - so a concurrent commit cannot grow the list
//! between one call's plan and its write; [`report_of`] folds the [`Built`]
//! picture with the [`Landed`] facts into the [`CreatedTextureReport`].
//!
//! The texture internals behind `build_texture` are reused through
//! [`crate::tools::textures`]; the arguments come from
//! [`CreateRequest`](super::CreateRequest) and the limits from the group's
//! root.

use serde::Serialize;
use serde_json::Value;

use crate::assets::server::AssetRef;
use crate::error::CyberError;
use crate::session::{LayerRole, Session, Source};
use crate::tools::imports::{apply_role, put_lump, region_write};
use crate::tools::textures::{
    Placement, TextureSet, definition, lump_copy, missing_names, place_definition, target_lump,
};
use crate::wad::classify::Namespace;
use crate::wad::lumps;
use crate::wad::name::LumpName;
use crate::wad::textures::{write_pnames, write_texture_lump};

use super::build::Built;
use super::{CreateRequest, MAX_TEXTURE_EDGE, NEXT_STEPS};

/// The lumps one commit writes, planned before anything changes.
#[derive(Debug)]
pub(super) struct Plan {
    /// The grown `PNAMES` bytes, when this call appends a name.
    pub(super) pnames_bytes: Option<Vec<u8>>,
    /// The re-encoded target texture lump.
    pub(super) texture_bytes: Vec<u8>,
    /// The target lump's name.
    pub(super) target: LumpName,
    /// The target lump's wire name.
    pub(super) lump: &'static str,
}

/// Plan the whole write: the collision rules first, then the grown
/// `PNAMES` list and the definition over a copy of the target lump.
///
/// # Errors
///
/// [`check_patch_clash`]' and the texture-name rule's
/// [`CyberError::AlreadyExists`], [`target_lump`]'
/// [`CyberError::InvalidArgument`], [`TextureSet::load`]'
/// [`CyberError::Format`] and the codecs' errors.
pub(super) fn plan_create(
    session: &Session,
    request: &CreateRequest,
    built: &Built,
) -> Result<Plan, CyberError> {
    check_patch_clash(session, request.patch, request.replace)?;
    let set = TextureSet::load(session)?;
    if set.find(&request.name).is_some() && !request.replace {
        return Err(CyberError::AlreadyExists(format!(
            "texture {} is already defined in the merged view: pass replace=true to redefine \
             it in place, or another name",
            request.name
        )));
    }
    let target = target_lump(&set, request.target.as_deref(), &request.name)?;
    let placements = [Placement {
        name: request.patch.to_string(),
        x: 0_i16,
        y: 0_i16,
    }];
    let added = missing_names(set.pnames(), &placements);
    let mut pnames: Vec<String> = set.pnames().to_vec();
    pnames.extend(added);
    let def = definition(&request.name, texture_size(built)?, &placements, &pnames)?;
    let mut lump = lump_copy(&set, target);
    place_definition(&mut lump, target, def, request.replace)?;
    Ok(Plan {
        pnames_bytes: (pnames != *set.pnames())
            .then(|| write_pnames(&pnames))
            .transpose()?,
        texture_bytes: write_texture_lump(&lump)?,
        target: target.name(),
        lump: target.as_str(),
    })
}

/// The `(width, height)` pair a texture record stores, refusing an edge
/// past the range it can hold.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] for a value outside `u16` (the size
/// rules keep this unreachable in a request the tool accepts; the check
/// keeps a broken size from storing a wrapped dimension).
pub(super) fn texture_size(built: &Built) -> Result<(u16, u16), CyberError> {
    let width = u16::try_from(built.width).map_err(|_conversion| oversize("width", built.width))?;
    let height =
        u16::try_from(built.height).map_err(|_conversion| oversize("height", built.height))?;
    Ok((width, height))
}

/// The refusal for an oversized texture edge.
fn oversize(what: &str, value: u32) -> CyberError {
    CyberError::InvalidArgument(format!(
        "{what} {value} is past the {MAX_TEXTURE_EDGE}-pixel texture limit"
    ))
}

/// Refuse a patch name a read-only layer holds as a patch while the
/// working archive holds none in its `patches` region - unless `replace`
/// was given. Only the region lookup can answer a working `patches` twin:
/// a same-named lump in another working region is no patch and must not
/// hide the layer's. A working `patches`-region twin is `put_lump`' own
/// refusal, with its own hint.
///
/// # Errors
///
/// [`CyberError::AlreadyExists`] naming the layer and the way out.
fn check_patch_clash(session: &Session, patch: LumpName, replace: bool) -> Result<(), CyberError> {
    if replace {
        return Ok(());
    }
    if let Some(entry) = session.find(patch, Some(Namespace::Patches)) {
        return match entry.source {
            Source::Working => Ok(()),
            Source::Layer(index) => Err(layer_clash(session, patch, index)),
        };
    }
    match session.find(patch, None).map(|entry| entry.source) {
        Some(Source::Layer(index)) => Err(layer_clash(session, patch, index)),
        Some(Source::Working) | None => Ok(()),
    }
}

/// The refusal for a patch name a read-only layer stands on.
fn layer_clash(session: &Session, patch: LumpName, index: usize) -> CyberError {
    CyberError::AlreadyExists(format!(
        "a patch named {patch} already exists in {} and other textures use it: pass patch_name \
         to store yours under another name, or replace=true to override the layer's copy in the \
         working archive",
        layer_label(session, index),
    ))
}

/// What a layer reads as in a message: the base game archive or a
/// resource, by its position in the stack.
pub(in crate::tools) fn layer_label(session: &Session, index: usize) -> String {
    session.layers().get(index).map_or_else(
        || format!("layer {index}"),
        |layer| match layer.role {
            LayerRole::Iwad => format!("the {}", layer.role.label()),
            LayerRole::Resource => format!("{} layer {index}", layer.role.label()),
        },
    )
}

/// The synchronous half: patch, `PNAMES` and definition as ONE undoable
/// edit.
///
/// # Errors
///
/// [`put_lump`]' placement and `already_exists` errors and
/// [`crate::wad::archive::Wad::put_global`]' stale-index [`CyberError::Internal`].
pub(super) fn commit_create(
    session: &mut Session,
    request: &CreateRequest,
    built: &Built,
    plan: &Plan,
) -> Result<(), CyberError> {
    let label = format!("create_texture {}", request.name);
    let write = region_write(
        request.patch,
        &built.patch_bytes,
        Namespace::Patches,
        request.replace,
    );
    session.edit(&label, |wad| {
        put_lump(wad, &write)?;
        if let Some(bytes) = plan.pnames_bytes.as_ref() {
            wad.put_global(lumps::PNAMES, bytes.clone())?;
        }
        wad.put_global(plan.target, plan.texture_bytes.clone())?;
        Ok(())
    })?;
    Ok(())
}

/// The committed facts a report is assembled from.
#[derive(Clone, Copy, Debug)]
pub(super) struct Landed {
    /// The lump that now defines the texture.
    pub(super) lump: &'static str,
    /// The role the theme now points at the texture, if any.
    pub(super) role: Option<&'static str>,
}

/// The whole session half of a create-texture under one lock:
/// [`plan_create`] grows `PNAMES` and the target definition from the
/// merged view exactly as it stands at this moment, [`commit_create`]
/// writes that plan as one edit, and [`apply_role`] lands the theme
/// override only once the commit took - so two concurrent creates each
/// keep their patch, their `PNAMES` entry and their definition.
///
/// # Errors
///
/// [`plan_create`]' refusals and read errors and [`commit_create`]'
/// placement and stale-index errors.
pub(super) fn land_create(
    session: &mut Session,
    request: &CreateRequest,
    built: &Built,
) -> Result<Landed, CyberError> {
    let plan = plan_create(session, request, built)?;
    commit_create(session, request, built, &plan)?;
    Ok(Landed {
        lump: plan.lump,
        role: apply_role(session, request.role, &request.name),
    })
}

/// Structured result of
/// [`crate::server::Cyberdemon::create_texture_from_image`].
#[derive(Debug, Serialize)]
pub(super) struct CreatedTextureReport {
    /// The texture name, uppercased.
    pub(super) texture: String,
    /// The patch lump name the picture was stored under.
    pub(super) patch: String,
    /// The texture width in pixels.
    pub(super) width: u32,
    /// The texture height in pixels.
    pub(super) height: u32,
    /// The lump that now defines the texture.
    pub(super) lump: &'static str,
    /// One measurement object per applied op, in order.
    pub(super) steps: Vec<Value>,
    /// Distinct opaque source colours.
    pub(super) colors_used: usize,
    /// Worst squared `RGB` distance to the palette.
    pub(super) max_color_error: i32,
    /// How the size was chosen and what the theme override means.
    pub(super) notes: Vec<String>,
    /// Non-fatal picture facts.
    pub(super) warnings: Vec<String>,
    /// The composited texture's `PNG`, ready to look at.
    pub(super) preview: AssetRef,
    /// The theme role now standing for this texture, if any.
    pub(super) role: Option<&'static str>,
    /// What to do next.
    pub(super) next_steps: &'static [&'static str],
}

/// Fold the request, the built picture, the landed facts and the published
/// preview into the documented result.
pub(super) fn report_of(
    request: &CreateRequest,
    built: &Built,
    landed: Landed,
    preview: AssetRef,
) -> CreatedTextureReport {
    let mut notes = built.notes.clone();
    if let Some(role) = landed.role {
        notes.push(format!(
            "theme override {role} -> {} set in session state: undo does not roll it back, \
             clear it with set_texture_theme overrides",
            request.name
        ));
    }
    CreatedTextureReport {
        texture: request.name.clone(),
        patch: request.patch.to_string(),
        width: built.width,
        height: built.height,
        lump: landed.lump,
        steps: built.steps.clone(),
        colors_used: built.colors_used,
        max_color_error: built.max_color_error,
        notes,
        warnings: built.warnings.clone(),
        preview,
        role: landed.role,
        next_steps: &NEXT_STEPS,
    }
}
