//! Tonal image operations.
//!
//! [`crate::gfx::ops::ImageOp::AutoLevels`] and [`crate::gfx::ops::ImageOp::Adjust`],
//! both integer lookups and per-channel arithmetic on `0..=255` values.

use serde_json::{Value, json};

use crate::error::CyberError;
use crate::gfx::image::RgbaImage;
use crate::gfx::ops::threshold::{BINS, luminance_histogram};
use crate::gfx::ops::{arg_error, check_i32_range, clamp_u8, luminance};
use crate::gfx::{BYTES_PER_PIXEL, to_pixel};

/// Largest accepted `clip_percent`.
const MAX_CLIP_PERCENT: u8 = 10;

/// Parameters of [`crate::gfx::ops::ImageOp::Adjust`].
pub(crate) struct AdjustSpec {
    /// Value added after contrast.
    pub(crate) brightness: i32,
    /// Contrast percentage about mid grey.
    pub(crate) contrast: i32,
    /// Saturation percentage about the pixel's own luminance.
    pub(crate) saturation: i32,
}

/// Stretches the `luminance` range of opaque pixels to full scale through a
/// shared per-channel lookup table.
pub(crate) fn auto_levels(
    img: RgbaImage,
    index: usize,
    clip_percent: u8,
) -> Result<(RgbaImage, Value), CyberError> {
    if clip_percent > MAX_CLIP_PERCENT {
        return Err(arg_error(
            index,
            "clip_percent",
            &format!("be within 0..={MAX_CLIP_PERCENT}, got {clip_percent}"),
        ));
    }
    let histogram = luminance_histogram(&img);
    let total: u32 = histogram.iter().sum();
    let Some((low, high)) =
        (total > 0).then(|| clip_bounds(&histogram, total, u32::from(clip_percent)))
    else {
        return Ok(report_levels(img, false, 0, 0));
    };
    if high <= low {
        return Ok(report_levels(img, false, low, high));
    }
    let lut = build_lut(low, high);
    Ok(report_levels(apply_lut(img, &lut), true, low, high))
}

/// The clipped `luminance` bounds: `low` is the smallest value with at
/// least `clip_percent` of the pixels at or below it, `high` the largest
/// value with at least that share at or above it.
fn clip_bounds(histogram: &[u32; BINS], total: u32, clip_percent: u32) -> (u32, u32) {
    let threshold = total
        .saturating_mul(clip_percent)
        .checked_div(100_u32)
        .unwrap_or(0_u32)
        .max(1_u32);
    (
        lowest_reaching(histogram, threshold),
        highest_reaching(histogram, threshold),
    )
}

/// Smallest present bin whose cumulative count reaches `threshold`.
fn lowest_reaching(histogram: &[u32; BINS], threshold: u32) -> u32 {
    let mut seen = 0_u32;
    for (value, count) in histogram.iter().enumerate() {
        seen = seen.saturating_add(*count);
        if *count > 0 && seen >= threshold {
            return u32::try_from(value).unwrap_or(255_u32);
        }
    }
    0_u32
}

/// Largest present bin whose cumulative count from the top reaches
/// `threshold`.
fn highest_reaching(histogram: &[u32; BINS], threshold: u32) -> u32 {
    let mut seen = 0_u32;
    for (value, count) in histogram.iter().enumerate().rev() {
        seen = seen.saturating_add(*count);
        if *count > 0 && seen >= threshold {
            return u32::try_from(value).unwrap_or(0_u32);
        }
    }
    255_u32
}

/// `v -> clamp((v - low) * 255 / (high - low))` for every channel value.
fn build_lut(low: u32, high: u32) -> [u8; BINS] {
    let range = high.saturating_sub(low).max(1_u32);
    let mut lut = [0_u8; BINS];
    for (value, slot) in lut.iter_mut().enumerate() {
        let channel = u32::try_from(value).unwrap_or(255_u32);
        let mapped = if channel <= low {
            0_u32
        } else if channel >= high {
            255_u32
        } else {
            channel
                .saturating_sub(low)
                .saturating_mul(255_u32)
                .checked_div(range)
                .unwrap_or(255_u32)
        };
        *slot = u8::try_from(mapped).unwrap_or(255_u8);
    }
    lut
}

/// Maps R, G and B of every pixel through the table; alpha is kept.
fn apply_lut(mut img: RgbaImage, lut: &[u8; BINS]) -> RgbaImage {
    for chunk in img.pixels.chunks_exact_mut(BYTES_PER_PIXEL) {
        let [red, green, blue, alpha] = to_pixel(chunk);
        chunk.copy_from_slice(&[
            *lut.get(usize::from(red)).unwrap_or(&red),
            *lut.get(usize::from(green)).unwrap_or(&green),
            *lut.get(usize::from(blue)).unwrap_or(&blue),
            alpha,
        ]);
    }
    img
}

/// Pairs the (possibly unchanged) image with its report entry.
fn report_levels(img: RgbaImage, changed: bool, low: u32, high: u32) -> (RgbaImage, Value) {
    (
        img,
        json!({"op": "auto_levels", "low": low, "high": high, "changed": changed}),
    )
}

/// Applies brightness, contrast and saturation to the colour channels.
pub(crate) fn adjust(
    img: RgbaImage,
    index: usize,
    spec: &AdjustSpec,
) -> Result<(RgbaImage, Value), CyberError> {
    check_i32_range(index, "brightness", spec.brightness, (-255_i32, 255_i32))?;
    check_i32_range(index, "contrast", spec.contrast, (0_i32, 400_i32))?;
    check_i32_range(index, "saturation", spec.saturation, (0_i32, 400_i32))?;
    let mut out = img;
    for chunk in out.pixels.chunks_exact_mut(BYTES_PER_PIXEL) {
        let [red, green, blue, alpha] = to_pixel(chunk);
        let shifted = [
            contrast_channel(red, spec.contrast, spec.brightness),
            contrast_channel(green, spec.contrast, spec.brightness),
            contrast_channel(blue, spec.contrast, spec.brightness),
            alpha,
        ];
        let [red2, green2, blue2, alpha2] = shifted;
        let luma = luminance(shifted);
        chunk.copy_from_slice(&[
            saturate_channel(red2, luma, spec.saturation),
            saturate_channel(green2, luma, spec.saturation),
            saturate_channel(blue2, luma, spec.saturation),
            alpha2,
        ]);
    }
    let (width, height) = (out.width, out.height);
    Ok((
        out,
        json!({
            "op": "adjust",
            "brightness": spec.brightness,
            "contrast": spec.contrast,
            "saturation": spec.saturation,
            "width": width,
            "height": height,
        }),
    ))
}

/// One channel after `clamp((v - 128) * contrast / 100 + 128 + brightness)`.
fn contrast_channel(value: u8, contrast: i32, brightness: i32) -> u8 {
    let centred = i32::from(value).saturating_sub(128);
    let stretched = centred
        .saturating_mul(contrast)
        .checked_div(100_i32)
        .unwrap_or(0_i32);
    clamp_u8(stretched.saturating_add(128).saturating_add(brightness))
}

/// One channel after `clamp(Y + (c - Y) * saturation / 100)`.
fn saturate_channel(value: u8, luma: u8, saturation: i32) -> u8 {
    let delta = i32::from(value).saturating_sub(i32::from(luma));
    let stretched = delta
        .saturating_mul(saturation)
        .checked_div(100_i32)
        .unwrap_or(0_i32);
    clamp_u8(stretched.saturating_add(i32::from(luma)))
}

#[cfg(test)]
mod tests {
    use super::{AdjustSpec, auto_levels, contrast_channel, saturate_channel};
    use crate::error::CyberError;
    use crate::gfx::image::RgbaImage;
    use crate::gfx::to_pixel;

    /// A 1-row grey ramp from `from` to `to` inclusive, fully opaque.
    fn grey_ramp(from: u8, to: u8) -> RgbaImage {
        let pixels: Vec<u8> = (from..=to)
            .flat_map(|value| [value, value, value, 255])
            .collect();
        let width = (from..=to).count();
        RgbaImage::new(u32::try_from(width).expect("small"), 1, pixels).expect("ramp is valid")
    }

    /// The error message of a failed auto-levels run.
    fn failure(result: Result<(RgbaImage, serde_json::Value), CyberError>) -> String {
        let CyberError::InvalidArgument(text) = result.expect_err("parameter must be rejected")
        else {
            panic!("expected InvalidArgument");
        };
        text
    }

    #[test]
    fn auto_levels_stretches_a_ramp_and_reports_the_bounds() {
        let (out, step) = auto_levels(grey_ramp(64, 192), 0, 1).expect("ramp auto-levels");
        assert_eq!(step["low"], 64_i32, "clip floor at the ramp start");
        assert_eq!(step["high"], 192_i32, "clip ceiling at the ramp end");
        assert_eq!(step["changed"], true, "the image was stretched");
        let first = out.get(0, 0).expect("ramp starts");
        assert_eq!(first[0], 0, "first value maps to black");
        let last = out.get(out.width.saturating_sub(1), 0).expect("ramp ends");
        assert_eq!(last[0], 255, "last value maps to white");
        let grays: Vec<u8> = (0..out.width)
            .filter_map(|x| out.get(x, 0))
            .map(|pixel| pixel[0])
            .collect();
        assert!(
            grays
                .windows(2)
                .all(|pair| matches!(pair, [low, high] if low <= high)),
            "the stretched ramp stays monotonic"
        );
        assert!(
            out.pixels.chunks_exact(4).all(|chunk| {
                let [red, green, blue, _] = to_pixel(chunk);
                red == green && green == blue
            }),
            "grey pixels stay neutral"
        );
    }

    #[test]
    fn auto_levels_leaves_a_uniform_image_unchanged() {
        let flat = grey_ramp(100, 100);
        let (out, step) = auto_levels(flat.clone(), 0, 1).expect("uniform auto-levels");
        assert_eq!(out, flat, "uniform pixels are untouched");
        assert_eq!(step["changed"], false, "unchanged is reported");
        assert_eq!(step["low"], 100_i32, "bounds reported");
        assert_eq!(step["high"], 100_i32, "bounds reported");
    }

    #[test]
    fn auto_levels_ignores_transparent_pixels_in_the_histogram() {
        let mut pixels: Vec<u8> = (64_u8..=192).flat_map(|v| [v, v, v, 255]).collect();
        pixels.extend_from_slice(&[0, 0, 0, 0]);
        let img = RgbaImage::new(130, 1, pixels).expect("130x1 is valid");
        let (_out, step) = auto_levels(img, 0, 1).expect("auto-levels runs");
        assert_eq!(
            step["low"], 64_i32,
            "the transparent black pixel is not counted"
        );
        assert_eq!(step["high"], 192_i32, "the opaque extremes bound the range");
    }

    #[test]
    fn auto_levels_rejects_a_clip_above_ten_percent() {
        let text = failure(auto_levels(grey_ramp(0, 255), 1, 11));
        assert!(
            text.contains("ops[1].clip_percent") && text.contains("0..=10"),
            "clip range error: {text}"
        );
    }

    #[test]
    fn adjust_applies_the_documented_formula() {
        let img = RgbaImage::new(1, 1, vec![100, 150, 200, 255]).expect("1x1 is valid");
        let spec = AdjustSpec {
            brightness: 50,
            contrast: 200,
            saturation: 50,
        };
        let (out, step) = super::adjust(img, 0, &spec).expect("parameters valid");
        assert_eq!(
            out.get(0, 0),
            Some([159, 208, 225, 255]),
            "contrast, then luminance-based saturation, alpha kept"
        );
        assert_eq!(step["op"], "adjust", "op name");
    }

    #[test]
    fn adjust_defaults_are_the_identity() {
        let img = RgbaImage::new(1, 1, vec![10, 128, 250, 77]).expect("1x1 is valid");
        let spec = AdjustSpec {
            brightness: 0,
            contrast: 100,
            saturation: 100,
        };
        let (out, _step) = super::adjust(img.clone(), 2, &spec).expect("defaults are valid");
        assert_eq!(out, img, "0 / 100 / 100 leaves every channel alone");
    }

    #[test]
    fn adjust_clamps_at_black_and_white() {
        let up = AdjustSpec {
            brightness: 255,
            contrast: 400,
            saturation: 400,
        };
        assert_eq!(
            contrast_channel(255, up.contrast, up.brightness),
            255,
            "white stays white"
        );
        assert_eq!(
            contrast_channel(200, up.contrast, up.brightness),
            255,
            "over-bright clamps"
        );
        let down = AdjustSpec {
            brightness: -255,
            contrast: 400,
            saturation: 0,
        };
        assert_eq!(
            contrast_channel(0, down.contrast, down.brightness),
            0,
            "black stays black"
        );
        assert_eq!(
            saturate_channel(255, 0, down.saturation),
            0,
            "zero saturation falls to the luminance"
        );
    }

    #[test]
    fn adjust_rejects_out_of_range_parameters() {
        let img = RgbaImage::new(1, 1, vec![0; 4]).expect("1x1 is valid");
        let spec = AdjustSpec {
            brightness: 0,
            contrast: 401,
            saturation: 100,
        };
        let text = failure(super::adjust(img, 0, &spec));
        assert!(
            text.contains("ops[0].contrast") && text.contains("0..=400"),
            "contrast range error: {text}"
        );
    }
}
