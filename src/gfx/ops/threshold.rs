//! Binarization and hairline opening for the ink-and-paper image ops.
//!
//! `binarize` implements [`crate::gfx::ops::ImageOp::Threshold`] and
//! `thin_stroke` implements [`crate::gfx::ops::ImageOp::ThinStrokeFilter`];
//! [`luminance_histogram`], [`otsu_level`] and [`binary_mask`] are the pure
//! helpers the sketch-tracing tools reuse.
//!
//! The cutoff is either an explicit level or Otsu's, which maximises the
//! between-class variance `w0 * w1 * (mu0 - mu1)^2` of the luminance
//! histogram; that quotient is evaluated in `u128` as
//! `(sum_total * w0 - sum0 * total)^2 / (w0 * total)` so no float appears.
//!
//! `thin_stroke` opens the [`binary_mask`] ink with the square
//! `(2r + 1) x (2r + 1)` element ([`erode`] then [`dilate`] from
//! [`crate::sketch::morph`]). An opening keeps exactly the ink that can hold
//! the element, which is why it drops hairline ruling and keeps walls: a
//! stroke thinner than the element erodes to nothing and the later dilation
//! has no core left to grow back, while a stroke `2r + 1` pixels or more
//! thick erodes to its inside line and dilates to its full original extent.
//! [`erode`] counts pixels outside the image as ink, so ink touching the
//! frame only needs its in-image neighbours to survive: a band pressed
//! against the border keeps the `r` pixel rows the element cannot reach
//! beyond the image, where the same band floating free inside the paper
//! would be too thin and vanish.

use serde_json::{Value, json};

use crate::error::CyberError;
use crate::gfx::image::RgbaImage;
use crate::gfx::ops::{arg_error, luminance};
use crate::gfx::{BYTES_PER_PIXEL, to_pixel};
use crate::sketch::mask::Mask;
use crate::sketch::morph::{dilate, erode};

/// Alpha at or above which a pixel counts toward the histogram.
pub(crate) const OPAQUE_ALPHA: u8 = 128;
/// One luminance bin per channel value.
pub const BINS: usize = 256;
/// Smallest accepted [`thin_stroke`] opening radius.
const MIN_THIN_RADIUS: u32 = 1;
/// Largest accepted [`thin_stroke`] opening radius.
const MAX_THIN_RADIUS: u32 = 16;

/// Counts opaque pixels per luminance value.
#[must_use]
#[inline]
pub fn luminance_histogram(img: &RgbaImage) -> [u32; BINS] {
    let mut histogram = [0_u32; BINS];
    for chunk in img.pixels.chunks_exact(BYTES_PER_PIXEL) {
        let pixel @ [_, _, _, alpha] = to_pixel(chunk);
        if alpha < OPAQUE_ALPHA {
            continue;
        }
        if let Some(slot) = histogram.get_mut(usize::from(luminance(pixel))) {
            *slot = slot.saturating_add(1);
        }
    }
    histogram
}

/// The Otsu cut-off of a luminance histogram: the level maximising the
/// between-class variance, ties won by the lowest level.
///
/// Levels that leave one class empty are skipped; if no split is valid
/// (a uniform histogram) the fallback is `0`.
#[must_use]
#[inline]
pub fn otsu_level(histogram: &[u32; BINS]) -> u8 {
    let total: u128 = histogram.iter().copied().map(u128::from).sum();
    let sum_total: u128 = histogram
        .iter()
        .enumerate()
        .map(|(value, &count)| {
            u128::try_from(value)
                .unwrap_or(255_u128)
                .saturating_mul(u128::from(count))
        })
        .sum();
    let mut below = Class::default();
    let mut best: Option<(u128, u8)> = None;
    for (value, &count) in histogram.iter().enumerate().take(BINS.saturating_sub(1)) {
        below.add(value, count);
        let Some(score) = below.variance(total, sum_total) else {
            continue;
        };
        let level = u8::try_from(value).unwrap_or(255_u8);
        if best.is_none_or(|(top, _)| score > top) {
            best = Some((score, level));
        }
    }
    best.map_or(0_u8, |(_, level)| level)
}

/// Per-pixel ink flags (row-major): `true` where the pixel is drawn.
///
/// Ink is luminance at or below `level`, or above it when `invert`ed;
/// pixels with alpha below `128` are never ink.
#[must_use]
#[inline]
pub fn binary_mask(img: &RgbaImage, level: u8, invert: bool) -> Vec<bool> {
    img.pixels
        .chunks_exact(BYTES_PER_PIXEL)
        .map(|chunk| {
            let pixel @ [_, _, _, alpha] = to_pixel(chunk);
            alpha >= OPAQUE_ALPHA && is_ink(luminance(pixel), level, invert)
        })
        .collect()
}

/// Whether `value` falls in the dark class at `level` under `invert`.
const fn is_ink(value: u8, level: u8, invert: bool) -> bool {
    if invert {
        value > level
    } else {
        value <= level
    }
}

/// Binarizes every pixel to opaque black (ink) or white (paper) at the
/// `(level, invert)` classification: `level` `None` picks Otsu's.
pub(crate) fn binarize(img: RgbaImage, level: Option<u8>, invert: bool) -> (RgbaImage, Value) {
    let detected = || (otsu_level(&luminance_histogram(&img)), true);
    let (cut, otsu) = level.map_or_else(detected, |explicit| (explicit, false));
    let mut out = img;
    for chunk in out.pixels.chunks_exact_mut(BYTES_PER_PIXEL) {
        let pixel = to_pixel(chunk);
        chunk.copy_from_slice(&binary_pixel(pixel, cut, invert));
    }
    (out, json!({"op": "threshold", "level": cut, "otsu": otsu}))
}

/// The binarized `RGBA` of one pixel: opaque ink black, paper white.
fn binary_pixel(pixel: [u8; 4], level: u8, invert: bool) -> [u8; 4] {
    let shade = if is_ink(luminance(pixel), level, invert) {
        0_u8
    } else {
        255_u8
    };
    [shade, shade, shade, 255_u8]
}

/// Drops hairline strokes: classifies the ink of `img` exactly as
/// [`binarize`] would at the `(radius, level, invert)` opening, opens it
/// with the square `(2r + 1) x (2r + 1)` element and paints the surviving
/// ink back as opaque black on opaque white paper.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] when `radius` is outside `1..=16`.
pub(crate) fn thin_stroke(
    img: RgbaImage,
    index: usize,
    opening: (u32, Option<u8>, bool),
) -> Result<(RgbaImage, Value), CyberError> {
    let (radius, level, invert) = opening;
    check_thin_radius(index, radius)?;
    let detected = || (otsu_level(&luminance_histogram(&img)), true);
    let (cut, otsu) = level.map_or_else(detected, |explicit| (explicit, false));
    let ink = Mask {
        width: img.width,
        height: img.height,
        bits: binary_mask(&img, cut, invert),
    };
    let opened = dilate(&erode(&ink, radius), radius);
    let (removed, kept) = count_opening(&ink, &opened);
    let out = paint_ink(img, &opened);
    Ok((
        out,
        json!({
            "op": "thin_stroke_filter",
            "radius": radius,
            "level": cut,
            "otsu": otsu,
            "removed_ink_pixels": removed,
            "kept_ink_pixels": kept,
        }),
    ))
}

/// Rejects a [`thin_stroke`] radius outside `1..=16`, naming the op field.
fn check_thin_radius(index: usize, radius: u32) -> Result<(), CyberError> {
    if (MIN_THIN_RADIUS..=MAX_THIN_RADIUS).contains(&radius) {
        return Ok(());
    }
    Err(arg_error(
        index,
        "radius",
        &format!("be within {MIN_THIN_RADIUS}..={MAX_THIN_RADIUS}, got {radius}"),
    ))
}

/// Ink pixels [`thin_stroke`]'s opening dropped, and ink pixels it kept.
fn count_opening(before: &Mask, after: &Mask) -> (usize, usize) {
    let kept = after.bits.iter().copied().filter(|set| *set).count();
    let removed = before
        .bits
        .iter()
        .zip(after.bits.iter())
        .filter(|&(was, kept_now)| *was && !*kept_now)
        .count();
    (removed, kept)
}

/// Paints `opened` as opaque black ink on opaque white paper, every alpha
/// normalised to `255` the way [`binarize`] does.
fn paint_ink(img: RgbaImage, opened: &Mask) -> RgbaImage {
    let mut out = img;
    for (chunk, &set) in out
        .pixels
        .chunks_exact_mut(BYTES_PER_PIXEL)
        .zip(opened.bits.iter())
    {
        let shade = if set { 0_u8 } else { 255_u8 };
        chunk.copy_from_slice(&[shade, shade, shade, 255_u8]);
    }
    out
}

/// The luminance class of pixels at or below the current level.
#[derive(Default)]
struct Class {
    /// Pixels in the class.
    count: u128,
    /// Sum of their luminances.
    sum: u128,
}

impl Class {
    /// Moves every pixel in bin `value` into this class.
    fn add(&mut self, value: usize, count: u32) {
        self.count = self.count.saturating_add(u128::from(count));
        self.sum = self.sum.saturating_add(
            u128::try_from(value)
                .unwrap_or(255_u128)
                .saturating_mul(u128::from(count)),
        );
    }

    /// `w0 * w1 * (mu0 - mu1)^2` as `(sum_total * w0 - sum0 * total)^2 / (w0 * w1)`,
    /// `None` when either class is empty.
    fn variance(&self, total: u128, sum_total: u128) -> Option<u128> {
        let above = total.checked_sub(self.count)?;
        if self.count == 0 || above == 0 {
            return None;
        }
        let left = sum_total.saturating_mul(self.count);
        let right = self.sum.saturating_mul(total);
        let delta = if left >= right {
            left.saturating_sub(right)
        } else {
            right.saturating_sub(left)
        };
        delta
            .saturating_mul(delta)
            .checked_div(self.count.saturating_mul(above))
    }
}

#[cfg(test)]
mod tests {
    use super::{BINS, binarize, binary_mask, luminance_histogram, otsu_level, thin_stroke};
    use crate::gfx::image::RgbaImage;
    use serde_json::Value;

    /// A 1-row image of the given grey values, all opaque.
    fn greys(values: &[u8]) -> RgbaImage {
        let pixels: Vec<u8> = values.iter().flat_map(|&v| [v, v, v, 255_u8]).collect();
        let width = u32::try_from(values.len()).expect("fixture fits u32");
        RgbaImage::new(width, 1, pixels).expect("fixture is valid")
    }

    /// A `width` x `height` image of opaque white paper.
    fn paper(width: u32, height: u32) -> RgbaImage {
        let count = usize::try_from(width)
            .expect("test width")
            .checked_mul(usize::try_from(height).expect("test height"))
            .expect("test pixels fit");
        RgbaImage::new(width, height, vec![255_u8; count.saturating_mul(4)]).expect("valid fixture")
    }

    /// Paints the inclusive rectangle `(x0, y0)..=(x1, y1)` of grey `shade`.
    fn paint(img: &mut RgbaImage, shade: u8, (x0, y0, x1, y1): (u32, u32, u32, u32)) {
        let points = (y0..=y1).flat_map(|y| (x0..=x1).map(move |x| (x, y)));
        for (x, y) in points {
            assert!(
                img.set(x, y, [shade, shade, shade, 255_u8]),
                "fixture in bounds"
            );
        }
    }

    /// Runs [`thin_stroke`] at radius `1` with the given classification.
    fn opened(img: RgbaImage, level: Option<u8>, invert: bool) -> (RgbaImage, Value) {
        thin_stroke(img, 0, (1, level, invert)).expect("fixture radius is valid")
    }

    /// Whether the pixel at `(x, y)` is opaque black ink.
    fn is_black(out: &RgbaImage, x: u32, y: u32) -> bool {
        out.get(x, y) == Some([0_u8, 0, 0, 255])
    }

    /// Whether the pixel at `(x, y)` is opaque white paper.
    fn is_white(out: &RgbaImage, x: u32, y: u32) -> bool {
        out.get(x, y) == Some([255_u8, 255, 255, 255])
    }

    /// A histogram of `count` pixels at each of the given levels.
    fn spike_histogram(levels: &[(usize, u32)]) -> [u32; BINS] {
        let mut histogram = [0_u32; BINS];
        for &(level, count) in levels {
            add_bin(&mut histogram, level, count);
        }
        histogram
    }

    /// Sets one histogram bin when the level is in range.
    fn add_bin(histogram: &mut [u32; BINS], level: usize, count: u32) {
        if let Some(slot) = histogram.get_mut(level) {
            *slot = count;
        }
    }

    /// The expected channel value for grey `value` at cut-off `level`.
    fn expected_shade(value: u8, level: u32) -> u8 {
        if u32::from(value) <= level {
            0_u8
        } else {
            255_u8
        }
    }

    #[test]
    fn otsu_splits_a_bimodal_histogram() {
        let level = otsu_level(&spike_histogram(&[(40, 100), (200, 100)]));
        assert!(
            (40..200).contains(&level),
            "level {level} sits between the two spikes"
        );
        assert_eq!(level, 40, "ties are won by the lowest level");
    }

    #[test]
    fn otsu_on_a_uniform_histogram_is_valid_and_calm() {
        assert_eq!(
            otsu_level(&spike_histogram(&[(70, 33)])),
            0,
            "fallback level"
        );
        assert_eq!(otsu_level(&[0_u32; BINS]), 0, "empty histogram");
    }

    #[test]
    fn otsu_weighs_uneven_classes() {
        // A tiny bright outlier must not pull the level away from the
        // dominant dark/light pair.
        let level = otsu_level(&spike_histogram(&[(10, 900), (250, 100)]));
        assert!(
            (10..250).contains(&level),
            "level {level} separates the bulk"
        );
    }

    #[test]
    fn histogram_ignores_transparent_pixels() {
        let mut img = greys(&[200, 200]);
        assert!(img.set(1, 0, [200, 200, 200, 100]), "in-bounds overwrite");
        let histogram = luminance_histogram(&img);
        assert_eq!(
            histogram[200] + histogram[0],
            1,
            "only the opaque pixel counts"
        );
        assert_eq!(histogram[200], 1, "the counted one is the opaque bin");
    }

    #[test]
    fn binarize_maps_levels_to_black_and_white_at_the_otsu_default() {
        let img = greys(&[30, 60, 180, 220]);
        let (out, step) = binarize(img, None, false);
        assert_eq!(step["op"], "threshold", "op name");
        assert_eq!(step["otsu"], true, "the default level is Otsu's");
        let level = u32::try_from(step["level"].as_u64().expect("numeric level")).expect("u8");
        for (x, &value) in [30_u8, 60, 180, 220].iter().enumerate() {
            let position = u32::try_from(x).expect("small");
            let pixel = out.get(position, 0).expect("in bounds");
            let expected = expected_shade(value, level);
            assert_eq!(pixel, [expected, expected, expected, 255], "pixel {x}");
        }
    }

    #[test]
    fn binarize_with_an_explicit_level_and_invert() {
        let img = greys(&[100, 130]);
        let (out, step) = binarize(img.clone(), Some(128), false);
        assert_eq!(step["level"], 128_i32, "explicit level");
        assert_eq!(step["otsu"], false, "an explicit level is not Otsu");
        assert_eq!(out.get(0, 0), Some([0, 0, 0, 255]), "at the level is black");
        assert_eq!(out.get(1, 0), Some([255, 255, 255, 255]), "above is white");
        let (swapped, _step) = binarize(img, Some(128), true);
        assert_eq!(
            swapped.get(0, 0),
            Some([255, 255, 255, 255]),
            "invert swaps black out"
        );
        assert_eq!(
            swapped.get(1, 0),
            Some([0, 0, 0, 255]),
            "invert pulls white down"
        );
    }

    #[test]
    fn binarize_forces_transparent_pixels_opaque() {
        let mut img = greys(&[10, 250]);
        assert!(img.set(0, 0, [10, 10, 10, 0]), "in-bounds overwrite");
        let (out, _step) = binarize(img, Some(128), false);
        assert_eq!(
            out.get(0, 0),
            Some([0, 0, 0, 255]),
            "alpha is normalised to opaque"
        );
    }

    #[test]
    fn binary_mask_marks_ink_and_respects_invert_and_alpha() {
        let mut img = greys(&[30, 200, 30]);
        assert!(img.set(2, 0, [30, 30, 30, 10]), "in-bounds overwrite");
        let mask = binary_mask(&img, 128, false);
        assert_eq!(
            mask,
            vec![true, false, false],
            "dark opaque is ink; transparent is not"
        );
        let flipped = binary_mask(&img, 128, true);
        assert_eq!(
            flipped,
            vec![false, true, false],
            "inverted picks the bright class"
        );
    }

    #[test]
    fn thin_stroke_drops_a_hairline_row_and_column() {
        let mut img = paper(9, 9);
        paint(&mut img, 100, (1, 4, 7, 4));
        paint(&mut img, 100, (4, 1, 4, 7));
        let (out, step) = opened(img, Some(128), false);
        assert_eq!(step["op"], "thin_stroke_filter", "the op name");
        assert_eq!(step["radius"], 1_i32, "the radius echoes");
        assert_eq!(step["level"], 128_i32, "explicit level");
        assert_eq!(step["otsu"], false, "an explicit level is not Otsu");
        assert_eq!(step["removed_ink_pixels"], 13_i32, "the whole 1-px cross");
        assert_eq!(step["kept_ink_pixels"], 0_i32, "nothing survived");
        let hairline_gone = (0..9_u32)
            .flat_map(|y| (0..9_u32).map(move |x| (x, y)))
            .all(|(x, y)| is_white(&out, x, y));
        assert!(hairline_gone, "every pixel is paper again");
    }

    #[test]
    fn thin_stroke_keeps_a_three_pixel_band_and_reports_the_otsu_default() {
        let mut img = paper(9, 9);
        paint(&mut img, 100, (3, 2, 5, 6));
        let (out, step) = opened(img, None, false);
        assert_eq!(step["otsu"], true, "level None is Otsu's");
        assert_eq!(step["level"], 100_i32, "Otsu splits at the grey spike");
        assert_eq!(step["removed_ink_pixels"], 0_i32, "the band keeps all ink");
        assert_eq!(step["kept_ink_pixels"], 15_i32, "3 x 5 pixels");
        let band_solid = (2..7_u32)
            .flat_map(|y| (3..6_u32).map(move |x| (x, y)))
            .all(|(x, y)| is_black(&out, x, y));
        assert!(band_solid, "the whole band stays ink");
        for (x, y) in [(2_u32, 4_u32), (6, 4), (3, 1), (5, 7), (0, 0), (8, 8)] {
            assert!(is_white(&out, x, y), "({x},{y}) stays paper");
        }
    }

    #[test]
    fn thin_stroke_keeps_frame_touching_ink_and_drops_the_free_band() {
        let mut img = paper(9, 9);
        paint(&mut img, 100, (1, 0, 7, 1));
        paint(&mut img, 100, (1, 4, 7, 5));
        let (out, step) = opened(img, Some(128), false);
        assert_eq!(step["removed_ink_pixels"], 14_i32, "the 2-px free band");
        assert_eq!(step["kept_ink_pixels"], 14_i32, "the frame band survives");
        for (x, y) in [(1_u32, 0_u32), (4, 0), (7, 0), (1, 1), (7, 1)] {
            assert!(is_black(&out, x, y), "({x},{y}) frame band keeps ink");
        }
        for (x, y) in [(1_u32, 4_u32), (4, 4), (7, 5)] {
            assert!(is_white(&out, x, y), "({x},{y}) free band is gone");
        }
    }

    #[test]
    fn thin_stroke_invert_flips_the_ink_class() {
        let mut img = paper(9, 9);
        paint(&mut img, 0, (0, 0, 8, 8));
        paint(&mut img, 255, (3, 0, 5, 8));
        let (as_band, step) = opened(img.clone(), Some(128), true);
        assert_eq!(step["level"], 128_i32, "explicit level carries through");
        assert_eq!(step["kept_ink_pixels"], 27_i32, "the bright band is ink");
        assert_eq!(
            step["removed_ink_pixels"], 0_i32,
            "a full-height band holds"
        );
        assert!(is_black(&as_band, 4, 4), "the ink class paints black");
        assert!(is_white(&as_band, 0, 4), "the paper class paints white");
        let (as_field, _step) = opened(img, Some(128), false);
        assert!(is_white(&as_field, 4, 4), "the bright band is paper now");
        assert!(is_black(&as_field, 0, 0), "the dark field is the ink");
    }

    #[test]
    fn thin_stroke_rejects_radii_outside_the_documented_range() {
        for radius in [0_u32, 17_u32] {
            let error = thin_stroke(paper(3, 3), 3, (radius, Some(128), false))
                .expect_err("out of range radius");
            assert_eq!(error.code(), "invalid_argument", "radius {radius} refuses");
            let message = error.to_string();
            assert!(
                message.contains("ops[3].radius") && message.contains("1..=16"),
                "the error names the field and range: {message}"
            );
        }
    }
}
