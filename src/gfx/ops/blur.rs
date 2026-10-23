//! Separable integer box blur for [`crate::gfx::ops::ImageOp::Blur`].
//!
//! The window is the `(2r + 1) x (2r + 1)` square clipped at the image
//! borders, applied per row and then per column with running sums: the
//! same window-count technique [`crate::sketch::morph`] uses for its
//! square structuring element, so a pass costs `O(width * height)`
//! regardless of the radius.
//!
//! Like the bilinear and area blends in [`crate::gfx::ops::resample`], the
//! blur weights colours by alpha, so transparent pixels never darken an
//! edge: the average colour of a window is `sum(color * alpha) /
//! sum(alpha)` (`0` when the window carries no alpha at all) and its alpha
//! is `sum(alpha)` over the clipped window's pixel count.

use serde_json::{Value, json};

use crate::error::CyberError;
use crate::gfx::image::RgbaImage;
use crate::gfx::ops::{arg_error, rounded_div};
use crate::gfx::{BYTES_PER_PIXEL, to_pixel};

/// Smallest accepted blur radius.
const MIN_RADIUS: u32 = 1;
/// Largest accepted blur radius.
const MAX_RADIUS: u32 = 256;

/// Box-blurs `img` by `radius`: every pixel becomes the average of the
/// `(2 * radius + 1)^2` window around it, clipped at the image borders,
/// colours weighted by alpha.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] when `radius` is outside `1..=256`.
pub(crate) fn blur(
    img: RgbaImage,
    index: usize,
    radius: u32,
) -> Result<(RgbaImage, Value), CyberError> {
    if !(MIN_RADIUS..=MAX_RADIUS).contains(&radius) {
        return Err(arg_error(
            index,
            "radius",
            &format!("be within {MIN_RADIUS}..={MAX_RADIUS}, got {radius}"),
        ));
    }
    let report = json!({"op": "blur", "radius": radius});
    let Ok(width) = usize::try_from(img.width) else {
        return Ok((img, report));
    };
    let reach = usize::try_from(radius).unwrap_or(usize::MAX);
    let rows = blur_rows(&to_pixels(&img), width, reach);
    let pixels = blur_columns(&rows, width, reach);
    let out = RgbaImage {
        width: img.width,
        height: img.height,
        pixels: pixels.iter().flatten().copied().collect(),
    };
    Ok((out, report))
}

/// The image's row-major pixels.
fn to_pixels(img: &RgbaImage) -> Vec<[u8; 4]> {
    img.pixels
        .chunks_exact(BYTES_PER_PIXEL)
        .map(to_pixel)
        .collect()
}

/// One horizontal pass: every row box-blurred with the window reach.
fn blur_rows(pixels: &[[u8; 4]], width: usize, reach: usize) -> Vec<[u8; 4]> {
    let mut out = Vec::with_capacity(pixels.len());
    for row in pixels.chunks_exact(width) {
        out.extend(blur_line(row, reach));
    }
    out
}

/// One vertical pass: every column box-blurred with the window reach.
fn blur_columns(pixels: &[[u8; 4]], width: usize, reach: usize) -> Vec<[u8; 4]> {
    let height = pixels.len().checked_div(width).unwrap_or(0);
    let mut out = vec![[0_u8; 4]; pixels.len()];
    for x in 0..width {
        let blurred = blur_line(&column_of(pixels, width, height, x), reach);
        write_column(&mut out, width, x, &blurred);
    }
    out
}

/// Writes one blurred column back into the row-major grid, ignoring
/// out-of-bounds rows.
fn write_column(out: &mut [[u8; 4]], width: usize, x: usize, column: &[[u8; 4]]) {
    for (y, pixel) in column.iter().enumerate() {
        if let Some(slot) = out.get_mut(y.saturating_mul(width).saturating_add(x)) {
            *slot = *pixel;
        }
    }
}

/// The pixels of column `x`, top to bottom.
fn column_of(pixels: &[[u8; 4]], width: usize, height: usize, x: usize) -> Vec<[u8; 4]> {
    (0..height)
        .map(|y| {
            pixels
                .get(y.saturating_mul(width).saturating_add(x))
                .copied()
                .unwrap_or([0_u8; 4])
        })
        .collect()
}

/// Box-averages one line of pixels: the window `[i - r, i + r]`, clipped
/// to the line and tracked by running premultiplied sums, the same sliding
/// shape [`crate::sketch::morph`]'s `window_counts` keeps.
fn blur_line(line: &[[u8; 4]], radius: usize) -> Vec<[u8; 4]> {
    let len = line.len();
    if len == 0 {
        return Vec::new();
    }
    let reach = radius.min(len.saturating_sub(1));
    let mut sums = line
        .iter()
        .take(reach.saturating_add(1))
        .fold(Sums::default(), |sums, &pixel| sums.absorb(pixel));
    let mut out = Vec::with_capacity(len);
    for i in 0..len {
        out.push(sums.average(window_span(i, radius, len)));
        if let Some(next) = line.get(i.saturating_add(reach).saturating_add(1)) {
            sums = sums.absorb(*next);
        }
        if let Some(old) = i.checked_sub(reach).and_then(|k| line.get(k)) {
            sums = sums.release(*old);
        }
    }
    out
}

/// The number of in-line window positions at `i`, i.e. the count that
/// divides the alpha average there.
fn window_span(i: usize, radius: usize, len: usize) -> usize {
    let left = i.saturating_sub(radius);
    let right = i.saturating_add(radius).min(len.saturating_sub(1));
    right.saturating_sub(left).saturating_add(1)
}

/// One window's premultiplied running sums: colours weighted by alpha,
/// plus the alpha itself.
#[derive(Clone, Copy, Default)]
struct Sums {
    /// `sum(red * alpha)`.
    red: u64,
    /// `sum(green * alpha)`.
    green: u64,
    /// `sum(blue * alpha)`.
    blue: u64,
    /// `sum(alpha)`.
    alpha: u64,
}

impl Sums {
    /// Adds one pixel's premultiplied contribution to the window.
    fn absorb(self, pixel: [u8; 4]) -> Self {
        self.moved(pixel, u64::saturating_add)
    }

    /// Takes one pixel's premultiplied contribution out of the window.
    fn release(self, pixel: [u8; 4]) -> Self {
        self.moved(pixel, u64::saturating_sub)
    }

    /// Combines one pixel's contribution into every channel by `step`.
    fn moved(self, pixel: [u8; 4], step: fn(u64, u64) -> u64) -> Self {
        let [red, green, blue, alpha] = pixel;
        let weight = u64::from(alpha);
        Self {
            red: step(self.red, u64::from(red).saturating_mul(weight)),
            green: step(self.green, u64::from(green).saturating_mul(weight)),
            blue: step(self.blue, u64::from(blue).saturating_mul(weight)),
            alpha: step(self.alpha, weight),
        }
    }

    /// The window's average pixel: alpha over the `span` in-image window
    /// positions, colours over the covered alpha; a window without any
    /// alpha gets colour `0` (`rounded_div` answers `0` for a zero
    /// denominator).
    fn average(self, span: usize) -> [u8; 4] {
        let divisor = u64::try_from(span).unwrap_or(1_u64).max(1_u64);
        let colors = [self.red, self.green, self.blue]
            .map(|sum| clamp_channel(rounded_div(sum, self.alpha)));
        let [red, green, blue] = colors;
        let alpha = clamp_channel(rounded_div(self.alpha, divisor));
        [red, green, blue, alpha]
    }
}

/// Saturates an averaged channel value into `0..=255`.
fn clamp_channel(value: u64) -> u8 {
    u8::try_from(value).unwrap_or(255_u8)
}

#[cfg(test)]
mod tests {
    use super::{Sums, blur, blur_line, window_span};
    use crate::error::CyberError;
    use crate::gfx::image::RgbaImage;

    /// A `size` x `size` opaque image of one colour.
    fn solid(size: u32, color: [u8; 3]) -> RgbaImage {
        let count = usize::try_from(size)
            .expect("fixture fits")
            .checked_mul(usize::try_from(size).expect("fixture fits"))
            .expect("fixture fits");
        let pixels: Vec<u8> = (0..count)
            .flat_map(|_| [color[0], color[1], color[2], 255_u8])
            .collect();
        RgbaImage::new(size, size, pixels).expect("fixture is valid")
    }

    /// The error message of a rejected blur run over a tiny image.
    fn failure(radius: u32) -> String {
        let error =
            blur(solid(2_u32, [0_u8, 0, 0]), 4, radius).expect_err("the radius must be rejected");
        let CyberError::InvalidArgument(text) = error else {
            panic!("expected InvalidArgument");
        };
        text
    }

    #[test]
    fn a_uniform_window_averages_to_its_only_colour() {
        let white = [200_u8, 200, 200, 255];
        let blurred = blur_line(&[white; 5], 1);
        assert_eq!(blurred, vec![white; 5], "uniform stays uniform");
    }

    #[test]
    fn blur_line_averages_black_and_white_with_clipped_windows() {
        let blurred = blur_line(&[[0, 0, 0, 255], [255, 255, 255, 255]], 1);
        assert_eq!(
            blurred,
            vec![[128, 128, 128, 255]; 2],
            "the clipped window is the whole line everywhere; 65025 / 510 rounds to 128"
        );
        let one_pixel = blur_line(&[[10, 200, 30, 9]], 5);
        assert_eq!(
            one_pixel,
            vec![[10_u8, 200, 30, 9]],
            "a radius cannot leave a 1-line"
        );
    }

    #[test]
    fn alpha_weighting_keeps_transparent_pixels_from_darkening() {
        let blurred = blur_line(&[[255, 0, 0, 255], [0, 0, 0, 0], [0, 0, 0, 0]], 1);
        assert_eq!(
            blurred[0],
            [255_u8, 0, 0, 128],
            "red keeps purity, alpha averages"
        );
        assert_eq!(
            blurred[1],
            [255_u8, 0, 0, 85],
            "the middle window reaches the red"
        );
        assert_eq!(
            blurred[2],
            [0_u8, 0, 0, 0],
            "a window without alpha gets colour 0"
        );
    }

    /// A freshly summed window `[i - r, i + r]`, clipped to the line.
    fn window_sum(pixels: &[[u8; 4]], i: usize, radius: usize) -> Sums {
        pixels
            .iter()
            .enumerate()
            .filter(|&(j, _)| j.abs_diff(i) <= radius)
            .map(|(_, &pixel)| pixel)
            .fold(Sums::default(), Sums::absorb)
    }

    /// Every running-window value must match the fresh sums at `radius`.
    fn check_running_sums(pixels: &[[u8; 4]], radius: usize) {
        for (i, got) in blur_line(pixels, radius).iter().enumerate() {
            let span = window_span(i, radius, pixels.len());
            assert_eq!(
                *got,
                window_sum(pixels, i, radius).average(span),
                "position {i}, radius {radius}"
            );
        }
    }

    #[test]
    fn running_sums_match_a_fresh_window_everywhere() {
        let pixels = [
            [7, 0, 0, 255],
            [0, 0, 0, 0],
            [199, 0, 0, 200],
            [0, 0, 0, 255],
        ];
        for radius in 1_usize..4 {
            check_running_sums(&pixels, radius);
        }
    }

    #[test]
    fn window_spans_clamp_at_both_borders() {
        assert_eq!(window_span(0, 1, 5), 2, "clipped at the left edge");
        assert_eq!(window_span(2, 1, 5), 3, "full window");
        assert_eq!(window_span(4, 1, 5), 2, "clipped at the right edge");
        assert_eq!(window_span(1, 9, 3), 3, "a huge radius covers the line");
    }

    #[test]
    fn blur_averages_both_axes_and_reports_the_radius() {
        let mut img = solid(3_u32, [0_u8, 0, 0]);
        assert!(img.set(1, 1, [255, 255, 255, 255]), "in-bounds overwrite");
        let (out, step) = blur(img, 0, 1).expect("radius 1 is valid");
        assert_eq!(step["op"], "blur", "op name");
        assert_eq!(step["radius"], 1_i32, "the radius echoes");
        assert_eq!((out.width, out.height), (3, 3), "the shape stands");
        let centre = out.get(1, 1).expect("in bounds");
        let corner = out.get(0, 0).expect("in bounds");
        assert!(
            centre[0] > 0,
            "the white dot spreads across the map: {}",
            centre[0]
        );
        assert!(
            corner[0] > centre[0],
            "the corner's clipped 2x2 window dilutes the dot less than the centre's 3x3 one: {} vs {}",
            corner[0],
            centre[0]
        );
        assert_eq!(centre[3], 255, "an opaque field stays opaque");
    }

    #[test]
    fn blur_of_a_uniform_picture_is_the_identity() {
        let img = solid(4_u32, [10_u8, 200, 30]);
        let (out, step) = blur(img.clone(), 0, 256).expect("the maximum radius is valid");
        assert_eq!(out, img, "averaging one colour returns it exactly");
        assert_eq!(step["radius"], 256_i32, "the big radius echoes");
    }

    #[test]
    fn blur_never_darkens_around_a_single_transparent_field() {
        let mut field = solid(3_u32, [255_u8, 0, 0]);
        for (x, y) in [
            (0, 0),
            (2, 0),
            (0, 2),
            (2, 2),
            (0, 1),
            (1, 0),
            (2, 1),
            (1, 2),
        ] {
            assert!(field.set(x, y, [0, 0, 0, 0]), "in-bounds overwrite");
        }
        let (out, _step) = blur(field, 2, 1).expect("radius 1 is valid");
        let centre = out.get(1, 1).expect("in bounds");
        assert_eq!(
            [centre[0], centre[1], centre[2]],
            [255, 0, 0],
            "the red core stays pure"
        );
        assert!(centre[3] > 0, "and it keeps alpha: {}", centre[3]);
        let corner = out.get(0, 0).expect("in bounds");
        assert_eq!(
            [corner[0], corner[1], corner[2]],
            [255, 0, 0],
            "the corner's blend of the core is pure red too"
        );
        assert!(
            corner[3] > 0,
            "and it carries the averaged alpha: {}",
            corner[3]
        );
    }

    #[test]
    fn blur_rejects_radii_outside_one_to_two_hundred_fifty_six() {
        for radius in [0_u32, 257_u32, u32::MAX] {
            let text = failure(radius);
            assert!(
                text.contains("ops[4].radius") && text.contains("1..=256"),
                "radius {radius} error: {text}"
            );
        }
    }
}
