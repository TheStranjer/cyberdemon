//! Binarization for [`crate::gfx::ops::ImageOp::Threshold`], plus the pure
//! helpers the sketch-tracing tools reuse: [`luminance_histogram`],
//! [`otsu_level`] and [`binary_mask`].
//!
//! The cutoff is either an explicit level or Otsu's, which maximises the
//! between-class variance `w0 * w1 * (mu0 - mu1)^2` of the luminance
//! histogram; that quotient is evaluated in `u128` as
//! `(sum_total * w0 - sum0 * total)^2 / (w0 * total)` so no float appears.

use serde_json::{Value, json};

use crate::gfx::image::RgbaImage;
use crate::gfx::ops::luminance;
use crate::gfx::{BYTES_PER_PIXEL, to_pixel};

/// Alpha at or above which a pixel counts toward the histogram.
pub(crate) const OPAQUE_ALPHA: u8 = 128;
/// One luminance bin per channel value.
pub const BINS: usize = 256;

/// Parameters of [`crate::gfx::ops::ImageOp::Threshold`].
pub(crate) struct ThresholdSpec {
    /// Fixed cut-off; `None` picks the Otsu level.
    pub(crate) level: Option<u8>,
    /// Whether to swap the black and white classes.
    pub(crate) invert: bool,
}

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

/// Binarizes every pixel to opaque black (ink) or white (paper).
pub(crate) fn binarize(img: RgbaImage, spec: &ThresholdSpec) -> (RgbaImage, Value) {
    let detected = || (otsu_level(&luminance_histogram(&img)), true);
    let (level, otsu) = spec
        .level
        .map_or_else(detected, |explicit| (explicit, false));
    let mut out = img;
    for chunk in out.pixels.chunks_exact_mut(BYTES_PER_PIXEL) {
        let pixel = to_pixel(chunk);
        chunk.copy_from_slice(&binary_pixel(pixel, level, spec.invert));
    }
    (
        out,
        json!({"op": "threshold", "level": level, "otsu": otsu}),
    )
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
    use super::{BINS, ThresholdSpec, binarize, binary_mask, luminance_histogram, otsu_level};
    use crate::gfx::image::RgbaImage;

    /// A 1-row image of the given grey values, all opaque.
    fn greys(values: &[u8]) -> RgbaImage {
        let pixels: Vec<u8> = values.iter().flat_map(|&v| [v, v, v, 255_u8]).collect();
        let width = u32::try_from(values.len()).expect("fixture fits u32");
        RgbaImage::new(width, 1, pixels).expect("fixture is valid")
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
        let (out, step) = binarize(
            img,
            &ThresholdSpec {
                level: None,
                invert: false,
            },
        );
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
        let spec = ThresholdSpec {
            level: Some(128),
            invert: false,
        };
        let (out, step) = binarize(img.clone(), &spec);
        assert_eq!(step["level"], 128_i32, "explicit level");
        assert_eq!(step["otsu"], false, "an explicit level is not Otsu");
        assert_eq!(out.get(0, 0), Some([0, 0, 0, 255]), "at the level is black");
        assert_eq!(out.get(1, 0), Some([255, 255, 255, 255]), "above is white");
        let flipped = ThresholdSpec {
            level: Some(128),
            invert: true,
        };
        let (swapped, _step) = binarize(img, &flipped);
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
        let spec = ThresholdSpec {
            level: Some(128),
            invert: false,
        };
        let (out, _step) = binarize(img, &spec);
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
}
