//! Shared builders for the `Doom` patch codec's tests: the hand-made lump
//! and post writers, the rule-driven image builder and the per-pixel rules
//! the topic modules reuse.
//!
//! [`codec`] decodes and encodes whole columns, the tall-patch cases and
//! the per-column painting helpers; [`header`] reads the eight header
//! bytes alone, rewrites the drawing offsets and refuses malformed input.

mod codec;
mod header;

use super::COLUMN_OFFSET_LEN;
use crate::gfx::image::IndexedImage;

/// The header-plus-table size of a `width`-column patch.
pub(super) fn table_end(width: u16) -> usize {
    8_usize
        .checked_add(
            COLUMN_OFFSET_LEN
                .checked_mul(usize::from(width))
                .expect("test width"),
        )
        .expect("test table fits")
}

/// The per-pixel rule [`build`] applies.
pub(super) type PixelRule = fn(u32, u32) -> (u8, bool);

/// A hand-built lump: header, one offset per column, then the column
/// bytes in order.
pub(super) fn patch_lump(
    width: u16,
    height: u16,
    offsets: (i16, i16),
    columns: &[&[u8]],
) -> Vec<u8> {
    let (left, top) = offsets;
    let mut data = Vec::new();
    data.extend_from_slice(&width.to_le_bytes());
    data.extend_from_slice(&height.to_le_bytes());
    data.extend_from_slice(&left.to_le_bytes());
    data.extend_from_slice(&top.to_le_bytes());
    let mut pos = table_end(width);
    for column in columns {
        data.extend_from_slice(&u32::try_from(pos).expect("test offset fits").to_le_bytes());
        pos = pos.checked_add(column.len()).expect("test body fits");
    }
    for column in columns {
        data.extend_from_slice(column);
    }
    data
}

/// One post: `topdelta`, length, pad, pixels and the trailing pad.
pub(super) fn post(delta: u8, pixels: &[u8]) -> Vec<u8> {
    let mut bytes = vec![delta, u8::try_from(pixels.len()).expect("test post fits")];
    bytes.push(0);
    bytes.extend_from_slice(pixels);
    bytes.push(0);
    bytes
}

/// An image from a per-pixel rule; transparent pixels get index 0,
/// exactly as decoding produces.
pub(super) fn build(width: u32, height: u32, pixel: PixelRule) -> IndexedImage {
    let columns = usize::try_from(width).expect("test width");
    let total = columns
        .checked_mul(usize::try_from(height).expect("test height"))
        .expect("test size");
    let mut indices = Vec::with_capacity(total);
    let mut opaque = Vec::with_capacity(total);
    for n in 0..total {
        let x =
            u32::try_from(n.checked_rem(columns).expect("non-zero columns")).expect("within width");
        let y = u32::try_from(n.checked_div(columns).expect("non-zero columns"))
            .expect("within height");
        let (index, visible) = pixel(x, y);
        indices.push(index.checked_mul(u8::from(visible)).unwrap_or(0));
        opaque.push(visible);
    }
    IndexedImage::new(width, height, indices, opaque).expect("test shapes are valid")
}

/// Deterministic speckles, a fully opaque and a fully empty column.
pub(super) fn speckle(x: u32, y: u32) -> (u8, bool) {
    if x == 3 {
        return (7, true);
    }
    if x == 16 {
        return (0, false);
    }
    let hash = x
        .wrapping_mul(0x0100_0193)
        .wrapping_add(y.wrapping_mul(0x85EB_CA6B))
        .wrapping_add(0x9E37_79B9);
    (
        u8::try_from(hash & 0xFF).expect("masked to one byte"),
        (hash & 3) != 0,
    )
}

/// A fully opaque pixel.
pub(super) fn full(_x: u32, _y: u32) -> (u8, bool) {
    (5, true)
}

/// Column 0 has runs at rows 10, 260 and 290; column 1 is a full
/// 300-row column; the rest stays transparent.
pub(super) fn tall(x: u32, y: u32) -> (u8, bool) {
    if x == 1 {
        return (9, true);
    }
    if x != 0 {
        return (0, false);
    }
    if (10..20).contains(&y) {
        (u8::try_from(y & 0xFF).expect("masked"), true)
    } else if (260..265).contains(&y) {
        (6, true)
    } else if (290..300).contains(&y) {
        (8, true)
    } else {
        (0, false)
    }
}

/// Opaque only in rows 590..600: reaching them needs relative
/// filler steps above row 254.
pub(super) fn top_run_only(_x: u32, y: u32) -> (u8, bool) {
    (8, (590..600).contains(&y))
}
