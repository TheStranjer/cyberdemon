//! `Doom` patch lumps: the column/post encoding behind sprites, `HUD`
//! graphics and texture components, with the `DeePsea` tall-patch
//! convention for images taller than `254` rows.
//!
//! [`decode_patch`] reads lump bytes into a [`Patch`] (two drawing offsets
//! plus an [`IndexedImage`]); [`encode_patch`] writes one back.
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
//! `0..=254` absolute range stay reachable. [`encode_patch`] uses that
//! reading (with zero-length filler posts where needed) whenever a plain
//! absolute `topdelta` cannot express a row; it writes plain absolute
//! deltas for short images.

use crate::error::CyberError;
use crate::gfx::image::IndexedImage;
use crate::gfx::pixel_count;
use crate::wad::bytes::{Reader, Writer, i16_at, u16_at, u32_to_usize, usize_to_u32};

/// Largest patch edge this codec accepts.
const MAX_PATCH_DIM: u32 = 4_096_u32;
/// Bytes in a patch header: width, height and the two drawing offsets.
const PATCH_HEADER_LEN: usize = 8_usize;
/// Bytes in one column-offset table entry.
const COLUMN_OFFSET_LEN: usize = 4_usize;
/// Byte offset of the header's first drawing-offset field (`left_offset`;
/// `top_offset` follows at `6`).
const OFFSET_FIELDS_START: usize = 4_usize;
/// Bytes in the header's two drawing-offset fields together.
const OFFSET_FIELDS_LEN: usize = 4_usize;
/// The `topdelta` byte that ends a column.
const COLUMN_END: u8 = 0xFF_u8;
/// Longest pixel run one post can carry.
const MAX_POST_LEN: u8 = 254_u8;
/// Largest `topdelta` that is not the column terminator.
const MAX_TOPDELTA: u8 = 254_u8;
/// Value written to the two unused pad bytes of a post.
const POST_PAD: u8 = 0_u8;

/// The encoded column bodies: table offsets counted from the lump start,
/// and the concatenated column bytes that follow the table.
type EncodedColumns = (Vec<u32>, Vec<u8>);

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

/// Encodes a [`Patch`] as `Doom` patch lump bytes.
///
/// Maximal opaque runs become posts; runs longer than `254` rows are
/// split. Short images use plain absolute `topdelta` values. When a row
/// cannot be reached absolutely, the column is written in the `DeePsea`
/// tall-patch convention, inserting zero-length filler posts as needed.
/// Transparent pixels are not stored; decoding gives them index `0`.
///
/// # Errors
///
/// [`CyberError::LimitExceeded`] when the image is wider or taller than
/// `4096` pixels or its offsets do not fit the `u32` table entries;
/// [`CyberError::InvalidArgument`] when an image dimension is zero.
#[inline]
pub fn encode_patch(patch: &Patch) -> Result<Vec<u8>, CyberError> {
    let width = encode_dimension("width", patch.image.width)?;
    let height = encode_dimension("height", patch.image.height)?;
    let table_len = table_length(width)?;
    let (offsets, body) = encode_columns(&patch.image, table_len)?;
    let mut out = Writer::with_capacity(table_len.saturating_add(body.len()));
    out.put_u16(width);
    out.put_u16(height);
    out.put_i16(patch.left_offset);
    out.put_i16(patch.top_offset);
    for offset in offsets {
        out.put_u32(offset);
    }
    out.put_bytes(&body);
    Ok(out.into_inner())
}

/// Validates one encoded dimension and returns its `u16` header form.
fn encode_dimension(name: &str, value: u32) -> Result<u16, CyberError> {
    if value == 0 {
        return Err(CyberError::InvalidArgument(format!(
            "patch {name} is 0; encode an image at least 1 pixel wide and high"
        )));
    }
    if value > MAX_PATCH_DIM {
        return Err(CyberError::LimitExceeded(format!(
            "patch {name} {value} exceeds the {MAX_PATCH_DIM}-pixel limit; resize the image first"
        )));
    }
    u16::try_from(value).map_err(|error| {
        CyberError::LimitExceeded(format!(
            "patch {name} {value} does not fit the 16-bit header field: {error}"
        ))
    })
}

/// Bytes of the header plus `width` column offset entries.
fn table_length(width: u16) -> Result<usize, CyberError> {
    let table = COLUMN_OFFSET_LEN
        .checked_mul(usize::from(width))
        .ok_or_else(|| CyberError::LimitExceeded("patch column table overflows".to_owned()))?;
    PATCH_HEADER_LEN.checked_add(table).ok_or_else(|| {
        CyberError::LimitExceeded("patch column table length does not fit".to_owned())
    })
}

/// Encodes every column, returning the table offsets and the concatenated
/// column bytes that follow the table.
fn encode_columns(image: &IndexedImage, table_len: usize) -> Result<EncodedColumns, CyberError> {
    let mut body = Writer::new();
    let mut offsets = Vec::with_capacity(usize::try_from(image.width).unwrap_or(0));
    for x in 0..image.width {
        let offset = table_len.checked_add(body.len()).ok_or_else(|| {
            CyberError::LimitExceeded(format!("patch column {x} offset does not fit"))
        })?;
        offsets.push(usize_to_u32(offset, "patch column offset")?);
        write_column(image, x, &mut body);
    }
    Ok((offsets, body.into_inner()))
}

/// Writes every post of column `x` plus its `0xFF` terminator.
fn write_column(image: &IndexedImage, x: u32, out: &mut Writer) {
    let mut last_top = 0_u32;
    for (start, pixels) in column_runs(image, x) {
        write_run(out, start, &pixels, &mut last_top);
    }
    out.put_u8(COLUMN_END);
}

/// The maximal opaque vertical runs of column `x`, each as its top row
/// and pixel bytes.
fn column_runs(image: &IndexedImage, x: u32) -> Vec<(u32, Vec<u8>)> {
    let mut runs = Vec::new();
    let mut run = PostRun::default();
    for y in 0..image.height {
        match image.get(x, y) {
            Some((index, true)) => run.push(y, index),
            Some((_, false)) | None => run.finish(&mut runs),
        }
    }
    run.finish(&mut runs);
    runs
}

/// The opaque run currently being collected in one column.
#[derive(Default)]
struct PostRun {
    /// Row of the run's first pixel.
    start: u32,
    /// Coloured pixels collected so far; empty between runs.
    pixels: Vec<u8>,
}

impl PostRun {
    /// Adds one pixel at row `y`, starting a run at `y` when needed.
    fn push(&mut self, y: u32, index: u8) {
        if self.pixels.is_empty() {
            self.start = y;
        }
        self.pixels.push(index);
    }

    /// Appends the collected run to `runs`, if any, and resets.
    fn finish(&mut self, runs: &mut Vec<(u32, Vec<u8>)>) {
        if !self.pixels.is_empty() {
            runs.push((self.start, core::mem::take(&mut self.pixels)));
        }
    }
}

/// Writes the posts covering one opaque run, split at the maximum post
/// length.
fn write_run(out: &mut Writer, start: u32, pixels: &[u8], last_top: &mut u32) {
    let stride = u32::from(MAX_POST_LEN);
    for (part, chunk) in pixels.chunks(usize::from(MAX_POST_LEN)).enumerate() {
        let rows = u32::try_from(part)
            .unwrap_or(u32::MAX)
            .saturating_mul(stride);
        if let Some(top) = start.checked_add(rows) {
            write_post(out, top, chunk, last_top);
        }
    }
}

/// Writes one post at row `top`, preceded by filler posts until a single
/// `topdelta` byte can reach it.
fn write_post(out: &mut Writer, top: u32, pixels: &[u8], last_top: &mut u32) {
    let delta = loop {
        if let Some(delta) = post_delta(*last_top, top) {
            break delta;
        }
        write_filler(out, last_top);
    };
    out.put_u8(delta);
    out.put_u8(u8::try_from(pixels.len()).unwrap_or(MAX_POST_LEN));
    out.put_u8(POST_PAD);
    out.put_bytes(pixels);
    out.put_u8(POST_PAD);
    *last_top = top;
}

/// The `topdelta` byte placing a post at `top` after one at `last_top`,
/// or `None` when no single byte can reach it. The very first post sits
/// at row `0` and writes `0` (it reads back relative to zero); later tops
/// must grow: an absolute byte needs `top` within `0..=254`, a relative
/// one needs the gap to fit both `254` and `last_top` so the decoder's
/// relative test recognises it.
fn post_delta(last_top: u32, top: u32) -> Option<u8> {
    if top == 0 && last_top == 0 {
        return Some(0_u8);
    }
    if top <= last_top {
        return None;
    }
    if top <= u32::from(MAX_TOPDELTA) {
        return u8::try_from(top).ok();
    }
    let delta = top.checked_sub(last_top)?;
    if delta <= u32::from(MAX_TOPDELTA) && delta <= last_top {
        return u8::try_from(delta).ok();
    }
    None
}

/// A zero-length filler post that lifts `last_top` toward the next real
/// post: absolute `254` while below it, relative `254` from there on.
fn write_filler(out: &mut Writer, last_top: &mut u32) {
    out.put_u8(MAX_TOPDELTA);
    out.put_u8(0_u8);
    out.put_u8(POST_PAD);
    out.put_u8(POST_PAD);
    if *last_top < u32::from(MAX_TOPDELTA) {
        *last_top = u32::from(MAX_TOPDELTA);
    } else {
        *last_top = last_top.saturating_add(u32::from(MAX_TOPDELTA));
    }
}

#[cfg(test)]
mod tests {
    use super::{
        COLUMN_OFFSET_LEN, Patch, decode_column, decode_patch, encode_patch, paint_post,
        patch_header, set_patch_offsets,
    };
    use crate::error::CyberError;
    use crate::gfx::image::IndexedImage;
    use crate::wad::bytes::u32_at;
    use crate::wad::classify::is_doom_patch;

    /// The header-plus-table size of a `width`-column patch.
    fn table_end(width: u16) -> usize {
        8_usize
            .checked_add(
                COLUMN_OFFSET_LEN
                    .checked_mul(usize::from(width))
                    .expect("test width"),
            )
            .expect("test table fits")
    }

    /// The per-pixel rule [`build`] applies.
    type PixelRule = fn(u32, u32) -> (u8, bool);

    /// A hand-built lump: header, one offset per column, then the column
    /// bytes in order.
    fn patch_lump(width: u16, height: u16, offsets: (i16, i16), columns: &[&[u8]]) -> Vec<u8> {
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
    fn post(delta: u8, pixels: &[u8]) -> Vec<u8> {
        let mut bytes = vec![delta, u8::try_from(pixels.len()).expect("test post fits")];
        bytes.push(0);
        bytes.extend_from_slice(pixels);
        bytes.push(0);
        bytes
    }

    /// An image from a per-pixel rule; transparent pixels get index 0,
    /// exactly as decoding produces.
    fn build(width: u32, height: u32, pixel: PixelRule) -> IndexedImage {
        let columns = usize::try_from(width).expect("test width");
        let total = columns
            .checked_mul(usize::try_from(height).expect("test height"))
            .expect("test size");
        let mut indices = Vec::with_capacity(total);
        let mut opaque = Vec::with_capacity(total);
        for n in 0..total {
            let x = u32::try_from(n.checked_rem(columns).expect("non-zero columns"))
                .expect("within width");
            let y = u32::try_from(n.checked_div(columns).expect("non-zero columns"))
                .expect("within height");
            let (index, visible) = pixel(x, y);
            indices.push(index.checked_mul(u8::from(visible)).unwrap_or(0));
            opaque.push(visible);
        }
        IndexedImage::new(width, height, indices, opaque).expect("test shapes are valid")
    }

    /// Deterministic speckles, a fully opaque and a fully empty column.
    fn speckle(x: u32, y: u32) -> (u8, bool) {
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
    fn full(_x: u32, _y: u32) -> (u8, bool) {
        (5, true)
    }

    /// Column 0 has runs at rows 10, 260 and 290; column 1 is a full
    /// 300-row column; the rest stays transparent.
    fn tall(x: u32, y: u32) -> (u8, bool) {
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

    #[test]
    fn decodes_a_hand_built_two_by_three_patch() {
        let column0 = [post(0, &[5, 7]), vec![0xFF_u8]].concat();
        let column1 = [post(2, &[9]), vec![0xFF_u8]].concat();
        let bytes = patch_lump(2, 3, (-3, -2), &[&column0, &column1]);
        let patch = decode_patch(&bytes).expect("the hand-built patch decodes");
        assert_eq!(patch.left_offset, -3, "left offset from the header");
        assert_eq!(patch.top_offset, -2, "top offset from the header");
        let expected = IndexedImage::new(
            2,
            3,
            vec![5, 0, 7, 0, 0, 9],
            vec![true, false, true, false, false, true],
        )
        .expect("2x3 is a valid shape");
        assert_eq!(
            patch.image, expected,
            "post pixels land in the right rows, holes stay transparent"
        );
    }

    #[test]
    fn round_trips_a_speckled_patch_with_holes_and_edges() {
        let patch = Patch {
            left_offset: -4,
            top_offset: -6,
            image: build(17, 40, speckle),
        };
        let bytes = encode_patch(&patch).expect("17x40 is encodable");
        assert!(is_doom_patch(&bytes), "an encoded patch classifies");
        let back = decode_patch(&bytes).expect("the bytes we just wrote decode");
        assert_eq!(
            back, patch,
            "speckles, holes, full and empty columns and the negative offsets round trip"
        );
        assert_eq!(
            back.image.get(16, 0),
            Some((0, false)),
            "the empty column stays transparent"
        );
        assert_eq!(
            back.image.get(3, 39),
            Some((7, true)),
            "the full column stays opaque"
        );
    }

    #[test]
    fn round_trips_a_tall_patch() {
        let patch = Patch {
            left_offset: 0,
            top_offset: 0,
            image: build(8, 300, tall),
        };
        let bytes = encode_patch(&patch).expect("8x300 is encodable");
        assert!(is_doom_patch(&bytes), "a tall patch still classifies");
        let column =
            usize::try_from(u32_at(&bytes, 8).expect("column 0 offset")).expect("test offset");
        let body = &bytes[column..];
        assert!(body.len() > 27, "column 0 holds all three posts");
        assert_eq!(body[0], 10, "the first post is absolute row 10");
        assert!(
            body.windows(4).any(|word| word == [254, 0, 0, 0]),
            "a filler post carries the column past row 254"
        );
        assert_eq!(body[18], 6, "relative topdelta reaches row 260");
        assert_eq!(body[27], 30, "relative topdelta reaches row 290");
        let back = decode_patch(&bytes).expect("the tall bytes decode");
        assert_eq!(back, patch, "tall posts round trip identically");
    }

    #[test]
    fn decodes_a_hand_made_dee_psea_column() {
        let mut column = post(200, &[1, 1]);
        column.extend_from_slice(&post(50, &[2, 2, 2]));
        column.extend_from_slice(&post(40, &[3]));
        column.push(0xFF);
        let bytes = patch_lump(1, 300, (0, 0), &[&column]);
        let patch = decode_patch(&bytes).expect("the DeePsea column decodes");
        assert_eq!(patch.image.get(0, 200), Some((1, true)), "absolute top 200");
        assert_eq!(
            patch.image.get(0, 201),
            Some((1, true)),
            "second pixel of the run"
        );
        assert_eq!(
            patch.image.get(0, 250),
            Some((2, true)),
            "relative 50 lands at 250"
        );
        assert_eq!(patch.image.get(0, 252), Some((2, true)), "run of three");
        assert_eq!(
            patch.image.get(0, 290),
            Some((3, true)),
            "relative 40 lands at 290"
        );
        assert_eq!(
            patch.image.get(0, 100),
            Some((0, false)),
            "gaps stay transparent"
        );
        assert_eq!(
            patch.image.get(0, 299),
            Some((0, false)),
            "below the last post too"
        );
    }

    /// Opaque only in rows 590..600: reaching them needs relative
    /// filler steps above row 254.
    fn top_run_only(_x: u32, y: u32) -> (u8, bool) {
        (8, (590..600).contains(&y))
    }

    #[test]
    fn reaches_the_top_of_a_very_tall_patch() {
        let patch = Patch {
            left_offset: 0,
            top_offset: 0,
            image: build(1, 600, top_run_only),
        };
        let bytes = encode_patch(&patch).expect("1x600 is encodable");
        assert!(is_doom_patch(&bytes), "a 600-row patch classifies");
        let back = decode_patch(&bytes).expect("the relative filler chain decodes");
        assert_eq!(back, patch, "filler posts and relative deltas round trip");
    }

    #[test]
    fn splits_long_runs_into_254_row_posts() {
        let patch = Patch {
            left_offset: 0,
            top_offset: 0,
            image: build(1, 300, full),
        };
        let bytes = encode_patch(&patch).expect("1x300 is encodable");
        assert!(is_doom_patch(&bytes), "a split column still classifies");
        let column =
            usize::try_from(u32_at(&bytes, 8).expect("column 0 offset")).expect("test offset");
        let body = &bytes[column..];
        assert!(body.len() > 308, "the split column is present in full");
        assert_eq!(body[0], 0, "the first post sits at row 0");
        assert_eq!(body[1], 254, "the run is split at 254 pixels");
        assert_eq!(body[258], 254, "the second post is absolute row 254");
        assert_eq!(body[259], 46, "and carries the remaining rows");
        assert_eq!(body[308], 0xFF, "then the column terminator");
        let back = decode_patch(&bytes).expect("the split column decodes");
        assert_eq!(back, patch, "split runs round trip identically");
    }

    #[test]
    fn decodes_a_column_paints_its_posts() {
        let column = [post(0, &[1, 2]), post(3, &[9]), vec![0xFF_u8]].concat();
        let bytes = patch_lump(1, 5, (0, 0), &[&column]);
        let mut image =
            IndexedImage::new(1, 5, vec![0; 5], vec![false; 5]).expect("1x5 is a valid shape");
        decode_column(&mut image, &bytes, 0, table_end(1)).expect("the column decodes");
        assert_eq!(image.get(0, 0), Some((1, true)), "first pixel of post one");
        assert_eq!(image.get(0, 1), Some((2, true)), "second pixel of post one");
        assert_eq!(
            image.get(0, 2),
            Some((0, false)),
            "the gap stays transparent"
        );
        assert_eq!(
            image.get(0, 3),
            Some((9, true)),
            "absolute deltas stay absolute"
        );
    }

    #[test]
    fn clips_posts_past_the_declared_height() {
        let bytes = patch_lump(
            1,
            2,
            (0, 0),
            &[&[post(1, &[4, 5, 6]), vec![0xFF_u8]].concat()],
        );
        let patch = decode_patch(&bytes).expect("overlong posts are tolerated");
        assert_eq!(
            patch.image.get(0, 1),
            Some((4, true)),
            "the first covered row"
        );
        assert_eq!(
            patch.image.get(0, 0),
            Some((0, false)),
            "row zero stays transparent"
        );
        assert_eq!(patch.image.height, 2, "the declared height is kept");
    }

    #[test]
    fn paint_post_ignores_out_of_bounds_columns() {
        let mut image =
            IndexedImage::new(1, 2, vec![0, 0], vec![false, false]).expect("1x2 is a valid shape");
        paint_post(&mut image, 5, 0, &[7, 8]);
        assert_eq!(
            image.get(0, 1),
            Some((0, false)),
            "an out-of-range x paints nothing"
        );
        paint_post(&mut image, 0, 1, &[7, 8]);
        assert_eq!(
            image.get(0, 1),
            Some((7, true)),
            "the in-range row is painted"
        );
    }

    #[test]
    fn patch_header_reads_the_eight_header_bytes_alone() {
        let bytes = patch_lump(2, 3, (-3, -2), &[&[0xFF_u8], &[0xFF_u8]]);
        let header = patch_header(&bytes).expect("a built patch starts with a header");
        assert_eq!((header.width, header.height), (2, 3), "dimensions");
        assert_eq!(
            (header.left_offset, header.top_offset),
            (-3, -2),
            "drawing offsets, negatives included"
        );
        let head_only: Vec<u8> = bytes.into_iter().take(8).collect();
        assert!(
            patch_header(&head_only).is_some(),
            "no column table is needed for the header"
        );
    }

    #[test]
    fn patch_header_rejects_bytes_that_carry_no_patch_header() {
        assert!(
            patch_header(&[1_u8, 0, 0]).is_none(),
            "shorter than a header"
        );
        assert!(
            patch_header(&patch_lump(0, 1, (0, 0), &[])).is_none(),
            "zero width"
        );
        assert!(
            patch_header(&patch_lump(4_097, 1, (0, 0), &[&[0xFF_u8]])).is_none(),
            "width past the cap"
        );
        assert!(patch_header(b"png?").is_none(), "four bytes of text");
    }

    #[test]
    fn set_patch_offsets_rewrites_only_the_two_header_fields() {
        let column0 = [post(0, &[5, 7]), vec![0xFF_u8]].concat();
        let bytes = patch_lump(2, 3, (-3, -2), &[&column0, &[0xFF_u8]]);
        let out = set_patch_offsets(&bytes, 5, -7).expect("a real header takes new offsets");
        assert_eq!(&out[..4], &bytes[..4], "width and height stay");
        assert_ne!(&out[4..8], &bytes[4..8], "the offset pair changed");
        assert_eq!(
            &out[8..],
            &bytes[8..],
            "table and bodies stay byte-identical"
        );
        let header = patch_header(&out).expect("the rewrite is still a header");
        assert_eq!(
            (header.left_offset, header.top_offset),
            (5, -7),
            "new offsets"
        );
        assert_eq!((header.width, header.height), (2, 3), "old dimensions");
        let before = decode_patch(&bytes).expect("the original decodes");
        let after = decode_patch(&out).expect("the rewrite round-trips through decode");
        assert_eq!(after.image, before.image, "the picture is untouched");
    }

    #[test]
    fn set_patch_offsets_refuses_non_patches_and_offsets_past_the_header() {
        let short = set_patch_offsets(b"png?", 1, 1).expect_err("four bytes are no header");
        assert_eq!(short.code(), "invalid_argument", "non-patch code");
        let zero = set_patch_offsets(&patch_lump(0, 1, (0, 0), &[]), 1, 1)
            .expect_err("zero width answers no header");
        assert_eq!(zero.code(), "invalid_argument", "corrupt header code");
        let bytes = patch_lump(1, 1, (0, 0), &[&[0xFF_u8]]);
        let far = set_patch_offsets(&bytes, 40000, 0).expect_err("past the i16 range");
        assert_eq!(far.code(), "invalid_argument", "far left code");
        assert!(
            far.to_string().contains("left_offset 40000 does not fit"),
            "the message names the field: {far}"
        );
        let low = set_patch_offsets(&bytes, 0, -40000).expect_err("past the i16 range");
        assert!(
            low.to_string().contains("top_offset -40000"),
            "the other field is named too: {low}"
        );
    }

    #[test]
    fn rejects_malformed_headers() {
        let zero =
            decode_patch(&patch_lump(0, 1, (0, 0), &[])).expect_err("zero width is no patch");
        assert_eq!(zero.code(), "format_error", "zero width code");
        assert!(
            zero.to_string().contains("width"),
            "names the field: {zero}"
        );
        let wide = decode_patch(&patch_lump(4_097, 1, (0, 0), &[&[0xFF_u8]]))
            .expect_err("width past the cap");
        assert_eq!(wide.code(), "format_error", "oversized width code");
        let tall = decode_patch(&patch_lump(1, 4_097, (0, 0), &[&[0xFF_u8]]))
            .expect_err("height past the cap");
        assert_eq!(tall.code(), "format_error", "oversized height code");
        let stub = decode_patch(&[1_u8, 0, 0]).expect_err("three bytes are no header");
        assert_eq!(stub.code(), "format_error", "truncated header code");
    }

    #[test]
    fn rejects_column_offsets_outside_the_lump() {
        let mut bytes = patch_lump(1, 1, (0, 0), &[&[0xFF_u8]]);
        bytes[8..12].copy_from_slice(&999_u32.to_le_bytes());
        let error = decode_patch(&bytes).expect_err("the offset points nowhere");
        assert_eq!(error.code(), "format_error", "bad offset code");
        assert!(
            error.to_string().contains("column 0"),
            "names the column: {error}"
        );
        let no_table = decode_patch(&[1_u8, 0, 1, 0, 0, 0, 0, 0]).expect_err("no table at all");
        assert_eq!(no_table.code(), "format_error", "missing table code");
        let half = decode_patch(&patch_lump(2, 1, (0, 0), &[&[0xFF_u8]]))
            .expect_err("width 2 needs two offsets");
        assert_eq!(half.code(), "format_error", "truncated table code");
    }

    #[test]
    fn rejects_truncated_posts_and_missing_terminators() {
        let cut = patch_lump(1, 2, (0, 0), &[&[0_u8, 5, 0, 1, 2]]);
        let error = decode_patch(&cut).expect_err("the post promises five bytes");
        assert_eq!(error.code(), "format_error", "truncated post code");
        let unterminated = patch_lump(1, 1, (0, 0), &[&post(0, &[7])]);
        let missing = decode_patch(&unterminated).expect_err("no 0xFF before the end");
        assert_eq!(missing.code(), "format_error", "missing terminator code");
    }

    #[test]
    fn encode_rejects_oversized_and_empty_images() {
        let wide = Patch {
            left_offset: 0,
            top_offset: 0,
            image: build(4_097, 1, full),
        };
        let CyberError::LimitExceeded(message) =
            encode_patch(&wide).expect_err("width past the cap")
        else {
            panic!("expected limit_exceeded for an oversized width");
        };
        assert!(message.contains("width"), "names the field: {message}");
        let tall = Patch {
            left_offset: 0,
            top_offset: 0,
            image: build(1, 4_097, full),
        };
        assert_eq!(
            encode_patch(&tall).expect_err("height past the cap").code(),
            "limit_exceeded",
            "oversized height code"
        );
        let empty = Patch {
            left_offset: 0,
            top_offset: 0,
            image: IndexedImage {
                width: 0,
                height: 1,
                indices: Vec::new(),
                opaque: Vec::new(),
            },
        };
        assert_eq!(
            encode_patch(&empty)
                .expect_err("no columns to encode")
                .code(),
            "invalid_argument",
            "zero width code"
        );
    }
}
