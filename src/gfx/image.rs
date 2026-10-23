//! Owned image buffers: direct `RGBA8` pixels and palette-indexed pixels.
//!
//! Both [`RgbaImage`] and [`IndexedImage`] guarantee their invariants
//! (element counts matching the shape, dimensions within
//! [`MAX_DIMENSION`]); the constructors validate
//! and every accessor stays in bounds.

use core::iter;

use crate::error::CyberError;
use crate::gfx::palette::Palette;
use crate::gfx::{
    BYTES_PER_PIXEL, MAX_DIMENSION, check_dimension, clamp_dimension, in_shape, pixel_count,
    pixel_index, rgba_byte_count, to_pixel,
};

/// An `RGBA8` image, row-major.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RgbaImage {
    /// Pixels per row, `1..=8192`.
    pub width: u32,
    /// Rows, `1..=8192`.
    pub height: u32,
    /// Four bytes per pixel (red, green, blue, alpha), row-major.
    pub pixels: Vec<u8>,
}

impl RgbaImage {
    /// Creates an image from raw `RGBA8` bytes.
    ///
    /// # Errors
    ///
    /// [`CyberError::InvalidArgument`] when a dimension is outside
    /// `1..=MAX_DIMENSION` or `pixels` does not hold exactly
    /// `width * height * 4` bytes.
    #[inline]
    pub fn new(width: u32, height: u32, pixels: Vec<u8>) -> Result<Self, CyberError> {
        check_dimension("width", width)?;
        check_dimension("height", height)?;
        let expected = rgba_byte_count(width, height).ok_or_else(|| {
            CyberError::InvalidArgument(format!("image shape {width}x{height} does not fit"))
        })?;
        if pixels.len() != expected {
            return Err(CyberError::InvalidArgument(format!(
                "expected {expected} bytes for a {width}x{height} RGBA8 image, got {}",
                pixels.len()
            )));
        }
        Ok(Self {
            width,
            height,
            pixels,
        })
    }

    /// The pixel at `(x, y)`, or `None` when out of bounds. A column past
    /// the right edge is out of bounds: it never reads on into the next
    /// row.
    #[must_use]
    #[inline]
    pub fn get(&self, x: u32, y: u32) -> Option<[u8; 4]> {
        if !in_shape(self.width, self.height, x, y) {
            return None;
        }
        let index = pixel_index(self.width, x, y)?;
        let chunk = self.pixels.chunks_exact(BYTES_PER_PIXEL).nth(index)?;
        Some(to_pixel(chunk))
    }

    /// Writes `pixel` at `(x, y)`; returns `false` when out of bounds,
    /// including a column past the right edge.
    #[inline]
    pub fn set(&mut self, x: u32, y: u32, pixel: [u8; 4]) -> bool {
        if !in_shape(self.width, self.height, x, y) {
            return false;
        }
        let Some(index) = pixel_index(self.width, x, y) else {
            return false;
        };
        if let Some(chunk) = self.pixels.chunks_exact_mut(BYTES_PER_PIXEL).nth(index) {
            chunk.copy_from_slice(&pixel);
            return true;
        }
        false
    }
}

/// A palette-indexed image: one `PLAYPAL` index plus an opacity flag per
/// pixel, row-major.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IndexedImage {
    /// Pixels per row, `1..=8192`.
    pub width: u32,
    /// Rows, `1..=8192`.
    pub height: u32,
    /// One palette index per pixel, row-major.
    pub indices: Vec<u8>,
    /// Whether each pixel is visible; the index of an invisible pixel is
    /// unused by [`IndexedImage::to_rgba`].
    pub opaque: Vec<bool>,
}

impl IndexedImage {
    /// Creates an indexed image from per-pixel index and opacity vectors.
    ///
    /// # Errors
    ///
    /// [`CyberError::InvalidArgument`] when a dimension is outside
    /// `1..=MAX_DIMENSION` or either vector's length is not
    /// `width * height`.
    #[inline]
    pub fn new(
        width: u32,
        height: u32,
        indices: Vec<u8>,
        opaque: Vec<bool>,
    ) -> Result<Self, CyberError> {
        check_dimension("width", width)?;
        check_dimension("height", height)?;
        let expected = pixel_count(width, height).ok_or_else(|| {
            CyberError::InvalidArgument(format!("image shape {width}x{height} does not fit"))
        })?;
        check_count("indices", indices.len(), expected)?;
        check_count("opaque", opaque.len(), expected)?;
        Ok(Self {
            width,
            height,
            indices,
            opaque,
        })
    }

    /// The `(index, opaque)` pair at `(x, y)`, or `None` when out of
    /// bounds. A column past the right edge is out of bounds: it never
    /// reads on into the next row.
    #[must_use]
    #[inline]
    pub fn get(&self, x: u32, y: u32) -> Option<(u8, bool)> {
        if !in_shape(self.width, self.height, x, y) {
            return None;
        }
        let position = pixel_index(self.width, x, y)?;
        let index = *self.indices.get(position)?;
        let visible = *self.opaque.get(position)?;
        Some((index, visible))
    }

    /// Writes the `(index, opaque)` pair at `(x, y)`; `false` when out of
    /// bounds, including a column past the right edge.
    #[inline]
    pub fn set(&mut self, x: u32, y: u32, pixel: (u8, bool)) -> bool {
        if !in_shape(self.width, self.height, x, y) {
            return false;
        }
        let Some(position) = pixel_index(self.width, x, y) else {
            return false;
        };
        let (Some(slot), Some(visibility)) = (
            self.indices.get_mut(position),
            self.opaque.get_mut(position),
        ) else {
            return false;
        };
        *slot = pixel.0;
        *visibility = pixel.1;
        true
    }

    /// Expands the indices through `palette`; non-opaque pixels keep the
    /// palette colour but get alpha `0`.
    #[must_use]
    #[inline]
    pub fn to_rgba(&self, palette: &Palette) -> RgbaImage {
        let mut pixels = Vec::with_capacity(self.indices.len().saturating_mul(BYTES_PER_PIXEL));
        for (value, visible) in iter::zip(&self.indices, &self.opaque) {
            let [r, g, b] = palette
                .color(usize::from(*value))
                .unwrap_or([0_u8, 0_u8, 0_u8]);
            pixels.extend_from_slice(&[r, g, b, alpha_for(*visible)]);
        }
        RgbaImage {
            width: self.width,
            height: self.height,
            pixels,
        }
    }
}

/// Resizes with integer nearest-neighbour sampling, mapping destination
/// coordinate `d` to `d * source / target`.
///
/// Requested dimensions are saturated into `1..=MAX_DIMENSION`, so the
/// result is always a valid image.
#[must_use]
#[inline]
pub fn resize_nearest(img: &RgbaImage, width: u32, height: u32) -> RgbaImage {
    let target_width = clamp_dimension(width);
    let target_height = clamp_dimension(height);
    let mut pixels = Vec::with_capacity(rgba_byte_count(target_width, target_height).unwrap_or(0));
    for row in 0..target_height {
        let source = source_row(img, row, target_height);
        for column in 0..target_width {
            pixels.extend_from_slice(&nearest_pixel(source, img.width, column, target_width));
        }
    }
    RgbaImage {
        width: target_width,
        height: target_height,
        pixels,
    }
}

/// The target dimensions of an integer scale: `factor.max(1)` times each
/// source edge, saturating to `MAX_DIMENSION` on overflow or excess.
fn scaled_dimensions(width: u32, height: u32, factor: u32) -> (u32, u32) {
    let multiplier = factor.max(1);
    let target_width = width
        .checked_mul(multiplier)
        .map_or(MAX_DIMENSION, clamp_dimension);
    let target_height = height
        .checked_mul(multiplier)
        .map_or(MAX_DIMENSION, clamp_dimension);
    (target_width, target_height)
}

/// Upscales by whole-pixel replication.
///
/// A factor below `1` becomes `1`; the resulting dimensions saturate to
/// `MAX_DIMENSION` rather than overflow.
#[must_use]
#[inline]
pub fn scale_integer(img: &RgbaImage, factor: u32) -> RgbaImage {
    let (width, height) = scaled_dimensions(img.width, img.height, factor);
    resize_nearest(img, width, height)
}

/// Mirrors an indexed image left-to-right, for mirrored sprite rotations.
#[must_use]
#[inline]
pub fn flip_horizontal(img: &IndexedImage) -> IndexedImage {
    let row_len = usize::try_from(img.width).unwrap_or(1);
    IndexedImage {
        width: img.width,
        height: img.height,
        indices: img
            .indices
            .chunks_exact(row_len)
            .flat_map(|row| row.iter().rev().copied())
            .collect(),
        opaque: img
            .opaque
            .chunks_exact(row_len)
            .flat_map(|row| row.iter().rev().copied())
            .collect(),
    }
}

/// The source row bytes for destination row `dst_y` of a resize, or `None`
/// when the mapping cannot be computed.
fn source_row(img: &RgbaImage, dst_y: u32, dst_height: u32) -> Option<&[u8]> {
    let src_y = axis_map(dst_y, img.height, dst_height)?;
    let row_bytes = pixel_count(img.width, 1)?.checked_mul(BYTES_PER_PIXEL)?;
    let start = src_y.checked_mul(row_bytes)?;
    img.pixels.get(start..start.checked_add(row_bytes)?)
}

/// Integer nearest-neighbour axis mapping: `dst * src_len / dst_len`.
fn axis_map(dst: u32, src_len: u32, dst_len: u32) -> Option<usize> {
    let position = usize::try_from(dst).ok()?;
    let source = usize::try_from(src_len).ok()?;
    let target = usize::try_from(dst_len).ok()?;
    position.checked_mul(source)?.checked_div(target)
}

/// The pixel at destination column `dst_x` of the selected source row.
fn nearest_pixel(row: Option<&[u8]>, src_width: u32, dst_x: u32, dst_width: u32) -> [u8; 4] {
    let transparent = [0_u8; BYTES_PER_PIXEL];
    let Some(selected) = row else {
        return transparent;
    };
    let Some(src_x) = axis_map(dst_x, src_width, dst_width) else {
        return transparent;
    };
    let Some(chunk) = selected.chunks_exact(BYTES_PER_PIXEL).nth(src_x) else {
        return transparent;
    };
    to_pixel(chunk)
}

/// The `RGBA8` alpha value an opacity flag stands for.
const fn alpha_for(visible: bool) -> u8 {
    if visible { 255_u8 } else { 0_u8 }
}

/// Checks one per-pixel vector's length against the pixel count.
fn check_count(name: &str, len: usize, expected: usize) -> Result<(), CyberError> {
    if len == expected {
        Ok(())
    } else {
        Err(CyberError::InvalidArgument(format!(
            "expected {expected} {name} entries for the image shape, got {len}"
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::{
        IndexedImage, MAX_DIMENSION, RgbaImage, flip_horizontal, resize_nearest, scale_integer,
        scaled_dimensions,
    };
    use crate::error::CyberError;
    use crate::gfx::palette::Palette;

    /// A 2x1 image: red on the left, blue on the right.
    fn sample() -> RgbaImage {
        RgbaImage::new(2, 1, vec![255, 0, 0, 255, 0, 0, 255, 255]).expect("2x1 RGBA8 is valid")
    }

    #[test]
    fn rgba_constructor_accepts_the_exact_byte_count() {
        let img = sample();
        assert_eq!(img.width, 2, "width kept");
        assert_eq!(img.height, 1, "height kept");
        assert_eq!(img.pixels.len(), 8, "pixel bytes kept");
    }

    #[test]
    fn rgba_constructor_rejects_bad_dimensions_and_lengths() {
        let zero = RgbaImage::new(0, 1, Vec::new()).expect_err("zero width must fail");
        assert_eq!(zero.code(), "invalid_argument", "zero width code");
        let huge = RgbaImage::new(8_193, 1, vec![0; 0x8004])
            .expect_err("width above MAX_DIMENSION must fail");
        assert_eq!(huge.code(), "invalid_argument", "oversized width code");
        let short = RgbaImage::new(2, 1, vec![0; 7]).expect_err("short pixel buffer must fail");
        assert_eq!(short.code(), "invalid_argument", "short buffer code");
    }

    #[test]
    fn rgba_get_and_set_stay_in_bounds() {
        let mut img = sample();
        assert_eq!(img.get(0, 0), Some([255, 0, 0, 255]), "first pixel");
        assert_eq!(img.get(1, 0), Some([0, 0, 255, 255]), "second pixel");
        assert_eq!(img.get(2, 0), None, "column beyond width");
        assert_eq!(img.get(0, 1), None, "row beyond height");
        assert!(img.set(1, 0, [7, 8, 9, 10]), "in-bounds set");
        assert_eq!(img.get(1, 0), Some([7, 8, 9, 10]), "set took effect");
        assert!(
            !img.set(5, 0, [0, 0, 0, 0]),
            "out-of-bounds set reports false"
        );
    }

    /// A 2x2 image: each row red then blue, so a column past the right
    /// edge would land on the next row's red unless the shape is checked.
    fn square() -> RgbaImage {
        RgbaImage::new(
            2,
            2,
            vec![
                255, 0, 0, 255, 0, 0, 255, 255, // row 0
                255, 0, 0, 255, 0, 0, 255, 255, // row 1
            ],
        )
        .expect("2x2 RGBA8 is valid")
    }

    #[test]
    fn rgba_accessors_do_not_wrap_a_column_into_the_next_row() {
        let mut img = square();
        assert_eq!(img.get(2, 0), None, "the column after the first row");
        assert_eq!(img.get(2, 1), None, "the column after the last row");
        assert_eq!(img.get(3, 0), None, "two columns past the edge");
        assert_eq!(img.get(0, 2), None, "the row after the picture");
        assert!(
            !img.set(2, 0, [9, 9, 9, 9]),
            "a wrapping write is refused, not painted on row 1"
        );
        assert_eq!(img.get(0, 1), Some([255, 0, 0, 255]), "row 1 is untouched");
        assert_eq!(img.get(1, 1), Some([0, 0, 255, 255]), "row 1 is untouched");
    }

    #[test]
    fn indexed_accessors_do_not_wrap_a_column_into_the_next_row() {
        let mut img = IndexedImage::new(2, 2, vec![3, 4, 5, 6], vec![true, false, true, true])
            .expect("2x2 indexed image is valid");
        assert_eq!(img.get(0, 1), Some((5, true)), "in-bounds read");
        assert_eq!(img.get(2, 0), None, "the column after the first row");
        assert_eq!(img.get(4, 0), None, "two columns past the edge");
        assert_eq!(img.get(0, 2), None, "the row after the picture");
        assert!(
            !img.set(2, 0, (0, false)),
            "a wrapping write is refused, not painted on row 1"
        );
        assert_eq!(img.get(0, 1), Some((5, true)), "row 1 is untouched");
    }

    #[test]
    fn indexed_constructor_validates_counts() {
        let good = IndexedImage::new(2, 1, vec![3, 4], vec![true, false]);
        assert!(good.is_ok(), "exact counts are accepted");
        let bad_indices = IndexedImage::new(2, 1, vec![3], vec![true, false])
            .expect_err("too few indices must fail");
        assert_eq!(bad_indices.code(), "invalid_argument", "short indices code");
        let bad_opaque = IndexedImage::new(2, 1, vec![3, 4], vec![true])
            .expect_err("too few opacity flags must fail");
        assert_eq!(bad_opaque.code(), "invalid_argument", "short opaque code");
        let zero =
            IndexedImage::new(0, 2, Vec::new(), Vec::new()).expect_err("zero width must fail");
        assert_eq!(zero.code(), "invalid_argument", "zero width code");
    }

    #[test]
    fn indexed_get_and_set_stay_in_bounds() {
        let mut img = IndexedImage::new(2, 1, vec![3, 4], vec![true, false])
            .expect("2x1 indexed image is valid");
        assert_eq!(img.get(0, 0), Some((3, true)), "first pixel");
        assert_eq!(img.get(1, 0), Some((4, false)), "second pixel");
        assert_eq!(img.get(2, 0), None, "out of bounds read");
        assert!(img.set(1, 0, (9, true)), "in-bounds set");
        assert_eq!(img.get(1, 0), Some((9, true)), "set took effect");
        assert!(
            !img.set(9, 9, (0, false)),
            "out-of-bounds set reports false"
        );
    }

    /// A palette with two colours: index 0 white, index 1 black.
    fn two_colour_palette() -> Palette {
        let mut colors = [[0_u8; 3]; 256];
        colors[0] = [255, 255, 255];
        Palette::from(colors)
    }

    #[test]
    fn to_rgba_maps_transparent_pixels_to_alpha_zero() {
        let img = IndexedImage::new(2, 1, vec![0, 1], vec![true, false])
            .expect("2x1 indexed image is valid");
        let rgba = img.to_rgba(&two_colour_palette());
        assert_eq!(rgba.width, 2, "width preserved");
        assert_eq!(
            rgba.pixels,
            vec![255, 255, 255, 255, 0, 0, 0, 0],
            "opaque pixel gets the palette colour, non-opaque gets alpha 0"
        );
    }

    #[test]
    fn resize_nearest_upsamples_and_downsamples() {
        let img = sample();
        let up = resize_nearest(&img, 4, 2);
        assert_eq!(
            up.pixels,
            vec![
                255, 0, 0, 255, 255, 0, 0, 255, 0, 0, 255, 255, 0, 0, 255, 255, 255, 0, 0, 255,
                255, 0, 0, 255, 0, 0, 255, 255, 0, 0, 255, 255,
            ],
            "each source pixel replicated 2x2"
        );
        let down = resize_nearest(&img, 1, 1);
        assert_eq!(
            down.pixels,
            vec![255, 0, 0, 255],
            "downsample keeps the top-left pixel"
        );
        let clamped = resize_nearest(&img, 0, 100_000);
        assert_eq!(
            (clamped.width, clamped.height),
            (1, 8_192),
            "dimensions saturate"
        );
    }

    #[test]
    fn scale_integer_replicates_whole_pixels() {
        let img = sample();
        let up = scale_integer(&img, 3);
        assert_eq!((up.width, up.height), (6, 3), "shape scaled by 3");
        assert_eq!(
            up.get(4, 2),
            Some([0, 0, 255, 255]),
            "bottom-right stays the second pixel"
        );
        let same = scale_integer(&img, 0);
        assert_eq!((same.width, same.height), (2, 1), "factor 0 behaves as 1");
    }

    #[test]
    fn scaled_dimensions_saturates_oversized_targets() {
        assert_eq!(
            scaled_dimensions(2, 1, 8_192),
            (8_192, 8_192),
            "oversized products clamp to MAX_DIMENSION"
        );
        assert_eq!(
            scaled_dimensions(2, 1, u32::MAX),
            (MAX_DIMENSION, MAX_DIMENSION),
            "overflowing products saturate to MAX_DIMENSION"
        );
    }

    #[test]
    fn flip_horizontal_mirrors_each_row() {
        let img = IndexedImage::new(
            3,
            2,
            vec![1, 2, 3, 4, 5, 6],
            vec![true, false, true, false, false, true],
        )
        .expect("3x2 indexed image is valid");
        let flipped = flip_horizontal(&img);
        assert_eq!(flipped.indices, vec![3, 2, 1, 6, 5, 4], "rows mirrored");
        assert_eq!(
            flipped.opaque,
            vec![true, false, true, true, false, false],
            "opacity flags mirror with the indices"
        );
    }

    #[test]
    fn to_rgba_error_paths_share_the_domain_type() {
        let err = RgbaImage::new(1, 1, Vec::new()).expect_err("empty buffer must fail");
        let CyberError::InvalidArgument(message) = err else {
            panic!("expected InvalidArgument, got {err:?}");
        };
        assert!(
            message.contains("expected 4 bytes"),
            "message names the expected size: {message}"
        );
    }
}
