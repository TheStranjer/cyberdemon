//! The image operation framework: [`ImageOp`] steps, [`apply_ops`] and the
//! helpers every operation group shares.
//!
//! Operations run in order over one [`RgbaImage`]; each returns its result
//! plus a `JSON` object of measurements, collected into an [`OpsReport`].
//! Geometry (crop/fit/rotate/flip) lives in [`geometry`], size-changing
//! sampling in [`resample`], tonal corrections (including grayscale) in
//! [`tone`], alpha handling in [`alpha`] and [`background`], seamless
//! tiling in [`tile`], binarization in [`threshold`], box blur in [`blur`]
//! and dithered quantization in [`dither`].
//! Everything here is pure: no MCP, no filesystem, no floats.

pub mod alpha;
pub mod background;
pub mod blur;
pub mod dither;
pub mod geometry;
pub mod resample;
pub mod threshold;
pub mod tile;
pub mod tone;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::CyberError;
use crate::gfx::image::RgbaImage;
use crate::gfx::{BYTES_PER_PIXEL, to_pixel};

/// Largest edge (pixels) an operation parameter may request and an
/// [`apply_ops`] result may have.
pub const MAX_OP_DIMENSION: u32 = 4_096;

/// How [`ImageOp::Fit`] maps the source onto the target rectangle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum FitMode {
    /// Resample to exactly the target size, ignoring the aspect ratio.
    Stretch,
    /// Scale uniformly until both edges cover the target, then centre-crop.
    Cover,
    /// Scale uniformly until both edges fit, then centre on the canvas.
    Contain,
}

/// Sampling filter used when a size changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Filter {
    /// Integer nearest-neighbour sampling.
    Nearest,
    /// Two-tap interpolation on `16.16` fixed-point source positions.
    Bilinear,
    /// Footprint averaging when shrinking, bilinear when growing.
    #[default]
    Area,
}

/// One image processing step; [`apply_ops`] runs a list of these in order.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum ImageOp {
    /// Cut out a rectangle; it must lie fully inside the image.
    Crop {
        /// Left edge of the rectangle, in source pixels.
        x: u32,
        /// Top edge of the rectangle, in source pixels.
        y: u32,
        /// Rectangle width, `1..=4096`.
        width: u32,
        /// Rectangle height, `1..=4096`.
        height: u32,
    },
    /// Resize to exactly `width` x `height` using one [`FitMode`].
    Fit {
        /// Target width, `1..=4096`.
        width: u32,
        /// Target height, `1..=4096`.
        height: u32,
        /// How the source maps onto the target rectangle.
        mode: FitMode,
        /// Sampling filter; default `area`.
        #[serde(default)]
        filter: Filter,
        /// `contain` canvas fill as `#RRGGBBAA` or `#RRGGBB`; default
        /// transparent.
        #[serde(default)]
        background: Option<String>,
    },
    /// Rotate clockwise by a quarter, half or three-quarter turn.
    Rotate {
        /// Clockwise `90`-degree turns, `1..=3`.
        quarter_turns: u8,
    },
    /// Mirror along the horizontal and/or vertical axis.
    Flip {
        /// Mirror left-to-right.
        horizontal: bool,
        /// Mirror top-to-bottom.
        vertical: bool,
    },
    /// Stretch the [`luminance`] range of opaque pixels to full scale.
    AutoLevels {
        /// Percentage of pixels allowed below `low` and above `high`,
        /// `0..=10`; default `1`.
        #[serde(default = "default_clip_percent")]
        clip_percent: u8,
    },
    /// Per-channel brightness, contrast and saturation correction.
    Adjust {
        /// Added after contrast, `-255..=255`; default `0`.
        #[serde(default)]
        brightness: i32,
        /// Contrast percentage about mid grey, `0..=400`; default `100`.
        #[serde(default = "default_hundred")]
        contrast: i32,
        /// Saturation percentage, `0..=400`; default `100`.
        #[serde(default = "default_hundred")]
        saturation: i32,
    },
    /// Make every pixel whose maximum channel difference to `color` fits
    /// within `tolerance` fully transparent.
    ColorKey {
        /// Key colour as `#RRGGBBAA` or `#RRGGBB` (the `#` is optional).
        color: String,
        /// Per-channel match tolerance, `0..=255`; default `0`.
        #[serde(default)]
        tolerance: u8,
    },
    /// Crop to the bounding box of pixels reaching `alpha_threshold`, then
    /// add `padding` transparent pixels on every side.
    Trim {
        /// Alpha at or above which a pixel counts as content; default `128`.
        #[serde(default = "default_trim_threshold")]
        alpha_threshold: u8,
        /// Transparent pixels added on each side; default `0`.
        #[serde(default)]
        padding: u32,
    },
    /// Make transparent every pixel reached by a flood fill from the image
    /// border that matches the background colour; unlike `color_key`,
    /// interior regions of the same colour survive.
    RemoveBackground {
        /// Per-channel match tolerance, `0..=255`; default `32`.
        #[serde(default = "default_background_tolerance")]
        tolerance: u8,
        /// Background colour as `#RRGGBBAA` or `#RRGGBB`; default: the
        /// average colour of the dominant border bucket.
        #[serde(default)]
        color: Option<String>,
    },
    /// Blend each edge into its opposite half-period shift so the result
    /// tiles seamlessly.
    MakeTileable {
        /// Blend width in pixels; default `width / 4` on the horizontal
        /// axis and `height / 4` on the vertical one, `1..=length / 2`
        /// on every enabled axis.
        #[serde(default)]
        band: Option<u32>,
        /// Blend the left and right edges.
        #[serde(default = "default_true")]
        horizontal: bool,
        /// Blend the top and bottom edges.
        #[serde(default = "default_true")]
        vertical: bool,
    },
    /// Binarize every pixel to opaque black or white by `luminance`.
    Threshold {
        /// Cut-off `0..=255`; `None` (the default) picks the Otsu level.
        #[serde(default)]
        level: Option<u8>,
        /// Swap the black and white classes.
        #[serde(default)]
        invert: bool,
    },
    /// Replace the colour channels of every opaque pixel with its
    /// [`luminance`], leaving every alpha untouched.
    Grayscale,
    /// Average every pixel over the `(2r + 1) x (2r + 1)` window around it,
    /// colours weighted by alpha so transparent pixels never darken edges.
    Blur {
        /// Blur radius, `1..=256`.
        radius: u32,
    },
    /// Drop hairline strokes: classify ink exactly like [`ImageOp::Threshold`]
    /// and keep only ink that can hold the `(2r + 1) x (2r + 1)` square
    /// (morphological opening), painted back as opaque black on opaque white.
    ThinStrokeFilter {
        /// Opening radius, `1..=16`; the result drops every stroke thinner
        /// than the `2r + 1` square element.
        radius: u32,
        /// Cut-off `0..=255`; `None` (the default) picks the Otsu level,
        /// the same convention as [`ImageOp::Threshold`].
        #[serde(default)]
        level: Option<u8>,
        /// Swap the black and white classes.
        #[serde(default)]
        invert: bool,
    },
}

/// Default [`ImageOp::AutoLevels::clip_percent`].
const fn default_clip_percent() -> u8 {
    1_u8
}

/// Default for [`ImageOp::Adjust`] percentages.
const fn default_hundred() -> i32 {
    100_i32
}

/// Default [`ImageOp::Trim::alpha_threshold`].
const fn default_trim_threshold() -> u8 {
    128_u8
}

/// Default [`ImageOp::RemoveBackground::tolerance`].
const fn default_background_tolerance() -> u8 {
    32_u8
}

/// Default for [`ImageOp::MakeTileable`]'s axis switches.
const fn default_true() -> bool {
    true
}

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
        | ImageOp::Flip { .. } => geometry_op(&img, index, op)?,
        ImageOp::AutoLevels { .. }
        | ImageOp::Adjust { .. }
        | ImageOp::ColorKey { .. }
        | ImageOp::Trim { .. }
        | ImageOp::RemoveBackground { .. }
        | ImageOp::MakeTileable { .. }
        | ImageOp::Threshold { .. }
        | ImageOp::Grayscale
        | ImageOp::Blur { .. }
        | ImageOp::ThinStrokeFilter { .. } => pixel_op(img, index, op)?,
    };
    check_output_size(index, &out)?;
    Ok((out, step))
}

/// Runs the shape-changing ops; the pixel dispatcher rejects these.
fn geometry_op(
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
        } => geometry::crop(
            img,
            index,
            &geometry::CropSpec {
                x,
                y,
                width,
                height,
            },
        )?,
        ImageOp::Fit {
            width,
            height,
            mode,
            filter,
            background,
        } => geometry::fit(
            img,
            index,
            &geometry::FitSpec {
                width,
                height,
                mode,
                filter,
                background: background.as_deref(),
            },
        )?,
        ImageOp::Rotate { quarter_turns } => geometry::rotate(img, index, quarter_turns)?,
        ImageOp::Flip {
            horizontal,
            vertical,
        } => geometry::flip(
            img,
            &geometry::FlipSpec {
                horizontal,
                vertical,
            },
        ),
        ImageOp::AutoLevels { .. }
        | ImageOp::Adjust { .. }
        | ImageOp::ColorKey { .. }
        | ImageOp::Trim { .. }
        | ImageOp::RemoveBackground { .. }
        | ImageOp::MakeTileable { .. }
        | ImageOp::Threshold { .. }
        | ImageOp::Grayscale
        | ImageOp::Blur { .. }
        | ImageOp::ThinStrokeFilter { .. } => {
            return Err(CyberError::Internal(format!(
                "ops[{index}]: pixel op reached the geometric dispatcher"
            )));
        }
    };
    Ok((out, step))
}

/// Runs the ops that never change the image shape.
///
/// The geometric variants are rejected here because [`apply_op`] routes
/// them to [`geometry`]; reaching this error is a dispatch bug.
fn pixel_op(img: RgbaImage, index: usize, op: ImageOp) -> Result<(RgbaImage, Value), CyberError> {
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
        ImageOp::RemoveBackground { .. }
        | ImageOp::MakeTileable { .. }
        | ImageOp::Threshold { .. }
        | ImageOp::ThinStrokeFilter { .. } => restoration_op(img, index, op),
        ImageOp::Crop { .. }
        | ImageOp::Fit { .. }
        | ImageOp::Rotate { .. }
        | ImageOp::Flip { .. } => Err(CyberError::Internal(format!(
            "ops[{index}]: geometric op reached the pixel dispatcher"
        ))),
    }
}

/// Runs the restoration group: border background removal, seamless tiling,
/// binarization and the hairline-opening filter; any other variant reaching
/// it is a dispatch bug.
fn restoration_op(
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
        | ImageOp::Crop { .. }
        | ImageOp::Fit { .. }
        | ImageOp::Rotate { .. }
        | ImageOp::Flip { .. } => Err(CyberError::Internal(format!(
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

/// Parses a `#RRGGBBAA` or `#RRGGBB` colour string (the leading `#` is
/// optional) into `RGBA` bytes; a six-digit colour is opaque.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] when `value` is not one of the two hex
/// forms.
#[inline]
pub fn parse_hex_color(value: &str) -> Result<[u8; 4], CyberError> {
    let malformed = || {
        CyberError::InvalidArgument(format!(
            "colour {value} is not a #RRGGBBAA or #RRGGBB hex value"
        ))
    };
    let digits = value.strip_prefix('#').unwrap_or(value);
    let alpha_explicit = match digits.len() {
        6 => false,
        8 => true,
        _ => return Err(malformed()),
    };
    let mut channels = [0_u8; 4];
    for (slot, pair) in channels.iter_mut().zip(digits.as_bytes().chunks_exact(2)) {
        *slot = hex_pair(pair).ok_or_else(malformed)?;
    }
    let [red, green, blue, alpha] = channels;
    if alpha_explicit {
        Ok([red, green, blue, alpha])
    } else {
        Ok([red, green, blue, 255_u8])
    }
}

/// Parses exactly two `ASCII` hex bytes into one channel value.
fn hex_pair(pair: &[u8]) -> Option<u8> {
    let text = core::str::from_utf8(pair).ok()?;
    u8::from_str_radix(text, 16).ok()
}

/// Integer luminance helper: `(299 * R + 587 * G + 114 * B) / 1000`.
#[must_use]
#[inline]
pub fn luminance(pixel: [u8; 4]) -> u8 {
    let [red, green, blue, _] = pixel;
    let weighted = u32::from(red)
        .saturating_mul(299)
        .saturating_add(u32::from(green).saturating_mul(587))
        .saturating_add(u32::from(blue).saturating_mul(114));
    let scaled = weighted.checked_div(1_000_u32).unwrap_or(0_u32);
    u8::try_from(scaled).unwrap_or(255_u8)
}

/// `InvalidArgument` reporting field `field` of op `index`; `expectation`
/// completes the sentence "ops[i].field must ...".
pub(crate) fn arg_error(index: usize, field: &str, expectation: &str) -> CyberError {
    CyberError::InvalidArgument(format!("ops[{index}].{field} must {expectation}"))
}

/// Rejects a pixel-dimension parameter outside `1..=4096`.
pub(crate) fn check_op_dimension(index: usize, field: &str, value: u32) -> Result<(), CyberError> {
    if value == 0 || value > MAX_OP_DIMENSION {
        return Err(arg_error(
            index,
            field,
            &format!("be within 1..={MAX_OP_DIMENSION}, got {value}"),
        ));
    }
    Ok(())
}

/// Rejects an `i32` parameter outside the inclusive `bounds`.
pub(crate) fn check_i32_range(
    index: usize,
    field: &str,
    value: i32,
    bounds: (i32, i32),
) -> Result<(), CyberError> {
    if bounds.0 <= value && value <= bounds.1 {
        return Ok(());
    }
    Err(arg_error(
        index,
        field,
        &format!("be within {}..={}, got {value}", bounds.0, bounds.1),
    ))
}

/// The pixel at `index` of a row-major `RGBA` byte buffer, `None` when out
/// of bounds.
pub(crate) fn pixel_at(pixels: &[u8], index: usize) -> Option<[u8; 4]> {
    let start = index.checked_mul(BYTES_PER_PIXEL)?;
    let chunk = pixels.get(start..start.checked_add(BYTES_PER_PIXEL)?)?;
    Some(to_pixel(chunk))
}

/// `numerator / denominator` rounded to nearest; `0` for a zero denominator.
pub(crate) fn rounded_div(numerator: u64, denominator: u64) -> u64 {
    let half = denominator.checked_div(2_u64).unwrap_or_default();
    let scaled = numerator.checked_add(half).unwrap_or(numerator);
    scaled.checked_div(denominator).unwrap_or_default()
}

/// `numerator / denominator` rounded up; saturates for a zero denominator.
pub(crate) fn div_ceil(numerator: u64, denominator: u64) -> u64 {
    let Some(whole) = numerator.checked_div(denominator) else {
        return u64::MAX;
    };
    if numerator.checked_rem(denominator).unwrap_or_default() == 0 {
        whole
    } else {
        whole.saturating_add(1)
    }
}

/// Clamps a signed channel value into `0..=255`.
pub(crate) fn clamp_u8(value: i32) -> u8 {
    if value < 0 {
        0_u8
    } else {
        u8::try_from(value).unwrap_or(255_u8)
    }
}

#[cfg(test)]
mod tests {
    use super::{
        Filter, FitMode, ImageOp, MAX_OP_DIMENSION, OpsReport, apply_ops, clamp_u8, div_ceil,
        luminance, parse_hex_color, rounded_div,
    };
    use crate::error::CyberError;
    use crate::gfx::image::RgbaImage;

    /// A 2x2 image: red, green, blue, white - all opaque.
    fn quad() -> RgbaImage {
        RgbaImage::new(
            2,
            2,
            vec![
                255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 255, 255,
            ],
        )
        .expect("2x2 RGBA8 is valid")
    }

    #[test]
    fn hex_color_parses_both_forms_and_optional_hash() {
        assert_eq!(
            parse_hex_color("#ff0000").expect("#RRGGBB parses"),
            [255, 0, 0, 255],
            "six digits get alpha 255"
        );
        assert_eq!(
            parse_hex_color("00ff0080").expect("RRGGBBAA parses"),
            [0, 255, 0, 128],
            "eight digits carry alpha"
        );
        assert_eq!(
            parse_hex_color("#FF0000").expect("uppercase parses"),
            [255, 0, 0, 255],
            "digits are case-insensitive"
        );
    }

    #[test]
    fn hex_color_rejects_bad_input() {
        for bad in ["", "xyz", "12345", "#ff00", "gg0000", "#ff0000ff00"] {
            let error = parse_hex_color(bad).expect_err("must not parse");
            assert_eq!(error.code(), "invalid_argument", "code for {bad}");
        }
    }

    #[test]
    fn luminance_matches_the_integer_formula() {
        assert_eq!(luminance([0, 0, 0, 255]), 0, "black");
        assert_eq!(luminance([255, 255, 255, 255]), 255, "white");
        assert_eq!(luminance([255, 0, 0, 255]), 76, "red: 299*255/1000");
        assert_eq!(luminance([0, 255, 0, 255]), 149, "green: 587*255/1000");
        assert_eq!(luminance([0, 0, 255, 255]), 29, "blue: 114*255/1000");
        assert_eq!(luminance([128, 128, 128, 0]), 128, "grey is alpha-free");
    }

    #[test]
    fn integer_helpers_round_and_clamp_as_documented() {
        assert_eq!(rounded_div(7, 2), 4, "rounds up on halves");
        assert_eq!(rounded_div(0, 0), 0, "zero denominator is zero");
        assert_eq!(div_ceil(7, 2), 4, "ceil on remainders");
        assert_eq!(div_ceil(6, 2), 3, "exact division");
        assert_eq!(div_ceil(1, 0), u64::MAX, "zero denominator saturates");
        assert_eq!(clamp_u8(-5), 0, "below black");
        assert_eq!(clamp_u8(300), 255, "above white");
        assert_eq!(clamp_u8(10), 10, "in range");
    }

    #[test]
    fn empty_op_list_returns_the_image_and_no_steps() {
        let (out, report) = apply_ops(quad(), &[]).expect("no ops cannot fail");
        assert_eq!(out.width, 2, "image passes through");
        assert_eq!(report, OpsReport::default(), "no steps for no ops");
    }

    #[test]
    fn apply_ops_chains_ops_with_one_entry_per_op() {
        let ops = [
            ImageOp::Flip {
                horizontal: true,
                vertical: false,
            },
            ImageOp::Crop {
                x: 0,
                y: 0,
                width: 2,
                height: 1,
            },
            ImageOp::Trim {
                alpha_threshold: 1,
                padding: 0,
            },
        ];
        let (out, report) = apply_ops(quad(), &ops).expect("chain applies");
        assert_eq!(
            report
                .steps
                .iter()
                .map(|step| step["op"].as_str().unwrap_or_default())
                .collect::<Vec<_>>(),
            vec!["flip", "crop", "trim"],
            "one named entry per op, in order"
        );
        assert_eq!((out.width, out.height), (2, 1), "crop sized the result");
    }

    #[test]
    fn the_new_ops_reach_their_modules_through_apply_ops() {
        let ops = [
            ImageOp::RemoveBackground {
                tolerance: 8,
                color: Some("#0000ff".to_owned()),
            },
            ImageOp::MakeTileable {
                band: Some(1),
                horizontal: true,
                vertical: false,
            },
            ImageOp::Threshold {
                level: Some(128),
                invert: false,
            },
        ];
        let (out, report) = apply_ops(quad(), &ops).expect("chain applies");
        assert_eq!(
            report
                .steps
                .iter()
                .map(|step| step["op"].as_str().unwrap_or_default())
                .collect::<Vec<_>>(),
            vec!["remove_background", "make_tileable", "threshold"],
            "each new op reports under its documented name"
        );
        assert_eq!(
            report.steps[0]["background"], "#0000FF",
            "the requested colour is reported"
        );
        assert_eq!(
            report.steps[0]["removed_pixels"], 1_i32,
            "only the blue border pixel floods"
        );
        assert_eq!(report.steps[1]["band_x"], 1_i32, "explicit horizontal band");
        assert!(
            report.steps[1]["band_y"].is_null(),
            "vertical axis disabled"
        );
        assert_eq!(
            report.steps[2]["level"], 128_i32,
            "explicit threshold level"
        );
        assert_eq!(
            out.get(0, 0),
            Some([255, 255, 255, 255]),
            "green's luminance 149 exceeds the level and binarizes to white"
        );
        assert_eq!(
            out.get(1, 0),
            Some([255, 255, 255, 255]),
            "the tiling pass overwrote blue with the white column, everything ends opaque"
        );
    }

    #[test]
    fn pixel_op_rejects_geometric_variants_it_was_not_routed_for() {
        let error = super::pixel_op(
            quad(),
            3,
            ImageOp::Flip {
                horizontal: false,
                vertical: false,
            },
        )
        .expect_err("a flip never belongs to the pixel dispatcher");
        assert_eq!(error.code(), "internal", "dispatch bugs are internal");
    }

    #[test]
    fn restoration_op_rejects_variants_it_was_not_routed_for() {
        let error = super::restoration_op(
            quad(),
            5,
            ImageOp::Trim {
                alpha_threshold: 128,
                padding: 0,
            },
        )
        .expect_err("a trim never belongs to the restoration dispatcher");
        let CyberError::Internal(message) = error else {
            panic!("expected Internal");
        };
        assert!(
            message.contains("ops[5]") && message.contains("wrong pixel dispatcher"),
            "misdispatch names the op and the fault: {message}"
        );
    }

    #[test]
    fn geometry_op_rejects_pixel_variants_it_was_not_routed_for() {
        let error = super::geometry_op(
            &quad(),
            1,
            ImageOp::Threshold {
                level: Some(128),
                invert: false,
            },
        )
        .expect_err("a threshold never belongs to the geometric dispatcher");
        assert_eq!(error.code(), "internal", "dispatch bugs are internal");
    }

    #[test]
    fn invalid_parameters_name_the_op_index_and_field() {
        let ops = [
            ImageOp::Flip {
                horizontal: true,
                vertical: false,
            },
            ImageOp::Rotate { quarter_turns: 2 },
            ImageOp::Crop {
                x: 0,
                y: 0,
                width: 0,
                height: 1,
            },
        ];
        let error = apply_ops(quad(), &ops).expect_err("zero width must fail");
        let CyberError::InvalidArgument(message) = error else {
            panic!("expected InvalidArgument");
        };
        assert!(
            message.contains("ops[2].width") && message.contains("1..=4096"),
            "message names the failing op: {message}"
        );
    }

    #[test]
    fn fit_parameters_are_capped_at_the_op_dimension() {
        let error = apply_ops(
            quad(),
            &[ImageOp::Fit {
                width: MAX_OP_DIMENSION + 1,
                height: 1,
                mode: FitMode::Stretch,
                filter: Filter::Nearest,
                background: None,
            }],
        )
        .expect_err("a 4097-wide target must fail");
        let CyberError::InvalidArgument(message) = error else {
            panic!("expected InvalidArgument");
        };
        assert!(
            message.contains("ops[0].width") && message.contains("1..=4096"),
            "parameter error names the field and range: {message}"
        );
    }

    #[test]
    fn results_above_the_op_dimension_are_rejected() {
        let wide = RgbaImage::new(8_000, 1, vec![255_u8; 32_000_usize]).expect("8000x1 fits core");
        let error = apply_ops(
            wide,
            &[ImageOp::Flip {
                horizontal: true,
                vertical: false,
            }],
        )
        .expect_err("an 8000-wide result exceeds the op limit");
        let CyberError::InvalidArgument(message) = error else {
            panic!("expected InvalidArgument");
        };
        assert!(
            message.contains("ops[0]") && message.contains("exceeds 4096"),
            "result error names the op and limit: {message}"
        );
    }

    #[test]
    fn json_op_lists_deserialize_with_defaults() {
        let text = r##"[
            {"op":"crop","x":1,"y":2,"width":3,"height":4},
            {"op":"fit","width":8,"height":8,"mode":"contain"},
            {"op":"rotate","quarter_turns":3},
            {"op":"flip","horizontal":false,"vertical":true},
            {"op":"auto_levels"},
            {"op":"adjust"},
            {"op":"color_key","color":"#ff0000"},
            {"op":"trim"}
        ]"##;
        let ops: Vec<ImageOp> = serde_json::from_str(text).expect("the documented JSON layout");
        assert_eq!(ops.len(), 8, "all eight ops deserialize");
        let ImageOp::Fit {
            filter,
            background,
            mode,
            ..
        } = ops[1].clone()
        else {
            panic!("expected Fit at index 1");
        };
        assert_eq!(filter, Filter::Area, "filter defaults to area");
        assert_eq!(background, None, "background defaults to absent");
        assert_eq!(mode, FitMode::Contain, "explicit mode parses");
        let ImageOp::AutoLevels { clip_percent } = ops[4].clone() else {
            panic!("expected AutoLevels at index 4");
        };
        assert_eq!(clip_percent, 1, "clip defaults to one percent");
        let ImageOp::Adjust {
            brightness,
            contrast,
            saturation,
        } = ops[5].clone()
        else {
            panic!("expected Adjust at index 5");
        };
        assert_eq!(
            (brightness, contrast, saturation),
            (0_i32, 100_i32, 100_i32),
            "adjust defaults"
        );
        let ImageOp::Trim {
            alpha_threshold,
            padding,
        } = ops[7].clone()
        else {
            panic!("expected Trim at index 7");
        };
        assert_eq!((alpha_threshold, padding), (128, 0), "trim defaults");
    }

    #[test]
    fn new_pixel_ops_deserialize_with_documented_defaults() {
        let text = r##"[
            {"op":"remove_background"},
            {"op":"remove_background","tolerance":8,"color":"#102030"},
            {"op":"make_tileable"},
            {"op":"make_tileable","band":3,"vertical":false},
            {"op":"threshold"},
            {"op":"threshold","level":128,"invert":true}
        ]"##;
        let ops: Vec<ImageOp> = serde_json::from_str(text).expect("the documented JSON layout");
        assert_eq!(ops.len(), 6, "all six entries parse");
        assert_eq!(background_of(&ops[0]), (32, None), "background defaults");
        assert_eq!(
            background_of(&ops[1]),
            (8, Some("#102030".to_owned())),
            "explicit background"
        );
        assert_eq!(tile_of(&ops[2]), (None, true, true), "tile defaults");
        assert_eq!(tile_of(&ops[3]), (Some(3), true, false), "explicit tile");
        assert_eq!(threshold_of(&ops[4]), (None, false), "threshold defaults");
        assert_eq!(
            threshold_of(&ops[5]),
            (Some(128), true),
            "explicit threshold"
        );
    }

    /// The `RemoveBackground` fields of an op that must be one.
    fn background_of(op: &ImageOp) -> (u8, Option<String>) {
        let ImageOp::RemoveBackground { tolerance, color } = op.clone() else {
            panic!("expected a RemoveBackground op");
        };
        (tolerance, color)
    }

    /// The `MakeTileable` fields of an op that must be one.
    fn tile_of(op: &ImageOp) -> (Option<u32>, bool, bool) {
        let ImageOp::MakeTileable {
            band,
            horizontal,
            vertical,
        } = op.clone()
        else {
            panic!("expected a MakeTileable op");
        };
        (band, horizontal, vertical)
    }

    /// The `Threshold` fields of an op that must be one.
    fn threshold_of(op: &ImageOp) -> (Option<u8>, bool) {
        let ImageOp::Threshold { level, invert } = op.clone() else {
            panic!("expected a Threshold op");
        };
        (level, invert)
    }

    #[test]
    fn grayscale_and_blur_deserialize_from_their_documented_json() {
        let text = r#"[
            {"op":"grayscale"},
            {"op":"blur","radius":2}
        ]"#;
        let ops: Vec<ImageOp> = serde_json::from_str(text).expect("the documented JSON layout");
        assert_eq!(ops.len(), 2, "both new ops parse");
        assert_eq!(
            ops[0],
            ImageOp::Grayscale,
            "grayscale carries no parameters"
        );
        let ImageOp::Blur { radius } = ops[1].clone() else {
            panic!("expected Blur at index 1");
        };
        assert_eq!(radius, 2_u32, "the blur radius carries through");
    }

    #[test]
    fn grayscale_and_blur_run_through_the_pixel_dispatcher() {
        let ops = [ImageOp::Grayscale, ImageOp::Blur { radius: 1 }];
        let (out, report) = apply_ops(quad(), &ops).expect("chain applies");
        assert_eq!(
            report
                .steps
                .iter()
                .map(|step| step["op"].as_str().unwrap_or_default())
                .collect::<Vec<_>>(),
            vec!["grayscale", "blur"],
            "one named entry per op, in order"
        );
        assert_eq!(
            report.steps[0]["distinct_colors_before"], 4_i32,
            "the quad's four opaque colours"
        );
        assert_eq!(
            report.steps[0]["distinct_colors_after"], 4_i32,
            "76, 149, 29 and 255 greys stay distinct"
        );
        assert_eq!(report.steps[1]["radius"], 1_i32, "the radius echoes");
        let uniform = out.get(0, 0).expect("in bounds");
        for (x, y) in [(0_u32, 0_u32), (1, 0), (0, 1), (1, 1)] {
            assert_eq!(out.get(x, y), Some(uniform), "({x},{y}) joins the blur");
        }
        assert_eq!(
            [uniform[0], uniform[1], uniform[2]],
            [128_u8, 128, 128],
            "the whole 2x2 window is every pixel's window: the grey mean rounds to 128"
        );
        assert_eq!(uniform[3], 255, "opaque stays opaque");
    }

    #[test]
    fn blur_radius_errors_name_the_op_index_and_field() {
        let error = apply_ops(quad(), &[ImageOp::Grayscale, ImageOp::Blur { radius: 0 }])
            .expect_err("radius 0 must fail");
        let CyberError::InvalidArgument(message) = error else {
            panic!("expected InvalidArgument");
        };
        assert!(
            message.contains("ops[1].radius") && message.contains("1..=256"),
            "the parameter error names the field and range: {message}"
        );
    }

    #[test]
    fn thin_stroke_filter_deserializes_with_documented_defaults() {
        let text = r#"[
            {"op":"thin_stroke_filter","radius":1},
            {"op":"thin_stroke_filter","radius":16,"level":128,"invert":true}
        ]"#;
        let ops: Vec<ImageOp> = serde_json::from_str(text).expect("the documented JSON layout");
        assert_eq!(
            thin_stroke_fields(&ops[0]),
            (1_u32, None, false),
            "level defaults to Otsu and invert to false"
        );
        assert_eq!(
            thin_stroke_fields(&ops[1]),
            (16_u32, Some(128), true),
            "every explicit field carries through"
        );
    }

    /// The `ThinStrokeFilter` fields of an op that must be one.
    fn thin_stroke_fields(op: &ImageOp) -> (u32, Option<u8>, bool) {
        let ImageOp::ThinStrokeFilter {
            radius,
            level,
            invert,
        } = op.clone()
        else {
            panic!("expected a ThinStrokeFilter op");
        };
        (radius, level, invert)
    }

    #[test]
    fn thin_stroke_filter_runs_through_the_restoration_dispatcher() {
        let ops = [
            ImageOp::Threshold {
                level: Some(128),
                invert: false,
            },
            ImageOp::ThinStrokeFilter {
                radius: 1,
                level: None,
                invert: false,
            },
        ];
        let (out, report) = apply_ops(quad(), &ops).expect("chain applies");
        assert_eq!(
            report.steps[1]["op"], "thin_stroke_filter",
            "the documented step name"
        );
        assert_eq!(report.steps[1]["radius"], 1_i32, "the radius echoes");
        assert_eq!(report.steps[1]["otsu"], true, "level None is Otsu's");
        assert!(report.steps[1]["level"].is_number(), "the level reports");
        assert_eq!(out.width, 2, "the op never changes the shape");
    }

    #[test]
    fn thin_stroke_radius_errors_name_the_op_index_and_field() {
        assert_thin_radius_rejected(0_u32);
        assert_thin_radius_rejected(17_u32);
    }

    /// Applies a `thin_stroke_filter` with the out-of-range `radius` after a
    /// leading op and checks the error names `ops[1].radius` and `1..=16`.
    fn assert_thin_radius_rejected(radius: u32) {
        let error = apply_ops(
            quad(),
            &[
                ImageOp::Flip {
                    horizontal: true,
                    vertical: false,
                },
                ImageOp::ThinStrokeFilter {
                    radius,
                    level: Some(128),
                    invert: false,
                },
            ],
        )
        .expect_err("a radius outside 1..=16 must fail");
        let CyberError::InvalidArgument(message) = error else {
            panic!("expected InvalidArgument");
        };
        assert!(
            message.contains("ops[1].radius") && message.contains("1..=16"),
            "the parameter error names the field and range: {message}"
        );
    }
}
