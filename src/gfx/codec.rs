//! Format sniffing and conversion between encoded bytes and owned images.
//!
//! [`decode_image`] accepts `PNG`, `JPEG`, `GIF` (first frame) and `BMP`;
//! [`encode_png`] writes `PNG` and can carry `Doom` graphic offsets in the
//! private `grAb` chunk from [`crate::gfx::grab`].

use std::io::Cursor;

use image::codecs::png::PngEncoder;
use image::{ExtendedColorType, ImageEncoder as _, ImageError, ImageFormat, ImageReader, Limits};

use crate::error::CyberError;
use crate::gfx::MAX_DIMENSION;
use crate::gfx::grab;
use crate::gfx::image::RgbaImage;

/// Decoder resource limits: [`MAX_DIMENSION`] per edge, no allocation cap
/// beyond the shape that implies.
fn decode_limits() -> Limits {
    let mut limits = Limits::no_limits();
    limits.max_image_width = Some(MAX_DIMENSION);
    limits.max_image_height = Some(MAX_DIMENSION);
    limits
}

/// Maps a decoder failure: limit violations keep their meaning, anything
/// else means the bytes are not valid data of the sniffed format.
fn map_decode_error(error: &ImageError) -> CyberError {
    if matches!(*error, ImageError::Limits(_)) {
        CyberError::LimitExceeded(format!(
            "image exceeds the {MAX_DIMENSION}x{MAX_DIMENSION} decode limit: {error}"
        ))
    } else {
        CyberError::Format(format!("cannot decode the image: {error}"))
    }
}

/// Checks the shape of a freshly decoded buffer against the [`RgbaImage`]
/// invariants.
fn check_decoded_size(width: u32, height: u32) -> Result<(), CyberError> {
    if width == 0 || height == 0 {
        return Err(CyberError::Format(
            "the decoded image has no pixels".to_owned(),
        ));
    }
    if width > MAX_DIMENSION || height > MAX_DIMENSION {
        return Err(CyberError::LimitExceeded(format!(
            "image is {width}x{height}, the decode limit is {MAX_DIMENSION} per edge"
        )));
    }
    Ok(())
}

/// Decodes `PNG`, `JPEG`, `GIF` (first frame) or `BMP` bytes into an
/// `RGBA8` [`RgbaImage`].
///
/// The format is sniffed from the bytes; the file name never matters.
///
/// # Errors
///
/// [`CyberError::Format`] when the bytes are not recognisable or are
/// corrupt; [`CyberError::Unsupported`] for any other format;
/// [`CyberError::LimitExceeded`] when the image is larger than
/// [`MAX_DIMENSION`] per edge.
#[inline]
pub fn decode_image(bytes: &[u8]) -> Result<RgbaImage, CyberError> {
    let format = image::guess_format(bytes).map_err(|error| {
        CyberError::Format(format!("cannot identify the image format: {error}"))
    })?;
    if !matches!(
        format,
        ImageFormat::Png | ImageFormat::Jpeg | ImageFormat::Gif | ImageFormat::Bmp
    ) {
        return Err(CyberError::Unsupported(format!(
            "{format:?} input cannot be decoded; supply PNG, JPEG, GIF or BMP bytes"
        )));
    }
    let mut reader = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|error| CyberError::Format(format!("cannot read the image header: {error}")))?;
    reader.limits(decode_limits());
    let decoded = reader.decode().map_err(|error| map_decode_error(&error))?;
    let rgba = decoded.into_rgba8();
    let width = rgba.width();
    let height = rgba.height();
    check_decoded_size(width, height)?;
    Ok(RgbaImage {
        width,
        height,
        pixels: rgba.into_raw(),
    })
}

/// Encodes an `RGBA8` image as `PNG`.
///
/// When `offsets` is `Some((x, y))` a `grAb` chunk holding them is
/// inserted right after `IHDR`, the way `ZDoom` and `SLADE` store `Doom`
/// graphic offsets.
///
/// # Errors
///
/// [`CyberError::Internal`] when the encoder itself fails;
/// [`CyberError::Format`] can only come from the `grAb` insertion and
/// cannot occur on freshly encoded bytes.
#[inline]
pub fn encode_png(img: &RgbaImage, offsets: Option<(i32, i32)>) -> Result<Vec<u8>, CyberError> {
    let mut buffer: Vec<u8> = Vec::new();
    PngEncoder::new(Cursor::new(&mut buffer))
        .write_image(&img.pixels, img.width, img.height, ExtendedColorType::Rgba8)
        .map_err(|error| CyberError::Internal(format!("PNG encoding failed: {error}")))?;
    match offsets {
        Some((x, y)) => grab::insert_grab(&buffer, x, y),
        None => Ok(buffer),
    }
}

#[cfg(test)]
mod tests {
    use super::{decode_image, encode_png};
    use crate::gfx::grab::read_grab;
    use crate::gfx::image::RgbaImage;

    /// A 3x2 image with six known fully-opaque pixels.
    fn sample() -> RgbaImage {
        let pixels: Vec<u8> = [
            [255_u8, 0, 0, 255],
            [0, 255, 0, 255],
            [0, 0, 255, 255],
            [255, 255, 0, 255],
            [255, 0, 255, 255],
            [0, 255, 255, 255],
        ]
        .into_iter()
        .flatten()
        .collect();
        RgbaImage::new(3, 2, pixels).expect("six pixels fit a 3x2 image")
    }

    /// Encodes with the `image` crate's own `JPEG` encoder (which takes
    /// `RGB`, not `RGBA`).
    fn encode_jpeg(img: &RgbaImage) -> Vec<u8> {
        use image::codecs::jpeg::JpegEncoder;
        use image::{ExtendedColorType, ImageEncoder as _};
        let rgb: Vec<u8> = img
            .pixels
            .chunks_exact(4)
            .flat_map(|pixel| {
                let [r, g, b, _] = crate::gfx::to_pixel(pixel);
                [r, g, b]
            })
            .collect();
        let mut out: Vec<u8> = Vec::new();
        JpegEncoder::new_with_quality(std::io::Cursor::new(&mut out), 95)
            .write_image(&rgb, img.width, img.height, ExtendedColorType::Rgb8)
            .expect("a 3x2 image encodes as JPEG");
        out
    }

    /// Encodes with the `image` crate's own `GIF` encoder (single frame).
    fn encode_gif(img: &RgbaImage) -> Vec<u8> {
        use image::codecs::gif::GifEncoder;
        use image::{ExtendedColorType, ImageEncoder as _};
        let mut out: Vec<u8> = Vec::new();
        GifEncoder::new(std::io::Cursor::new(&mut out))
            .write_image(&img.pixels, img.width, img.height, ExtendedColorType::Rgba8)
            .expect("a 3x2 image encodes as GIF");
        out
    }

    /// Encodes with the `image` crate's own `BMP` encoder.
    fn encode_bmp(img: &RgbaImage) -> Vec<u8> {
        use image::codecs::bmp::BmpEncoder;
        use image::{ExtendedColorType, ImageEncoder as _};
        let mut out: Vec<u8> = Vec::new();
        BmpEncoder::new(&mut out)
            .write_image(&img.pixels, img.width, img.height, ExtendedColorType::Rgba8)
            .expect("a 3x2 image encodes as BMP");
        out
    }

    /// Asserts both images share a shape and every byte is within
    /// `tolerance` of its counterpart.
    fn assert_close(actual: &RgbaImage, expected: &RgbaImage, tolerance: i32, context: &str) {
        assert_eq!(
            (actual.width, actual.height),
            (expected.width, expected.height),
            "{context}: shape differs"
        );
        assert_eq!(
            actual.pixels.len(),
            expected.pixels.len(),
            "{context}: byte count"
        );
        for (number, (got, want)) in actual.pixels.iter().zip(expected.pixels.iter()).enumerate() {
            let delta = i32::from(*got).saturating_sub(i32::from(*want)).abs();
            assert!(
                delta <= tolerance,
                "{context}: byte {number} differs by {delta}: {got} vs {want}"
            );
        }
    }

    #[test]
    fn png_round_trips_exactly() {
        let img = sample();
        let encoded = encode_png(&img, None).expect("encoding the sample works");
        let decoded = decode_image(&encoded).expect("the encoded PNG decodes");
        assert_eq!(decoded, img, "PNG is lossless");
    }

    #[test]
    fn encode_png_can_carry_grab_offsets() {
        let img = sample();
        let encoded =
            encode_png(&img, Some((-7_i32, 12_i32))).expect("encoding with offsets works");
        assert_eq!(
            read_grab(&encoded).expect("the encoded PNG parses"),
            Some((-7_i32, 12_i32)),
            "the grAb chunk survives encoding"
        );
        assert_eq!(
            decode_image(&encoded).expect("offsets do not break decoding"),
            img,
            "pixels are untouched"
        );
    }

    #[test]
    fn jpeg_decodes_loosely() {
        let img = sample();
        let decoded = decode_image(&encode_jpeg(&img)).expect("the JPEG decodes");
        assert_close(&decoded, &img, 20, "JPEG is lossy");
    }

    #[test]
    fn gif_decodes_the_first_frame() {
        let img = sample();
        let decoded = decode_image(&encode_gif(&img)).expect("the GIF decodes");
        assert_close(&decoded, &img, 0, "GIF keeps the six exact colours");
    }

    #[test]
    fn bmp_decodes_exactly() {
        let img = sample();
        let decoded = decode_image(&encode_bmp(&img)).expect("the BMP decodes");
        assert_eq!(decoded, img, "32-bit BMP is lossless");
    }

    #[test]
    fn other_formats_are_unsupported() {
        let mut qoi_like: Vec<u8> = b"qoif".to_vec();
        qoi_like.extend_from_slice(&[0_u8, 0, 0, 3, 0, 0, 0, 2]);
        let error = decode_image(&qoi_like).expect_err("QOI must be refused");
        assert_eq!(error.code(), "unsupported", "undecodable format code");
        assert!(
            error.to_string().contains("Qoi"),
            "message names the format: {error}"
        );
    }

    #[test]
    fn garbage_is_a_format_error() {
        let error = decode_image(&[0_u8, 1, 2, 3, 4, 5]).expect_err("no format in five bytes");
        assert_eq!(error.code(), "format_error", "unrecognisable bytes code");
        let gif_header = b"GIF89a\x01\x00\x01\x00\x00\x00\x00";
        let short_error = decode_image(gif_header).expect_err("a header without data is corrupt");
        assert_eq!(short_error.code(), "format_error", "truncated stream code");
    }

    #[test]
    fn corrupted_png_bytes_are_a_format_error() {
        let png = encode_png(&sample(), None).expect("encode");
        let middle = png.len().checked_div(2).expect("the file has bytes");
        let mut broken = png;
        broken[middle] = broken[middle].wrapping_add(0xA5);
        let error = decode_image(&broken).expect_err("corrupted PNG data must fail");
        assert_eq!(error.code(), "format_error", "corrupted PNG code");
    }

    #[test]
    fn oversized_images_hit_the_limit() {
        use image::codecs::png::PngEncoder;
        use image::{ExtendedColorType, ImageEncoder as _};
        let mut encoded: Vec<u8> = Vec::new();
        PngEncoder::new(std::io::Cursor::new(&mut encoded))
            .write_image(&vec![7_u8; 0x8CA0], 9_000, 1, ExtendedColorType::Rgba8)
            .expect("encoding an oversized PNG works");
        let error = decode_image(&encoded).expect_err("decoding must enforce the limit");
        assert_eq!(error.code(), "limit_exceeded", "oversized decode code");
    }

    #[test]
    fn constructors_still_guard_the_same_limit() {
        let error =
            RgbaImage::new(9_000, 1, vec![0; 0x8CA0]).expect_err("9000 exceeds MAX_DIMENSION");
        assert_eq!(error.code(), "invalid_argument", "constructor limit code");
    }
}
