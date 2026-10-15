//! Border-driven background removal for
//! [`crate::gfx::ops::ImageOp::RemoveBackground`].
//!
//! The background colour is either the caller's `color` or the average of
//! the most common border-pixel bucket (top `4` bits per channel). A
//! `4`-connected flood fill starts from every border pixel within
//! `tolerance` of that colour and clears everything it reaches to alpha
//! `0`; enclosed regions of the same colour survive because the fill can
//! never jump the border of the object drawn around them.

use serde_json::{Value, json};

use crate::error::CyberError;
use crate::gfx::image::RgbaImage;
use crate::gfx::ops::{arg_error, parse_hex_color, rounded_div};
use crate::gfx::{BYTES_PER_PIXEL, to_pixel};

/// Bucket edge width in channel units (`256 / 16`).
const BUCKET_SIZE: u32 = 16;
/// Total `4 x 4 x 4`-bit buckets (`16^3`).
const BUCKET_COUNT: usize = 4_096;
/// The alpha channel index of a pixel.
const ALPHA_INDEX: usize = 3;

/// Parameters of [`crate::gfx::ops::ImageOp::RemoveBackground`].
pub(crate) struct BackgroundSpec<'key> {
    /// Background colour as text; `None` detects it from the border.
    pub(crate) color: Option<&'key str>,
    /// Per-channel match tolerance.
    pub(crate) tolerance: u8,
}

/// Clears the border-connected background and reports the colour used.
pub(crate) fn remove_background(
    img: RgbaImage,
    index: usize,
    spec: &BackgroundSpec<'_>,
) -> Result<(RgbaImage, Value), CyberError> {
    let background = match spec.color {
        Some(text) => explicit_color(text, index)?,
        None => dominant_border_color(&img),
    };
    let mut out = img;
    let removed = flood_clear(&mut out, background, spec.tolerance);
    let [red, green, blue] = background;
    Ok((
        out,
        json!({
            "op": "remove_background",
            "background": format!("#{red:02X}{green:02X}{blue:02X}"),
            "removed_pixels": removed,
        }),
    ))
}

/// Parses an explicit colour and keeps only its `RGB`.
fn explicit_color(text: &str, index: usize) -> Result<[u8; 3], CyberError> {
    let Ok([red, green, blue, _]) = parse_hex_color(text) else {
        return Err(arg_error(
            index,
            "color",
            &format!("be a #RRGGBBAA or #RRGGBB hex colour, got {text}"),
        ));
    };
    Ok([red, green, blue])
}

/// The average `RGB` of the border pixels in the most common bucket; ties
/// go to the lowest packed `r<<8|g<<4|b` bucket value.
fn dominant_border_color(img: &RgbaImage) -> [u8; 3] {
    let mut counts = vec![0_u32; BUCKET_COUNT];
    let mut sums = vec![[0_u32; 3]; BUCKET_COUNT];
    for position in border_pixels(img) {
        let Some(pixel) = pixel_at_rgb(img, position) else {
            continue;
        };
        let bucket = bucket_of(pixel);
        let (Some(count), Some(channels)) = (counts.get_mut(bucket), sums.get_mut(bucket)) else {
            continue;
        };
        *count = count.saturating_add(1);
        for (slot, value) in channels.iter_mut().zip(pixel) {
            *slot = slot.saturating_add(u32::from(value));
        }
    }
    average_bucket(&counts, &sums)
}

/// The `444` bit bucket key of a colour.
fn bucket_of(pixel: [u8; 3]) -> usize {
    let mut key = 0_u32;
    for channel in pixel {
        let bucket = u32::from(channel).checked_div(BUCKET_SIZE).unwrap_or(0_u32);
        key = key
            .checked_shl(4)
            .unwrap_or(0_u32)
            .checked_add(bucket)
            .unwrap_or(0_u32);
    }
    usize::try_from(key).unwrap_or(0_usize)
}

/// The mean colour of the most-filled bucket, black when every border
/// pixel was unaddressable.
fn average_bucket(counts: &[u32], sums: &[[u32; 3]]) -> [u8; 3] {
    let best = counts
        .iter()
        .enumerate()
        .fold((0_u32, 0_usize), |lead, (bucket, &count)| {
            if count > lead.0 {
                (count, bucket)
            } else {
                lead
            }
        });
    let Some(total) = counts.get(best.1).copied().filter(|&count| count > 0) else {
        return [0_u8; 3];
    };
    let mut average = [0_u8; 3];
    for (slot, value) in average
        .iter_mut()
        .zip(sums.get(best.1).copied().unwrap_or([0_u32; 3]))
    {
        *slot = u8::try_from(rounded_div(u64::from(value), u64::from(total))).unwrap_or(255_u8);
    }
    average
}

/// The row-major positions of every border pixel, corners once.
fn border_pixels(img: &RgbaImage) -> Vec<usize> {
    let width = usize::try_from(img.width).unwrap_or(0_usize);
    let height = usize::try_from(img.height).unwrap_or(0_usize);
    let mut positions: Vec<usize> = Vec::new();
    if width == 0 || height == 0 {
        return positions;
    }
    positions.extend(0..width);
    let last_row = height.saturating_sub(1).saturating_mul(width);
    if height > 1 {
        positions.extend(last_row..last_row.saturating_add(width));
    }
    positions.extend(sides(width, height));
    positions
}

/// The first and last pixel numbers of an interior row.
fn row_sides(width: usize, row: usize) -> [Option<usize>; 2] {
    let start = row.saturating_mul(width);
    let last = start.saturating_add(width).saturating_sub(1);
    [Some(start), (width > 1).then_some(last)]
}

/// The border pixels of every row strictly between the edge rows.
fn sides(width: usize, height: usize) -> Vec<usize> {
    (1..height.saturating_sub(1))
        .flat_map(|row| row_sides(width, row).into_iter().flatten())
        .collect()
}

/// The `RGB` of border pixel `position`, `None` when unaddressable.
fn pixel_at_rgb(img: &RgbaImage, position: usize) -> Option<[u8; 3]> {
    let start = position.checked_mul(BYTES_PER_PIXEL)?;
    let chunk = img.pixels.get(start..start.checked_add(BYTES_PER_PIXEL)?)?;
    let [red, green, blue, _] = to_pixel(chunk);
    Some([red, green, blue])
}

/// Whether a pixel is within `tolerance` (per channel) of `background`.
fn is_background(pixel: [u8; 4], background: [u8; 3], tolerance: u8) -> bool {
    let [red, green, blue, _] = pixel;
    [red, green, blue]
        .into_iter()
        .zip(background)
        .all(|(value, target)| {
            let delta = i32::from(value)
                .checked_sub(i32::from(target))
                .unwrap_or(0_i32)
                .checked_abs()
                .unwrap_or(0_i32);
            u32::try_from(delta).unwrap_or(255_u32) <= u32::from(tolerance)
        })
}

/// Floods the border-connected background to alpha `0`; returns the
/// number of pixels newly cleared.
fn flood_clear(img: &mut RgbaImage, background: [u8; 3], tolerance: u8) -> u32 {
    let width = usize::try_from(img.width).unwrap_or(0);
    let border = border_pixels(img);
    let count = img.pixels.chunks_exact(BYTES_PER_PIXEL).len();
    let mut flood = Flood {
        pixels: &mut img.pixels,
        width,
        visited: vec![false; count],
        stack: Vec::new(),
        background,
        tolerance,
        cleared: 0_u32,
    };
    flood.seed(&border);
    flood.run()
}

/// Explicit-stack, `4`-connected flood state over one pixel buffer.
struct Flood<'buffer> {
    /// The `RGBA` buffer whose alpha slots get cleared.
    pixels: &'buffer mut [u8],
    /// Pixels per row, in buffer units.
    width: usize,
    /// One flag per pixel, set when it enters (or is rejected from) the stack.
    visited: Vec<bool>,
    /// Depth-first stack of pixel numbers still to clear.
    stack: Vec<usize>,
    /// The colour the fill matches against.
    background: [u8; 3],
    /// Per-channel match tolerance.
    tolerance: u8,
    /// Pixels newly set to alpha `0`.
    cleared: u32,
}

impl Flood<'_> {
    /// Stacks every matching border pixel as a seed.
    fn seed(&mut self, border: &[usize]) {
        for &position in border {
            self.absorb(position);
        }
    }

    /// Drains the stack, clearing each pixel and its matching neighbours.
    fn run(&mut self) -> u32 {
        while let Some(position) = self.stack.pop() {
            self.clear(position);
            self.absorb_neighbours(position);
        }
        self.cleared
    }

    /// Queues every matching `4`-connected neighbour of `position`.
    fn absorb_neighbours(&mut self, position: usize) {
        for neighbour in neighbours(position, self.width) {
            self.absorb(neighbour);
        }
    }

    /// Marks `position` visited and stacks it when it matches the background.
    fn absorb(&mut self, position: usize) {
        if !self.unvisited(position) || !self.matches(position) {
            return;
        }
        if let Some(seen) = self.visited.get_mut(position) {
            *seen = true;
        }
        self.stack.push(position);
    }

    /// Whether `position` is in bounds and not yet seen.
    fn unvisited(&self, position: usize) -> bool {
        self.visited.get(position).copied() == Some(false)
    }

    /// Whether the pixel at `position` is within tolerance of the background.
    fn matches(&self, position: usize) -> bool {
        let Some(start) = position.checked_mul(BYTES_PER_PIXEL) else {
            return false;
        };
        let Some(end) = start.checked_add(BYTES_PER_PIXEL) else {
            return false;
        };
        let Some(pixel) = self.pixels.get(start..end).map(to_pixel) else {
            return false;
        };
        is_background(pixel, self.background, self.tolerance)
    }

    /// Sets the pixel's alpha to `0`, counting it when newly transparent.
    fn clear(&mut self, position: usize) {
        let Some(alpha) = position
            .checked_mul(BYTES_PER_PIXEL)
            .and_then(|base| base.checked_add(ALPHA_INDEX))
        else {
            return;
        };
        let Some(slot) = self.pixels.get_mut(alpha) else {
            return;
        };
        if *slot != 0 {
            *slot = 0_u8;
            self.cleared = self.cleared.saturating_add(1);
        }
    }
}

/// The `4`-connected neighbours of a pixel, as pixel numbers.
fn neighbours(position: usize, width: usize) -> impl Iterator<Item = usize> {
    let Some(x) = position.checked_rem(width) else {
        return [None; 4].into_iter().flatten();
    };
    let [west, east, north, south] = [
        x.checked_sub(1).map(|_| position.saturating_sub(1)),
        x.checked_add(1)
            .filter(|&next| next < width)
            .map(|_| position.saturating_add(1)),
        position.checked_sub(width),
        position.checked_add(width),
    ];
    [west, east, north, south].into_iter().flatten()
}

#[cfg(test)]
mod tests {
    use super::{BackgroundSpec, bucket_of, remove_background};
    use crate::error::CyberError;
    use crate::gfx::image::RgbaImage;

    /// Whether `(x, y)` of the island fixture sits on the black ring.
    fn on_ring(size: usize, x: usize, y: usize) -> bool {
        let last = size.saturating_sub(1);
        let edge = x == 0 || y == 0 || x == last || y == last;
        let band = x == 1 || y == 1 || x == size.saturating_sub(2) || y == size.saturating_sub(2);
        band && !edge
    }

    /// The island fixture pixel at `(x, y)`: black on the ring, white inside.
    fn ring_pixel(size: usize, x: usize, y: usize) -> [u8; 4] {
        if on_ring(size, x, y) {
            [0_u8, 0, 0, 255]
        } else {
            [255_u8; 4]
        }
    }

    /// A `size x size` image: white everywhere except a black ring at
    /// radius one and a white island inside it.
    fn island_fixture(size: u32) -> RgbaImage {
        let columns = usize::try_from(size).expect("fixture fits");
        let pixels: Vec<u8> = (0..columns)
            .flat_map(|row| (0..columns).map(move |column| ring_pixel(columns, column, row)))
            .flatten()
            .collect();
        RgbaImage::new(size, size, pixels).expect("fixture is valid")
    }

    /// The noisy-border fixture pixel at `(x, y)`.
    fn border_or_red(x: usize, y: usize) -> [u8; 4] {
        let border = x == 0 || y == 0 || x == 3 || y == 3;
        if !border {
            return [200_u8, 0, 0, 255_u8];
        }
        let even = x.checked_add(y).and_then(|sum| sum.checked_rem(2)) == Some(0);
        let shade = if even { 250_u8 } else { 254_u8 };
        [shade, shade, shade, 255_u8]
    }

    /// A 4x4 border-noisy fixture: the outer ring alternates shades of
    /// near-white, the centre four pixels are solid red.
    fn noisy_border() -> RgbaImage {
        let pixels: Vec<u8> = (0_usize..4)
            .flat_map(|y| (0_usize..4).map(move |x| border_or_red(x, y)))
            .flatten()
            .collect();
        RgbaImage::new(4, 4, pixels).expect("fixture is valid")
    }

    /// The default spec (detected colour, tolerance `32`).
    fn detected() -> BackgroundSpec<'static> {
        BackgroundSpec {
            color: None,
            tolerance: 32,
        }
    }

    #[test]
    fn removes_the_white_surround_and_keeps_the_white_island() {
        let (out, step) =
            remove_background(island_fixture(7), 0, &detected()).expect("detection cannot fail");
        assert_eq!(step["op"], "remove_background", "op name");
        assert_eq!(step["background"], "#FFFFFF", "white surround detected");
        assert_eq!(out.get(0, 0).expect("in bounds")[3], 0, "surround cleared");
        assert_eq!(out.get(3, 3).expect("in bounds")[3], 255, "island kept");
        assert_eq!(out.get(1, 3).expect("in bounds")[3], 255, "black ring kept");
        let removed = step["removed_pixels"].as_u64().expect("numeric count");
        assert_eq!(removed, 24, "the 7x7 minus ring-and-inside area");
    }

    #[test]
    fn detects_the_average_of_a_noisy_border_bucket() {
        let (out, step) =
            remove_background(noisy_border(), 1, &detected()).expect("detection cannot fail");
        assert_eq!(step["background"], "#FCFCFC", "average of 250 and 254");
        assert_eq!(
            out.get(0, 0).expect("in bounds")[3],
            0,
            "noisy border removed"
        );
        assert_eq!(out.get(1, 1).expect("in bounds")[3], 255, "red centre kept");
    }

    #[test]
    fn explicit_colour_overrides_detection() {
        let spec = BackgroundSpec {
            color: Some("#000000"),
            tolerance: 0,
        };
        let (out, step) = remove_background(island_fixture(7), 4, &spec).expect("colour parses");
        assert_eq!(
            step["background"], "#000000",
            "the requested colour is reported"
        );
        assert_eq!(
            step["removed_pixels"], 0_i32,
            "no border pixel matches black"
        );
        assert_eq!(
            out.get(0, 0).expect("in bounds")[3],
            255,
            "white surround kept"
        );
        assert_eq!(
            out.get(1, 3).expect("in bounds")[3],
            255,
            "the ring is not a seed"
        );
    }

    #[test]
    fn tolerance_boundary_is_inclusive() {
        let row: Vec<u8> = [
            [100_u8, 100, 100, 255],
            [110, 110, 110, 255],
            [111, 111, 111, 255],
        ]
        .into_iter()
        .flatten()
        .collect();
        let strip = RgbaImage::new(3, 1, row).expect("3x1 is valid");
        let spec = BackgroundSpec {
            color: Some("#646464"),
            tolerance: 10,
        };
        let (out, step) = remove_background(strip.clone(), 0, &spec).expect("colour parses");
        assert_eq!(out.get(1, 0).expect("in bounds")[3], 0, "delta 10 matches");
        assert_eq!(
            out.get(2, 0).expect("in bounds")[3],
            255,
            "delta 11 blocks the fill"
        );
        assert_eq!(step["removed_pixels"], 2_i32, "seed plus reached neighbour");
        let loose = BackgroundSpec {
            color: Some("#646464"),
            tolerance: 11,
        };
        let (wide_out, _loose_step) = remove_background(strip, 0, &loose).expect("colour parses");
        assert_eq!(
            wide_out.get(2, 0).expect("in bounds")[3],
            0,
            "one more clears it"
        );
    }

    #[test]
    fn a_malformed_colour_names_the_op_and_field() {
        let spec = BackgroundSpec {
            color: Some("beige"),
            tolerance: 8,
        };
        let error = remove_background(noisy_border(), 3, &spec).expect_err("beige is not hex");
        let CyberError::InvalidArgument(text) = error else {
            panic!("expected InvalidArgument");
        };
        assert!(
            text.contains("ops[3].color") && text.contains("beige"),
            "error names the op, field and value: {text}"
        );
    }

    #[test]
    fn bucket_packs_the_top_nibbles() {
        assert_eq!(bucket_of([0, 0, 0]), 0, "black");
        assert_eq!(bucket_of([255, 255, 255]), 4_095, "white");
        assert_eq!(bucket_of([16, 0, 0]), 256, "top nibble of red only");
        assert!(
            bucket_of([0, 16, 0]) > bucket_of([0, 0, 16]),
            "green outweighs blue"
        );
    }

    #[test]
    fn a_one_pixel_wide_column_lets_every_pixel_seed_the_fill() {
        let column = RgbaImage::new(
            1,
            3,
            vec![255_u8, 255, 255, 255, 250, 250, 250, 255, 0, 0, 0, 255],
        )
        .expect("1x3 is valid");
        let (out, step) = remove_background(column, 0, &detected()).expect("detection runs");
        assert_eq!(
            step["background"], "#FDFDFD",
            "rounded mean of the border whites 255 and 250"
        );
        assert_eq!(out.get(0, 0).expect("top")[3], 0, "top pixel cleared");
        assert_eq!(
            out.get(0, 1).expect("middle")[3],
            0,
            "middle pixel connected"
        );
        assert_eq!(
            out.get(0, 2).expect("bottom")[3],
            255,
            "black bottom is its own border"
        );
        assert_eq!(step["removed_pixels"], 2_i32, "two pixels cleared");
    }
}
