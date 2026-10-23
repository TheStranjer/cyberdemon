//! `Doom` patch lumps: the column/post encoding behind sprites, `HUD`
//! graphics and texture components, with the `DeePsea` tall-patch
//! convention for images taller than `254` rows.
//!
//! [`decode_patch`] reads lump bytes into a [`Patch`] (two drawing offsets
//! plus an [`IndexedImage`]); [`encode::encode_patch`] writes one back.
//! [`set_patch_offsets`] rewrites just the two drawing-offset fields of
//! existing patch bytes, leaving the encoded columns byte-identical. A
//! patch is stored column-first: a header, then `width` column offsets
//! counted from the lump start, then per column a series of posts. Each
//! post is `topdelta u8`, `length u8`, one unused pad byte, `length`
//! palette indices and one unused pad byte; a `topdelta` of `0xFF` ends
//! the column. Pixels no post covers stay transparent.
//!
//! `DeePsea` tall patches: a `topdelta` at or below the previous post's
//! actual top row is relative to that row, so rows beyond the plain
//! `0..=254` absolute range stay reachable. [`encode::encode_patch`] uses
//! that reading (with zero-length filler posts where needed) whenever a
//! plain absolute `topdelta` cannot express a row; it writes plain
//! absolute deltas for short images. Building those columns is the
//! [`encode`] module; this file carries the format's shape, header and
//! decoder.

pub mod encode;

use crate::error::CyberError;
use crate::gfx::image::IndexedImage;
use crate::gfx::pixel_count;
use crate::wad::bytes::{Reader, Writer, i16_at, u16_at, u32_to_usize};

pub use encode::encode_patch;

/// Largest patch edge this codec accepts.
pub(super) const MAX_PATCH_DIM: u32 = 4_096_u32;
/// Bytes in a patch header: width, height and the two drawing offsets.
pub(super) const PATCH_HEADER_LEN: usize = 8_usize;
/// Bytes in one column-offset table entry.
pub(super) const COLUMN_OFFSET_LEN: usize = 4_usize;
/// Byte offset of the header's first drawing-offset field (`left_offset`;
/// `top_offset` follows at `6`).
const OFFSET_FIELDS_START: usize = 4_usize;
/// Bytes in the header's two drawing-offset fields together.
const OFFSET_FIELDS_LEN: usize = 4_usize;
/// The `topdelta` byte that ends a column.
pub(super) const COLUMN_END: u8 = 0xFF_u8;
/// Longest pixel run one post can carry.
pub(super) const MAX_POST_LEN: u8 = 254_u8;
/// Largest `topdelta` that is not the column terminator.
pub(super) const MAX_TOPDELTA: u8 = 254_u8;
/// Value written to the two unused pad bytes of a post.
pub(super) const POST_PAD: u8 = 0_u8;

/// The encoded column bodies: table offsets counted from the lump start,
/// and the concatenated column bytes that follow the table.
pub(super) type EncodedColumns = (Vec<u32>, Vec<u8>);

/// A decoded `Doom` patch: its drawing offsets and pixels.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Patch {
    /// Horizontal offset of the image's left edge, in pixels.
    pub left_offset: i16,
    /// Vertical offset of the image's top edge, in pixels.
    pub top_offset: i16,
    /// The patch pixels; places no post covers are transparent with
    /// index 0.
    pub image: IndexedImage,
}

/// The header fields of a `Doom` patch lump, without the column bodies:
/// what a listing needs to describe a patch without decoding it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PatchHeader {
    /// Width in pixels.
    pub width: u16,
    /// Height in pixels.
    pub height: u16,
    /// Horizontal offset of the image's left edge, in pixels.
    pub left_offset: i16,
    /// Vertical offset of the image's top edge, in pixels.
    pub top_offset: i16,
}

/// Reads only the eight header bytes of a `Doom` patch lump: dimensions
/// and drawing offsets, without touching the column table or bodies.
///
/// `None` when the lump is shorter than a header or declares a dimension
/// outside `1..=4096`; a lump may still hold corrupt columns that this
/// never looks at.
#[must_use]
#[inline]
pub fn patch_header(bytes: &[u8]) -> Option<PatchHeader> {
    let width = u16_at(bytes, 0).ok()?;
    let height = u16_at(bytes, 2).ok()?;
    let left_offset = i16_at(bytes, 4).ok()?;
    let top_offset = i16_at(bytes, 6).ok()?;
    let sane = fits_patch_dim(width) && fits_patch_dim(height);
    sane.then_some(PatchHeader {
        width,
        height,
        left_offset,
        top_offset,
    })
}

/// Whether a header dimension is inside the codec's `1..=4096` range.
fn fits_patch_dim(value: u16) -> bool {
    let pixels = u32::from(value);
    pixels != 0 && pixels <= MAX_PATCH_DIM
}

/// Rewrites the two drawing-offset fields of an existing `Doom` patch
/// lump: the `i16` pair at byte offsets `4` and `6`. Width, height, the
/// column offset table and every column body stay byte-identical.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] when `bytes` answer no [`patch_header`]
/// or an offset is outside the `-32768..=32767` range the header stores.
#[inline]
pub fn set_patch_offsets(bytes: &[u8], left: i32, top: i32) -> Result<Vec<u8>, CyberError> {
    if patch_header(bytes).is_none() {
        return Err(no_patch_header(bytes));
    }
    let (head, tail) = split_offsets(bytes)?;
    let mut out = Writer::with_capacity(bytes.len());
    out.put_bytes(head);
    out.put_i16(header_offset("left_offset", left)?);
    out.put_i16(header_offset("top_offset", top)?);
    out.put_bytes(tail);
    Ok(out.into_inner())
}

/// The header bytes before the two offset fields and the bytes after the
/// header, as [`split_offsets`] reports them.
type OffsetSplit<'data> = (&'data [u8], &'data [u8]);

/// The bytes before the two offset fields and everything after the header.
/// [`patch_header`] has already ruled out shorter bytes when this runs.
///
/// # Errors
///
/// [`no_patch_header`]' [`CyberError::InvalidArgument`] for bytes that do
/// not hold a full header.
fn split_offsets(bytes: &[u8]) -> Result<OffsetSplit<'_>, CyberError> {
    let (head, rest) = bytes
        .split_at_checked(OFFSET_FIELDS_START)
        .ok_or_else(|| no_patch_header(bytes))?;
    let (_, tail) = rest
        .split_at_checked(OFFSET_FIELDS_LEN)
        .ok_or_else(|| no_patch_header(bytes))?;
    Ok((head, tail))
}

/// The refusal for bytes that carry no [`patch_header`] to rewrite.
fn no_patch_header(bytes: &[u8]) -> CyberError {
    CyberError::InvalidArgument(format!(
        "{} bytes carry no Doom patch header to rewrite: pass the bytes of a Doom patch lump",
        bytes.len()
    ))
}

/// One drawing offset narrowed to the `i16` a patch header stores.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] naming the field and the stored range.
fn header_offset(field: &str, value: i32) -> Result<i16, CyberError> {
    i16::try_from(value).map_err(|error| {
        CyberError::InvalidArgument(format!(
            "{field} {value} does not fit the -32768..=32767 range a patch header stores: \
             {error}; pass a smaller offset"
        ))
    })
}

/// Decodes a `Doom` patch lump, accepting `DeePsea` tall patches.
///
/// Posts running past the declared height are clipped rather than
/// rejected, matching how source ports read them. Overlapping or shared
/// column offsets are fine; later posts paint over earlier ones.
///
/// # Errors
///
/// [`CyberError::Format`] when the header is truncated, a dimension is
/// outside `1..=4096`, the column offset table is incomplete, a column
/// offset points past the end of the lump, a post is truncated, or a
/// column has no `0xFF` terminator before the lump ends.
#[inline]
pub fn decode_patch(bytes: &[u8]) -> Result<Patch, CyberError> {
    let mut reader = Reader::new(bytes);
    let raw_width = reader.read_u16()?;
    let raw_height = reader.read_u16()?;
    let left_offset = reader.read_i16()?;
    let top_offset = reader.read_i16()?;
    check_patch_range("width", raw_width)?;
    check_patch_range("height", raw_height)?;
    let columns = read_column_offsets(&mut reader, usize::from(raw_width), bytes)?;
    let mut image = empty_patch_image(u32::from(raw_width), u32::from(raw_height))?;
    paint_columns(&columns, bytes, &mut image)?;
    Ok(Patch {
        left_offset,
        top_offset,
        image,
    })
}

/// Rejects a header dimension outside `1..=4096`.
fn check_patch_range(name: &str, value: u16) -> Result<(), CyberError> {
    if u32::from(value) == 0 || u32::from(value) > MAX_PATCH_DIM {
        return Err(CyberError::Format(format!(
            "patch {name} is {value}, outside the supported 1..={MAX_PATCH_DIM} range; \
             the lump is corrupt or not a Doom patch"
        )));
    }
    Ok(())
}

/// Reads the column offset table, checking every offset lands inside the
/// lump. A truncated table fails as a truncated read.
fn read_column_offsets(
    reader: &mut Reader<'_>,
    width: usize,
    bytes: &[u8],
) -> Result<Vec<usize>, CyberError> {
    let mut offsets = Vec::with_capacity(width);
    for column in 0_usize..width {
        let raw = reader.read_u32()?;
        let offset = u32_to_usize(raw)?;
        if offset >= bytes.len() {
            return Err(CyberError::Format(format!(
                "patch column {column} offset {raw} is past the end of the {}-byte lump; \
                 the column table is corrupt",
                bytes.len()
            )));
        }
        offsets.push(offset);
    }
    Ok(offsets)
}

/// A fully transparent, index-0 patch image of a validated shape.
fn empty_patch_image(width: u32, height: u32) -> Result<IndexedImage, CyberError> {
    let count = pixel_count(width, height).ok_or_else(|| {
        CyberError::Format(format!(
            "patch shape {width}x{height} does not fit in memory"
        ))
    })?;
    Ok(IndexedImage {
        width,
        height,
        indices: vec![0_u8; count],
        opaque: vec![false; count],
    })
}

/// Decodes and paints every column of the patch into `image`.
fn paint_columns(
    columns: &[usize],
    bytes: &[u8],
    image: &mut IndexedImage,
) -> Result<(), CyberError> {
    for (column, &start) in columns.iter().enumerate() {
        let x = u32::try_from(column).map_err(|error| {
            CyberError::Internal(format!("patch column {column} does not fit u32: {error}"))
        })?;
        decode_column(image, bytes, x, start)?;
    }
    Ok(())
}

/// Decodes one column and paints its posts. A truncated post or a
/// missing `0xFF` terminator fails as a truncated read of the lump.
fn decode_column(
    image: &mut IndexedImage,
    bytes: &[u8],
    x: u32,
    start: usize,
) -> Result<(), CyberError> {
    let mut cursor = Reader::at(bytes, start);
    let mut last_top = 0_u32;
    loop {
        let delta = cursor.read_u8()?;
        if delta == COLUMN_END {
            return Ok(());
        }
        let length = cursor.read_u8()?;
        cursor.read_u8()?;
        let data = cursor.read_bytes(usize::from(length))?;
        cursor.read_u8()?;
        last_top = post_top(last_top, delta);
        paint_post(image, x, last_top, data);
    }
}

/// The actual top row of a post: a `topdelta` at or below the previous
/// post's top reads as relative to it (`DeePsea` tall patches).
fn post_top(last_top: u32, delta: u8) -> u32 {
    let value = u32::from(delta);
    if value <= last_top {
        last_top.saturating_add(value)
    } else {
        value
    }
}

/// Writes one post's pixels into `image`, clipping rows past the height.
fn paint_post(image: &mut IndexedImage, x: u32, top: u32, data: &[u8]) {
    for (row, value) in data.iter().enumerate() {
        let offset = u32::try_from(row).unwrap_or(u32::MAX);
        let Some(y) = top.checked_add(offset) else {
            break;
        };
        if y >= image.height {
            break;
        }
        image.set(x, y, (*value, true));
    }
}

#[cfg(test)]
mod tests;
