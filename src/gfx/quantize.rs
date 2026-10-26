//! Nearest-colour quantization of `RGBA` images into indexed images.
//!
//! Colour matching uses squared Euclidean distance in `i32` with ties won
//! by the lower palette index; per-colour answers are memoised in a
//! [`HashMap`] that is only ever looked up, never iterated, so the output
//! is independent of hashing order.
//!
//! One pass over the pixels folds the picture ([`quantize`]), the measures
//! ([`quantize_stats`]), or both at once ([`quantize_measured`]), so a
//! caller that stores a picture and reports on it searches the palette once
//! per distinct colour and reads its numbers off the indices it stored.

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
pub(crate) fn is_transparent(rgb: [u8; 3], alpha: u8, mode: Transparency) -> bool {
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

/// What one [`Walk`] folds out of the pixels it sees.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Gather {
    /// The indexed picture, nothing else.
    Picture,
    /// The measures of the opaque colours, nothing else.
    Measures,
    /// Both, from the one walk.
    Both,
}

impl Gather {
    /// Whether this walk stores an index and an opacity flag per pixel.
    const fn pictures(self) -> bool {
        matches!(self, Self::Picture | Self::Both)
    }

    /// Whether this walk folds the measures of the opaque colours.
    const fn measures(self) -> bool {
        matches!(self, Self::Measures | Self::Both)
    }
}

/// The answer of one quantization walk: the indexed picture beside the
/// colour measures taken from the very indices it stored.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Quantized {
    /// The palette indices and opacity flags of the source picture.
    pub image: IndexedImage,
    /// Distinct colours among the opaque pixels.
    pub colors_used: usize,
    /// Worst squared `RGB` distance from an opaque colour to its palette
    /// entry.
    pub max_color_error: i32,
}

/// The state of one walk of a picture against a palette: every distinct
/// `RGB` triple is searched once and the picture, the measures, or both are
/// folded off that single search.
struct Walk<'pal> {
    /// Palette to match against.
    palette: &'pal Palette,
    /// Transparency and reserved-index policy of the call.
    options: &'pal QuantizeOptions,
    /// What the walk folds.
    gather: Gather,
    /// `RGB` to its nearest index: every colour searched at most once.
    cache: HashMap<[u8; 3], u8>,
    /// Distinct opaque colours seen so far.
    unique: HashSet<[u8; 3]>,
    /// Worst squared distance from an opaque colour to its palette entry.
    max_error: i32,
    /// Indices produced so far, row-major.
    indices: Vec<u8>,
    /// Opacity flags produced so far, row-major.
    opaque: Vec<bool>,
}

impl<'pal> Walk<'pal> {
    /// A walk over `img` against `palette` that folds `gather`, sized for
    /// the picture when it stores one.
    fn new(
        img: &RgbaImage,
        palette: &'pal Palette,
        options: &'pal QuantizeOptions,
        gather: Gather,
    ) -> Self {
        let count = if gather.pictures() {
            img.pixels.chunks_exact(BYTES_PER_PIXEL).len()
        } else {
            0_usize
        };
        Self {
            palette,
            options,
            gather,
            cache: HashMap::new(),
            unique: HashSet::new(),
            max_error: 0_i32,
            indices: Vec::with_capacity(count),
            opaque: Vec::with_capacity(count),
        }
    }

    /// Records one source pixel: its nearest index where the walk stores a
    /// picture, and its colour's distance where the walk measures, opaque
    /// pixels only.
    fn add(&mut self, [red, green, blue, alpha]: [u8; 4]) {
        let rgb = [red, green, blue];
        let visible = !is_transparent(rgb, alpha, self.options.transparency);
        if !visible && !self.gather.pictures() {
            return;
        }
        let index = nearest_cached(&mut self.cache, rgb, self.palette, &self.options.reserved);
        if self.gather.pictures() {
            self.indices.push(index);
            self.opaque.push(visible);
        }
        if visible && self.gather.measures() && self.unique.insert(rgb) {
            let color = self.palette.color(usize::from(index)).unwrap_or([0_u8; 3]);
            self.max_error = self.max_error.max(squared_distance(rgb, color));
        }
    }

    /// The picture's dimensions with everything the walk folded.
    fn finish(self, img: &RgbaImage) -> Quantized {
        Quantized {
            image: IndexedImage {
                width: img.width,
                height: img.height,
                indices: self.indices,
                opaque: self.opaque,
            },
            colors_used: self.unique.len(),
            max_color_error: self.max_error,
        }
    }
}

/// Walks every pixel of `img` once against `palette`, folding `gather`.
fn walk(
    img: &RgbaImage,
    palette: &Palette,
    options: &QuantizeOptions,
    gather: Gather,
) -> Quantized {
    let mut walker = Walk::new(img, palette, options, gather);
    for pixel in img.pixels.chunks_exact(BYTES_PER_PIXEL).map(to_pixel) {
        walker.add(pixel);
    }
    walker.finish(img)
}

/// Quantizes `img` to `palette`.
///
/// Transparent pixels get an opacity flag of `false` but still receive a
/// best-effort index. The closest colour per distinct `RGB` triple is
/// computed once and memoised.
#[must_use]
#[inline]
pub fn quantize(img: &RgbaImage, palette: &Palette, options: &QuantizeOptions) -> IndexedImage {
    walk(img, palette, options, Gather::Picture).image
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
    let measured = walk(img, palette, options, Gather::Measures);
    (measured.colors_used, measured.max_color_error)
}

/// [`quantize`] and [`quantize_stats`] in the one walk, so a caller that
/// stores the picture and reports on it searches the palette once per
/// distinct colour and reads its measures off the indices it stored.
#[must_use]
#[inline]
pub fn quantize_measured(
    img: &RgbaImage,
    palette: &Palette,
    options: &QuantizeOptions,
) -> Quantized {
    walk(img, palette, options, Gather::Both)
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
        quantize_measured, quantize_stats,
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

    /// A 4x1 picture: near red twice, that same colour dropped by alpha and
    /// a white pixel.
    fn mixed() -> RgbaImage {
        let pixels = vec![
            250, 5, 5, 255, // near red
            255, 255, 255, 255, // white
            250, 5, 5, 0, // the first colour, dropped
            250, 5, 5, 255, // near red again
        ];
        RgbaImage::new(4, 1, pixels).expect("4x1 is valid")
    }

    #[test]
    fn quantize_measured_is_the_picture_and_the_stats_of_one_walk() {
        let pal = test_palette();
        let options = QuantizeOptions::default();
        let img = mixed();
        let measured = quantize_measured(&img, &pal, &options);
        assert_eq!(
            measured.image,
            quantize(&img, &pal, &options),
            "the stored picture is the one the walk wrote"
        );
        assert_eq!(
            (measured.colors_used, measured.max_color_error),
            quantize_stats(&img, &pal, &options),
            "the measures are the same walk's, not a second one's"
        );
        assert_eq!(
            measured.image.opaque,
            vec![true, true, false, true],
            "alpha kept"
        );
        assert_eq!(
            measured.image.indices,
            vec![2, 1, 2, 2],
            "the dropped pixel still carries its best-effort index"
        );
        assert_eq!(measured.colors_used, 2_usize, "two distinct opaque colours");
        assert_eq!(
            measured.max_color_error, 75_i32,
            "the error is read off the stored index"
        );
    }

    #[test]
    fn quantize_measured_measures_the_indices_it_stored() {
        let pal = test_palette();
        let exact = two_pixels([255, 0, 0, 255], [0, 255, 255, 255]);
        let measured = quantize_measured(&exact, &pal, &QuantizeOptions::default());
        assert_eq!(
            measured.image.indices,
            vec![2, 3],
            "both exact palette colours"
        );
        assert_eq!((measured.colors_used, measured.max_color_error), (2, 0_i32));
        let gone = two_pixels([1, 2, 3, 0], [4, 5, 6, 7]);
        let empty = quantize_measured(&gone, &pal, &QuantizeOptions::default());
        assert_eq!(empty.image.indices.len(), 2, "the picture is still stored");
        assert_eq!(
            (empty.colors_used, empty.max_color_error),
            (0_usize, 0_i32),
            "and nothing opaque is measured"
        );
    }
}
