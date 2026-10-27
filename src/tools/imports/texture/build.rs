//! The picture side of one `create_texture_from_image` call:
//! [`build_picture`] runs the user's ops, the [`size`](super::size) rule
//! over their result and the automatic [`shape_picture`] steps, quantizes
//! the shaped picture opaquely against the session palette and encodes it
//! as the [`Built`] this module hands on: the patch bytes, the composited
//! `PNG` preview and the measured facts the report carries.
//!
//! The only session access here is the `PLAYPAL` read; the collision
//! rules and the write live in [`commit`](super::commit) and the
//! arguments, already validated, come from the group's root as a
//! [`CreateRequest`](super::CreateRequest).

use serde_json::Value;

use crate::assets::fetch::Fetched;
use crate::error::CyberError;
use crate::gfx::codec::encode_png;
use crate::gfx::composite::{Component, compose};
use crate::gfx::image::IndexedImage;
use crate::gfx::ops::FitMode;
use crate::gfx::palette::Palette;
use crate::gfx::patch::{Patch, encode_patch};
use crate::gfx::quantize::Transparency;
use crate::state::AppState;
use crate::tools::imports::apply_extra;
use crate::tools::imports::processed::{
    Processed, automatic, fill_low_alpha, process, shape_ops, to_indexed,
};
use crate::tools::palette::load_palette;

use super::CreateRequest;
use super::size::{WallSize, choose_size};

/// The automatic `Fit` (when the size changed) and `MakeTileable` (when
/// asked) ops of [`shape_ops`], run through [`apply_extra`] and appended to
/// the processed picture and its report.
///
/// # Errors
///
/// [`apply_extra`]' refusal of the first op that fails, widened by
/// [`automatic`] to say the automatic step rather than a user op failed.
pub(super) fn shape_picture(
    processed: Processed,
    size: &WallSize,
    fit: FitMode,
    tileable: bool,
) -> Result<Processed, CyberError> {
    let wanted = (size.width, size.height);
    let current = (processed.rgba.width, processed.rgba.height);
    let ops = shape_ops((current != wanted).then_some(wanted), fit, tileable, None);
    apply_extra(processed, &ops).map_err(|error| {
        automatic(
            "resize/tiling",
            "adjust width/height or pass tileable=false",
            &error,
        )
    })
}

/// The shaped, quantized and encoded picture, ready to plan and commit.
pub(super) struct Built {
    /// Texture width in pixels.
    pub(super) width: u32,
    /// Texture height in pixels.
    pub(super) height: u32,
    /// The encoded `Doom` patch bytes with zero offsets.
    pub(super) patch_bytes: Vec<u8>,
    /// The composited texture's `PNG` bytes.
    pub(super) preview_png: Vec<u8>,
    /// One measurement object per applied op, in order.
    pub(super) steps: Vec<Value>,
    /// Distinct opaque source colours.
    pub(super) colors_used: usize,
    /// Worst squared `RGB` distance to the palette.
    pub(super) max_color_error: i32,
    /// How the size was chosen.
    pub(super) notes: Vec<String>,
    /// Non-fatal picture facts.
    pub(super) warnings: Vec<String>,
}

/// Run the whole picture side: user ops, the size rule, the automatic
/// shape ops, opaque quantization, patch encoding and the composited
/// preview. One quantization pass answers with the indexed image both
/// halves are made from: the preview paints it and the patch stores it, so
/// what an asset server serves is the plane the lump carries. The palette
/// read is the only session access here.
///
/// # Errors
///
/// [`process`]' errors for the bytes and the user ops, [`load_palette`]'
/// [`CyberError::NotFound`], [`shape_picture`]' and [`encode_patch`]'
/// errors and [`composite_png`]' errors.
pub(super) fn build_picture(
    state: &AppState,
    request: &CreateRequest,
    fetched: &Fetched,
) -> Result<Built, CyberError> {
    let processed = process(&fetched.bytes, &request.shared)?;
    let size = choose_size(request, &processed.rgba);
    let shaped = shape_picture(processed, &size, request.fit, request.tileable)?;
    let palette =
        state.with_session(|session| load_palette(session, request.shared.palette_index()))?;
    let dither = request.shared.dither_choice();
    let mut indexed = to_indexed(&shaped, &palette, Transparency::None, dither);
    let filled = fill_low_alpha(&mut indexed.image, &shaped.rgba, &palette);
    let (colors_used, max_color_error) = (indexed.colors_used, indexed.max_color_error);
    let preview_png = composite_png(&request.name, &indexed.image, &palette)?;
    let patch = Patch {
        left_offset: 0_i16,
        top_offset: 0_i16,
        image: indexed.image,
    };
    let patch_bytes = encode_patch(&patch)?;
    Ok(Built {
        width: size.width,
        height: size.height,
        patch_bytes,
        preview_png,
        steps: shaped.report.steps,
        colors_used,
        max_color_error,
        notes: size.note.into_iter().collect(),
        warnings: filled_warning(filled),
    })
}

/// The warning for repainted holes, empty for a fully opaque picture.
fn filled_warning(filled: usize) -> Vec<String> {
    if filled == 0 {
        return Vec::new();
    }
    vec![format!(
        "{filled} pixels below alpha 128 became the palette colour nearest to black: walls are \
         opaque - use import_image_as_patch for a masked patch"
    )]
}

/// The texture's `PNG` through the compositor: the record's single patch
/// folded onto its canvas and coloured with the quantization palette.
///
/// # Errors
///
/// [`compose`]' and [`encode_png`]' errors.
pub(super) fn composite_png(
    name: &str,
    image: &IndexedImage,
    palette: &Palette,
) -> Result<Vec<u8>, CyberError> {
    let component = Component {
        name,
        image: Some(image),
        x: 0_i32,
        y: 0_i32,
    };
    let composite = compose(name, image.width, image.height, &[component])?;
    encode_png(&composite.image.to_rgba(palette), None)
}
