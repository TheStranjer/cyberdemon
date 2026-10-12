//! Nearest-colour quantization of `RGBA` images into indexed images.
//!
//! Colour matching uses squared Euclidean distance in `i32` with ties won
//! by the lower palette index; per-colour answers are memoised in a
//! [`HashMap`] that is only ever looked up, never iterated, so the output
//! is independent of hashing order.

use std::collections::{HashMap, HashSet};

use crate::gfx::BYTES_PER_PIXEL;
use crate::gfx::image::{IndexedImage, RgbaImage};
use crate::gfx::palette::Palette;
use crate::gfx::to_pixel;

/// Alpha value below which a pixel becomes transparent by default.
pub const DEFAULT_ALPHA_THRESHOLD: u8 = 128;

/// How [`quantize`] decides which source pixels are transparent.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Transparency {
    /// Pixels whose alpha is below `threshold` are transparent.
    Alpha {
        /// The exclusive alpha cutoff.
        threshold: u8,
    },
    /// Pixels whose colour equals `rgb` exactly are transparent (commonly
    /// cyan for `Doom` sprite sources).
    ColorKey {
        /// The key colour.
        rgb: [u8; 3],
    },
    /// Every pixel is opaque.
    None,
}

impl Default for Transparency {
    /// [`Transparency::Alpha`] with [`DEFAULT_ALPHA_THRESHOLD`].
    #[inline]
    fn default() -> Self {
        Self::Alpha {
            threshold: DEFAULT_ALPHA_THRESHOLD,
        }
    }
}

/// Judgment parameters for [`quantize`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct QuantizeOptions {
    /// Which pixels count as transparent.
    pub transparency: Transparency,
    /// Palette indices that are never chosen (e.g. the colour an engine
    /// renders as fully transparent). If every index is reserved, index
    /// `0` is returned.
    pub reserved: Vec<u8>,
}

impl Default for QuantizeOptions {
    /// The alpha-threshold mode with no reserved indices.
    #[inline]
    fn default() -> Self {
        Self {
            transparency: Transparency::default(),
            reserved: Vec::new(),
        }
    }
}

/// Squared Euclidean distance between two colours in `i32`.
///
/// The true maximum (`3 * 255^2`) fits with room to spare; the saturating
/// operations exist only to satisfy the arithmetic lints.
fn squared_distance(a: [u8; 3], b: [u8; 3]) -> i32 {
    let mut sum = 0_i32;
    for (x, y) in a.iter().zip(b) {
        let delta = i32::from(*x).saturating_sub(i32::from(y));
        sum = sum.saturating_add(delta.saturating_mul(delta));
    }
    sum
}

/// Whether the pixel is transparent under `mode`.
fn is_transparent(rgb: [u8; 3], alpha: u8, mode: Transparency) -> bool {
    match mode {
        Transparency::Alpha { threshold } => alpha < threshold,
        Transparency::ColorKey { rgb: key } => rgb == key,
        Transparency::None => false,
    }
}

/// The closest non-reserved palette colour to `rgb`.
///
/// Distance is squared Euclidean; ties go to the lowest index. If every
/// index is reserved the answer falls back to `0`.
#[must_use]
#[inline]
pub fn nearest_index(rgb: [u8; 3], palette: &Palette, reserved: &[u8]) -> u8 {
    let best = palette
        .colors()
        .enumerate()
        .filter(|&(index, _)| !reserved.iter().any(|value| usize::from(*value) == index))
        .map(|(index, color)| (squared_distance(rgb, *color), index))
        .min();
    best.map_or(0_u8, |(_, index)| u8::try_from(index).unwrap_or(0_u8))
}

/// Quantizes `img` to `palette`.
///
/// Transparent pixels get an opacity flag of `false` but still receive a
/// best-effort index. The closest colour per distinct `RGB` triple is
/// computed once and memoised.
#[must_use]
#[inline]
pub fn quantize(img: &RgbaImage, palette: &Palette, options: &QuantizeOptions) -> IndexedImage {
    let mut cache: HashMap<[u8; 3], u8> = HashMap::new();
    let count = img.pixels.chunks_exact(BYTES_PER_PIXEL).len();
    let mut indices = Vec::with_capacity(count);
    let mut opaque = Vec::with_capacity(count);
    for [r, g, b, alpha] in img.pixels.chunks_exact(BYTES_PER_PIXEL).map(to_pixel) {
        let rgb = [r, g, b];
        opaque.push(!is_transparent(rgb, alpha, options.transparency));
        indices.push(nearest_cached(&mut cache, rgb, palette, &options.reserved));
    }
    IndexedImage {
        width: img.width,
        height: img.height,
        indices,
        opaque,
    }
}

/// Number of distinct colours among the opaque pixels, and the worst
/// quantization error (squared distance) over them. A fully transparent
/// image measures `(0, 0)`.
#[must_use]
#[inline]
pub fn quantize_stats(
    img: &RgbaImage,
    palette: &Palette,
    options: &QuantizeOptions,
) -> (usize, i32) {
    let mut cache: HashMap<[u8; 3], u8> = HashMap::new();
    let mut unique: HashSet<[u8; 3]> = HashSet::new();
    let mut max_error = 0_i32;
    for [r, g, b, alpha] in img.pixels.chunks_exact(BYTES_PER_PIXEL).map(to_pixel) {
        let rgb = [r, g, b];
        if is_transparent(rgb, alpha, options.transparency) {
            continue;
        }
        unique.insert(rgb);
        let index = nearest_cached(&mut cache, rgb, palette, &options.reserved);
        let color = palette.color(usize::from(index)).unwrap_or([0_u8; 3]);
        max_error = max_error.max(squared_distance(rgb, color));
    }
    (unique.len(), max_error)
}

/// Memoised [`nearest_index`] lookup against a shared cache.
fn nearest_cached(
    cache: &mut HashMap<[u8; 3], u8>,
    rgb: [u8; 3],
    palette: &Palette,
    reserved: &[u8],
) -> u8 {
    *cache
        .entry(rgb)
        .or_insert_with(|| nearest_index(rgb, palette, reserved))
}

#[cfg(test)]
mod tests {
    use super::{
        DEFAULT_ALPHA_THRESHOLD, QuantizeOptions, Transparency, nearest_index, quantize,
        quantize_stats,
    };
    use crate::gfx::image::{IndexedImage, RgbaImage};
    use crate::gfx::palette::Palette;

    /// A palette whose first four entries are black, white, red and cyan;
    /// every later entry repeats black.
    fn test_palette() -> Palette {
        let mut colors = [[0_u8; 3]; 256];
        colors[1] = [255, 255, 255];
        colors[2] = [255, 0, 0];
        colors[3] = [0, 255, 255];
        Palette::from(colors)
    }

    /// A 2x1 image of the two given pixels.
    fn two_pixels(first: [u8; 4], second: [u8; 4]) -> RgbaImage {
        let mut pixels = first.to_vec();
        pixels.extend_from_slice(&second);
        RgbaImage::new(2, 1, pixels).expect("two RGBA8 pixels fit a 2x1 image")
    }

    #[test]
    fn defaults_are_documented() {
        assert_eq!(DEFAULT_ALPHA_THRESHOLD, 128, "alpha cutoff default");
        assert_eq!(
            Transparency::default(),
            Transparency::Alpha { threshold: 128 },
            "default mode is the threshold"
        );
        assert_eq!(
            QuantizeOptions::default(),
            QuantizeOptions {
                transparency: Transparency::Alpha { threshold: 128 },
                reserved: Vec::new(),
            },
            "default options reserve nothing"
        );
    }

    #[test]
    fn exact_palette_colours_map_to_their_indices() {
        let pal = test_palette();
        assert_eq!(nearest_index([0, 0, 0], &pal, &[]), 0, "black");
        assert_eq!(nearest_index([255, 255, 255], &pal, &[]), 1, "white");
        assert_eq!(nearest_index([255, 0, 0], &pal, &[]), 2, "red");
        assert_eq!(nearest_index([0, 255, 255], &pal, &[]), 3, "cyan");
    }

    #[test]
    fn closest_colour_wins() {
        let pal = test_palette();
        assert_eq!(nearest_index([250, 5, 5], &pal, &[]), 2, "near red");
        assert_eq!(nearest_index([10, 10, 10], &pal, &[]), 0, "near black");
    }

    #[test]
    fn ties_go_to_the_lowest_index() {
        let mut colors = [[0_u8; 3]; 256];
        colors[0] = [30, 10, 10];
        colors[1] = [10, 10, 10];
        assert_eq!(
            nearest_index([20, 10, 10], &Palette::from(colors), &[]),
            0,
            "equidistant colours pick the first"
        );
        let mut swapped = [[0_u8; 3]; 256];
        swapped[0] = [10, 10, 10];
        swapped[1] = [30, 10, 10];
        assert_eq!(
            nearest_index([20, 10, 10], &Palette::from(swapped), &[]),
            0,
            "the lowest index wins regardless of colour order"
        );
    }

    #[test]
    fn reserved_indices_are_skipped() {
        let pal = test_palette();
        assert_eq!(
            nearest_index([0, 0, 0], &pal, &[0]),
            4,
            "the first entry is reserved, the duplicate black at 4 is next"
        );
        assert_eq!(
            nearest_index([255, 255, 255], &pal, &[1]),
            3,
            "white is reserved; cyan differs from white in one channel only"
        );
    }

    #[test]
    fn fully_reserved_palette_falls_back_to_zero() {
        let all: Vec<u8> = (0..=255_u8).collect();
        assert_eq!(
            nearest_index([9, 9, 9], &test_palette(), &all),
            0,
            "documented fallback"
        );
    }

    #[test]
    fn quantize_marks_alpha_transparency_at_the_threshold() {
        let img = two_pixels([255, 0, 0, 127], [255, 0, 0, 128]);
        let indexed = quantize(&img, &test_palette(), &QuantizeOptions::default());
        assert_eq!(
            indexed.opaque,
            vec![false, true],
            "127 is under, 128 is not"
        );
        assert_eq!(indexed.indices, vec![2, 2], "both keep the red index");
        assert_eq!((indexed.width, indexed.height), (2, 1), "shape preserved");
    }

    #[test]
    fn quantize_supports_colour_key_and_no_transparency() {
        let img = two_pixels([0, 255, 255, 255], [255, 255, 255, 3]);
        let by_key = QuantizeOptions {
            transparency: Transparency::ColorKey { rgb: [0, 255, 255] },
            reserved: Vec::new(),
        };
        let indexed = quantize(&img, &test_palette(), &by_key);
        assert_eq!(
            indexed.opaque,
            vec![false, true],
            "only the key colour drops out"
        );
        assert_eq!(indexed.indices, vec![3, 1], "cyan and white");
        let opaque_all = QuantizeOptions {
            transparency: Transparency::None,
            reserved: Vec::new(),
        };
        let solid = quantize(&img, &test_palette(), &opaque_all);
        assert_eq!(solid.opaque, vec![true, true], "None keeps everything");
    }

    #[test]
    fn quantize_reuses_cached_lookups_for_repeated_colours() {
        let pixels = vec![10_u8, 20, 30, 255, 10, 20, 30, 0, 9, 9, 9, 255];
        let img = RgbaImage::new(3, 1, pixels).expect("three RGBA8 pixels fit");
        let indexed = quantize(&img, &test_palette(), &QuantizeOptions::default());
        assert_eq!(indexed.indices.len(), 3, "one index per pixel");
        assert_eq!(
            indexed.indices[0], indexed.indices[1],
            "same colour, same index"
        );
        let expected = nearest_index([10, 20, 30], &test_palette(), &[]);
        assert_eq!(
            indexed.indices[0], expected,
            "cached answer matches a direct lookup"
        );
    }

    #[test]
    fn quantize_round_trips_through_to_rgba() {
        let pal = test_palette();
        let img = two_pixels([255, 255, 255, 255], [0, 0, 0, 10]);
        let indexed = quantize(&img, &pal, &QuantizeOptions::default());
        let expected =
            IndexedImage::new(2, 1, vec![1, 0], vec![true, false]).expect("2x1 indexed image");
        assert_eq!(indexed, expected, "exact colours and flags");
        let rgba = indexed.to_rgba(&pal);
        assert_eq!(
            rgba.pixels,
            vec![255, 255, 255, 255, 0, 0, 0, 0],
            "transparent pixel"
        );
    }

    #[test]
    fn quantize_stats_counts_unique_colours_and_error() {
        let pal = test_palette();
        let img = two_pixels([250, 5, 5, 255], [250, 5, 5, 0]);
        let (unique, error) = quantize_stats(&img, &pal, &QuantizeOptions::default());
        assert_eq!(unique, 1, "the transparent pixel is not counted");
        assert_eq!(error, 75_i32, "squared distance of (250,5,5) to red");
        let exact = two_pixels([255, 0, 0, 255], [0, 255, 255, 255]);
        assert_eq!(
            quantize_stats(&exact, &pal, &QuantizeOptions::default()),
            (2, 0_i32),
            "palette colours have zero error"
        );
        let gone = two_pixels([1, 2, 3, 0], [4, 5, 6, 7]);
        assert_eq!(
            quantize_stats(&gone, &pal, &QuantizeOptions::default()),
            (0, 0_i32),
            "a fully transparent image measures zero"
        );
    }
}
