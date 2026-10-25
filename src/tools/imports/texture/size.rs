//! The tiling size rule of one `create_texture_from_image` call: the
//! checks [`super::parse_create`] runs over the explicit edges, the
//! [`VALID_WIDTHS`] a refused width is offered from, and the
//! [`WallSize`] [`choose_size`] answers with - an explicit edge wins, a
//! missing one follows the picture's aspect ratio, and a picture no
//! vanilla engine can tile falls back to [`FALLBACK_SIZE`] square with
//! the note that says so.
//!
//! The [`super::CreateRequest`] this reads carries the already-validated
//! edges; every limit lives in the group's root beside the parameters
//! that produced them.

use crate::error::CyberError;
use crate::fixed::div_round;
use crate::gfx::image::RgbaImage;
use crate::gfx::ops::rounded_div;

use super::{
    CreateRequest, FALLBACK_SIZE, MAX_TEXTURE_EDGE, MIN_WIDTH, SIZE_ROUND_STEP, VALID_WIDTHS,
};

/// The explicit `width`: a power of two in `16..=1024`; the refusal names
/// the nearest valid widths.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] for any other value.
pub(super) fn check_width(value: u32) -> Result<u32, CyberError> {
    if value.is_power_of_two() && (MIN_WIDTH..=MAX_TEXTURE_EDGE).contains(&value) {
        return Ok(value);
    }
    Err(CyberError::InvalidArgument(format!(
        "width {value} is not a power of two in {MIN_WIDTH}..={MAX_TEXTURE_EDGE}: pass one \
         of the nearest valid widths {} (vanilla engines tile a texture by its width)",
        nearest_widths(value),
    )))
}

/// The explicit `height`: `1..=1024`.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] for any other value.
pub(super) fn check_height(value: u32) -> Result<u32, CyberError> {
    if value == 0 || value > MAX_TEXTURE_EDGE {
        return Err(CyberError::InvalidArgument(format!(
            "height {value} is out of range: pass a value within 1..={MAX_TEXTURE_EDGE}"
        )));
    }
    Ok(value)
}

/// The two [`VALID_WIDTHS`] nearest `value`, as `"64 or 128"` (one name
/// at the ends of the range).
pub(super) fn nearest_widths(value: u32) -> String {
    let below = VALID_WIDTHS.iter().copied().rfind(|w| *w <= value);
    let above = VALID_WIDTHS.iter().copied().find(|w| *w >= value);
    let mut names: Vec<String> = Vec::new();
    for width in below.into_iter().chain(above) {
        let text = width.to_string();
        if !names.contains(&text) {
            names.push(text);
        }
    }
    names.join(" or ")
}

/// The [`VALID_WIDTHS`] member nearest `value`; ties take the smaller.
pub(super) fn nearest_width(value: u32) -> u32 {
    VALID_WIDTHS
        .iter()
        .copied()
        .min_by_key(|width| {
            width
                .saturating_sub(value)
                .max(value.saturating_sub(*width))
        })
        .unwrap_or(FALLBACK_SIZE)
}

/// The texture size one build writes, with the note the fallback rule
/// adds.
#[derive(Debug)]
pub(super) struct WallSize {
    /// Texture width in pixels.
    pub(super) width: u32,
    /// Texture height in pixels.
    pub(super) height: u32,
    /// Why a picture that could not keep its own size got `128x128`.
    pub(super) note: Option<String>,
}

/// The size rule over the processed picture: explicit dimensions
/// (already validated by [`super::parse_create`]) win, a missing one
/// follows the picture's aspect ratio, and with neither a tileable
/// picture is kept and any other falls back to [`FALLBACK_SIZE`] square
/// with a note.
pub(super) fn choose_size(request: &CreateRequest, picture: &RgbaImage) -> WallSize {
    match (request.width, request.height) {
        (Some(width), Some(height)) => WallSize {
            width,
            height,
            note: None,
        },
        (Some(width), None) => WallSize {
            width,
            height: derive_height(width, picture),
            note: None,
        },
        (None, Some(height)) => WallSize {
            width: derive_width(height, picture),
            height,
            note: None,
        },
        (None, None) => keep_or_fallback(picture),
    }
}

/// The aspect-derived height for an explicit `width`, rounded to a
/// multiple of [`SIZE_ROUND_STEP`] and kept inside the edge limits.
fn derive_height(width: u32, picture: &RgbaImage) -> u32 {
    let scaled = rounded_div(
        u64::from(width).saturating_mul(u64::from(picture.height)),
        u64::from(picture.width),
    );
    let exact = u32::try_from(scaled).unwrap_or(MAX_TEXTURE_EDGE);
    round_to_step(exact).clamp(SIZE_ROUND_STEP, MAX_TEXTURE_EDGE)
}

/// The aspect-derived width for an explicit `height`, taken as the
/// nearest member of [`VALID_WIDTHS`].
fn derive_width(height: u32, picture: &RgbaImage) -> u32 {
    let scaled = rounded_div(
        u64::from(height).saturating_mul(u64::from(picture.width)),
        u64::from(picture.height),
    );
    nearest_width(u32::try_from(scaled).unwrap_or(u32::MAX))
}

/// Keep the processed size when a vanilla engine can tile it, else the
/// square fallback with its note.
pub(super) fn keep_or_fallback(picture: &RgbaImage) -> WallSize {
    let keeps = picture.width.is_power_of_two()
        && (MIN_WIDTH..=MAX_TEXTURE_EDGE).contains(&picture.width)
        && picture.height <= MAX_TEXTURE_EDGE;
    if keeps {
        return WallSize {
            width: picture.width,
            height: picture.height,
            note: None,
        };
    }
    WallSize {
        width: FALLBACK_SIZE,
        height: FALLBACK_SIZE,
        note: Some(format!(
            "resized to {FALLBACK_SIZE}x{FALLBACK_SIZE}; pass width/height to choose"
        )),
    }
}

/// `value` to the nearest multiple of [`SIZE_ROUND_STEP`], rounding the
/// step count through [`div_round`] (halves away from zero).
pub(super) fn round_to_step(value: u32) -> u32 {
    div_round(i128::from(value), i128::from(SIZE_ROUND_STEP))
        .and_then(|steps| u32::try_from(steps).ok())
        .and_then(|steps| steps.checked_mul(SIZE_ROUND_STEP))
        .unwrap_or(value)
}
