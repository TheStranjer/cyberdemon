//! The private `grAb` `PNG` chunk that `ZDoom` and `SLADE` use to store
//! `Doom` graphic offsets.
//!
//! Layout: a big-endian `u32` length (always 8), the type `grAb`, the
//! payload (`x` and `y` as big-endian `i32`), and the `CRC-32` computed
//! over type plus payload. The crate denies `to_be_bytes`/`from_be_bytes`,
//! so big-endian values are produced and read by reversing the
//! little-endian byte arrays.

use crate::error::CyberError;

/// The `PNG` file signature every chunk walk starts from.
pub(crate) const PNG_SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1a, b'\n'];
/// The `grAb` chunk type.
const GRAB_TYPE: [u8; 4] = *b"grAb";
/// The mandatory first chunk of every `PNG`.
const IHDR_TYPE: [u8; 4] = *b"IHDR";
/// Size of the chunk length field, the chunk type, the chunk `CRC-32` and
/// one `grAb` offset word: all four bytes.
const FIELD_BYTES: usize = 4;
/// Bytes before a chunk payload: length plus type.
const HEADER_BYTES: usize = 8;
/// Bytes of a `grAb` payload: two offset words.
const GRAB_DATA_BYTES: usize = 8;
/// The `grAb` payload length as it appears in the big-endian length field.
const GRAB_DATA_LENGTH: u32 = 8_u32;
/// The whole `grAb` chunk: length, type, payload and `CRC-32`.
const GRAB_CHUNK_BYTES: usize = 20;

/// A pair of graphic offsets, as carried by a `grAb` chunk: `(x, y)`.
pub type GrabOffsets = (i32, i32);

/// A parsed chunk: its type, its payload and the bytes following it.
pub(crate) type Chunk<'bytes> = ([u8; 4], &'bytes [u8], &'bytes [u8]);

/// The error reported for any malformed chunk stream.
fn truncated() -> CyberError {
    CyberError::Format("PNG chunk stream is truncated or malformed".to_owned())
}

/// Reads a big-endian `u32` by reversing the bytes before decoding them
/// little-endian (the `big_endian_bytes` lint denies `from_be_bytes`).
pub(crate) fn read_u32_be(bytes: &[u8]) -> Option<u32> {
    let mut word = *bytes.first_chunk::<FIELD_BYTES>()?;
    word.reverse();
    Some(u32::from_le_bytes(word))
}

/// Reads a big-endian `i32` by reversing the bytes before decoding them
/// little-endian.
fn read_i32_be(bytes: &[u8]) -> Option<i32> {
    let mut word = *bytes.first_chunk::<FIELD_BYTES>()?;
    word.reverse();
    Some(i32::from_le_bytes(word))
}

/// Encodes a `u32` big-endian by reversing its little-endian bytes.
fn be_u32(value: u32) -> [u8; 4] {
    let mut word = value.to_le_bytes();
    word.reverse();
    word
}

/// Encodes an `i32` big-endian by reversing its little-endian bytes.
fn be_i32(value: i32) -> [u8; 4] {
    let mut word = value.to_le_bytes();
    word.reverse();
    word
}

/// The chunk stream after a valid `PNG` signature.
fn after_signature(png: &[u8]) -> Result<&[u8], CyberError> {
    png.strip_prefix(&PNG_SIGNATURE)
        .ok_or_else(|| CyberError::Format("not a PNG file: bad signature".to_owned()))
}

/// Splits the next chunk into its type, payload and the remaining bytes;
/// `None` when the stream ends mid-chunk.
pub(crate) fn next_chunk(bytes: &[u8]) -> Option<Chunk<'_>> {
    let length = usize::try_from(read_u32_be(bytes)?).ok()?;
    let data_end = HEADER_BYTES.checked_add(length)?;
    let total = data_end.checked_add(FIELD_BYTES)?;
    let kind = *bytes
        .get(FIELD_BYTES..HEADER_BYTES)?
        .first_chunk::<FIELD_BYTES>()?;
    Some((
        kind,
        bytes.get(HEADER_BYTES..data_end)?,
        bytes.get(total..)?,
    ))
}

/// Decodes the two big-endian offset words of a `grAb` payload.
fn parse_offsets(data: &[u8]) -> Result<(i32, i32), CyberError> {
    let bad = || {
        CyberError::Format(format!(
            "grAb chunk holds {} bytes, expected {GRAB_DATA_BYTES}",
            data.len()
        ))
    };
    if data.len() != GRAB_DATA_BYTES {
        return Err(bad());
    }
    let mut words = data.chunks_exact(FIELD_BYTES);
    let x = read_i32_be(words.next().ok_or_else(bad)?).ok_or_else(bad)?;
    let y = read_i32_be(words.next().ok_or_else(bad)?).ok_or_else(bad)?;
    Ok((x, y))
}

/// Builds a complete `grAb` chunk: length, type, offsets and `CRC-32`
/// over type plus payload.
fn grab_chunk(x: i32, y: i32) -> Vec<u8> {
    let mut chunk = Vec::with_capacity(GRAB_CHUNK_BYTES);
    chunk.extend_from_slice(&be_u32(GRAB_DATA_LENGTH));
    chunk.extend_from_slice(&GRAB_TYPE);
    chunk.extend_from_slice(&be_i32(x));
    chunk.extend_from_slice(&be_i32(y));
    let crc = crc32fast::hash(chunk.split_at(FIELD_BYTES).1);
    chunk.extend_from_slice(&be_u32(crc));
    chunk
}

/// The byte offset just after the `IHDR` chunk.
fn ihdr_end(png: &[u8]) -> Result<usize, CyberError> {
    let mut offset = PNG_SIGNATURE.len();
    let mut rest = after_signature(png)?;
    loop {
        let (kind, _data, next) = next_chunk(rest).ok_or_else(truncated)?;
        let consumed = rest.len().saturating_sub(next.len());
        let Some(advanced) = offset.checked_add(consumed) else {
            return Err(CyberError::LimitExceeded(
                "PNG chunk stream exceeds addressable size".to_owned(),
            ));
        };
        offset = advanced;
        if kind == IHDR_TYPE {
            return Ok(offset);
        }
        if next.is_empty() {
            return Err(CyberError::Format("PNG has no IHDR chunk".to_owned()));
        }
        rest = next;
    }
}

/// Re-assembles a chunk stream with any `grAb` chunk removed.
fn strip_grab(chunks: &[u8]) -> Result<Vec<u8>, CyberError> {
    let mut out: Vec<u8> = Vec::new();
    let mut rest = chunks;
    loop {
        let (kind, _data, next) = next_chunk(rest).ok_or_else(truncated)?;
        if kind != GRAB_TYPE {
            let kept = rest.len().saturating_sub(next.len());
            out.extend_from_slice(rest.get(..kept).unwrap_or_default());
        }
        if next.is_empty() {
            return Ok(out);
        }
        rest = next;
    }
}

/// The `grAb` offsets stored in a `PNG`, or `None` when it has no such chunk.
///
/// Chunk `CRC-32`s are not verified.
///
/// # Errors
///
/// [`CyberError::Format`] when `png` does not start with the `PNG`
/// signature, its chunk stream is truncated, or a `grAb` payload is not
/// 8 bytes long.
#[inline]
pub fn read_grab(png: &[u8]) -> Result<Option<GrabOffsets>, CyberError> {
    let mut rest = after_signature(png)?;
    loop {
        let (kind, data, next) = next_chunk(rest).ok_or_else(truncated)?;
        if kind == GRAB_TYPE {
            return Ok(Some(parse_offsets(data)?));
        }
        if next.is_empty() {
            return Ok(None);
        }
        rest = next;
    }
}

/// Inserts a `grAb` chunk with offsets `(x, y)` right after `IHDR`,
/// replacing any `grAb` chunk already present.
///
/// # Errors
///
/// [`CyberError::Format`] when `png` is not a well-formed chunked `PNG`
/// (bad signature, truncated stream, or no `IHDR`).
#[inline]
pub fn insert_grab(png: &[u8], x: i32, y: i32) -> Result<Vec<u8>, CyberError> {
    let insert_at = ihdr_end(png)?;
    let prefix = png.get(..insert_at).ok_or_else(truncated)?;
    let tail = png.get(insert_at..).ok_or_else(truncated)?;
    let chunk = grab_chunk(x, y);
    let mut out = Vec::with_capacity(png.len().saturating_add(chunk.len()));
    out.extend_from_slice(prefix);
    out.extend_from_slice(&chunk);
    out.extend_from_slice(&strip_grab(tail)?);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::{
        FIELD_BYTES, GRAB_CHUNK_BYTES, GRAB_DATA_BYTES, GRAB_DATA_LENGTH, PNG_SIGNATURE, be_i32,
        be_u32, insert_grab, read_grab, read_u32_be,
    };

    /// A minimal chunked PNG: signature, `IHDR`, `IDAT` and `IEND`, built
    /// by the `image` crate's encoder.
    fn minimal_png() -> Vec<u8> {
        use image::codecs::png::PngEncoder;
        use image::{ExtendedColorType, ImageEncoder as _};
        let mut buffer: Vec<u8> = Vec::new();
        PngEncoder::new(std::io::Cursor::new(&mut buffer))
            .write_image(&[1, 2, 3, 4], 1, 1, ExtendedColorType::Rgba8)
            .expect("encoding a 1x1 PNG works");
        buffer
    }

    /// The expected `grAb` chunk bytes for `(x, y)`, with an independently
    /// assembled `CRC-32` over type plus payload.
    fn expected_chunk(x: i32, y: i32) -> Vec<u8> {
        let mut chunk = be_u32(GRAB_DATA_LENGTH).to_vec();
        chunk.extend_from_slice(b"grAb");
        chunk.extend_from_slice(&be_i32(x));
        chunk.extend_from_slice(&be_i32(y));
        let crc = crc32fast::hash(chunk.split_at(FIELD_BYTES).1);
        chunk.extend_from_slice(&be_u32(crc));
        chunk
    }

    /// The byte offset just after the `IHDR` chunk of a PNG.
    fn after_ihdr(png: &[u8]) -> usize {
        let length_start = PNG_SIGNATURE.len();
        let length_end = length_start.checked_add(FIELD_BYTES).expect("offset fits");
        let type_end = length_end.checked_add(FIELD_BYTES).expect("offset fits");
        let ihdr_len = read_u32_be(
            png.get(length_start..length_end)
                .expect("IHDR length present"),
        )
        .expect("the length field parses");
        type_end
            .checked_add(usize::try_from(ihdr_len).expect("IHDR length fits"))
            .and_then(|n| n.checked_add(FIELD_BYTES))
            .expect("offset fits")
    }

    #[test]
    fn insert_then_read_returns_the_offsets() {
        let png = minimal_png();
        assert!(
            read_grab(&png).expect("valid PNG").is_none(),
            "a fresh PNG has no grAb"
        );
        let with_grab = insert_grab(&png, 16, -32).expect("inserting into a valid PNG works");
        assert_eq!(
            read_grab(&with_grab).expect("the result parses"),
            Some((16_i32, -32_i32)),
            "negative y survives the round trip"
        );
    }

    #[test]
    fn grab_chunk_sits_right_after_ihdr_with_a_correct_crc() {
        let png = minimal_png();
        let with_grab = insert_grab(&png, -5, 9).expect("insert works");
        let start = after_ihdr(&with_grab);
        let grab_end = start
            .checked_add(GRAB_CHUNK_BYTES)
            .expect("chunk offset fits");
        assert_eq!(
            with_grab.get(start..grab_end),
            Some(expected_chunk(-5, 9).as_slice()),
            "the chunk right after IHDR is a grAb chunk with the right CRC"
        );
    }

    #[test]
    fn insert_replaces_an_existing_chunk_instead_of_duplicating() {
        let png = minimal_png();
        let once = insert_grab(&png, 1, 2).expect("first insert");
        let twice = insert_grab(&once, 3, 4).expect("second insert");
        assert_eq!(twice.len(), once.len(), "replacement keeps the file size");
        assert_eq!(
            read_grab(&twice).expect("parses"),
            Some((3_i32, 4_i32)),
            "new offsets win"
        );
        assert_eq!(
            twice
                .windows(FIELD_BYTES)
                .filter(|window| *window == *b"grAb")
                .count(),
            1,
            "exactly one grAb chunk remains"
        );
    }

    #[test]
    fn bad_signature_is_a_format_error() {
        let error = read_grab(b"not a png at all").expect_err("signature must be checked");
        assert_eq!(error.code(), "format_error", "bad signature code");
        let insert_error = insert_grab(b"JUNK", 0, 0).expect_err("insert checks too");
        assert_eq!(
            insert_error.code(),
            "format_error",
            "insert bad signature code"
        );
    }

    #[test]
    fn truncated_stream_is_a_format_error() {
        let png = minimal_png();
        let signature_only = png.get(..PNG_SIGNATURE.len()).expect("signature present");
        let error = read_grab(signature_only).expect_err("a file with no chunks is truncated");
        assert_eq!(error.code(), "format_error", "truncated stream code");
        let insert_error = insert_grab(signature_only, 0, 0).expect_err("no IHDR either");
        assert_eq!(insert_error.code(), "format_error", "missing IHDR code");
    }

    #[test]
    fn chunk_claiming_more_bytes_than_present_is_a_format_error() {
        let mut crafted: Vec<u8> = PNG_SIGNATURE.to_vec();
        crafted.extend_from_slice(&be_u32(1_000_u32));
        crafted.extend_from_slice(b"grAb");
        crafted.extend_from_slice(&[0_u8; GRAB_DATA_BYTES]);
        let error = read_grab(&crafted).expect_err("declared length exceeds the file");
        assert_eq!(error.code(), "format_error", "oversized chunk code");
    }

    #[test]
    fn grab_with_wrong_payload_length_is_a_format_error() {
        let mut crafted: Vec<u8> = PNG_SIGNATURE.to_vec();
        crafted.extend_from_slice(&be_u32(4_u32));
        crafted.extend_from_slice(b"grAb");
        crafted.extend_from_slice(&[0_u8; FIELD_BYTES]);
        crafted.extend_from_slice(&[0_u8; FIELD_BYTES]);
        let error = read_grab(&crafted).expect_err("four-byte grAb payload is invalid");
        assert_eq!(error.code(), "format_error", "short payload code");
        assert!(
            error.to_string().contains("expected 8"),
            "message names the expected length: {error}"
        );
    }

    #[test]
    fn zero_offsets_read_back() {
        let png = minimal_png();
        let with_grab = insert_grab(&png, 0, 0).expect("insert (0, 0)");
        assert_eq!(
            read_grab(&with_grab).expect("parses"),
            Some((0_i32, 0_i32)),
            "zero round trip"
        );
    }
}
