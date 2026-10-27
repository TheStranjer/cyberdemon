//! The [`apply_ops`] runner: one [`ImageOp`] list folded into a finished
//! [`RgbaImage`] and its [`OpsReport`]. The one dispatch `match` names
//! every variant and calls that variant's own module directly —
//! [`geometry`] and [`perspective`] for the shape-changing ops, [`tone`],
//! [`alpha`], [`background`], [`tile`], [`adaptive`], [`threshold`] and
//! [`blur`] for the rest — so an op is routed in exactly one place, and
//! every result is bounded by [`MAX_OP_DIMENSION`].
//!
//! The op vocabulary itself, [`ImageOp`] with its [`FitMode`] and
//! [`Filter`], lives in the [`ops`](super) root beside the shared helpers
//! the operation modules use.

use serde::Serialize;
use serde_json::Value;

use crate::error::CyberError;
use crate::gfx::image::RgbaImage;
use crate::gfx::ops::{
    ImageOp, MAX_OP_DIMENSION, adaptive, alpha, background, blur, geometry, perspective, threshold,
    tile, tone,
};

/// Measurements collected by [`apply_ops`]: one `JSON` object per op.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct OpsReport {
    /// Entries in application order, each `{"op": <name>, ...measurements}`.
    pub steps: Vec<Value>,
}

/// Applies `ops` to `img` in order, collecting one report entry per op.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] when a parameter is out of range (the
/// message names the op index and field, e.g. `ops[2].width`), when a
/// result would exceed [`MAX_OP_DIMENSION`] on an edge, or when an op's
/// precondition fails (a crop outside the image, nothing left to trim).
/// The nothing-left-to-trim refusal arrives as a [`CyberError::Emptied`]
/// marker around its `invalid_argument`, so a caller that trims on the
/// user's behalf can reword it without reading the text. The first failing
/// op's error stops the chain.
#[inline]
pub fn apply_ops(img: RgbaImage, ops: &[ImageOp]) -> Result<(RgbaImage, OpsReport), CyberError> {
    let mut current = img;
    let mut steps = Vec::with_capacity(ops.len());
    for (index, op) in ops.iter().cloned().enumerate() {
        let (next, step) = apply_op(current, index, op)?;
        check_output_size(index, &next)?;
        current = next;
        steps.push(step);
    }
    Ok((current, OpsReport { steps }))
}

/// Runs one op through the function of the module that owns it.
///
/// Every [`ImageOp`] variant is named exactly once here, so this `match` is
/// the whole routing table: adding an op means adding one arm and nothing
/// else, and no arm can be reached by the wrong variant.
fn apply_op(img: RgbaImage, index: usize, op: ImageOp) -> Result<(RgbaImage, Value), CyberError> {
    match op {
        ImageOp::Crop {
            x,
            y,
            width,
            height,
        } => geometry::crop(&img, index, (x, y, width, height)),
        ImageOp::Fit {
            width,
            height,
            mode,
            filter,
            background,
        } => geometry::fit(&img, index, (width, height, mode, filter), background),
        ImageOp::Rotate { quarter_turns } => geometry::rotate(&img, index, quarter_turns),
        ImageOp::Flip {
            horizontal,
            vertical,
        } => Ok(geometry::flip(&img, (horizontal, vertical))),
        ImageOp::PerspectiveCorrect {
            corners,
            width,
            height,
        } => perspective::correct(&img, index, (corners, width, height)),
        ImageOp::AutoLevels { clip_percent } => tone::auto_levels(img, index, clip_percent),
        ImageOp::Adjust {
            brightness,
            contrast,
            saturation,
        } => tone::adjust(img, index, (brightness, contrast, saturation)),
        ImageOp::ColorKey { color, tolerance } => alpha::color_key(img, index, &color, tolerance),
        ImageOp::Trim {
            alpha_threshold,
            padding,
        } => alpha::trim(&img, index, (alpha_threshold, padding)),
        ImageOp::RemoveBackground { tolerance, color } => {
            background::remove_background(img, index, color.as_deref(), tolerance)
        }
        ImageOp::MakeTileable {
            band,
            horizontal,
            vertical,
        } => tile::make_tileable(img, index, (band, horizontal, vertical)),
        ImageOp::Threshold { level, invert } => Ok(threshold::binarize(img, level, invert)),
        ImageOp::AdaptiveThreshold {
            radius,
            offset,
            invert,
        } => adaptive::adaptive_threshold(img, index, (radius, offset, invert)),
        ImageOp::Grayscale => Ok(tone::grayscale(img)),
        ImageOp::Blur { radius } => blur::blur(img, index, radius),
        ImageOp::ThinStrokeFilter {
            radius,
            level,
            invert,
        } => threshold::thin_stroke(img, index, (radius, level, invert)),
    }
}

/// Rejects results larger than `4096x4096`.
fn check_output_size(index: usize, img: &RgbaImage) -> Result<(), CyberError> {
    let (name, value) = if img.width > MAX_OP_DIMENSION {
        ("width", img.width)
    } else if img.height > MAX_OP_DIMENSION {
        ("height", img.height)
    } else {
        return Ok(());
    };
    Err(CyberError::InvalidArgument(format!(
        "ops[{index}] result {name} {value} exceeds {MAX_OP_DIMENSION}; use smaller parameters"
    )))
}
