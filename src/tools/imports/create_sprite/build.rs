//! The picture side of one `create_sprite_from_image` call: the automatic
//! background-removal and trim ops, the height rule, the shaped and
//! quantized [`BuiltSprite`], the rotation warnings [`collect_warnings`]
//! reads in the commit's own lock through [`land_sprite`],
//! [`commit_sprite`]' single [`Session::edit`] placing every named lump
//! and the [`CreatedSpriteReport`] [`create_sprite_build`] answers with.
//!
//! The arguments are validated in the group's root: [`super::parse_create`]
//! hands this module the [`SpriteRequest`] it runs, its [`Cleanup`] half
//! carrying the border switches, beside the constants it reads.

use serde::Serialize;
use serde_json::Value;

use crate::assets::fetch::Fetched;
use crate::assets::server::AssetRef;
use crate::error::CyberError;
use crate::gfx::codec::encode_png;
use crate::gfx::image::RgbaImage;
use crate::gfx::ops::{Filter, FitMode, ImageOp, rounded_div};
use crate::gfx::patch::{Patch, decode_patch, encode_patch};
use crate::gfx::quantize::{DEFAULT_ALPHA_THRESHOLD, Transparency};
use crate::session::Session;
use crate::state::AppState;
use crate::tools::imports::create_sprite::{
    AUTO_HEIGHT, Cleanup, CreateSpriteParams, NEXT_STEPS, PNG_MIME, SpriteRequest, TALL_HEIGHT,
    parse_create,
};
use crate::tools::imports::image_pipeline::{OffsetDefault, resolve_offsets, to_i16_offsets};
use crate::tools::imports::processed::{Processed, process, to_indexed};
use crate::tools::imports::sprite::{Naming, warnings_for};
use crate::tools::imports::{apply_extra, put_lump, region_write};
use crate::tools::palette::load_palette;
use crate::wad::classify::Namespace;
use crate::wad::name::LumpName;

/// The automatic background-removal and trim ops, in pipeline order.
pub(super) fn background_ops(cleanup: &Cleanup) -> Vec<ImageOp> {
    let mut ops: Vec<ImageOp> = Vec::new();
    if cleanup.remove {
        ops.push(ImageOp::RemoveBackground {
            tolerance: cleanup.tolerance,
            color: cleanup.color.clone(),
        });
    }
    if cleanup.trim {
        ops.push(ImageOp::Trim {
            alpha_threshold: DEFAULT_ALPHA_THRESHOLD,
            padding: 0_u32,
        });
    }
    ops
}

/// The `height` plan for the final picture, and the note the automatic
/// rule adds. `None` keeps the trimmed picture exactly as it is.
pub(super) struct HeightPlan {
    /// Target width in pixels, keeping the aspect.
    pub(super) width: u32,
    /// Target height in pixels.
    pub(super) height: u32,
    /// Why the too-tall rule scaled the picture, when it fired.
    pub(super) note: Option<String>,
}

/// The height rule: the explicit `height` wins (when it changes anything),
/// else a picture taller than [`TALL_HEIGHT`] is planned for
/// [`AUTO_HEIGHT`] with its note.
pub(super) fn choose_height(request: &SpriteRequest, picture: &RgbaImage) -> Option<HeightPlan> {
    match request.height {
        Some(height) => {
            let width = scaled_width(picture, height);
            let changes = picture.width != width || picture.height != height;
            changes.then_some(HeightPlan {
                width,
                height,
                note: None,
            })
        }
        None if picture.height > TALL_HEIGHT => Some(HeightPlan {
            width: scaled_width(picture, AUTO_HEIGHT),
            height: AUTO_HEIGHT,
            note: Some(format!(
                "scaled to {AUTO_HEIGHT} px tall; pass height to choose"
            )),
        }),
        None => None,
    }
}

/// The aspect-kept width for a picture scaled to `height`, at least one.
pub(super) fn scaled_width(picture: &RgbaImage, height: u32) -> u32 {
    let scaled = rounded_div(
        u64::from(picture.width).saturating_mul(u64::from(height)),
        u64::from(picture.height.max(1_u32)),
    );
    u32::try_from(scaled)
        .unwrap_or(crate::gfx::ops::MAX_OP_DIMENSION)
        .max(1_u32)
}

/// The aspect-kept `Fit` to the height rule's target, with its note.
///
/// # Errors
///
/// [`apply_ops`]' [`CyberError::InvalidArgument`] widened to say the
/// failure came from the automatic step, not the user's ops.
pub(super) fn fit_step(
    processed: Processed,
    request: &SpriteRequest,
) -> Result<(Processed, Option<String>), CyberError> {
    let Some(plan) = choose_height(request, &processed.rgba) else {
        return Ok((processed, None));
    };
    let ops = [ImageOp::Fit {
        width: plan.width,
        height: plan.height,
        mode: FitMode::Contain,
        filter: Filter::Area,
        background: None,
    }];
    let shaped = apply_extra(processed, &ops).map_err(|error| {
        CyberError::InvalidArgument(format!(
            "the automatic height step failed: {error}: pass a smaller height or crop the picture"
        ))
    })?;
    Ok((shaped, plan.note))
}

/// Rewrite a failed automatic background/trim step into the documented
/// instruction; any other error stands as it is. The step's own refusal is
/// the typed [`CyberError::is_emptied`] outcome [`trim`](crate::gfx::ops::alpha::trim)
/// reports when the picture is left with no content, so nothing here reads
/// the message.
pub(super) fn emptied(cleanup: &Cleanup, error: CyberError) -> CyberError {
    if !error.is_emptied() {
        return error;
    }
    if cleanup.remove {
        return CyberError::InvalidArgument(
            "the whole image was removed as background; lower background_tolerance or set \
             remove_background false"
                .to_owned(),
        );
    }
    CyberError::InvalidArgument(
        "nothing was left to trim: every pixel sits below alpha 128; check the source picture \
         or pass trim=false"
            .to_owned(),
    )
}

/// The shaped, quantized and encoded picture, ready to commit.
pub(super) struct BuiltSprite {
    /// The encoded `Doom` patch bytes every lump is written as.
    pub(super) data: Vec<u8>,
    /// The first lump's `PNG`, read back through the sprite decoder.
    pub(super) preview_png: Vec<u8>,
    /// Final width in pixels.
    pub(super) width: u32,
    /// Final height in pixels.
    pub(super) height: u32,
    /// Stored horizontal drawing offset.
    pub(super) left_offset: i32,
    /// Stored vertical drawing offset.
    pub(super) top_offset: i32,
    /// One measurement object per applied op, in order.
    pub(super) steps: Vec<Value>,
    /// Distinct opaque source colours.
    pub(super) colors_used: usize,
    /// Pixels the alpha rule dropped.
    pub(super) transparent_pixels: usize,
    /// What the height rule did.
    pub(super) notes: Vec<String>,
}

/// Run the whole picture side: user ops, background removal and trim, the
/// height fit, alpha quantization, patch encoding and the decoded
/// preview. The palette read is the only session access here.
///
/// # Errors
///
/// [`process`]' errors, [`apply_extra`]' and [`fit_step`]' (through
/// [`emptied`] for an emptied picture), [`load_palette`]'
/// [`CyberError::NotFound`], [`to_i16_offsets`], [`encode_patch`],
/// [`decode_patch`] and [`encode_png`]' errors.
pub(super) fn build_sprite(
    state: &AppState,
    request: &SpriteRequest,
    fetched: &Fetched,
) -> Result<BuiltSprite, CyberError> {
    let processed = process(&fetched.bytes, &request.shared)?;
    let cleared = apply_extra(processed, &background_ops(&request.cleanup))
        .map_err(|error| emptied(&request.cleanup, error))?;
    let (shaped, note) = fit_step(cleared, request)?;
    let palette =
        state.with_session(|session| load_palette(session, request.shared.palette_index()))?;
    let indexed = to_indexed(
        &shaped,
        &palette,
        Transparency::default(),
        request.shared.dither_choice(),
    );
    let pair = resolve_offsets(
        request.offsets.as_ref(),
        None,
        OffsetDefault::CenterBottom,
        &shaped.rgba,
    );
    let (left_offset, top_offset) = to_i16_offsets(pair)?;
    let (colors_used, transparent_pixels) = (indexed.colors_used, indexed.transparent_pixels);
    let patch = Patch {
        left_offset,
        top_offset,
        image: indexed.image,
    };
    let data = encode_patch(&patch)?;
    let decoded = decode_patch(&data)?;
    let grab = (i32::from(left_offset), i32::from(top_offset));
    let preview_png = encode_png(&decoded.image.to_rgba(&palette), Some(grab))?;
    Ok(BuiltSprite {
        data,
        preview_png,
        width: shaped.rgba.width,
        height: shaped.rgba.height,
        left_offset: i32::from(left_offset),
        top_offset: i32::from(top_offset),
        steps: shaped.report.steps,
        colors_used,
        transparent_pixels,
        notes: note.into_iter().collect(),
    })
}

/// The first lump's name, which labels the edit and is previewed.
///
/// # Errors
///
/// [`CyberError::Internal`] for a request whose naming resolved to
/// nothing - [`parse_sprite_names`](super::parse_sprite_names) guarantees
/// it cannot happen.
pub(super) fn first_name(names: &[Naming]) -> Result<LumpName, CyberError> {
    names
        .first()
        .map(|naming| naming.name)
        .ok_or_else(|| CyberError::Internal("sprite naming resolved to no lump name".to_owned()))
}

/// The rotation-coverage warnings for every lump of one call, deduplicated
/// in order, read from the merged sprites region in the same lock as the
/// commit.
///
/// # Errors
///
/// [`warnings_for`]' errors for the merged scope.
pub(super) fn collect_warnings(
    session: &Session,
    names: &[Naming],
) -> Result<Vec<String>, CyberError> {
    let mut warnings: Vec<String> = Vec::new();
    for naming in names {
        let mut fresh: Vec<String> = warnings_for(session, naming)?
            .into_iter()
            .filter(|warning| !warnings.contains(warning))
            .collect();
        warnings.append(&mut fresh);
    }
    Ok(warnings)
}

/// The whole session half of a create-sprite under one lock:
/// [`collect_warnings`] reads the merged sprites region as it stands right
/// now - so lumps a concurrent import just placed are counted - and
/// [`commit_sprite`] then places every named lump as one edit.
///
/// # Errors
///
/// [`collect_warnings`]' and [`commit_sprite`]' errors.
pub(super) fn land_sprite(
    session: &mut Session,
    request: &SpriteRequest,
    built: &BuiltSprite,
) -> Result<Vec<String>, CyberError> {
    let warnings = collect_warnings(session, &request.names)?;
    commit_sprite(session, request, built)?;
    Ok(warnings)
}

/// The synchronous half: one undoable edit placing every named lump in the
/// working archive's sprites region.
///
/// # Errors
///
/// [`first_name`]' internal error and [`put_lump`]' placement and
/// `already_exists` errors; a refusal leaves the archive untouched.
pub(super) fn commit_sprite(
    session: &mut Session,
    request: &SpriteRequest,
    built: &BuiltSprite,
) -> Result<(), CyberError> {
    let label = format!("create_sprite {}", first_name(&request.names)?);
    session.edit(&label, |wad| {
        for naming in &request.names {
            let write = region_write(
                naming.name,
                &built.data,
                Namespace::Sprites,
                request.replace,
            );
            put_lump(wad, &write)?;
        }
        Ok(())
    })?;
    Ok(())
}

/// Structured result of [`crate::server::Cyberdemon::create_sprite_from_image`].
#[derive(Debug, Serialize)]
pub(super) struct CreatedSpriteReport {
    /// Every sprite lump name this call wrote, in call order.
    lumps: Vec<String>,
    /// Final width in pixels.
    width: u32,
    /// Final height in pixels.
    height: u32,
    /// Stored horizontal drawing offset.
    left_offset: i32,
    /// Stored vertical drawing offset.
    top_offset: i32,
    /// One measurement object per applied op, in order.
    steps: Vec<Value>,
    /// Distinct opaque source colours.
    colors_used: usize,
    /// Pixels the alpha rule dropped.
    transparent_pixels: usize,
    /// The first lump's `PNG`, read back through the sprite decoder.
    preview: AssetRef,
    /// Rotations the session's other sprites already serve.
    warnings: Vec<String>,
    /// What the height rule did.
    notes: Vec<String>,
    /// What to do next.
    next_steps: &'static [&'static str],
}

/// The measured body of [`create_sprite_run`](super::create_sprite_run):
/// validate, fetch and process outside every lock, then read the warnings
/// and commit every lump as one session step, and finally publish the
/// preview.
///
/// # Errors
///
/// [`parse_create`]' argument errors, [`AppState::fetch_source`]' and
/// [`build_sprite`]' errors, [`land_sprite`]' refusals and
/// [`AppState::publish`]' errors; a refusal leaves the working archive
/// untouched.
pub(super) async fn create_sprite_build(
    state: &AppState,
    args: &CreateSpriteParams,
) -> Result<CreatedSpriteReport, CyberError> {
    let request = parse_create(args)?;
    let fetched = state.fetch_source(&request.source).await?;
    let built = build_sprite(state, &request, &fetched)?;
    let warnings = state.with_session(|session| land_sprite(session, &request, &built))?;
    let first = first_name(&request.names)?;
    let preview = state.publish(
        &format!("{first}_sprite.png"),
        PNG_MIME,
        built.preview_png.clone(),
    )?;
    Ok(report_of(&request, &built, warnings, preview))
}

/// Fold the request, the built picture and the read facts into the
/// documented result.
fn report_of(
    request: &SpriteRequest,
    built: &BuiltSprite,
    warnings: Vec<String>,
    preview: AssetRef,
) -> CreatedSpriteReport {
    CreatedSpriteReport {
        lumps: request
            .names
            .iter()
            .map(|naming| naming.name.to_string())
            .collect(),
        width: built.width,
        height: built.height,
        left_offset: built.left_offset,
        top_offset: built.top_offset,
        steps: built.steps.clone(),
        colors_used: built.colors_used,
        transparent_pixels: built.transparent_pixels,
        preview,
        warnings,
        notes: built.notes.clone(),
        next_steps: &NEXT_STEPS,
    }
}
