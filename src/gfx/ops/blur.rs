//! Separable integer box blur for [`crate::gfx::ops::ImageOp::Blur`].
//!
//! The window is the `(2r + 1) x (2r + 1)` square clipped at the image
//! borders, run by [`crate::gfx::filter::box_filter`]: every row's windows
//! are averaged first and every column of those averages second, so a pass
//! costs `O(width * height)` regardless of the radius.
//!
//! Like the bilinear and area blends in [`crate::gfx::ops::resample`], the
//! blur weights colours by alpha, so transparent pixels never darken an
//! edge: the average colour of a window is `sum(color * alpha) /
//! sum(alpha)` (`0` when the window carries no alpha at all) and its alpha
//! is `sum(alpha)` over the clipped window's pixel count. That weighting is
//! this module's own `Sums`, which the shared pass folds: the fold runs at
//! the end of each pass, so the vertical pass blurs the horizontal pass's
//! already-rounded pixels, exactly as the op always did.

use serde_json::{Value, json};

use crate::error::CyberError;
use crate::gfx::filter::{Window, box_filter};
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
    let blurred = box_filter(&to_pixels(&img), width, reach, average_window);
    let out = RgbaImage {
        width: img.width,
        height: img.height,
        pixels: blurred.iter().flatten().copied().collect(),
    };
    Ok((out, report))
}

/// The fold [`box_filter`] runs: one window's average pixel over the count
/// of in-line samples its clipped window covers.
fn average_window(window: &Sums, span: usize) -> [u8; 4] {
    window.average(span)
}

/// The image's row-major pixels.
fn to_pixels(img: &RgbaImage) -> Vec<[u8; 4]> {
    img.pixels
        .chunks_exact(BYTES_PER_PIXEL)
        .map(to_pixel)
        .collect()
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

impl Window<[u8; 4]> for Sums {
    #[inline]
    fn enter(&mut self, sample: [u8; 4]) {
        self.moved(sample, u64::saturating_add);
    }

    #[inline]
    fn exit(&mut self, sample: [u8; 4]) {
        self.moved(sample, u64::saturating_sub);
    }
}

impl Sums {
    /// Combines one pixel's premultiplied contribution into every channel
    /// by `step`.
    fn moved(&mut self, pixel: [u8; 4], step: fn(u64, u64) -> u64) {
        let [red, green, blue, alpha] = pixel;
        let weight = u64::from(alpha);
        self.red = step(self.red, u64::from(red).saturating_mul(weight));
        self.green = step(self.green, u64::from(green).saturating_mul(weight));
        self.blue = step(self.blue, u64::from(blue).saturating_mul(weight));
        self.alpha = step(self.alpha, weight);
    }

    /// The window's average pixel: alpha over the `span` in-image window
    /// positions, colours over the covered alpha; a window without any
    /// alpha gets colour `0` (`rounded_div` answers `0` for a zero
    /// denominator).
    fn average(&self, span: usize) -> [u8; 4] {
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
    use super::{Sums, blur};
    use crate::error::CyberError;
    use crate::gfx::filter::{Window, window_line, window_span};
    use crate::gfx::image::RgbaImage;
    use crate::gfx::to_pixel;

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

    /// One horizontal pass: the shared line window, folded the way the blur
    /// folds every window it walks.
    fn blur_line(line: &[[u8; 4]], radius: usize) -> Vec<[u8; 4]> {
        window_line::<Sums, [u8; 4]>(line, radius)
            .iter()
            .enumerate()
            .map(|(position, window)| {
                super::average_window(window, window_span(position, radius, line.len()))
            })
            .collect()
    }

    /// The samples whose window position `i` reaches at `radius`.
    fn in_window(pixels: &[[u8; 4]], i: usize, radius: usize) -> Vec<[u8; 4]> {
        pixels
            .iter()
            .enumerate()
            .filter_map(|(position, &pixel)| (position.abs_diff(i) <= radius).then_some(pixel))
            .collect()
    }

    /// A freshly summed window `[i - r, i + r]`, clipped to the line.
    fn window_sum(pixels: &[[u8; 4]], i: usize, radius: usize) -> Sums {
        let mut sums = Sums::default();
        for pixel in in_window(pixels, i, radius) {
            Window::enter(&mut sums, pixel);
        }
        sums
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
    /// moving, so a box average depends on the window's exact shape.
    fn speckle(width: u32, height: u32) -> RgbaImage {
        let count = width.checked_mul(height).unwrap_or(0_u32);
        let pixels: Vec<u8> = (0..count).flat_map(speckle_pixel).collect();
        RgbaImage::new(width, height, pixels).expect("the speckle fixture is valid")
    }

    /// An image's pixels as arrays, for the golden comparisons.
    fn picture(img: &RgbaImage) -> Vec<[u8; 4]> {
        img.pixels.chunks_exact(4_usize).map(to_pixel).collect()
    }

    /// The `6 x 5` speckle blurred at radius `1`, pixel by pixel.
    const BLUR_ONE: [[u8; 4]; 30] = [
        [92, 52, 121, 192],
        [92, 52, 121, 128],
        [56, 88, 149, 64],
        [135, 112, 176, 106],
        [151, 153, 152, 170],
        [158, 169, 155, 224],
        [131, 69, 109, 192],
        [131, 69, 109, 128],
        [96, 105, 151, 64],
        [118, 129, 139, 106],
        [134, 138, 124, 170],
        [141, 149, 119, 224],
        [183, 103, 86, 192],
        [183, 103, 86, 128],
        [118, 139, 128, 64],
        [84, 163, 116, 106],
        [100, 140, 101, 170],
        [107, 147, 96, 224],
        [177, 137, 148, 192],
        [177, 137, 148, 128],
        [170, 173, 161, 64],
        [67, 197, 92, 106],
        [77, 142, 110, 170],
        [73, 144, 109, 224],
        [160, 154, 179, 192],
        [160, 154, 179, 128],
        [167, 190, 164, 64],
        [59, 214, 106, 106],
        [65, 159, 130, 170],
        [56, 161, 140, 224],
    ];

    /// The same picture blurred at radius `3`.
    const BLUR_THREE: [[u8; 4]; 30] = [
        [133, 84, 121, 112],
        [124, 113, 121, 140],
        [128, 117, 118, 149],
        [128, 117, 118, 149],
        [119, 141, 126, 128],
        [117, 139, 118, 128],
        [138, 101, 135, 112],
        [121, 130, 132, 140],
        [122, 128, 131, 149],
        [122, 128, 131, 149],
        [112, 150, 136, 128],
        [106, 146, 127, 128],
        [138, 101, 135, 112],
        [121, 130, 132, 140],
        [122, 128, 131, 149],
        [122, 128, 131, 149],
        [112, 150, 136, 128],
        [106, 146, 127, 128],
        [138, 101, 135, 112],
        [121, 130, 132, 140],
        [122, 128, 131, 149],
        [122, 128, 131, 149],
        [112, 150, 136, 128],
        [106, 146, 127, 128],
        [163, 118, 119, 112],
        [131, 147, 119, 140],
        [126, 137, 116, 149],
        [126, 137, 116, 149],
        [104, 155, 124, 128],
        [91, 149, 116, 128],
    ];

    #[test]
    fn blur_holds_its_golden_bytes_at_radius_one_and_three() {
        let (one, first_step) = blur(speckle(6, 5), 0, 1).expect("radius 1 is valid");
        assert_eq!(picture(&one), BLUR_ONE.to_vec(), "radius 1");
        assert_eq!(first_step["radius"], 1_i32, "the small radius echoes");
        let (three, second_step) = blur(speckle(6, 5), 0, 3).expect("radius 3 is valid");
        assert_eq!(picture(&three), BLUR_THREE.to_vec(), "radius 3");
        assert_eq!(second_step["radius"], 3_i32, "the wider radius echoes");
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
