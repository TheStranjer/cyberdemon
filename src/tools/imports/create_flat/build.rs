//! The picture side of one `create_flat_from_image` call: the automatic
//! square fit and tiling ops, the quantized and encoded [`BuiltFlat`], the
//! name-collision rule [`check_flat_clash`] reads in the same lock as its
//! commit, [`commit_flat`]' single [`Session::edit`] placing the lump, the
//! theme override [`apply_role`] setting outside that history, the whole
//! session half landed as one [`land_flat`] step, and the
//! [`CreatedFlatReport`] [`create_flat_build`] answers with.
//!
//! The arguments are validated in the tool's root: [`super::parse_create_flat`]
//! hands this module the [`FlatRequest`] it runs, beside the constants and
//! the [`super::parse_flat_role`] check the theme part reads.

use serde::Serialize;
use serde_json::Value;

use crate::assets::fetch::Fetched;
use crate::assets::server::AssetRef;
use crate::error::CyberError;
use crate::gfx::codec::encode_png;
use crate::gfx::flat::{decode_flat, encode_flat};
use crate::gfx::ops::dither::Dither;
use crate::gfx::ops::{Filter, FitMode, ImageOp};
use crate::gfx::quantize::Transparency;
use crate::session::{Session, Source};
use crate::state::AppState;
use crate::tools::imports::create_flat::{
    CreateFlatParams, FLAT_SIDE, FlatRequest, NEXT_STEPS, PNG_MIME, TILE_BAND, parse_create_flat,
};
use crate::tools::imports::processed::{Processed, process, to_indexed};
use crate::tools::imports::texture::layer_label;
use crate::tools::imports::transparent_alpha_pixels;
use crate::tools::imports::{apply_extra, apply_role, put_lump, region_write};
use crate::tools::palette::load_palette;
use crate::wad::classify::Namespace;
use crate::wad::name::LumpName;

/// The automatic `Fit` to the `size` square and `MakeTileable` (when
/// asked) ops, appended to the processed picture and its report.
///
/// # Errors
///
/// [`apply_extra`]' [`CyberError::InvalidArgument`] widened to say the
/// failure came from the automatic step, not the user's ops.
pub(super) fn shape_flat(
    processed: Processed,
    fit: FitMode,
    tileable: bool,
    size: u32,
) -> Result<Processed, CyberError> {
    let mut extra: Vec<ImageOp> = vec![ImageOp::Fit {
        width: size,
        height: size,
        mode: fit,
        filter: Filter::Area,
        background: None,
    }];
    if tileable {
        extra.push(ImageOp::MakeTileable {
            band: Some(TILE_BAND),
            horizontal: true,
            vertical: true,
        });
    }
    apply_extra(processed, &extra).map_err(|error| {
        CyberError::InvalidArgument(format!(
            "the automatic flat sizing step failed: {error}: adjust fit or size, or pass \
             tileable=false"
        ))
    })
}

/// The shaped, quantized and encoded flat, ready to plan and commit.
pub(super) struct BuiltFlat {
    /// The raw `PLAYPAL` index bytes of the stored flat.
    pub(super) data: Vec<u8>,
    /// The stored flat's `PNG`, read back through the flat decoder.
    pub(super) preview_png: Vec<u8>,
    /// One measurement object per applied op, in order.
    pub(super) steps: Vec<Value>,
    /// Distinct source colours.
    pub(super) colors_used: usize,
    /// Worst squared `RGB` distance to the palette.
    pub(super) max_color_error: i32,
    /// Non-fatal picture facts.
    pub(super) warnings: Vec<String>,
}

/// Run the whole picture side: user ops, the automatic square fit and
/// tiling, opaque dithered quantization, flat encoding and the decoded
/// preview. The palette read is the only session access here.
///
/// # Errors
///
/// [`process`]' errors for the bytes and the user ops, [`shape_flat`]'
/// errors, [`load_palette`]' [`CyberError::NotFound`], [`encode_flat`],
/// [`decode_flat`] and [`encode_png`]' errors.
pub(super) fn build_flat(
    state: &AppState,
    request: &FlatRequest,
    fetched: &Fetched,
) -> Result<BuiltFlat, CyberError> {
    let processed = process(&fetched.bytes, &request.shared)?;
    let shaped = shape_flat(processed, request.fit, request.tileable, request.size)?;
    let ignored = transparent_alpha_pixels(&shaped.rgba);
    let palette =
        state.with_session(|session| load_palette(session, request.shared.palette_index()))?;
    let indexed = to_indexed(
        &shaped,
        &palette,
        Transparency::None,
        request.shared.dither_choice(),
    );
    let (data, _flattened) = encode_flat(&indexed.image)?;
    let stored = decode_flat(&data)?;
    let preview_png = encode_png(&stored.to_rgba(&palette), None)?;
    Ok(BuiltFlat {
        data,
        preview_png,
        steps: shaped.report.steps,
        colors_used: indexed.colors_used,
        max_color_error: indexed.max_color_error,
        warnings: flat_warnings(ignored, request.size),
    })
}

/// The non-fatal facts of one built flat: kept-opaque holes and, past the
/// vanilla side, the port requirement of the stored shape.
pub(super) fn flat_warnings(ignored: usize, size: u32) -> Vec<String> {
    let mut warnings = opaque_warning(ignored);
    warnings.extend(vanilla_size_warning(size));
    warnings
}

/// The warning for a side past the `64x64` a vanilla engine reads, `None`
/// for the vanilla square.
pub(super) fn vanilla_size_warning(size: u32) -> Option<String> {
    if size <= FLAT_SIDE {
        return None;
    }
    Some(format!(
        "{size}x{size} is past the vanilla shape: vanilla Doom loads only 64x64 flats, so this \
         flat needs a ZDoom-family port (GZDoom, ZDoom)"
    ))
}

/// The warning for kept-opaque holes, empty for a fully opaque picture.
pub(super) fn opaque_warning(ignored: usize) -> Vec<String> {
    if ignored == 0 {
        return Vec::new();
    }
    vec![format!(
        "{ignored} pixels below alpha 128 were kept opaque: flats store no transparency - the \
         colour they show is the source's own"
    )]
}

/// Refuse a flat name that already serves, unless `replace` was given,
/// and answer whether this call will overwrite or override.
///
/// # Errors
///
/// [`CyberError::AlreadyExists`] for a working or layer-held twin without
/// `replace`; the layer message names the everywhere-used override.
pub(super) fn check_flat_clash(
    session: &Session,
    name: LumpName,
    replace: bool,
) -> Result<bool, CyberError> {
    let Some(entry) = session.find(name, Some(Namespace::Flats)) else {
        return Ok(false);
    };
    if replace {
        return Ok(true);
    }
    match entry.source {
        Source::Working => Err(CyberError::AlreadyExists(format!(
            "flat {name} already exists in the working archive's flats region: pass replace=true \
             to overwrite it in place, or another name"
        ))),
        Source::Layer(index) => Err(CyberError::AlreadyExists(format!(
            "flat {name} already exists in {}: this would override {name} everywhere it is \
             used; pass replace=true to do that or choose another name",
            layer_label(session, index),
        ))),
    }
}

/// The synchronous half: one undoable edit placing the flat bytes in the
/// working archive's flats region.
///
/// # Errors
///
/// [`put_lump`]' placement and `already_exists` errors.
pub(super) fn commit_flat(
    session: &mut Session,
    request: &FlatRequest,
    built: &BuiltFlat,
) -> Result<(), CyberError> {
    let label = format!("create_flat {}", request.name);
    let write = region_write(request.name, &built.data, Namespace::Flats, request.replace);
    session.edit(&label, |wad| put_lump(wad, &write).map(|_| ()))?;
    Ok(())
}

/// Structured result of [`crate::server::Cyberdemon::create_flat_from_image`].
#[derive(Debug, Serialize)]
pub(super) struct CreatedFlatReport {
    /// The flat name, uppercased.
    pub(super) flat: String,
    /// The square side of the stored flat, in pixels.
    pub(super) size: u32,
    /// The dither mode the quantization used.
    pub(super) dither: Dither,
    /// One measurement object per applied op, in order.
    pub(super) steps: Vec<Value>,
    /// Distinct source colours.
    pub(super) colors_used: usize,
    /// Worst squared `RGB` distance to the palette.
    pub(super) max_color_error: i32,
    /// Whether this call overwrote or overrode a same-name flat.
    pub(super) overrides_existing: bool,
    /// The stored flat's `PNG`, read back through the flat decoder.
    pub(super) preview: AssetRef,
    /// The theme role now standing for this flat, if any.
    pub(super) role: Option<&'static str>,
    /// Non-fatal picture facts and what the theme override means.
    pub(super) warnings: Vec<String>,
    /// What to do next.
    pub(super) next_steps: &'static [&'static str],
}

/// The committed facts a report is assembled from.
#[derive(Clone, Copy, Debug)]
pub(super) struct Landed {
    /// Whether this call overwrote or overrode a same-name flat.
    pub(super) overrides_existing: bool,
    /// The theme role now standing for this flat, if any.
    pub(super) role: Option<&'static str>,
}

/// The whole session half of a create-flat under one lock:
/// [`check_flat_clash`] reads the merged view as it stands right now (a
/// concurrent add cannot slip between the check and the write),
/// [`commit_flat`] places the bytes, and [`apply_role`] points the theme
/// at the result only after the commit succeeded.
///
/// # Errors
///
/// [`check_flat_clash`]' [`CyberError::AlreadyExists`] and
/// [`commit_flat`]' placement errors.
pub(super) fn land_flat(
    session: &mut Session,
    request: &FlatRequest,
    built: &BuiltFlat,
) -> Result<Landed, CyberError> {
    let overrides_existing = check_flat_clash(session, request.name, request.replace)?;
    commit_flat(session, request, built)?;
    Ok(Landed {
        overrides_existing,
        role: apply_role(session, request.role, &request.name.to_string()),
    })
}

/// Fold the request, the built flat and the landed facts into the
/// documented result.
fn report_of(
    request: &FlatRequest,
    built: &BuiltFlat,
    landed: Landed,
    preview: AssetRef,
) -> CreatedFlatReport {
    let mut warnings = built.warnings.clone();
    if let Some(assigned) = landed.role {
        warnings.push(format!(
            "theme override {assigned} -> {name} set in session state: undo does not roll it \
             back, clear it with set_texture_theme overrides",
            name = request.name,
        ));
    }
    CreatedFlatReport {
        flat: request.name.to_string(),
        size: request.size,
        dither: request.shared.dither_choice(),
        steps: built.steps.clone(),
        colors_used: built.colors_used,
        max_color_error: built.max_color_error,
        overrides_existing: landed.overrides_existing,
        preview,
        role: landed.role,
        warnings,
        next_steps: &NEXT_STEPS,
    }
}

/// The measured body of [`crate::tools::imports::create_flat::create_flat_run`]:
/// validate, fetch and process outside every lock, then check the
/// collisions and commit as one session step, and finally publish the
/// preview.
///
/// # Errors
///
/// [`parse_create_flat`]' argument errors, [`AppState::fetch_source`]' and
/// [`build_flat`]' errors, [`land_flat`]' refusals and
/// [`AppState::publish`]' errors; a refusal leaves the working archive
/// untouched.
pub(super) async fn create_flat_build(
    state: &AppState,
    args: &CreateFlatParams,
) -> Result<CreatedFlatReport, CyberError> {
    let request = parse_create_flat(args)?;
    let fetched = state.fetch_source(&request.source).await?;
    let built = build_flat(state, &request, &fetched)?;
    let landed = state.with_session(|session| land_flat(session, &request, &built))?;
    let preview = state.publish(
        &format!("{}_flat.png", request.name),
        PNG_MIME,
        built.preview_png.clone(),
    )?;
    Ok(report_of(&request, &built, landed, preview))
}
