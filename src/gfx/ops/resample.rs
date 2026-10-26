//! Size changes: `resample` with nearest, bilinear and area filters, all in
//! integer arithmetic.
//!
//! Bilinear sampling uses the `16.16` fixed-point source positions of
//! destination pixel centres and the shared two-tap [`bilinear_kernel`];
//! area sampling weights each source pixel by its integer footprint overlap
//! into the same [`Kernel`] shape. Both then blend through [`blend`], which
//! weights colours by alpha so transparent pixels never darken an edge.

use crate::gfx::filter::{Kernel, Tap, WEIGHT_ONE, bilinear_kernel, blend, last_centre};
use crate::gfx::image::{RgbaImage, resize_nearest};
use crate::gfx::ops::{Filter, div_ceil};
use crate::gfx::{clamp_dimension, rgba_byte_count};

/// Half of [`WEIGHT_ONE`]: the centre offset of a pixel (`32768`).
const FIXED_HALF: i64 = 0x8000;

/// The kernel used along one axis of a tap-based resample.
#[derive(Clone, Copy)]
enum AxisMode {
    /// Two-tap interpolation between pixel centres.
    Bilinear,
    /// Footprint-overlap weights (used where the source shrinks).
    Area,
}

/// The per-axis kernels of one tap-based resample.
#[derive(Clone, Copy)]
struct Axes {
    /// Horizontal kernel.
    x: AxisMode,
    /// Vertical kernel.
    y: AxisMode,
}

/// Resamples `img` to `width` x `height` with `filter`.
///
/// Requested dimensions saturate into `1..=MAX_DIMENSION` exactly like
/// [`resize_nearest`], so the result is always a valid image.
#[must_use]
#[inline]
pub fn resample(img: &RgbaImage, width: u32, height: u32, filter: Filter) -> RgbaImage {
    let target_width = clamp_dimension(width);
    let target_height = clamp_dimension(height);
    match filter {
        Filter::Nearest => resize_nearest(img, target_width, target_height),
        Filter::Bilinear => resample_taps(
            img,
            target_width,
            target_height,
            Axes {
                x: AxisMode::Bilinear,
                y: AxisMode::Bilinear,
            },
        ),
        Filter::Area => resample_taps(
            img,
            target_width,
            target_height,
            Axes {
                x: axis_mode(img.width, target_width),
                y: axis_mode(img.height, target_height),
            },
        ),
    }
}

/// Picks the kernel for one axis of the area filter: area while shrinking,
/// bilinear while growing.
const fn axis_mode(src_len: u32, dst_len: u32) -> AxisMode {
    if src_len >= dst_len {
        AxisMode::Area
    } else {
        AxisMode::Bilinear
    }
}

/// Resamples through precomputed per-axis kernels.
fn resample_taps(img: &RgbaImage, width: u32, height: u32, axes: Axes) -> RgbaImage {
    let columns: Vec<Kernel> = (0..width)
        .map(|dst| axis_line(img.width, width, dst, axes.x))
        .collect();
    let rows: Vec<Kernel> = (0..height)
        .map(|dst| axis_line(img.height, height, dst, axes.y))
        .collect();
    let mut pixels = Vec::with_capacity(rgba_byte_count(width, height).unwrap_or(0));
    for row in &rows {
        for column in &columns {
            pixels.extend_from_slice(&blend(img, row, column));
        }
    }
    RgbaImage {
        width,
        height,
        pixels,
    }
}

/// The kernel of destination coordinate `dst` along one axis.
fn axis_line(src_len: u32, dst_len: u32, dst: u32, mode: AxisMode) -> Kernel {
    match mode {
        AxisMode::Bilinear => bilinear_kernel(bilinear_position(src_len, dst_len, dst), src_len),
        AxisMode::Area => area_taps(src_len, dst_len, dst),
    }
}

/// The `16.16` source position of destination pixel `dst`'s centre:
/// `((2*dst + 1) * src_len * 65536) / (2*dst_len) - 32768`, clamped to the
/// source's pixel-centre range.
fn bilinear_position(src_len: u32, dst_len: u32, dst: u32) -> i64 {
    let centre = u64::from(dst).saturating_mul(2).saturating_add(1);
    let scaled = centre
        .saturating_mul(u64::from(src_len))
        .saturating_mul(WEIGHT_ONE);
    let divisor = u64::from(dst_len).saturating_mul(2);
    let mapped = scaled.checked_div(divisor).unwrap_or(0_u64);
    let shifted = i64::try_from(mapped)
        .unwrap_or(i64::MAX)
        .checked_sub(FIXED_HALF)
        .unwrap_or(0_i64);
    let last = last_centre(src_len);
    shifted.clamp(0_i64, last)
}

/// The footprint taps of destination coordinate `dst`: every source pixel
/// the box covers, weighted by its overlap. Weights are in `1 / dst_len`
/// source-pixel units, so spans cross-multiply and never need fractions.
fn area_taps(src_len: u32, dst_len: u32, dst: u32) -> Kernel {
    let source = u64::from(src_len);
    let target = u64::from(dst_len);
    let span_start = u64::from(dst).saturating_mul(source);
    let span_end = span_start.saturating_add(source);
    let first = span_start.checked_div(target).unwrap_or(0_u64);
    let last = div_ceil(span_end, target).min(source);
    let taps: Vec<Tap> = (first..last)
        .filter_map(|index| overlap_weight(index, span_start, span_end, target))
        .collect();
    let total = taps.iter().map(|tap| tap.weight).sum();
    Kernel { taps, total }
}

/// The [`Tap`] of source pixel `index` (in `1 / dst_len` units), or `None`
/// when the overlap is empty or the index does not fit.
fn overlap_weight(index: u64, span_start: u64, span_end: u64, target: u64) -> Option<Tap> {
    let pixel_start = index.saturating_mul(target);
    let start = span_start.max(pixel_start);
    let end = span_end.min(pixel_start.saturating_add(target));
    let weight = end.checked_sub(start)?;
    let source = u32::try_from(index).ok()?;
    (weight > 0).then_some(Tap {
        index: source,
        weight,
    })
}

#[cfg(test)]
mod tests {
    use super::{AxisMode, Kernel, axis_line, axis_mode, bilinear_position, resample};
    use crate::gfx::image::RgbaImage;
    use crate::gfx::ops::Filter;
    use crate::gfx::to_pixel;

    /// A 2x1 image: opaque black on the left, opaque white on the right.
    fn black_white() -> RgbaImage {
        RgbaImage::new(2, 1, vec![0, 0, 0, 255, 255, 255, 255, 255]).expect("2x1 is valid")
    }

    /// One kernel's taps as `(source index, weight)` pairs, for the
    /// golden comparisons.
    fn tap_row(kernel: &Kernel) -> Vec<(u32, u64)> {
        kernel
            .taps
            .iter()
            .map(|tap| (tap.index, tap.weight))
            .collect()
    }

    /// A 2x1 image: an opaque red pixel next to a fully transparent one.
    fn red_transparent() -> RgbaImage {
        RgbaImage::new(2, 1, vec![255, 0, 0, 255, 0, 0, 0, 0]).expect("2x1 is valid")
    }

    /// The alphas [`speckle`] cycles through, one per pixel.
    const ALPHAS: [u8; 6] = [255_u8, 128, 0, 64, 255, 192];

    /// One speckle channel: `(n * step + offset)` wrapped into a byte.
    fn channel(step: u32, offset: u32, n: u32) -> u8 {
        let scaled = n
            .checked_mul(step)
            .and_then(|value| value.checked_add(offset));
        let wrapped = scaled
            .and_then(|value| value.checked_rem(256_u32))
            .unwrap_or(0_u32);
        u8::try_from(wrapped).unwrap_or(0_u8)
    }

    /// The cycling alpha of speckle pixel `n`.
    fn speckle_alpha(n: u32) -> u8 {
        let slot = n
            .checked_rem(6_u32)
            .and_then(|rest| usize::try_from(rest).ok());
        slot.and_then(|index| ALPHAS.get(index).copied())
            .unwrap_or(255_u8)
    }

    /// The four bytes of speckle pixel `n`, the row-major index.
    fn speckle_pixel(n: u32) -> [u8; 4] {
        [
            channel(37, 11, n),
            channel(91, 5, n),
            channel(53, 200, n),
            speckle_alpha(n),
        ]
    }

    /// A `width` x `height` picture with every channel and every alpha
    /// moving, so a blend depends on the exact taps and weights.
    fn speckle(width: u32, height: u32) -> RgbaImage {
        let count = width.checked_mul(height).unwrap_or(0_u32);
        let pixels: Vec<u8> = (0..count).flat_map(speckle_pixel).collect();
        RgbaImage::new(width, height, pixels).expect("the speckle fixture is valid")
    }

    /// An image's pixels as arrays, for the golden comparisons.
    fn picture(img: &RgbaImage) -> Vec<[u8; 4]> {
        img.pixels.chunks_exact(4_usize).map(to_pixel).collect()
    }

    /// The `3 x 2` speckle bilinearly upscaled to `5 x 4`, pixel by pixel.
    const BILINEAR_5X4: [[u8; 4]; 20] = [
        [11, 5, 200, 255],
        [20, 28, 213, 204],
        [48, 96, 253, 128],
        [48, 96, 253, 51],
        [0, 0, 0, 0],
        [20, 6, 193, 207],
        [44, 39, 200, 188],
        [92, 103, 214, 160],
        [125, 134, 213, 93],
        [196, 204, 209, 48],
        [59, 12, 158, 112],
        [107, 68, 165, 156],
        [143, 111, 170, 223],
        [169, 157, 189, 176],
        [196, 204, 209, 144],
        [122, 22, 103, 64],
        [149, 88, 142, 140],
        [159, 113, 156, 255],
        [179, 161, 184, 217],
        [196, 204, 209, 192],
    ];

    /// The `4 x 3` speckle area-blended to `2 x 5` (shrinking columns,
    /// growing rows), pixel by pixel.
    const AREA_2X5: [[u8; 4]; 10] = [
        [23, 35, 218, 192],
        [122, 22, 103, 32],
        [90, 86, 201, 204],
        [152, 60, 40, 96],
        [175, 152, 179, 224],
        [160, 69, 24, 192],
        [160, 135, 176, 109],
        [148, 144, 92, 211],
        [88, 56, 165, 32],
        [141, 186, 131, 224],
    ];

    #[test]
    fn the_tap_blends_hold_their_golden_bytes() {
        let up = resample(&speckle(3, 2), 5, 4, Filter::Bilinear);
        assert_eq!(picture(&up), BILINEAR_5X4.to_vec(), "bilinear 5x4");
        let down = resample(&speckle(4, 3), 2, 5, Filter::Area);
        assert_eq!(picture(&down), AREA_2X5.to_vec(), "area 2x5");
    }

    #[test]
    fn nearest_matches_resize_nearest() {
        let img = black_white();
        let out = resample(&img, 4, 2, Filter::Nearest);
        assert_eq!((out.width, out.height), (4, 2), "target shape");
        assert_eq!(out.get(0, 0), Some([0, 0, 0, 255]), "top-left black");
        assert_eq!(
            out.get(3, 1),
            Some([255, 255, 255, 255]),
            "bottom-right white"
        );
    }

    #[test]
    fn bilinear_ramps_between_black_and_white_with_exact_endpoints() {
        let out = resample(&black_white(), 4, 1, Filter::Bilinear);
        let grays: Vec<u8> = (0..4)
            .filter_map(|x| out.get(x, 0))
            .map(|pixel| pixel[0])
            .collect();
        assert_eq!(grays.first(), Some(&0), "left endpoint stays black");
        assert_eq!(grays.last(), Some(&255), "right endpoint stays white");
        assert!(
            grays
                .windows(2)
                .all(|pair| matches!(pair, [low, high] if low <= high)),
            "the ramp is non-decreasing: {grays:?}"
        );
        assert!(
            grays[1] > 0 && grays[2] < 255,
            "the middle pixels are true blends: {grays:?}"
        );
    }

    #[test]
    fn bilinear_position_follows_the_fixed_point_formula() {
        assert_eq!(bilinear_position(1, 2, 0), 0, "clamped at the first centre");
        assert_eq!(
            bilinear_position(4, 4, 1),
            0x0001_0000,
            "an identity resize maps destination centre 1.5 onto source centre 1.0"
        );
        assert_eq!(
            bilinear_position(2, 4, 3),
            0x0001_0000,
            "clamped at the last centre"
        );
    }

    /// Checkerboard row starting with a black pixel: four opaque pixels.
    const ROW_BLACK: [u8; 16] = [
        0, 0, 0, 255, 255, 255, 255, 255, 0, 0, 0, 255, 255, 255, 255, 255,
    ];
    /// Checkerboard row starting with a white pixel.
    const ROW_WHITE: [u8; 16] = [
        255, 255, 255, 255, 0, 0, 0, 255, 255, 255, 255, 255, 0, 0, 0, 255,
    ];

    #[test]
    fn area_downscale_of_a_checkerboard_is_mid_grey() {
        let mut pixels = Vec::new();
        for _ in 0..2_u32 {
            pixels.extend_from_slice(&ROW_BLACK);
            pixels.extend_from_slice(&ROW_WHITE);
        }
        let board = RgbaImage::new(4, 4, pixels).expect("4x4 is valid");
        let out = resample(&board, 2, 2, Filter::Area);
        assert_eq!((out.width, out.height), (2, 2), "target shape");
        for (position, chunk) in out.pixels.chunks_exact(4).enumerate() {
            let [red, green, blue, alpha] = to_pixel(chunk);
            assert!(
                (127..=128).contains(&red) && red == green && green == blue,
                "each blend is neutral mid grey at {position}, got {red},{green},{blue}"
            );
            assert_eq!(alpha, 255, "opaque inputs stay opaque");
        }
    }

    /// Every partially red destination pixel keeps red at full purity.
    fn assert_red_pure(out: &RgbaImage) {
        for x in 0..3 {
            let [red, green, blue, alpha] = out.get(x, 0).expect("in bounds");
            assert!(alpha > 0, "x={x} still carries alpha {alpha}");
            assert_eq!([red, green, blue], [255, 0, 0], "red x={x} stays pure");
        }
        assert_eq!(out.get(0, 0), Some([255, 0, 0, 255]), "x=0 opaque");
        let faded = out.get(1, 0).unwrap_or([0, 0, 0, 255])[3];
        assert!(faded < 255, "x=1 blends in transparency: {faded}");
        assert_eq!(
            out.get(3, 0),
            Some([0, 0, 0, 0]),
            "fully transparent gets colour 0"
        );
    }

    #[test]
    fn alpha_weighted_upscale_keeps_red_pure() {
        for filter in [Filter::Bilinear, Filter::Area] {
            let out = resample(&red_transparent(), 4, 1, filter);
            assert_red_pure(&out);
        }
    }

    #[test]
    fn area_upscale_axis_falls_back_to_bilinear_per_axis() {
        assert!(matches!(axis_mode(8, 4), AxisMode::Area), "shrinking axis");
        assert!(
            matches!(axis_mode(4, 8), AxisMode::Bilinear),
            "growing axis"
        );
        // A 1x1 red source grown to 2x1 through Area interpolates both axes.
        let red = RgbaImage::new(1, 1, vec![255, 0, 0, 255]).expect("1x1 is valid");
        let out = resample(&red, 2, 1, Filter::Area);
        assert_eq!(out.get(1, 0), Some([255, 0, 0, 255]), "edge extends");
    }

    #[test]
    fn area_taps_weight_every_covered_source_pixel() {
        let lines: Vec<Vec<(u32, u64)>> = (0..3_u32)
            .map(|dst| tap_row(&axis_line(7, 3, dst, AxisMode::Area)))
            .collect();
        assert_eq!(
            lines,
            vec![
                vec![(0, 3), (1, 3), (2, 1)],
                vec![(2, 2), (3, 3), (4, 2)],
                vec![(4, 1), (5, 3), (6, 3)],
            ],
            "7 source pixels split across 3 destinations by cross-multiplied overlaps"
        );
        let totals: Vec<u64> = lines
            .iter()
            .map(|line| line.iter().map(|&(_, weight)| weight).sum())
            .collect();
        assert_eq!(
            totals,
            vec![7_u64; 3],
            "each footprint carries the source once"
        );
    }

    #[test]
    fn bilinear_taps_keep_unit_weight_at_the_edges() {
        let edge = axis_line(2, 4, 0, AxisMode::Bilinear);
        assert_eq!(edge.total, 0x0001_0000, "weights are in 16.16 units");
        assert_eq!(
            edge.taps.iter().map(|tap| tap.weight).sum::<u64>(),
            edge.total,
            "tap weights sum to the total"
        );
    }
}
