//! Graphics building blocks shared by every WAD graphics tool.
//!
//! Pure data processing on owned buffers: [`image`] types with validated
//! constructors, [`codec`] decode/encode for `PNG`, `JPEG`, `GIF` and `BMP`,
//! the private [`grab`] `grAb` `PNG` chunk, [`palette`] handling,
//! [`quantize`] colour mapping, the [`ops`] image-operation framework and
//! the `Doom` lump codecs [`patch`] (columns, posts and `DeePsea` tall
//! patches) and [`flat`] (raw indexed planes). Nothing here knows about MCP
//! or the filesystem; later sprite and texture tools compose these pieces.

pub mod codec;
pub mod flat;
pub mod grab;
pub mod image;
pub mod ops;
pub mod palette;
pub mod patch;
pub mod quantize;

use crate::error::CyberError;

/// Largest edge (width or height) any image constructor accepts.
pub const MAX_DIMENSION: u32 = 8_192;

/// Bytes per `RGBA8` pixel.
pub(crate) const BYTES_PER_PIXEL: usize = 4;

/// Rejects an image dimension outside `1..=MAX_DIMENSION`.
pub(crate) fn check_dimension(name: &str, value: u32) -> Result<(), CyberError> {
    if value == 0 || value > MAX_DIMENSION {
        return Err(CyberError::InvalidArgument(format!(
            "image {name} must be within 1..={MAX_DIMENSION}, got {value}"
        )));
    }
    Ok(())
}

/// Saturates a requested dimension into `1..=MAX_DIMENSION`.
pub(crate) const fn clamp_dimension(value: u32) -> u32 {
    if value == 0 {
        1
    } else if value > MAX_DIMENSION {
        MAX_DIMENSION
    } else {
        value
    }
}

/// `width * height` as a `usize`, or `None` if the product does not fit.
pub(crate) fn pixel_count(width: u32, height: u32) -> Option<usize> {
    let columns = usize::try_from(width).ok()?;
    let rows = usize::try_from(height).ok()?;
    columns.checked_mul(rows)
}

/// `width * height * 4`, the `RGBA8` buffer length for a shape, or `None`
/// if it does not fit in memory addressing.
pub(crate) fn rgba_byte_count(width: u32, height: u32) -> Option<usize> {
    pixel_count(width, height)?.checked_mul(BYTES_PER_PIXEL)
}

/// The row-major pixel number of `(x, y)` in an image `width` wide, or
/// `None` if the coordinates or the product do not fit.
pub(crate) fn pixel_index(width: u32, x: u32, y: u32) -> Option<usize> {
    let row_start = pixel_count(width, y)?;
    row_start.checked_add(usize::try_from(x).ok()?)
}

/// Copies a 4-byte slice from a `chunks_exact(4)` walk into a pixel array.
pub(crate) fn to_pixel(chunk: &[u8]) -> [u8; 4] {
    let mut pixel = [0_u8; BYTES_PER_PIXEL];
    pixel.copy_from_slice(chunk);
    pixel
}
