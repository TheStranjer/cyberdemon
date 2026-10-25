//! Background-flattening binarization for
//! [`crate::gfx::ops::ImageOp::AdaptiveThreshold`].
//!
//! The uneven-light variant of [`crate::gfx::ops::ImageOp::Threshold`]:
//! every pixel is measured against its own local background instead of one
//! global cut. The background is the mean [`luminance`] of the
//! `(2r + 1) x (2r + 1)` window around the pixel (`r` = `radius`, clipped
//! at the image borders), so a dark line drawn on paper lit from one side
//! stays ink everywhere: a pixel is ink when it sits `offset` or more below
//! its local mean (and, when `invert`ed, `offset` or more above it — the
//! mirrored test).
//!
//! The mean runs on separable integer sums exactly like
//! [`crate::sketch::morph`]'s `window_counts`: a row pass then a column
//! pass, each carrying one `i64` window sum forward (absorbing the value
//! entering, releasing the one leaving), so the whole background costs
//! `O(width * height)` regardless of the radius. The report's
//! `background_min` / `background_max` are the local-mean extremes over the
//! picture, the measurable proof of how far the gradient was flattened.
//!
//! Transparent pixels (alpha below the same `128` cut the other binarizers
//! use) count as paper: they enter the background as white and are emitted
//! opaque white. Every output pixel ends opaque black or white, matching
//! `threshold::binarize`.

use serde_json::{Value, json};

use crate::error::CyberError;
use crate::gfx::image::RgbaImage;
use crate::gfx::ops::threshold::OPAQUE_ALPHA;
use crate::gfx::ops::{arg_error, luminance};
use crate::gfx::{BYTES_PER_PIXEL, to_pixel};

/// Smallest accepted background window radius.
const MIN_RADIUS: u32 = 1;
/// Largest accepted background window radius.
const MAX_RADIUS: u32 = 128;
/// Largest accepted ink offset.
const MAX_OFFSET: u32 = 255;
/// The luminance paper carries: the value transparent pixels contribute to
/// the background and the shade the paper class is painted in.
const PAPER: u8 = 255;

/// Parameters of [`crate::gfx::ops::ImageOp::AdaptiveThreshold`].
pub(crate) struct AdaptiveThresholdSpec {
    /// Background window radius, `1..=128`.
    pub(crate) radius: u32,
    /// How far below the local mean counts as ink, `0..=255`.
    pub(crate) offset: u8,
    /// Whether to swap the ink and paper classes.
    pub(crate) invert: bool,
}

/// Flattens the background and binarizes: every pixel is compared to the
/// mean [`luminance`] of its `(2r + 1) x (2r + 1)` window and painted
/// opaque black (ink) or opaque white (paper).
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] when `spec.radius` is outside `1..=128`
/// or `spec.offset` is outside `0..=255`.
pub(crate) fn adaptive_threshold(
    img: RgbaImage,
    index: usize,
    spec: &AdaptiveThresholdSpec,
) -> Result<(RgbaImage, Value), CyberError> {
    check_radius(index, spec.radius)?;
    check_offset(index, u32::from(spec.offset))?;
    let means = background_means(&img, spec.radius);
    let (out, ink_pixels) = paint(img, &means, spec);
    let (background_min, background_max) = extremes(&means);
    Ok((
        out,
        json!({
            "op": "adaptive_threshold",
            "radius": spec.radius,
            "offset": spec.offset,
            "background_min": background_min,
            "background_max": background_max,
            "ink_pixels": ink_pixels,
            "otsu": false,
        }),
    ))
}

/// Rejects a background window radius outside `1..=128`, naming the op field.
fn check_radius(index: usize, radius: u32) -> Result<(), CyberError> {
    if (MIN_RADIUS..=MAX_RADIUS).contains(&radius) {
        return Ok(());
    }
    Err(arg_error(
        index,
        "radius",
        &format!("be within {MIN_RADIUS}..={MAX_RADIUS}, got {radius}"),
    ))
}

/// Rejects an offset above the documented `255`, naming the op field. The
/// op's wire type is `u8`, so the wire can never carry a worse value than
/// [`check_offset`] accepts; the guard is the documented bound, checked on
/// the widened value.
fn check_offset(index: usize, offset: u32) -> Result<(), CyberError> {
    if offset <= MAX_OFFSET {
        return Ok(());
    }
    Err(arg_error(
        index,
        "offset",
        &format!("be within 0..={MAX_OFFSET}, got {offset}"),
    ))
}

/// The luminance a pixel contributes to the background: transparent pixels
/// count as paper white, opaque ones their own [`luminance`].
fn background_value(pixel: [u8; 4]) -> u8 {
    let [_, _, _, alpha] = pixel;
    if alpha < OPAQUE_ALPHA {
        PAPER
    } else {
        luminance(pixel)
    }
}

/// Whether one source pixel is ink against its local mean `mean`: opaque
/// pixels `offset` or more below it, or `offset` or more above it when
/// `invert`ed; transparent pixels are always paper.
fn counts_as_ink(pixel: [u8; 4], mean: i64, spec: &AdaptiveThresholdSpec) -> bool {
    let [_, _, _, alpha] = pixel;
    if alpha < OPAQUE_ALPHA {
        return false;
    }
    let value = i64::from(luminance(pixel));
    let offset = i64::from(spec.offset);
    if spec.invert {
        value.saturating_sub(offset) >= mean
    } else {
        value.saturating_add(offset) <= mean
    }
}

/// The local background of every pixel, row-major: the mean luminance of
/// the `(2r + 1) x (2r + 1)` window around it. The box sum comes from the
/// separable [`window_sums`] passes (every row, then the row sums down
/// every column) and divides by the clipped window's pixel count. An image
/// without pixels answers no means.
fn background_means(img: &RgbaImage, radius: u32) -> Vec<i64> {
    let width = match usize::try_from(img.width) {
        Ok(width) if width > 0 && !img.pixels.is_empty() => width,
        _ => return Vec::new(),
    };
    let reach = usize::try_from(radius).unwrap_or(usize::MAX);
    let values: Vec<i64> = img
        .pixels
        .chunks_exact(BYTES_PER_PIXEL)
        .map(|chunk| i64::from(background_value(to_pixel(chunk))))
        .collect();
    let rows = pass_rows(&values, width, reach);
    let boxes = pass_columns(&rows, width, reach);
    means_of(&boxes, width, reach)
}

/// The row pass: every row's running window sums.
fn pass_rows(values: &[i64], width: usize, reach: usize) -> Vec<i64> {
    let mut rows = Vec::with_capacity(values.len());
    for row in values.chunks_exact(width) {
        rows.extend(window_sums(row, reach));
    }
    rows
}

/// The column pass: the row sums taken down every column window.
fn pass_columns(rows: &[i64], width: usize, reach: usize) -> Vec<i64> {
    let height = rows.len().checked_div(width).unwrap_or(0_usize);
    let mut boxes = vec![0_i64; rows.len()];
    for x in 0..width {
        let column = column_of(rows, width, height, x);
        let sums = window_sums(&column, reach);
        write_column(&mut boxes, width, x, &sums);
    }
    boxes
}

/// Turns box sums into means over the clipped window areas.
fn means_of(boxes: &[i64], width: usize, reach: usize) -> Vec<i64> {
    let height = boxes.len().checked_div(width).unwrap_or(0_usize);
    let mut means = Vec::with_capacity(boxes.len());
    for (y, row) in boxes.chunks_exact(width).enumerate() {
        let hspan = i64::try_from(window_span(y, reach, height)).unwrap_or(1_i64);
        for (x, &total) in row.iter().enumerate() {
            let vspan = i64::try_from(window_span(x, reach, width)).unwrap_or(1_i64);
            means.push(
                total
                    .checked_div(hspan.saturating_mul(vspan))
                    .unwrap_or(0_i64),
            );
        }
    }
    means
}

/// The values of column `x`, top to bottom.
fn column_of(rows: &[i64], width: usize, height: usize, x: usize) -> Vec<i64> {
    (0..height)
        .map(|y| {
            rows.get(y.saturating_mul(width).saturating_add(x))
                .copied()
                .unwrap_or(0_i64)
        })
        .collect()
}

/// Writes one column of values back into the row-major grid, ignoring
/// out-of-bounds rows.
fn write_column(boxes: &mut [i64], width: usize, x: usize, column: &[i64]) {
    for (y, &value) in column.iter().enumerate() {
        if let Some(slot) = boxes.get_mut(y.saturating_mul(width).saturating_add(x)) {
            *slot = value;
        }
    }
}

/// For every position of `line`, the sum of the window `[i - r, i + r]`
/// clipped to the line, carried by a running `i64` sum: the shape
/// [`crate::sketch::morph`]'s `window_counts` keeps, with values in place
/// of set bits.
fn window_sums(line: &[i64], radius: usize) -> Vec<i64> {
    let len = line.len();
    let mut sums = Vec::with_capacity(len);
    if len == 0 {
        return sums;
    }
    let reach = radius.min(len.saturating_sub(1));
    let mut running: i64 = line.iter().take(reach.saturating_add(1)).sum();
    for i in 0..len {
        sums.push(running);
        if let Some(next) = line.get(i.saturating_add(reach).saturating_add(1)) {
            running = running.saturating_add(*next);
        }
        let leaving = if i >= reach {
            line.get(i.saturating_sub(reach)).copied()
        } else {
            None
        };
        if let Some(old) = leaving {
            running = running.saturating_sub(old);
        }
    }
    sums
}

/// The number of in-line window positions at `i`, i.e. the count that
/// divides the running sum there.
fn window_span(i: usize, radius: usize, len: usize) -> usize {
    let left = i.saturating_sub(radius);
    let right = i.saturating_add(radius).min(len.saturating_sub(1));
    right.saturating_sub(left).saturating_add(1)
}

/// Paints the classification over `img`: opaque black where `means` makes
/// the pixel ink, opaque white everywhere else; answers the ink count.
fn paint(img: RgbaImage, means: &[i64], spec: &AdaptiveThresholdSpec) -> (RgbaImage, usize) {
    let mut out = img;
    let mut ink_pixels = 0_usize;
    for (chunk, mean) in out
        .pixels
        .chunks_exact_mut(BYTES_PER_PIXEL)
        .zip(means.iter().copied())
    {
        let ink = counts_as_ink(to_pixel(chunk), mean, spec);
        if ink {
            ink_pixels = ink_pixels.saturating_add(1);
        }
        let shade = if ink { 0_u8 } else { PAPER };
        chunk.copy_from_slice(&[shade, shade, shade, 255_u8]);
    }
    (out, ink_pixels)
}

/// The smallest and largest value of `means`, `(0, 0)` for no means at all.
fn extremes(means: &[i64]) -> (i64, i64) {
    let min = means.iter().copied().min().unwrap_or(0_i64);
    let max = means.iter().copied().max().unwrap_or(0_i64);
    (min, max)
}

#[cfg(test)]
mod tests {
    use super::{
        AdaptiveThresholdSpec, adaptive_threshold, check_offset, window_span, window_sums,
    };
    use crate::error::CyberError;
    use crate::gfx::image::RgbaImage;
    use crate::gfx::ops::threshold::{luminance_histogram, otsu_level};
    use serde_json::Value;

    /// Builds the spec of an adaptive run at `radius` and `offset`.
    fn spec(radius: u32, offset: u8, invert: bool) -> AdaptiveThresholdSpec {
        AdaptiveThresholdSpec {
            radius,
            offset,
            invert,
        }
    }

    /// Runs [`adaptive_threshold`] at index `0` with fixture parameters.
    fn adaptive(img: RgbaImage, radius: u32, offset: u8, invert: bool) -> (RgbaImage, Value) {
        adaptive_threshold(img, 0, &spec(radius, offset, invert)).expect("fixture parameters")
    }

    /// The fixture's grey level at row `y`: the band's luminance `30`
    /// across rows `8..=10`, the `5`-per-row white-to-grey ramp elsewhere.
    fn ramp_row_level(y: u32) -> u8 {
        if (8..=10_u32).contains(&y) {
            return 30_u8;
        }
        let stepped = y.checked_mul(5_u32).expect("fixture fits");
        u8::try_from(255_u32.saturating_sub(stepped)).expect("ramp fits u8")
    }

    /// A `40` x `40` opaque picture: a vertical white-to-grey luminance
    /// ramp (`255` at the top, `60` at the bottom, `5` per row) carrying a
    /// constant dark band of luminance `30` across rows `8..=10`.
    fn ramp_with_band() -> RgbaImage {
        let pixels: Vec<u8> = (0..40_u32)
            .flat_map(|y| (0..40_u32).map(move |_| ramp_row_level(y)))
            .flat_map(|level| [level, level, level, 255_u8])
            .collect();
        RgbaImage::new(40_u32, 40_u32, pixels).expect("a 40x40 RGBA8 is valid")
    }

    /// Whether the pixel at `(x, y)` is opaque black ink.
    fn is_black(out: &RgbaImage, x: u32, y: u32) -> bool {
        out.get(x, y) == Some([0_u8, 0, 0, 255])
    }

    /// Whether the pixel at `(x, y)` is opaque white paper.
    fn is_white(out: &RgbaImage, x: u32, y: u32) -> bool {
        out.get(x, y) == Some([255_u8, 255, 255, 255])
    }

    #[test]
    fn a_dark_band_stays_ink_across_a_white_to_grey_ramp() {
        let (out, step) = adaptive(ramp_with_band(), 2, 15, false);
        assert_eq!(step["op"], "adaptive_threshold", "the op name");
        assert_eq!(step["radius"], 2_i32, "the radius echoes");
        assert_eq!(step["offset"], 15_i32, "the offset echoes");
        assert_eq!(step["otsu"], false, "the test is local, never Otsu");
        assert_eq!(step["ink_pixels"], 120_i32, "exactly the 3 x 40 band");
        assert_eq!(
            step["background_min"], 65_i32,
            "the flattened bottom edge: mean(70, 65, 60)"
        );
        assert_eq!(
            step["background_max"], 250_i32,
            "the flattened top edge: mean(255, 250, 245)"
        );
        for y in 8..=10_u32 {
            assert!(
                (0..40_u32).all(|x| is_black(&out, x, y)),
                "the whole band row {y} is ink, from the bright to the dark end of the ramp"
            );
        }
        let paper: Vec<(u32, u32)> = (0..8_u32)
            .chain(11..40_u32)
            .flat_map(|y| (0..40_u32).map(move |x| (x, y)))
            .collect();
        assert!(
            paper.iter().all(|&(x, y)| is_white(&out, x, y)),
            "all {} ramp pixels stay paper",
            paper.len()
        );
    }

    #[test]
    fn one_global_otsu_level_cannot_separate_the_band_from_the_ramp_foot() {
        let img = ramp_with_band();
        let level = otsu_level(&luminance_histogram(&img));
        // Keeping the band (luminance 30) as ink asks for `level >= 30`;
        // keeping the ramp's dark foot (luminances 60 and up) as paper asks
        // for `level < 60`. No single global cut does both, and Otsu's lands
        // in the middle of the gradient, inking the whole dark half of the
        // ramp along with the band — where adaptive_threshold keeps both
        // classes exact.
        assert!(
            level > 60,
            "Otsu's global cut cannot avoid the foot: {level}"
        );
        assert!(level < 255, "and there is a cut at all: {level}");
    }

    #[test]
    fn uniform_paper_yields_no_ink() {
        let img =
            RgbaImage::new(16_u32, 16_u32, vec![255_u8; 1_024_usize]).expect("16x16 is valid");
        let (out, step) = adaptive(img, 3, 15, false);
        assert_eq!(
            step["ink_pixels"], 0_i32,
            "nothing is below a flat background"
        );
        assert_eq!(step["background_min"], 255_i32, "the mean is paper");
        assert_eq!(step["background_max"], 255_i32, "everywhere");
        assert!(
            (0..16_u32)
                .flat_map(|y| (0..16_u32).map(move |x| (x, y)))
                .all(|(x, y)| is_white(&out, x, y)),
            "the whole picture stays opaque white"
        );
    }

    #[test]
    fn transparent_pixels_are_paper_in_the_mean_and_in_the_output() {
        let pixels = [
            [0_u8, 0, 0, 0],   // transparent black: counts as paper
            [30, 30, 30, 255], // opaque dark: ink against its neighbourhood
            [30, 30, 30, 255], // opaque dark: ink
            [0, 0, 0, 0],      // transparent black: counts as paper
        ];
        let img = RgbaImage::new(4_u32, 1_u32, pixels.concat()).expect("4x1 is valid");
        let (out, step) = adaptive(img, 1, 15, false);
        assert_eq!(step["ink_pixels"], 2_i32, "only the opaque dark pair");
        assert_eq!(step["background_min"], 105_i32, "mean(255, 30, 30)");
        assert_eq!(step["background_max"], 142_i32, "mean(255, 30)");
        assert!(is_black(&out, 1, 0), "the first dark pixel is ink");
        assert!(is_black(&out, 2, 0), "the second dark pixel is ink");
        assert!(
            is_white(&out, 0, 0) && is_white(&out, 3, 0),
            "transparent pixels end opaque white"
        );
    }

    #[test]
    fn invert_inks_a_bright_band_on_dark_paper() {
        let pixels: Vec<u8> = (0..1_600_usize)
            .flat_map(|_| [100_u8, 100, 100, 255])
            .collect();
        let mut img = RgbaImage::new(40_u32, 40_u32, pixels).expect("40x40 is valid");
        let band = (8..=10_u32).flat_map(|y| (0..40_u32).map(move |x| (x, y)));
        for (x, y) in band {
            assert!(img.set(x, y, [255_u8, 255, 255, 255]), "in bounds");
        }
        let (out, step) = adaptive(img.clone(), 2, 15, true);
        assert_eq!(step["otsu"], false, "the inverted test is local too");
        assert_eq!(step["ink_pixels"], 120_i32, "the bright band is the ink");
        for y in 8..=10_u32 {
            assert!(
                (0..40_u32).all(|x| is_black(&out, x, y)),
                "the whole bright band row {y} inks under the mirrored test"
            );
        }
        assert!(is_white(&out, 4, 4), "the dark field stays paper");
        assert!(is_white(&out, 39, 39), "even at the corner");
        let (straight, halo) = adaptive(img, 2, 15, false);
        assert_eq!(
            halo["ink_pixels"], 160_i32,
            "uninverted, the dark rows next to the bright band halo into ink"
        );
        assert!(
            is_black(&straight, 4, 6) && is_black(&straight, 4, 7),
            "rows 6 and 7 are the upper halo"
        );
        assert!(
            is_black(&straight, 35, 11) && is_black(&straight, 35, 12),
            "rows 11 and 12 the lower one"
        );
        assert!(is_white(&straight, 9, 9), "the bright band itself is paper");
        assert!(is_white(&straight, 4, 5), "and so is the far field");
    }

    #[test]
    fn a_radius_beyond_the_picture_averages_the_whole_image() {
        let mut img = RgbaImage::new(3_u32, 3_u32, vec![255_u8; 36_usize]).expect("3x3 is valid");
        assert!(img.set(1, 1, [30_u8, 30, 30, 255]), "in bounds");
        let (out, step) = adaptive(img, 128, 15, false);
        assert_eq!(step["ink_pixels"], 1_i32, "the single dark core");
        assert_eq!(step["background_min"], 230_i32, "mean(8 * 255 + 30) / 9");
        assert_eq!(step["background_max"], 230_i32, "every window is the whole");
        assert!(is_black(&out, 1, 1), "the core is ink");
        assert!(is_white(&out, 0, 0), "the paper is paper");
    }

    /// A freshly summed window `[i - r, i + r]`, clipped to the line: the
    /// definition [`window_sums`]'s running sum must reproduce.
    fn fresh_window_sum(line: &[i64], i: usize, radius: usize) -> i64 {
        line.iter()
            .enumerate()
            .filter(|&(j, _)| j.abs_diff(i) <= radius)
            .map(|(_, &value)| value)
            .sum()
    }

    #[test]
    fn window_sums_match_a_fresh_window_everywhere() {
        let line = [5_i64, 250, 30, 100, 7];
        for radius in 0_usize..5 {
            let expected: Vec<i64> = (0..line.len())
                .map(|i| fresh_window_sum(&line, i, radius))
                .collect();
            assert_eq!(
                window_sums(&line, radius),
                expected,
                "radius {radius} matches a fresh clipped window at every position"
            );
        }
        assert_eq!(window_sums(&[], 2), Vec::<i64>::new(), "an empty line");
    }

    #[test]
    fn window_spans_clamp_at_both_borders() {
        assert_eq!(window_span(0, 1, 5), 2, "clipped at the left edge");
        assert_eq!(window_span(2, 1, 5), 3, "full window");
        assert_eq!(window_span(4, 1, 5), 2, "clipped at the right edge");
        assert_eq!(window_span(1, 9, 3), 3, "a huge radius covers the line");
    }

    #[test]
    fn radii_outside_one_to_one_hundred_twenty_eight_refuse_by_name() {
        for radius in [0_u32, 129_u32, u32::MAX] {
            let error = adaptive_threshold(
                RgbaImage::new(3_u32, 3_u32, vec![255_u8; 36_usize]).expect("3x3 is valid"),
                3,
                &spec(radius, 15, false),
            )
            .expect_err("the radius must be rejected");
            assert_eq!(
                error.code(),
                "invalid_argument",
                "radius {radius} refuses by code"
            );
            let message = error.to_string();
            assert!(
                message.contains("ops[3].radius") && message.contains("1..=128"),
                "radius {radius} error names the field and range: {message}"
            );
        }
    }

    #[test]
    fn offsets_outside_zero_to_two_hundred_fifty_five_refuse_by_name() {
        check_offset(2, 255).expect("the whole u8 range passes");
        check_offset(2, 0).expect("zero offset is legal");
        let error = check_offset(2, 256).expect_err("256 must be rejected");
        let CyberError::InvalidArgument(message) = error else {
            panic!("expected InvalidArgument");
        };
        assert!(
            message.contains("ops[2].offset") && message.contains("0..=255"),
            "offset 256 error names the field and range: {message}"
        );
    }

    #[test]
    fn an_image_without_pixels_passes_through_reporting_zero_background() {
        let empty = RgbaImage {
            width: 0_u32,
            height: 1_u32,
            pixels: Vec::new(),
        };
        let (out, step) = adaptive_threshold(empty, 0, &spec(4, 15, false)).expect("parameters");
        assert_eq!(out.pixels.len(), 0_usize, "nothing to paint");
        assert_eq!(step["op"], "adaptive_threshold", "the op still reports");
        assert_eq!(step["ink_pixels"], 0_i32, "no pixels, no ink");
        assert_eq!(step["background_min"], 0_i32, "no means to span");
        assert_eq!(step["background_max"], 0_i32, "on either side");
    }
}
