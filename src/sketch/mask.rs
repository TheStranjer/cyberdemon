//! The [`Mask`] raster and the two measurement primitives that read an
//! image: [`ink_mask`] and [`stroke_width`].
//!
//! A mask bit is set where the sketch has ink; cleared bits are paper.
//! [`InkMode`] picks the definition of ink: dark-on-light strokes found by
//! [`luminance`](crate::gfx::ops::luminance) threshold (explicit or
//! `Otsu`-detected), or pixels close to one pen colour. [`stroke_width`]
//! estimates the pen width from the median of all short straight runs,
//! which the later morphology radii key off.

use crate::error::CyberError;
use crate::gfx::image::RgbaImage;
use crate::gfx::ops::parse_hex_color;
use crate::gfx::ops::threshold::{OPAQUE_ALPHA, binary_mask, luminance_histogram, otsu_level};
use crate::gfx::{BYTES_PER_PIXEL, pixel_count, pixel_index, to_pixel};

/// Longest straight run (pixels) [`stroke_width`] counts as a pen sample;
/// walls longer than this are extents, not thicknesses.
const MAX_STROKE_RUN: u32 = 64;

/// A one-bit-per-pixel raster, row-major; `true` means ink / set.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mask {
    /// Pixels per row, `1..=8192`.
    pub width: u32,
    /// Rows, `1..=8192`.
    pub height: u32,
    /// One bit per pixel, row-major.
    pub bits: Vec<bool>,
}

impl Mask {
    /// A `width` x `height` mask with every bit cleared; a shape that does
    /// not fit memory addressing yields an empty bit vector.
    #[must_use]
    #[inline]
    pub fn blank(width: u32, height: u32) -> Self {
        let count = pixel_count(width, height).unwrap_or_default();
        Self {
            width,
            height,
            bits: vec![false; count],
        }
    }

    /// The bit at `(x, y)`, or `None` when out of bounds.
    #[must_use]
    #[inline]
    pub fn get(&self, x: u32, y: u32) -> Option<bool> {
        let index = self.index_of(x, y)?;
        self.count_at(index)
    }

    /// Writes `value` at `(x, y)`; returns `false` when out of bounds.
    #[inline]
    pub fn set(&mut self, x: u32, y: u32, value: bool) -> bool {
        let Some(index) = self.index_of(x, y) else {
            return false;
        };
        let Some(slot) = self.bits.get_mut(index) else {
            return false;
        };
        *slot = value;
        true
    }

    /// The bit at row-major position `index`, `None` when out of bounds.
    #[must_use]
    #[inline]
    pub(crate) fn count_at(&self, index: usize) -> Option<bool> {
        self.bits.get(index).copied()
    }

    /// The row-major index of `(x, y)`, `None` when out of bounds.
    #[must_use]
    #[inline]
    pub(crate) fn index_of(&self, x: u32, y: u32) -> Option<usize> {
        if x >= self.width || y >= self.height {
            return None;
        }
        pixel_index(self.width, x, y)
    }

    /// The bits set in this mask and cleared in `other`, same shape.
    #[must_use]
    pub(crate) fn without(&self, other: &Self) -> Self {
        let bits = self
            .bits
            .iter()
            .zip(other.bits.iter())
            .map(|(&mine, &theirs)| mine && !theirs)
            .collect();
        Self {
            width: self.width,
            height: self.height,
            bits,
        }
    }
}

/// How [`ink_mask`] decides which pixels are ink.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InkMode {
    /// Dark strokes: `luminance` at or below `level`, or the
    /// [`otsu_level`] of the image when `None`. Fully transparent pixels
    /// are always paper.
    Dark {
        /// Fixed cut-off `0..=255`; `None` detects it.
        level: Option<u8>,
    },
    /// One pen colour: ink where the largest channel difference to `rgb`
    /// is at most `tolerance`. Fully transparent pixels are paper.
    Color {
        /// The pen colour as `RGB` bytes.
        rgb: [u8; 3],
        /// Per-channel match tolerance `0..=255`.
        tolerance: u8,
    },
}

impl InkMode {
    /// Parses a mode from a colour string as [`parse_hex_color`] accepts
    /// it, with a per-channel `tolerance`: an [`InkMode::Color`] over the
    /// `RGB` part.
    ///
    /// # Errors
    ///
    /// [`CyberError::InvalidArgument`] when `value` is not a valid hex
    /// colour.
    #[inline]
    pub fn from_hex(value: &str, tolerance: u8) -> Result<Self, CyberError> {
        let [red, green, blue, _] = parse_hex_color(value)?;
        Ok(Self::Color {
            rgb: [red, green, blue],
            tolerance,
        })
    }
}

/// The ink mask of `img` under `mode`, plus the threshold level applied
/// (`Some` only for [`InkMode::Dark`], including the detected `Otsu` one).
#[must_use]
#[inline]
pub fn ink_mask(img: &RgbaImage, mode: InkMode) -> (Mask, Option<u8>) {
    let (bits, level) = match mode {
        InkMode::Dark { level } => {
            let resolved = level.unwrap_or_else(|| otsu_level(&luminance_histogram(img)));
            (binary_mask(img, resolved, false), Some(resolved))
        }
        InkMode::Color { rgb, tolerance } => (
            img.pixels
                .chunks_exact(BYTES_PER_PIXEL)
                .map(|chunk| color_ink(to_pixel(chunk), rgb, tolerance))
                .collect(),
            None,
        ),
    };
    (
        Mask {
            width: img.width,
            height: img.height,
            bits,
        },
        level,
    )
}

/// Whether one pixel is `Color`-mode ink: opaque, and every channel within
/// `tolerance` of the pen colour.
fn color_ink(pixel: [u8; 4], rgb: [u8; 3], tolerance: u8) -> bool {
    let [red, green, blue, alpha] = pixel;
    let [want_red, want_green, want_blue] = rgb;
    let limit = u32::from(tolerance);
    alpha >= OPAQUE_ALPHA
        && channel_diff(red, want_red) <= limit
        && channel_diff(green, want_green) <= limit
        && channel_diff(blue, want_blue) <= limit
}

/// The absolute difference of two channel values.
fn channel_diff(one: u8, other: u8) -> u32 {
    let (big, small) = (u32::from(one), u32::from(other));
    if big >= small {
        big.saturating_sub(small)
    } else {
        small.saturating_sub(big)
    }
}

/// The median pen width of `mask` (pixels, minimum `1`): the lower median
/// of every horizontal and vertical ink run of at most 64 pixels. An
/// empty or inkless mask measures `1`.
#[must_use]
#[inline]
pub fn stroke_width(mask: &Mask) -> u32 {
    let Ok(width) = usize::try_from(mask.width) else {
        return 1;
    };
    let Ok(height) = usize::try_from(mask.height) else {
        return 1;
    };
    if width == 0 || height == 0 || mask.bits.len() != width.saturating_mul(height) {
        return 1;
    }
    let mut runs = Vec::new();
    for row in mask.bits.chunks_exact(width) {
        collect_runs(row, &mut runs);
    }
    for x in 0..width {
        let column: Vec<bool> = mask
            .bits
            .chunks_exact(width)
            .map(|row| row.get(x).copied().unwrap_or_default())
            .collect();
        collect_runs(&column, &mut runs);
    }
    lower_median(&runs)
}

/// Appends every set-run length of `line` that is at most
/// [`MAX_STROKE_RUN`] to `runs`; the line ends on virtual paper.
fn collect_runs(line: &[bool], runs: &mut Vec<u32>) {
    let mut length = 0_u32;
    let bits = line.iter().copied().chain(core::iter::once(false));
    for bit in bits {
        if bit {
            length = length.saturating_add(1);
            continue;
        }
        if length != 0 {
            push_run(runs, length);
            length = 0;
        }
    }
}

/// Pushes one run length when it is short enough to be a pen sample.
fn push_run(runs: &mut Vec<u32>, length: u32) {
    if length <= MAX_STROKE_RUN {
        runs.push(length);
    }
}

/// The lower median of `runs`, or `1` when there are none.
fn lower_median(runs: &[u32]) -> u32 {
    if runs.is_empty() {
        return 1;
    }
    let mut sorted = runs.to_vec();
    sorted.sort_unstable();
    let half = sorted.len().saturating_sub(1).checked_div(2).unwrap_or(0);
    sorted.into_iter().nth(half).unwrap_or(1)
}

#[cfg(test)]
mod tests {
    use super::{InkMode, MAX_STROKE_RUN, Mask, channel_diff, color_ink, ink_mask, stroke_width};
    use crate::gfx::image::RgbaImage;

    /// An inclusive rectangle `(x0, y0, x1, y1)`.
    type Rect = (u32, u32, u32, u32);

    /// A `width` x `height` image of one opaque colour.
    fn solid(width: u32, height: u32, pixel: [u8; 4]) -> RgbaImage {
        let mut pixels = Vec::new();
        for _ in 0..width.saturating_mul(height) {
            pixels.extend_from_slice(&pixel);
        }
        RgbaImage::new(width, height, pixels).expect("fixture")
    }

    /// Paints the inclusive rectangle of `pixel` onto `img`.
    fn paint(img: &mut RgbaImage, pixel: [u8; 4], (x0, y0, x1, y1): Rect) {
        let points = (y0..=y1).flat_map(|y| (x0..=x1).map(move |x| (x, y)));
        for (x, y) in points {
            assert!(img.set(x, y, pixel), "fixture pixel in bounds");
        }
    }

    /// A white image with the given inclusive rectangles painted.
    fn rectangles(width: u32, height: u32, pixel: [u8; 4], boxes: &[Rect]) -> RgbaImage {
        let mut img = solid(width, height, [255, 255, 255, 255]);
        for box_ in boxes {
            paint(&mut img, pixel, *box_);
        }
        img
    }

    /// A mask of the given shape with all columns in `range` set.
    fn columns(width: u32, height: u32, range: core::ops::Range<u32>) -> Mask {
        let mut mask = Mask::blank(width, height);
        let points = (0..height).flat_map(move |y| range.clone().map(move |x| (x, y)));
        for (x, y) in points {
            assert!(mask.set(x, y, true), "fixture in bounds");
        }
        mask
    }

    #[test]
    fn mask_get_set_and_index_helpers() {
        let mut mask = Mask::blank(3, 2);
        assert_eq!(mask.bits.len(), 6, "blank sized by shape");
        assert_eq!(mask.get(0, 0), Some(false), "blank starts paper");
        assert!(mask.set(2, 1, true), "in-bounds set succeeds");
        assert_eq!(mask.get(2, 1), Some(true), "set reads back");
        assert_eq!(mask.get(3, 0), None, "x out of bounds");
        assert_eq!(mask.get(0, 2), None, "y out of bounds");
        assert!(!mask.set(9, 9, true), "out-of-bounds set reports false");
        assert_eq!(mask.count_at(5), Some(true), "row-major index 5 is (2,1)");
        assert_eq!(mask.count_at(6), None, "index past the end");
    }

    #[test]
    fn mask_without_subtracts_rasters() {
        let mut a = Mask::blank(2, 2);
        assert!(a.set(0, 0, true), "in bounds");
        assert!(a.set(1, 1, true), "in bounds");
        let mut b = Mask::blank(2, 2);
        assert!(b.set(0, 0, true), "in bounds");
        let difference = a.without(&b);
        assert_eq!(
            difference.get(0, 0),
            Some(false),
            "difference removes the shared bit"
        );
        assert_eq!(
            difference.get(1, 1),
            Some(true),
            "difference keeps the lone bit"
        );
        assert_eq!(difference.width, 2, "shape carried over");
        assert_eq!(difference.height, 2, "shape carried over");
        let untouched = a.without(&Mask::blank(2, 2));
        assert_eq!(
            untouched.bits, a.bits,
            "subtracting nothing changes nothing"
        );
    }

    #[test]
    fn ink_mask_dark_respects_explicit_level_and_transparency() {
        let mut img = rectangles(2, 1, [0, 0, 0, 255], &[(0, 0, 0, 0)]);
        assert!(img.set(1, 0, [100, 100, 100, 255]), "grey in bounds");
        let (mask, level) = ink_mask(&img, InkMode::Dark { level: Some(128) });
        assert_eq!(level, Some(128), "the explicit level is reported");
        assert_eq!(mask.get(0, 0), Some(true), "black is ink at 128");
        assert_eq!(mask.get(1, 0), Some(true), "grey 100 is ink at 128");
        let paper = ink_mask(
            &solid(1, 1, [255, 255, 255, 255]),
            InkMode::Dark { level: Some(128) },
        );
        assert_eq!(paper.0.get(0, 0), Some(false), "white is paper");
        let mut ghost = solid(1, 1, [0, 0, 0, 255]);
        assert!(ghost.set(0, 0, [0, 0, 0, 10]), "fully transparent black");
        let (ghost_mask, _) = ink_mask(&ghost, InkMode::Dark { level: Some(128) });
        assert_eq!(
            ghost_mask.get(0, 0),
            Some(false),
            "transparent black is paper"
        );
    }

    #[test]
    fn ink_mask_dark_detects_the_otsu_level() {
        let mut img = solid(4, 1, [220, 220, 220, 255]);
        assert!(img.set(0, 0, [30, 30, 30, 255]), "dark pixel");
        assert!(img.set(1, 0, [30, 30, 30, 255]), "dark pixel");
        let (mask, level) = ink_mask(&img, InkMode::Dark { level: None });
        assert_eq!(level, Some(30), "Otsu splits at the lower spike");
        assert_eq!(mask.count_at(0), Some(true), "dark pixels are ink");
        assert_eq!(mask.count_at(2), Some(false), "light pixels are paper");
    }

    #[test]
    fn ink_mask_color_picks_only_the_pen_colour() {
        let img = rectangles(3, 1, [255, 0, 0, 255], &[(0, 0, 0, 0), (2, 0, 2, 0)]);
        let mode = InkMode::Color {
            rgb: [255, 0, 0],
            tolerance: 32,
        };
        let (mask, level) = ink_mask(&img, mode);
        assert_eq!(level, None, "colour mode has no threshold");
        assert_eq!(mask.get(0, 0), Some(true), "red is ink");
        assert_eq!(mask.get(1, 0), Some(false), "white paper is not");
        assert_eq!(mask.get(2, 0), Some(true), "the second red pixel is ink");
        assert!(
            !color_ink([0, 0, 0, 0], [0, 0, 0], 255),
            "transparent black is paper"
        );
        assert!(
            color_ink([230, 10, 10, 255], [255, 0, 0], 32),
            "within tolerance"
        );
        assert!(
            !color_ink([200, 10, 10, 255], [255, 0, 0], 32),
            "outside tolerance"
        );
        assert_eq!(channel_diff(3, 10), 7, "absolute channel difference");
    }

    #[test]
    fn ink_mode_from_hex_builds_a_colour_mode() {
        let mode = InkMode::from_hex("#ff0000", 8).expect("valid colour");
        assert_eq!(
            mode,
            InkMode::Color {
                rgb: [255, 0, 0],
                tolerance: 8
            },
            "hex parsed into the colour mode"
        );
        let error = InkMode::from_hex("nope", 8).expect_err("garbage rejected");
        assert_eq!(error.code(), "invalid_argument", "hex error code");
    }

    #[test]
    fn stroke_width_median_of_short_runs() {
        assert_eq!(stroke_width(&Mask::blank(5, 5)), 1, "inkless mask");
        assert_eq!(
            stroke_width(&columns(9, 9, 4..5)),
            1,
            "a 1-px column measures 1"
        );
        assert_eq!(
            stroke_width(&columns(9, 9, 3..6)),
            3,
            "a 3-px band measures 3"
        );
        let long = columns(MAX_STROKE_RUN + 10, 3, 0..MAX_STROKE_RUN + 10);
        assert_eq!(
            stroke_width(&long),
            3,
            "the 74-px horizontal run is ignored, vertical runs are 3"
        );
    }

    #[test]
    fn stroke_width_rejects_mismatched_and_zero_shapes() {
        let broken = Mask {
            width: 4,
            height: 2,
            bits: vec![true; 5],
        };
        assert_eq!(stroke_width(&broken), 1, "ragged bits are unmeasurable");
        let zero = Mask::blank(0, 3);
        assert_eq!(stroke_width(&zero), 1, "empty shape measures 1");
    }
}
