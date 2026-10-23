//! The patch encoder: a [`Patch`] to `Doom` patch lump bytes.
//!
//! [`encode_patch`] writes the header, the `width` column offsets counted
//! from the lump start and the concatenated column bodies. Each column's
//! maximal opaque vertical runs become posts, split at the longest run one
//! post carries; a post is `topdelta u8`, `length u8`, one unused pad
//! byte, the palette indices and one unused pad byte, and `0xFF` ends the
//! column. Rows a plain absolute `topdelta` cannot reach are written in
//! the `DeePsea` tall-patch convention, with zero-length filler posts
//! lifting the previous top until a single delta reaches the next post,
//! which is how [`crate::gfx::patch`] reads the column back.

use crate::error::CyberError;
use crate::gfx::image::IndexedImage;
use crate::gfx::patch::{
    COLUMN_END, COLUMN_OFFSET_LEN, EncodedColumns, MAX_PATCH_DIM, MAX_POST_LEN, MAX_TOPDELTA,
    PATCH_HEADER_LEN, POST_PAD, Patch,
};
use crate::wad::bytes::{Writer, usize_to_u32};

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
