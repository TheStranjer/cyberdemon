//! Dithered palette quantization: [`quantize_dithered`] with [`Dither`].
//!
//! [`Dither::None`] defers to [`crate::gfx::quantize::quantize`].
//! [`Dither::FloydSteinberg`] walks the image in a serpentine scan and
//! pushes each rounding error to the not-yet-processed neighbours in
//! `7/16`, `3/16`, `5/16` and `1/16` shares; every share is an integer
//! division and its remainder rides along with the `7/16` share, so the
//! whole error survives. Each result is written at its own pixel's
//! position, so the backward-scan rows come out unmirrored.
//! [`Dither::Ordered`] adds a fixed `4x4` `Bayer`
//! cell offset to each channel instead of propagating error.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::gfx::image::{IndexedImage, RgbaImage};
use crate::gfx::ops::clamp_u8;
use crate::gfx::palette::Palette;
use crate::gfx::quantize::{QuantizeOptions, is_transparent, nearest_index, quantize};
use crate::gfx::{BYTES_PER_PIXEL, to_pixel};

/// Colour channels of a pixel.
const CHANNELS: usize = 3;
/// Error denominator of the `Floyd-Steinberg` kernel.
const ERROR_TOTAL: i32 = 16;
/// Side of the `Bayer` matrix.
const MATRIX_SIDE: usize = 4;
/// Middle of the `Bayer` range, subtracted from every cell.
const BAYER_CENTER: i32 = 8;
/// Step between neighbouring `Bayer` cells.
const BAYER_STEP: i32 = 4;

/// The error-diffusion mode of [`quantize_dithered`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Dither {
    /// Plain nearest-colour quantization, no error handling.
    None,
    /// Serpentine `Floyd-Steinberg` error diffusion.
    FloydSteinberg,
    /// Fixed `4x4` `Bayer` threshold offset per pixel.
    Ordered,
}

/// The `4x4` `Bayer` matrix (cells `0..=15`).
const BAYER: [[u8; MATRIX_SIDE]; MATRIX_SIDE] =
    [[0, 8, 2, 10], [12, 4, 14, 6], [3, 11, 1, 9], [15, 7, 13, 5]];

/// Quantizes `img` to `palette`, spreading or dithering the rounding error
/// according to `dither`.
///
/// Transparent pixels (per `options.transparency`) keep their transparency
/// and neither receive nor spread error.
#[must_use]
#[inline]
pub fn quantize_dithered(
    img: &RgbaImage,
    palette: &Palette,
    options: &QuantizeOptions,
    dither: Dither,
) -> IndexedImage {
    match dither {
        Dither::None => quantize(img, palette, options),
        Dither::FloydSteinberg => floyd_steinberg(img, palette, options),
        Dither::Ordered => ordered(img, palette, options),
    }
}

/// Channel shares of one pixel error, remainders folded into `wide`.
struct Kernel {
    /// `7/16` (plus all remainders), the same-row neighbour.
    wide: [i32; CHANNELS],
    /// `3/16`, the next row away from the scan direction.
    back: [i32; CHANNELS],
    /// `5/16`, the next row below.
    straight: [i32; CHANNELS],
    /// `1/16`, the next row toward the scan direction.
    narrow: [i32; CHANNELS],
}

/// Splits a per-channel error into the kernel shares.
fn split_error(error: [i32; CHANNELS]) -> Kernel {
    let [red, green, blue] = error;
    let (rw, rb, rs, rn) = split_channel(red);
    let (gw, gb, gs, gn) = split_channel(green);
    let (bw, bb, bs, bn) = split_channel(blue);
    Kernel {
        wide: [rw, gw, bw],
        back: [rb, gb, bb],
        straight: [rs, gs, bs],
        narrow: [rn, gn, bn],
    }
}

/// The `7/16, 3/16, 5/16, 1/16` shares of one channel; the truncated
/// divisions' remainder joins the `7/16` share.
fn split_channel(error: i32) -> (i32, i32, i32, i32) {
    let three = share(error, 3);
    let five = share(error, 5);
    let one = share(error, 1);
    let seven = error
        .saturating_sub(three)
        .saturating_sub(five)
        .saturating_sub(one);
    (seven, three, five, one)
}

/// `error * numerator / 16`, truncated toward zero.
fn share(error: i32, numerator: i32) -> i32 {
    error
        .saturating_mul(numerator)
        .checked_div(ERROR_TOTAL)
        .unwrap_or(0_i32)
}

/// The diffusion errors stored for the current and the next row.
struct ErrorRows {
    /// `RGB` error accumulated for the row being scanned.
    current: Vec<i32>,
    /// `RGB` error already pushed into the row below.
    following: Vec<i32>,
}

impl ErrorRows {
    /// Zeroed buffers for a row of `width` pixels.
    fn new(width: usize) -> Self {
        let cells = width.saturating_mul(CHANNELS);
        Self {
            current: vec![0_i32; cells],
            following: vec![0_i32; cells],
        }
    }

    /// Removes and returns the error waiting at `x` in the current row.
    fn take_current(&mut self, x: usize) -> [i32; CHANNELS] {
        take_error(&mut self.current, x)
    }

    /// Drops any error sent to a pixel that neither receives nor spreads.
    fn drop_current(&mut self, x: usize) {
        let _unused = take_error(&mut self.current, x);
    }

    /// Moves `error` to the not-yet-processed neighbours of `x`.
    fn spread(&mut self, x: usize, forward: bool, error: [i32; CHANNELS]) {
        let kernel = split_error(error);
        let (near, back, ahead) = if forward {
            (x.checked_add(1), x.checked_sub(1), x.checked_add(1))
        } else {
            (x.checked_sub(1), x.checked_add(1), x.checked_sub(1))
        };
        add_error(&mut self.current, near, kernel.wide);
        add_error(&mut self.following, back, kernel.back);
        add_error(&mut self.following, Some(x), kernel.straight);
        add_error(&mut self.following, ahead, kernel.narrow);
    }

    /// Rolls to the next row, discarding the exhausted buffer.
    fn advance_row(&mut self) {
        core::mem::swap(&mut self.current, &mut self.following);
        self.following.fill(0_i32);
    }
}

/// Removes and returns the stored error at `x`, zeros when out of range.
fn take_error(row: &mut [i32], x: usize) -> [i32; CHANNELS] {
    let mut error = [0_i32; CHANNELS];
    let Some(slots) = row_slots_mut(row, x) else {
        return error;
    };
    for (slot, value) in slots.iter_mut().zip(error.iter_mut()) {
        *value = *slot;
        *slot = 0_i32;
    }
    error
}

/// Saturating-adds `delta` to the stored error at `x`, if it exists.
fn add_error(row: &mut [i32], x: Option<usize>, delta: [i32; CHANNELS]) {
    let Some(position) = x else {
        return;
    };
    let Some(slots) = row_slots_mut(row, position) else {
        return;
    };
    for (slot, value) in slots.iter_mut().zip(delta) {
        *slot = slot.saturating_add(value);
    }
}

/// The three error slots of pixel `x`, `None` when out of bounds.
fn row_slots_mut(row: &mut [i32], x: usize) -> Option<&mut [i32]> {
    let start = x.checked_mul(CHANNELS)?;
    let end = start.checked_add(CHANNELS)?;
    row.get_mut(start..end)
}

/// Source colour plus stored error, clamped into `0..=255`.
fn adjust_pixel(raw: [u8; CHANNELS], error: [i32; CHANNELS]) -> [u8; CHANNELS] {
    let [red, green, blue] = raw;
    let [red_error, green_error, blue_error] = error;
    [
        clamp_u8(i32::from(red).saturating_add(red_error)),
        clamp_u8(i32::from(green).saturating_add(green_error)),
        clamp_u8(i32::from(blue).saturating_add(blue_error)),
    ]
}

/// The running state of one `Floyd-Steinberg` diffusion.
///
/// The scan is serpentine, but the output is written at each pixel's own
/// position, so `indices` and `opaque` stay in row-major raster order.
struct Diffuser<'pal> {
    /// Palette to match against.
    palette: &'pal Palette,
    /// Transparency and reserved-index policy.
    options: &'pal QuantizeOptions,
    /// Current and next row error buffers.
    errors: ErrorRows,
    /// Output indices, in raster order.
    indices: Vec<u8>,
    /// Output opacity flags, in raster order.
    opaque: Vec<bool>,
    /// `indices` offset of the first pixel of the row being scanned.
    row_start: usize,
}

impl Diffuser<'_> {
    /// Scans one row forward (even) or backward (odd), serpentine style.
    fn add_row(&mut self, row: &[u8], width: usize, forward: bool) {
        for step in 0..width {
            let position = scan_column(width, step, forward);
            self.add_pixel(row, position, forward);
        }
    }

    /// Quantizes one pixel and spreads its error to the open neighbours.
    fn add_pixel(&mut self, row: &[u8], x: usize, forward: bool) {
        let Some(pixel) = row.chunks_exact(BYTES_PER_PIXEL).nth(x).map(to_pixel) else {
            return;
        };
        let [red, green, blue, alpha] = pixel;
        let raw = [red, green, blue];
        let output = self.row_start.saturating_add(x);
        if is_transparent(raw, alpha, self.options.transparency) {
            self.errors.drop_current(x);
            let index = nearest_index(raw, self.palette, &self.options.reserved);
            self.set_pixel(output, index, false);
            return;
        }
        let adjusted = adjust_pixel(raw, self.errors.take_current(x));
        let index = nearest_index(adjusted, self.palette, &self.options.reserved);
        let chosen = self.palette.color(usize::from(index));
        self.errors
            .spread(x, forward, quantization_error(adjusted, chosen));
        self.set_pixel(output, index, true);
    }

    /// Stores one pixel's result at its raster position `output`.
    fn set_pixel(&mut self, output: usize, index: u8, visible: bool) {
        if let Some(slot) = self.indices.get_mut(output) {
            *slot = index;
        }
        if let Some(slot) = self.opaque.get_mut(output) {
            *slot = visible;
        }
    }
}

/// The scanned column of `step` within a `width`-wide serpentine row.
const fn scan_column(width: usize, step: usize, forward: bool) -> usize {
    if forward {
        step
    } else {
        width.saturating_sub(1).saturating_sub(step)
    }
}

/// Serpentine `Floyd-Steinberg` error diffusion into `palette`.
fn floyd_steinberg(img: &RgbaImage, palette: &Palette, options: &QuantizeOptions) -> IndexedImage {
    let width = usize::try_from(img.width).unwrap_or(1);
    let row_bytes = width.saturating_mul(BYTES_PER_PIXEL);
    let count = img.pixels.chunks_exact(BYTES_PER_PIXEL).len();
    let mut diffuser = Diffuser {
        palette,
        options,
        errors: ErrorRows::new(width),
        indices: vec![0_u8; count],
        opaque: vec![false; count],
        row_start: 0,
    };
    for (row, pixels) in img.pixels.chunks_exact(row_bytes).enumerate() {
        diffuser.row_start = row.saturating_mul(width);
        let forward = row.checked_rem(2).unwrap_or(0) == 0;
        diffuser.add_row(pixels, width, forward);
        diffuser.errors.advance_row();
    }
    IndexedImage {
        width: img.width,
        height: img.height,
        indices: diffuser.indices,
        opaque: diffuser.opaque,
    }
}

/// The chosen-minus-palette colour difference, saturating on gaps.
fn quantization_error(adjusted: [u8; CHANNELS], color: Option<[u8; CHANNELS]>) -> [i32; CHANNELS] {
    let [red, green, blue] = color.unwrap_or([0_u8; CHANNELS]);
    let [ared, agreen, ablue] = adjusted;
    [
        i32::from(ared).saturating_sub(i32::from(red)),
        i32::from(agreen).saturating_sub(i32::from(green)),
        i32::from(ablue).saturating_sub(i32::from(blue)),
    ]
}

/// `Bayer` threshold offset for one pixel, in channel units.
fn bayer_bias(x: usize, y: usize) -> i32 {
    let row = BAYER
        .get(y.checked_rem(MATRIX_SIDE).unwrap_or(0))
        .unwrap_or(&[0_u8; MATRIX_SIDE]);
    let cell = row
        .get(x.checked_rem(MATRIX_SIDE).unwrap_or(0))
        .unwrap_or(&0_u8);
    i32::from(*cell)
        .saturating_sub(BAYER_CENTER)
        .saturating_mul(BAYER_STEP)
}

/// Fixed-pattern `Bayer` dithering into `palette`.
fn ordered(img: &RgbaImage, palette: &Palette, options: &QuantizeOptions) -> IndexedImage {
    let width = usize::try_from(img.width).unwrap_or(1);
    let count = img.pixels.chunks_exact(BYTES_PER_PIXEL).len();
    let mut indices = Vec::with_capacity(count);
    let mut opaque = Vec::with_capacity(count);
    for (position, chunk) in img.pixels.chunks_exact(BYTES_PER_PIXEL).enumerate() {
        let [red, green, blue, alpha] = to_pixel(chunk);
        let raw = [red, green, blue];
        if is_transparent(raw, alpha, options.transparency) {
            indices.push(nearest_index(raw, palette, &options.reserved));
            opaque.push(false);
            continue;
        }
        let x = position.checked_rem(width).unwrap_or(0);
        let y = position.checked_div(width).unwrap_or(0);
        let bias = bayer_bias(x, y);
        let offset = [bias, bias, bias];
        indices.push(nearest_index(
            adjust_pixel(raw, offset),
            palette,
            &options.reserved,
        ));
        opaque.push(true);
    }
    IndexedImage {
        width: img.width,
        height: img.height,
        indices,
        opaque,
    }
}

#[cfg(test)]
mod tests {
    use super::{Dither, quantize_dithered};
    use crate::gfx::image::{IndexedImage, RgbaImage};
    use crate::gfx::palette::Palette;
    use crate::gfx::quantize::{QuantizeOptions, Transparency};

    /// Black at index `0`, white at index `1`, duplicate black elsewhere.
    fn black_white() -> Palette {
        let mut colors = [[0_u8; 3]; 256];
        colors[1] = [255, 255, 255];
        Palette::from(colors)
    }

    /// Options that reserve every palette entry but black and white.
    fn two_colours() -> QuantizeOptions {
        QuantizeOptions {
            transparency: Transparency::default(),
            reserved: (2_u8..=255).collect(),
        }
    }

    /// A `size x size` mid-grey (128) opaque image.
    fn grey_square(size: u32) -> RgbaImage {
        let pixels: Vec<u8> = (0..size.saturating_mul(size))
            .flat_map(|_| [128_u8, 128, 128, 255])
            .collect();
        RgbaImage::new(size, size, pixels).expect("square is valid")
    }

    /// A 1-row image of the given opaque mid-grey pixels.
    fn grey_row(count: usize) -> RgbaImage {
        let pixels: Vec<u8> = (0..count).flat_map(|_| [128_u8, 128, 128, 255]).collect();
        let width = u32::try_from(count).expect("small");
        RgbaImage::new(width, 1, pixels).expect("row is valid")
    }

    /// The source pixel at raster position `p` of a `left_weighted`
    /// picture: `left` in the left `half` of a `2 * half`-wide row, black
    /// in the right.
    fn weighted_pixel(p: usize, row_width: usize, half: usize, left: u8) -> [u8; 4] {
        let x = p.checked_rem(row_width).expect("test rows");
        let shade = if x < half { left } else { 0_u8 };
        [shade, shade, shade, 255_u8]
    }

    /// A `2 * half`-wide, 4-row opaque picture: the left half at the
    /// uniform grey `left`, the right half pure black.
    fn left_weighted(half: usize, left: u8) -> RgbaImage {
        let row_width = half.saturating_mul(2);
        let pixels: Vec<u8> = (0..row_width.saturating_mul(4))
            .flat_map(|p| weighted_pixel(p, row_width, half, left))
            .collect();
        let width = u32::try_from(row_width).expect("small");
        RgbaImage::new(width, 4, pixels).expect("picture is valid")
    }

    /// The number of white-index pixels in `part` of an output row.
    fn count_whites(part: &[u8]) -> usize {
        part.iter().fold(0_usize, |total, &value| {
            total.saturating_add(usize::from(value == 1))
        })
    }

    /// The white-pixel counts of the two halves of one output row.
    fn half_counts(row: &[u8]) -> (usize, usize) {
        let mid = row.len().checked_div(2).expect("a whole half");
        let (left, right) = row.split_at(mid);
        (count_whites(left), count_whites(right))
    }

    /// The raster rows whose left half is not brighter than the right.
    fn flipped_rows(indices: &[u8], row_width: usize) -> Vec<usize> {
        indices
            .chunks_exact(row_width)
            .enumerate()
            .filter_map(|(row, line)| {
                let (left, right) = half_counts(line);
                (left <= right).then_some(row)
            })
            .collect()
    }

    /// One mode's placement check: a fully opaque output whose every row
    /// equals `expected`, in raster order.
    fn assert_mode_places_each_pixel(indexed: &IndexedImage, mode: Dither, expected: &[u8]) {
        assert!(
            indexed.opaque.iter().all(|visible| *visible),
            "opaque source with {mode:?}"
        );
        assert!(
            indexed
                .indices
                .chunks_exact(expected.len())
                .all(|row| row == expected),
            "every row must sit where the source is with {mode:?}"
        );
    }

    #[test]
    fn exact_palette_pixels_land_at_their_own_positions_in_every_mode() {
        let img = left_weighted(4, 255);
        let expected: [u8; 8] = [1, 1, 1, 1, 0, 0, 0, 0];
        for mode in [Dither::None, Dither::FloydSteinberg, Dither::Ordered] {
            let indexed = quantize_dithered(&img, &black_white(), &two_colours(), mode);
            assert_mode_places_each_pixel(&indexed, mode, &expected);
        }
    }

    #[test]
    fn every_dither_mode_keeps_a_left_weighted_picture_left_weighted() {
        let img = left_weighted(8, 128);
        for mode in [Dither::None, Dither::FloydSteinberg, Dither::Ordered] {
            let indexed = quantize_dithered(&img, &black_white(), &two_colours(), mode);
            assert_eq!(
                flipped_rows(&indexed.indices, 16),
                Vec::<usize>::new(),
                "no row loses its left weighting with {mode:?}: {:?}",
                indexed.indices
            );
        }
    }

    #[test]
    fn floyd_steinberg_diffuses_mid_grey_to_a_balanced_black_and_white() {
        let indexed = quantize_dithered(
            &grey_square(32),
            &black_white(),
            &two_colours(),
            Dither::FloydSteinberg,
        );
        assert_eq!(indexed.indices.len(), 1_024, "one index per pixel");
        assert!(
            indexed.indices.iter().all(|&value| value <= 1),
            "reserved indices are never used"
        );
        let black = indexed.indices.iter().fold(0_usize, |total, &value| {
            total.saturating_add(usize::from(value == 0))
        });
        let percent = black
            .saturating_mul(100)
            .checked_div(1_024)
            .expect("non-zero");
        assert!(
            (45..=55).contains(&percent),
            "black share is {percent}%, expected 45..=55"
        );
    }

    #[test]
    fn floyd_steinberg_keeps_transparent_pixels_out_of_the_error_flow() {
        let pixels = vec![
            128, 128, 128, 255, // rounds to white and pushes error east ...
            128, 128, 128, 0, // ... which this transparent pixel swallows ...
            128, 128, 128, 255, // ... so it rounds like the first pixel
        ];
        let img = RgbaImage::new(3, 1, pixels).expect("3x1 is valid");
        let indexed =
            quantize_dithered(&img, &black_white(), &two_colours(), Dither::FloydSteinberg);
        assert_eq!(indexed.opaque, vec![true, false, true], "transparency kept");
        assert_eq!(
            indexed.indices,
            vec![1, 1, 1],
            "with the error dropped, every opaque pixel stays white"
        );
        let solid = QuantizeOptions {
            transparency: Transparency::None,
            reserved: two_colours().reserved,
        };
        let diffused =
            quantize_dithered(&grey_row(3), &black_white(), &solid, Dither::FloydSteinberg);
        assert_eq!(
            diffused.opaque,
            vec![true, true, true],
            "nothing transparent"
        );
        assert_eq!(
            diffused.indices,
            vec![1, 0, 1],
            "the surviving error darkens the middle pixel"
        );
    }

    #[test]
    fn ordered_is_deterministic_and_differs_from_none() {
        let img = grey_square(8);
        let pal = black_white();
        let options = two_colours();
        let first = quantize_dithered(&img, &pal, &options, Dither::Ordered);
        let second = quantize_dithered(&img, &pal, &options, Dither::Ordered);
        assert_eq!(first, second, "no error state, identical results");
        let plain = quantize_dithered(&img, &pal, &options, Dither::None);
        assert_ne!(
            first.indices, plain.indices,
            "the Bayer offset must break the flat mid-grey tie"
        );
        assert!(
            first.indices.contains(&0) && first.indices.contains(&1),
            "ordered dither mixes black and white"
        );
        assert_eq!(first.width, 8, "shape kept");
    }

    #[test]
    fn none_reproduces_the_plain_quantizer_exactly() {
        let img = grey_square(4);
        let pal = black_white();
        let options = two_colours();
        let dithered = quantize_dithered(&img, &pal, &options, Dither::None);
        let plain = crate::gfx::quantize::quantize(&img, &pal, &options);
        assert_eq!(dithered, plain, "None is quantize");
    }

    #[test]
    fn dither_modes_deserialize_from_snake_case() {
        let text = r#"["none", "floyd_steinberg", "ordered"]"#;
        let modes: Vec<Dither> = serde_json::from_str(text).expect("documented names");
        assert_eq!(
            modes,
            vec![Dither::None, Dither::FloydSteinberg, Dither::Ordered],
            "one name per variant"
        );
    }

    #[test]
    fn dither_modes_serialize_back_to_their_names() {
        let modes: Vec<Dither> = vec![Dither::None, Dither::FloydSteinberg, Dither::Ordered];
        let text = serde_json::to_string(&modes).expect("the modes serialize");
        assert_eq!(
            text, r#"["none","floyd_steinberg","ordered"]"#,
            "a result echoes the word the argument took"
        );
    }
}
