//! Landing one `create_texture_from_image` call: [`plan_create`] runs the
//! collision rules ([`check_patch_clash`] and the texture-name refusal)
//! over a [`Session`] and grows the `PNAMES` list and the target
//! `TEXTURE1`/`TEXTURE2` record in memory as a [`Plan`];
//! [`commit_create`] writes patch, `PNAMES` and definition as ONE
//! [`crate::session::Session::edit`] labelled `create_texture NAME`,
//! [`apply_role`] points a theme wall role at the result (session state,
//! outside that history) and [`report_of`] folds the [`Built`] picture
//! with the [`Landed`] facts into the [`CreatedTextureReport`].
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
use crate::state::AppState;
use crate::tools::imports::image_pipeline::{LumpWrite, put_lump};
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

/// Refuse a patch name a read-only layer holds while the working archive
/// does not - unless `replace` was given. A working-name twin is
/// `put_lump`' own refusal, with its own hint.
///
/// # Errors
///
/// [`CyberError::AlreadyExists`] naming the layer and the way out.
fn check_patch_clash(session: &Session, patch: LumpName, replace: bool) -> Result<(), CyberError> {
    if replace || session.working().find_last(patch).is_some() {
        return Ok(());
    }
    let Some(entry) = session.find(patch, None) else {
        return Ok(());
    };
    match entry.source {
        Source::Working => Ok(()),
        Source::Layer(index) => Err(CyberError::AlreadyExists(format!(
            "a patch named {patch} already exists in {} and other textures use it: pass \
             patch_name to store yours under another name, or replace=true to override the \
             layer's copy in the working archive",
            layer_label(session, index),
        ))),
    }
}

/// What a layer reads as in a message: the base game archive or a
/// resource, by its position in the stack.
pub(in crate::tools) fn layer_label(session: &Session, index: usize) -> String {
    session.layers().get(index).map_or_else(
        || format!("layer {index}"),
        |layer| match layer.role {
            LayerRole::Iwad => "the IWAD".to_owned(),
            LayerRole::Resource => format!("resource layer {index}"),
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
    let write = LumpWrite {
        name: request.patch,
        data: &built.patch_bytes,
        namespace: Some(Namespace::Patches),
        replace: request.replace,
    };
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

/// Point the requested theme role at the new texture. Theme state is
/// session state: no undo step records it.
///
/// # Errors
///
/// [`AppState::with_session`]' lock errors.
pub(super) fn apply_role(
    state: &AppState,
    request: &CreateRequest,
) -> Result<Option<&'static str>, CyberError> {
    let Some(role) = request.role else {
        return Ok(None);
    };
    let name = request.name.clone();
    state.with_session(move |session| {
        session.texture_theme_mut().overrides.insert(role, name);
        Ok(Some(role.as_str()))
    })
}

/// The committed facts a report is assembled from.
pub(super) struct Landed {
    /// The lump that now defines the texture.
    pub(super) lump: &'static str,
    /// The role the theme now points at the texture, if any.
    pub(super) role: Option<&'static str>,
    /// The published composited preview.
    pub(super) preview: AssetRef,
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

/// Fold the request, the built picture and the landed facts into the
/// documented result.
pub(super) fn report_of(
    request: &CreateRequest,
    built: &Built,
    landed: Landed,
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
        preview: landed.preview,
        role: landed.role,
        next_steps: &NEXT_STEPS,
    }
}
