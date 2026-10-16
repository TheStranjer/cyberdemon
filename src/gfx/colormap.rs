//! `COLORMAP`: the vanilla brightness tables, read from lumps or derived
//! from a palette.
//!
//! A `COLORMAP` lump holds 34 tables of 256 palette indices: maps
//! `0..=31` are the light levels from full brightness down to near-black,
//! map `32` is the invulnerability tint (inverted greyscale, using
//! [`crate::gfx::ops::luminance`]) and map `33` renders everything the
//! nearest colour to black.
//!
//! [`colormaps_from_bytes`] reads those tables from merged lump bytes;
//! [`colormaps`] generates the same shape from a [`Palette`] by
//! nearest-colour matching, the recipe `set_palette`'s
//! `regenerate_colormap` writes and the renderers fall back to when a
//! session has no usable `COLORMAP`.

use crate::fixed::mul_div_round;
use crate::gfx::ops::luminance;
use crate::gfx::palette::Palette;
use crate::gfx::quantize::nearest_index;

/// Brightness tables at the head of a `COLORMAP` (full to darkest).
const LEVELS: usize = 32;
/// Tables in one `COLORMAP`: 32 levels plus invulnerability and all-black.
pub const TABLES: usize = 34;
/// Bytes (indices) in one `COLORMAP` table.
pub const TABLE_BYTES: usize = 256;
/// Smallest a `COLORMAP` lump must be for its tables to be usable.
pub const MIN_BYTES: usize = TABLES.saturating_mul(TABLE_BYTES);
/// Denominator of the brightness ramp: map `k` scales by `(32 - k)/32`.
const LIGHT_SCALE: i64 = 32;
/// Opaque alpha, for reading a palette colour through [`luminance`].
const OPAQUE: u8 = 255;

/// The 34 tables derived from `palette`: 32 light-level ramps, the
/// invulnerability tint and an all-black map.
///
/// Maps `0..=31` scale colours by `(32 - k)/32` to the nearest palette
/// entry, map `32` is the inverted greyscale (invulnerability) and map
/// `33` maps everything to the nearest colour to black.
#[must_use]
#[inline]
pub fn colormaps(palette: &Palette) -> Vec<[u8; TABLE_BYTES]> {
    let mut tables: Vec<[u8; TABLE_BYTES]> = (0..LEVELS)
        .map(|level| ramp_table(palette, level))
        .collect();
    tables.push(inverse_table(palette));
    tables.push(black_table(palette));
    tables
}

/// The same tables as the 8704 bytes one `COLORMAP` lump holds.
#[must_use]
#[inline]
pub fn colormap_bytes(palette: &Palette) -> Vec<u8> {
    colormaps(palette).concat()
}

/// The first [`TABLES`] tables of `COLORMAP` lump bytes, `None` when the
/// data is shorter than [`MIN_BYTES`] and therefore unusable.
#[must_use]
#[inline]
pub fn colormaps_from_bytes(data: &[u8]) -> Option<Vec<[u8; TABLE_BYTES]>> {
    if data.len() < MIN_BYTES {
        return None;
    }
    data.chunks_exact(TABLE_BYTES)
        .take(TABLES)
        .map(<[u8; TABLE_BYTES]>::try_from)
        .collect::<Result<Vec<[u8; TABLE_BYTES]>, _>>()
        .ok()
}

/// The `COLORMAP` table of light level `level` (0 full bright to 31).
fn ramp_table(palette: &Palette, level: usize) -> [u8; TABLE_BYTES] {
    let num = LIGHT_SCALE.saturating_sub(i64::try_from(level).unwrap_or(i64::MAX));
    let mut table = [0_u8; TABLE_BYTES];
    for (entry, color) in table.iter_mut().zip(palette.colors()) {
        *entry = nearest_index(scale_color(*color, num, LIGHT_SCALE), palette, &[]);
    }
    table
}

/// The invulnerability table: every colour's inverted greyscale as its
/// nearest palette index.
fn inverse_table(palette: &Palette) -> [u8; TABLE_BYTES] {
    let mut table = [0_u8; TABLE_BYTES];
    for (entry, color) in table.iter_mut().zip(palette.colors()) {
        *entry = inverse_entry(*color, palette);
    }
    table
}

/// The all-black table: every index mapped to the nearest colour to black.
fn black_table(palette: &Palette) -> [u8; TABLE_BYTES] {
    [nearest_index([0_u8; 3], palette, &[]); TABLE_BYTES]
}

/// The channel value scaled by `num`/`den`, rounded to nearest.
fn scale_channel(value: u8, num: i64, den: i64) -> u8 {
    mul_div_round(i64::from(value), num, den)
        .and_then(|scaled| u8::try_from(scaled).ok())
        .unwrap_or(value)
}

/// The colour scaled by `num`/`den`.
fn scale_color([red, green, blue]: [u8; 3], num: i64, den: i64) -> [u8; 3] {
    [
        scale_channel(red, num, den),
        scale_channel(green, num, den),
        scale_channel(blue, num, den),
    ]
}

/// The invulnerability-table entry of one palette colour: its inverted
/// greyscale as a nearest-match index.
fn inverse_entry(color: [u8; 3], palette: &Palette) -> u8 {
    let [red, green, blue] = color;
    let shade = luminance([red, green, blue, OPAQUE]);
    let inverse = 255_u8.saturating_sub(shade);
    nearest_index([inverse, inverse, inverse], palette, &[])
}

#[cfg(test)]
mod tests {
    use super::{MIN_BYTES, TABLE_BYTES, TABLES, colormaps, colormaps_from_bytes};
    use crate::gfx::palette::Palette;

    /// The fixture palette: colour `i` is `(i, 255 - i, i)`.
    fn fixture_palette() -> Palette {
        let mut colors = [[0_u8; 3]; 256];
        for (index, color) in colors.iter_mut().enumerate() {
            let byte = u8::try_from(index).expect("the palette covers exactly the u8 range");
            *color = [byte, 255_u8.wrapping_sub(byte), byte];
        }
        Palette::from(colors)
    }

    /// The `position`th byte of the generated 34-table `COLORMAP`.
    fn byte_at(maps: &[[u8; TABLE_BYTES]], map: usize, position: usize) -> u8 {
        *maps
            .get(map)
            .and_then(|table| table.get(position))
            .expect("test positions are inside the 34 tables")
    }

    #[test]
    fn the_colormap_ramps_from_identity_through_grey_to_black() {
        let maps = colormaps(&fixture_palette());
        assert_eq!(maps.len(), TABLES, "34 tables");
        let identity: Vec<u8> = (0_u8..=255).collect();
        assert_eq!(
            maps.first().expect("map 0 exists"),
            identity.as_slice(),
            "map 0 is exact"
        );
        assert!(
            byte_at(&maps, 16, 255) < 255,
            "map 16 darkens the top colour"
        );
        assert_eq!(
            byte_at(&maps, 32, 0),
            120,
            "index 0 (0,255,0) inverts to 106-grey, nearest at 120"
        );
        assert_eq!(
            byte_at(&maps, 32, 255),
            135,
            "index 255 (255,0,255) inverts to 150-grey, nearest at 135"
        );
        let black = byte_at(&maps, 33, 0);
        assert!(
            maps.get(33)
                .expect("map 33 exists")
                .iter()
                .all(|&value| value == black),
            "map 33 is uniform"
        );
        assert_eq!(
            black, 85,
            "the fixture's nearest colour to black is entry 85"
        );
    }

    #[test]
    fn lump_bytes_are_read_as_tables_only_when_whole() {
        let palette = fixture_palette();
        let bytes = super::colormap_bytes(&palette);
        assert_eq!(bytes.len(), MIN_BYTES, "the lump is 34 tables of 256");
        let read = colormaps_from_bytes(&bytes).expect("8704 bytes are usable");
        assert_eq!(read, colormaps(&palette), "the round trip is exact");
        assert!(
            colormaps_from_bytes(&bytes[..MIN_BYTES - 1]).is_none(),
            "one byte short is refused, not padded"
        );
        let longer = [bytes.as_slice(), b"junk"].concat();
        assert!(
            colormaps_from_bytes(&longer).is_some(),
            "a longer lump still answers with its first 34 tables"
        );
        assert!(colormaps_from_bytes(&[]).is_none(), "empty is no COLORMAP");
    }
}
