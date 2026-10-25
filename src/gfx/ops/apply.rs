//! The [`apply_ops`] runner: one [`ImageOp`] list folded into a finished
//! [`RgbaImage`] and its [`OpsReport`], dispatched by shape - the
//! shape-changing ops through [`geometry`] and [`perspective`], the rest
//! through the pixel and restoration dispatchers that reach every other
//! operation module - and bounded by [`MAX_OP_DIMENSION`] after each step.
//!
//! The op vocabulary itself, [`ImageOp`] with its [`FitMode`] and
//! [`Filter`], lives in the [`ops`](super) root beside the shared helpers
//! the operation modules use.

use serde::Serialize;
use serde_json::Value;

use crate::error::CyberError;
use crate::gfx::image::RgbaImage;
use crate::gfx::ops::perspective::Corner;
use crate::gfx::ops::{
    Filter, FitMode, ImageOp, MAX_OP_DIMENSION, adaptive, alpha, background, blur, geometry,
    perspective, threshold, tile, tone,
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
/// The first failing op's error stops the chain.
#[inline]
pub fn apply_ops(img: RgbaImage, ops: &[ImageOp]) -> Result<(RgbaImage, OpsReport), CyberError> {
    let mut current = img;
    let mut steps = Vec::with_capacity(ops.len());
    for (index, op) in ops.iter().cloned().enumerate() {
        let (next, step) = apply_op(current, index, op)?;
        current = next;
        steps.push(step);
    }
    Ok((current, OpsReport { steps }))
}

/// Runs one op and enforces the result-size limit.
fn apply_op(img: RgbaImage, index: usize, op: ImageOp) -> Result<(RgbaImage, Value), CyberError> {
    let (out, step) = match op {
        ImageOp::Crop { .. }
        | ImageOp::Fit { .. }
        | ImageOp::Rotate { .. }
        | ImageOp::Flip { .. }
        | ImageOp::PerspectiveCorrect { .. } => geometry_op(&img, index, op)?,
        ImageOp::AutoLevels { .. }
        | ImageOp::Adjust { .. }
        | ImageOp::ColorKey { .. }
        | ImageOp::Trim { .. }
        | ImageOp::RemoveBackground { .. }
        | ImageOp::MakeTileable { .. }
        | ImageOp::Threshold { .. }
        | ImageOp::AdaptiveThreshold { .. }
        | ImageOp::Grayscale
        | ImageOp::Blur { .. }
        | ImageOp::ThinStrokeFilter { .. } => pixel_op(img, index, op)?,
    };
    check_output_size(index, &out)?;
    Ok((out, step))
}

/// Runs the shape-changing ops; the pixel dispatcher rejects these.
pub(super) fn geometry_op(
    img: &RgbaImage,
    index: usize,
    op: ImageOp,
) -> Result<(RgbaImage, Value), CyberError> {
    let (out, step) = match op {
        ImageOp::Crop {
            x,
            y,
            width,
            height,
        } => crop_arm(img, index, (x, y, width, height))?,
        ImageOp::Fit {
            width,
            height,
            mode,
            filter,
            background,
        } => {
            let rect = (width, height, mode, filter);
            fit_arm(img, index, rect, background.as_deref())?
        }
        ImageOp::Rotate { quarter_turns } => geometry::rotate(img, index, quarter_turns)?,
        ImageOp::Flip {
            horizontal,
            vertical,
        } => flip_arm(img, (horizontal, vertical)),
        ImageOp::PerspectiveCorrect {
            corners,
            width,
            height,
        } => perspective_arm(img, index, (corners, width, height))?,
        ImageOp::AutoLevels { .. }
        | ImageOp::Adjust { .. }
        | ImageOp::ColorKey { .. }
        | ImageOp::Trim { .. }
        | ImageOp::RemoveBackground { .. }
        | ImageOp::MakeTileable { .. }
        | ImageOp::Threshold { .. }
        | ImageOp::AdaptiveThreshold { .. }
        | ImageOp::Grayscale
        | ImageOp::Blur { .. }
        | ImageOp::ThinStrokeFilter { .. } => {
            return Err(geometry_mispatch(index));
        }
    };
    Ok((out, step))
}

/// The `Crop` arm: spec assembled, run through [`geometry`].
fn crop_arm(
    img: &RgbaImage,
    index: usize,
    rect: (u32, u32, u32, u32),
) -> Result<(RgbaImage, Value), CyberError> {
    let (x, y, width, height) = rect;
    geometry::crop(
        img,
        index,
        &geometry::CropSpec {
            x,
            y,
            width,
            height,
        },
    )
}

/// The `Fit` arm: spec assembled, run through [`geometry`].
fn fit_arm(
    img: &RgbaImage,
    index: usize,
    rect: (u32, u32, FitMode, Filter),
    background: Option<&str>,
) -> Result<(RgbaImage, Value), CyberError> {
    let (width, height, mode, filter) = rect;
    let spec = geometry::FitSpec {
        width,
        height,
        mode,
        filter,
        background,
    };
    geometry::fit(img, index, &spec)
}

/// The `Flip` arm: spec assembled, run through [`geometry`].
fn flip_arm(img: &RgbaImage, axes: (bool, bool)) -> (RgbaImage, Value) {
    let (horizontal, vertical) = axes;
    geometry::flip(
        img,
        &geometry::FlipSpec {
            horizontal,
            vertical,
        },
    )
}

/// The `PerspectiveCorrect` arm: spec assembled, run through
/// [`perspective`].
fn perspective_arm(
    img: &RgbaImage,
    index: usize,
    quad: ([Corner; 4], u32, u32),
) -> Result<(RgbaImage, Value), CyberError> {
    let (corners, width, height) = quad;
    let spec = perspective::PerspectiveSpec {
        corners,
        width,
        height,
    };
    perspective::correct(img, index, &spec)
}

/// The dispatch-bug error for a pixel op reaching [`geometry_op`]; any
/// variant not routed to [`geometry`] is reported by name of the fault.
fn geometry_mispatch(index: usize) -> CyberError {
    CyberError::Internal(format!(
        "ops[{index}]: pixel op reached the geometric dispatcher"
    ))
}

/// Runs the ops that never change the image shape.
///
/// The geometric variants are rejected here because [`apply_op`] routes
/// them to [`geometry`]; reaching this error is a dispatch bug.
pub(super) fn pixel_op(
    img: RgbaImage,
    index: usize,
    op: ImageOp,
) -> Result<(RgbaImage, Value), CyberError> {
    match op {
        ImageOp::AutoLevels { clip_percent } => tone::auto_levels(img, index, clip_percent),
        ImageOp::Adjust {
            brightness,
            contrast,
            saturation,
        } => {
            let spec = tone::AdjustSpec {
                brightness,
                contrast,
                saturation,
            };
            tone::adjust(img, index, &spec)
        }
        ImageOp::ColorKey { color, tolerance } => {
            let spec = alpha::ColorKeySpec {
                color: &color,
                tolerance,
            };
            alpha::color_key(img, index, &spec)
        }
        ImageOp::Trim {
            alpha_threshold,
            padding,
        } => alpha::trim(
            &img,
            index,
            &alpha::TrimSpec {
                alpha_threshold,
                padding,
            },
        ),
        ImageOp::Grayscale => Ok(tone::grayscale(img)),
        ImageOp::Blur { radius } => blur::blur(img, index, radius),
        ImageOp::AdaptiveThreshold {
            radius,
            offset,
            invert,
        } => {
            let spec = adaptive::AdaptiveThresholdSpec {
                radius,
                offset,
                invert,
            };
            adaptive::adaptive_threshold(img, index, &spec)
        }
        ImageOp::RemoveBackground { .. }
        | ImageOp::MakeTileable { .. }
        | ImageOp::Threshold { .. }
        | ImageOp::ThinStrokeFilter { .. } => restoration_op(img, index, op),
        ImageOp::Crop { .. }
        | ImageOp::Fit { .. }
        | ImageOp::Rotate { .. }
        | ImageOp::Flip { .. }
        | ImageOp::PerspectiveCorrect { .. } => Err(CyberError::Internal(format!(
            "ops[{index}]: geometric op reached the pixel dispatcher"
        ))),
    }
}

/// Runs the restoration group: border background removal, seamless tiling,
/// binarization and the hairline-opening filter; any other variant reaching
/// it is a dispatch bug.
pub(super) fn restoration_op(
    img: RgbaImage,
    index: usize,
    op: ImageOp,
) -> Result<(RgbaImage, Value), CyberError> {
    match op {
        ImageOp::RemoveBackground { tolerance, color } => {
            let spec = background::BackgroundSpec {
                color: color.as_deref(),
                tolerance,
            };
            background::remove_background(img, index, &spec)
        }
        ImageOp::MakeTileable {
            band,
            horizontal,
            vertical,
        } => {
            let spec = tile::TileSpec {
                band,
                horizontal,
                vertical,
            };
            tile::make_tileable(img, index, &spec)
        }
        ImageOp::Threshold { level, invert } => {
            let spec = threshold::ThresholdSpec { level, invert };
            Ok(threshold::binarize(img, &spec))
        }
        ImageOp::ThinStrokeFilter {
            radius,
            level,
            invert,
        } => {
            let spec = threshold::ThinStrokeSpec {
                radius,
                level,
                invert,
            };
            threshold::thin_stroke(img, index, &spec)
        }
        ImageOp::AutoLevels { .. }
        | ImageOp::Adjust { .. }
        | ImageOp::ColorKey { .. }
        | ImageOp::Trim { .. }
        | ImageOp::Grayscale
        | ImageOp::Blur { .. }
        | ImageOp::AdaptiveThreshold { .. }
        | ImageOp::Crop { .. }
        | ImageOp::Fit { .. }
        | ImageOp::Rotate { .. }
        | ImageOp::Flip { .. }
        | ImageOp::PerspectiveCorrect { .. } => Err(CyberError::Internal(format!(
            "ops[{index}]: op reached the wrong pixel dispatcher"
        ))),
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
