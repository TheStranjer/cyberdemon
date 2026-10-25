//! The image operation framework: [`ImageOp`] steps, [`apply_ops`] and the
//! helpers every operation group shares.
//!
//! Operations run in order over one
//! [`RgbaImage`](crate::gfx::image::RgbaImage); each returns its result plus
//! a `JSON` object of measurements, collected into an [`OpsReport`].
//! Geometry (crop/fit/rotate/flip) lives in [`geometry`], the
//! corner-quad-to-rectangle warp in [`perspective`], size-changing
//! sampling in [`resample`], tonal corrections (including grayscale) in
//! [`tone`], alpha handling in [`alpha`] and [`background`], seamless
//! tiling in [`tile`], binarization in [`threshold`] (with the
//! background-flattening uneven-light variant in [`adaptive`]), box blur in
//! [`blur`] and dithered quantization in [`dither`].
//! Everything here is pure: no MCP, no filesystem, no floats.
//!
//! The framework itself is split: this root keeps the op vocabulary -
//! [`ImageOp`] with its [`FitMode`] and [`Filter`] knobs and the serde
//! defaults - while the private `apply` module runs a list of them,
//! [`colour`] parses the hex colours and measures [`luminance`], and
//! [`checks`] holds the range guards and integer helpers every operation
//! module shares. The unit tests live in `gfx/ops/tests.rs` with its
//! `colour`, `dispatch` and `json` topics.

pub mod adaptive;
pub mod alpha;
pub mod background;
pub mod blur;
pub mod dither;
pub mod geometry;
pub mod perspective;
pub mod resample;
pub mod threshold;
pub mod tile;
pub mod tone;

mod apply;
// `checks` and `colour` are public for one reason: they hold `pub(crate)`
// helpers, which `redundant_pub_crate` only accepts inside a public module.
pub mod checks;
pub mod colour;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::gfx::ops::perspective::Corner;

pub use apply::{OpsReport, apply_ops};
pub(crate) use checks::{
    arg_error, check_i32_range, check_op_dimension, div_ceil, pixel_at, rounded_div,
};
pub(crate) use colour::clamp_u8;
pub use colour::{luminance, parse_hex_color};

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
    /// Warp a four-corner source quad onto a straight rectangle: the
    /// destination's corner pixels sample exactly the given positions and
    /// the projective map between is solved in `16.16` fixed point.
    PerspectiveCorrect {
        /// The quad's corners in source pixel coordinates, ordered
        /// top-left, top-right, bottom-right, bottom-left; they may be
        /// negative, but must stay distinct with no three on one line.
        corners: [Corner; 4],
        /// Destination width, `1..=4096`.
        width: u32,
        /// Destination height, `1..=4096`.
        height: u32,
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
    /// Binarize against a flattened local background: the uneven-light
    /// variant of [`ImageOp::Threshold`]. A pixel is ink when its
    /// [`luminance`] sits `offset` or more below the mean of the
    /// `(2r + 1) x (2r + 1)` window around it (clipped at the borders);
    /// transparent pixels are paper.
    AdaptiveThreshold {
        /// Background window radius, `1..=128`; small for line art, a large
        /// value (about `80`) for photographs.
        radius: u32,
        /// How far below the local mean counts as ink, `0..=255`;
        /// default `15`.
        #[serde(default = "default_adaptive_offset")]
        offset: u8,
        /// Swap the ink and paper classes: then pixels `offset` or more
        /// above the local mean are the ink.
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

/// Default [`ImageOp::AdaptiveThreshold::offset`].
const fn default_adaptive_offset() -> u8 {
    15_u8
}

#[cfg(test)]
mod tests;
