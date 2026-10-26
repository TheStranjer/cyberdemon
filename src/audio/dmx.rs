//! `Doom` `DS*` sound lumps: the `DMX` format-3 encoding of unsigned
//! `8`-bit mono audio, with the vanilla leading/trailing pad bytes.
//!
//! [`decode_dmx`] detects and strips the padding the engine never plays;
//! [`encode_dmx`] writes it in exactly the shape the decoder looks for, or
//! leaves it off.

use crate::audio::pcm::Pcm;
use crate::error::CyberError;
use crate::wad::bytes::{Reader, Writer, u32_to_usize, usize_to_u32};

/// The `DMX` format tag of `Doom` `8`-bit unsigned sound data.
const DMX_FORMAT: u16 = 3_u16;
/// The same tag read back from bytes written big-endian by some `Mac`
/// tools; the sample rate and the sample count are byte-swapped in those
/// lumps too.
const DMX_FORMAT_SWAPPED: u16 = 0x0300_u16;
/// Header size: format, sample rate and sample count.
const DMX_HEADER_LEN: usize = 8;
/// Pad bytes the vanilla engine keeps at each end of the sample data.
const DMX_PADDING: usize = 16;
/// Silence in unsigned `8`-bit storage, the fallback pad byte.
const DMX_SILENCE: u8 = 128_u8;

/// Decodes a `DMX` sound lump into a mono 16-bit [`Pcm`].
///
/// A lump written big-endian by some `Mac` tools has its format tag,
/// sample rate *and* sample count all byte-swapped; every header field is
/// restored here.
///
/// Padding is detected by its bytes, not assumed: when the sample data is
/// longer than `32` bytes and its first `16` bytes all repeat the byte
/// after them (or its last `16` all repeat the byte before them), that end
/// is the vanilla pad and is stripped; each end is checked on its own. A
/// body that does not look padded is kept whole, so
/// `decode(encode(x, pad = false))` loses nothing — the one exception is
/// a sound that itself opens (or closes) with `16` identical sample bytes,
/// which is indistinguishable from the pad `DMX` writes and is trimmed.
///
/// # Errors
///
/// [`CyberError::Format`] when the lump is shorter than its header, the
/// format tag is neither `3` nor its byte-swapped form, the sample count
/// is zero, or the declared samples run past the end of the lump.
#[inline]
pub fn decode_dmx(bytes: &[u8]) -> Result<Pcm, CyberError> {
    let mut reader = Reader::new(bytes);
    let raw_format = reader.read_u16()?;
    if raw_format != DMX_FORMAT && raw_format != DMX_FORMAT_SWAPPED {
        return Err(CyberError::Format(format!(
            "DMX sound lump declares format {raw_format}, only format {DMX_FORMAT} is a Doom sound; check the lump name starts with DS"
        )));
    }
    let swapped = raw_format == DMX_FORMAT_SWAPPED;
    let raw_rate = reader.read_u16()?;
    let sample_rate = if swapped {
        raw_rate.swap_bytes()
    } else {
        raw_rate
    };
    let raw_count = reader.read_u32()?;
    let count = u32_to_usize(if swapped {
        raw_count.swap_bytes()
    } else {
        raw_count
    })?;
    if count == 0 {
        return Err(CyberError::Format(
            "DMX sound lump declares zero samples; re-export the sound".to_owned(),
        ));
    }
    let end = DMX_HEADER_LEN
        .checked_add(count)
        .filter(|last| *last <= bytes.len())
        .ok_or_else(|| {
            CyberError::Format(format!(
                "DMX sound lump declares {count} samples but holds only {}",
                bytes.len().saturating_sub(DMX_HEADER_LEN)
            ))
        })?;
    let body = bytes.get(DMX_HEADER_LEN..end).unwrap_or(&[]);
    Ok(Pcm::from_u8_unsigned(
        strip_padding(body),
        u32::from(sample_rate),
    ))
}

/// Drops the vanilla `16`-byte pad from either end of the sample data
/// when that end is filled the way `DMX` (and [`encode_dmx`]) fill it:
/// `16` copies of the adjacent sample byte. Bodies of `32` bytes or
/// fewer are returned untouched.
fn strip_padding(body: &[u8]) -> &[u8] {
    let pad_total = DMX_PADDING.saturating_mul(2);
    if body.len() <= pad_total {
        return body;
    }
    let start = usize::from(has_leading_pad(body)).saturating_mul(DMX_PADDING);
    let trimmed = usize::from(has_trailing_pad(body)).saturating_mul(DMX_PADDING);
    let end = body.len().saturating_sub(trimmed);
    body.get(start..end).unwrap_or(body)
}

/// Whether the first `16` bytes all repeat the `17th` byte, the shape of
/// the leading pad `DMX` writes.
fn has_leading_pad(body: &[u8]) -> bool {
    let pad = body.get(..DMX_PADDING).unwrap_or(&[]);
    body.get(DMX_PADDING)
        .is_some_and(|&sample| pad.iter().all(|&byte| byte == sample))
}

/// Whether the last `16` bytes all repeat the byte before them, the shape
/// of the trailing pad `DMX` writes.
fn has_trailing_pad(body: &[u8]) -> bool {
    let pad_start = body.len().saturating_sub(DMX_PADDING);
    let pad = body.get(pad_start..).unwrap_or(&[]);
    body.get(pad_start.saturating_sub(1))
        .is_some_and(|&sample| pad.iter().all(|&byte| byte == sample))
}

/// Encodes a mono [`Pcm`] as a `DMX` sound lump.
///
/// When `pad` is set the vanilla `16` pad bytes are written at each end,
/// filled with the first and last sample; `sample_count` grows with them,
/// exactly as the engine's own converter does. That fill is what
/// [`decode_dmx`] detects, so a padded lump round-trips to the byte: with
/// `pad` clear the lump is written unpadded and [`decode_dmx`] keeps every
/// sample.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] when the input is not mono, has no
/// samples, or needs a sample rate wider than `16` bits.
#[inline]
pub fn encode_dmx(pcm: &Pcm, pad: bool) -> Result<Vec<u8>, CyberError> {
    if pcm.channels != 1 {
        return Err(CyberError::InvalidArgument(format!(
            "DMX sounds are mono, got {} channels; pass the audio through Pcm::to_mono() first",
            pcm.channels
        )));
    }
    if pcm.samples.is_empty() {
        return Err(CyberError::InvalidArgument(
            "DMX sounds need at least one sample; the input Pcm is empty".to_owned(),
        ));
    }
    let rate = u16::try_from(pcm.sample_rate).map_err(|error| {
        CyberError::InvalidArgument(format!(
            "DMX sample rate must fit 16 bits (0..=65535), got {}; resample into that range: {error}",
            pcm.sample_rate
        ))
    })?;
    let body = pcm.to_u8_unsigned();
    let extra = if pad {
        DMX_PADDING.saturating_mul(2)
    } else {
        0_usize
    };
    let total = body
        .len()
        .checked_add(extra)
        .ok_or_else(|| CyberError::LimitExceeded("DMX sample count overflowed usize".to_owned()))?;
    let mut writer = Writer::new();
    writer.put_u16(DMX_FORMAT);
    writer.put_u16(rate);
    writer.put_u32(usize_to_u32(total, "DMX sample")?);
    if pad {
        let first = body.first().copied().unwrap_or(DMX_SILENCE);
        let last = body.last().copied().unwrap_or(DMX_SILENCE);
        for _ in 0..DMX_PADDING {
            writer.put_u8(first);
        }
        writer.put_bytes(&body);
        for _ in 0..DMX_PADDING {
            writer.put_u8(last);
        }
    } else {
        writer.put_bytes(&body);
    }
    Ok(writer.into_inner())
}

#[cfg(test)]
mod tests {
    use super::{DMX_FORMAT, DMX_HEADER_LEN, decode_dmx, encode_dmx};
    use crate::audio::pcm::Pcm;
    use crate::error::CyberError;

    /// Builds a lump: format, rate, the sample count and the sample bytes.
    fn lump(format: u16, rate: u16, samples: &[u8]) -> Vec<u8> {
        let count = u32::try_from(samples.len()).expect("test lumps fit u32");
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&format.to_le_bytes());
        bytes.extend_from_slice(&rate.to_le_bytes());
        bytes.extend_from_slice(&count.to_le_bytes());
        bytes.extend_from_slice(samples);
        bytes
    }

    /// Mono `Pcm` holding exactly these `8`-bit bytes.
    fn pcm_from(bytes: &[u8], rate: u32) -> Pcm {
        Pcm::from_u8_unsigned(bytes, rate)
    }

    #[test]
    fn decodes_padded_lump_with_padding_stripped() {
        let payload: Vec<u8> = (64_u8..82).collect();
        let mut samples = vec![64_u8; 16];
        samples.extend_from_slice(&payload);
        samples.extend_from_slice(&[81_u8; 16]);
        let bytes = lump(3, 22050, &samples);
        let pcm = decode_dmx(&bytes).expect("a well-formed padded lump");
        assert_eq!(pcm.sample_rate, 22050, "rate from the header");
        assert_eq!(pcm.channels, 1, "DMX sounds are mono");
        assert_eq!(pcm.frames(), payload.len(), "the 32 pad bytes are gone");
        assert_eq!(
            pcm.samples,
            pcm_from(&payload, 22050).samples,
            "unsigned 8-bit mapping"
        );
    }

    #[test]
    fn short_lump_keeps_every_sample() {
        let bytes = lump(3, 11025, &[128_u8, 200_u8, 40_u8]);
        let pcm = decode_dmx(&bytes).expect("a 3-sample lump");
        assert_eq!(pcm.frames(), 3, "counts up to 32 are not treated as padded");
        assert_eq!(pcm.samples, vec![0, 18432, -22528], "converted verbatim");
    }

    #[test]
    fn count_of_exactly_32_is_kept_whole() {
        let bytes = lump(3, 8000, &[128_u8; 32]);
        let pcm = decode_dmx(&bytes).expect("a 32-sample lump");
        assert_eq!(pcm.frames(), 32, "32 samples stay, only >32 is padded");
    }

    /// The `bytes` of `value` in big-endian order, reversed from
    /// little-endian so the denied `to_be_bytes` never appears.
    fn swapped_bytes<const N: usize>(value: [u8; N]) -> [u8; N] {
        let mut bytes = value;
        bytes.reverse();
        bytes
    }

    /// Builds a big-endian (Mac-tool) lump: swapped format, swapped rate,
    /// swapped count, then the sample bytes.
    fn swapped_lump(rate: u16, samples: &[u8]) -> Vec<u8> {
        let count = u32::try_from(samples.len()).expect("test lumps fit u32");
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&swapped_bytes(DMX_FORMAT.to_le_bytes()));
        bytes.extend_from_slice(&swapped_bytes(rate.to_le_bytes()));
        bytes.extend_from_slice(&swapped_bytes(count.to_le_bytes()));
        bytes.extend_from_slice(samples);
        bytes
    }

    #[test]
    fn byte_swapped_header_swaps_the_rate_and_the_count() {
        // A big-endian writer puts 00 03 in the format field (u16 LE read:
        // 0x0300), 56 22 in the rate field, which is 0x2256 read back, and
        // the count bytes reversed too.
        let payload: Vec<u8> = (48_u8..60).collect();
        let bytes = swapped_lump(22050, &payload);
        let pcm = decode_dmx(&bytes).expect("the swapped magic is accepted");
        assert_eq!(pcm.sample_rate, 22050, "the swapped rate is restored");
        assert_eq!(pcm.frames(), payload.len(), "the swapped count is restored");
        assert_eq!(
            pcm.samples,
            pcm_from(&payload, 22050).samples,
            "the samples come through"
        );
    }

    #[test]
    fn byte_swapped_padded_lump_round_trips() {
        let pcm = pcm_from(&[10_u8, 200, 3, 44, 99, 1], 11025);
        let plain = encode_dmx(&pcm, true).expect("mono encodes");
        let body = plain
            .get(DMX_HEADER_LEN..)
            .expect("the encoded lump holds its body");
        let bytes = swapped_lump(11025, body);
        let back = decode_dmx(&bytes).expect("the swapped twin of the encoded lump");
        assert_eq!(back, pcm, "swapped header, padded body, all restored");
    }

    #[test]
    fn unpadded_bodies_past_the_pad_length_survive_decoding() {
        // 40 varied samples: longer than the 32 pad bytes, but no end
        // repeats its neighbour, so nothing looks like a pad.
        let payload: Vec<u8> = (0_u8..40)
            .map(|i| i.wrapping_mul(7).wrapping_add(3))
            .collect();
        let pcm = pcm_from(&payload, 8000);
        let bytes = encode_dmx(&pcm, false).expect("mono encodes");
        let back = decode_dmx(&bytes).expect("our own unpadded lump");
        assert_eq!(back.frames(), payload.len(), "no samples are trimmed");
        assert_eq!(back, pcm, "the unpadded round trip is exact");
    }

    #[test]
    fn padded_bodies_past_the_pad_length_round_trip() {
        let payload: Vec<u8> = (0_u8..200)
            .map(|i| i.wrapping_mul(3).wrapping_add(1))
            .collect();
        let pcm = pcm_from(&payload, 8000);
        let bytes = encode_dmx(&pcm, true).expect("mono encodes");
        let back = decode_dmx(&bytes).expect("our own padded lump");
        assert_eq!(back, pcm, "the padded round trip is exact");
    }

    #[test]
    fn a_fully_constant_sound_cannot_be_told_apart_from_padding() {
        // The documented cost of reading the pads by their bytes: 16
        // identical sample bytes at an end are exactly what a pad is.
        let pcm = pcm_from(&[128_u8; 50], 8000);
        let bytes = encode_dmx(&pcm, false).expect("mono encodes");
        let back = decode_dmx(&bytes).expect("our own lump");
        assert_eq!(back.frames(), 18, "the constant ends read as pads and go");
    }

    #[test]
    fn each_pad_end_is_detected_on_its_own() {
        let payload: Vec<u8> = (200_u8..232).collect();
        let expected = pcm_from(&payload, 8000).samples;
        let mut leading_only = vec![*payload.first().expect("payload"); 16];
        leading_only.extend_from_slice(&payload);
        let front = decode_dmx(&lump(3, 8000, &leading_only)).expect("a leading-pad body");
        assert_eq!(front.samples, expected, "the leading pad goes");

        let mut trailing_only = payload.clone();
        trailing_only.extend_from_slice(&[*payload.last().expect("payload"); 16]);
        let back = decode_dmx(&lump(3, 8000, &trailing_only)).expect("a trailing-pad body");
        assert_eq!(back.samples, expected, "the trailing pad goes");
    }

    #[test]
    fn bad_magic_is_a_format_error() {
        let error = decode_dmx(&lump(2, 11025, &[128_u8])).expect_err("format 2 is not Doom");
        assert_eq!(error.code(), "format_error", "wrong tag code");
        assert!(
            error.to_string().contains("format 2"),
            "message names the tag: {error}"
        );
        assert_eq!(
            decode_dmx(&[3_u8])
                .expect_err("one byte holds no header")
                .code(),
            "format_error",
            "short lump code"
        );
    }

    #[test]
    fn zero_count_and_count_past_the_end_are_format_errors() {
        let empty = decode_dmx(&lump(3, 11025, &[])).expect_err("no samples declared");
        assert_eq!(empty.code(), "format_error", "zero count code");
        let mut bytes = lump(3, 11025, &[128_u8, 128_u8]);
        bytes[4..8].copy_from_slice(&1000_u32.to_le_bytes());
        let over = decode_dmx(&bytes).expect_err("count runs past the lump");
        assert_eq!(over.code(), "format_error", "count past end code");
        assert!(
            over.to_string().contains("1000"),
            "message names the count: {over}"
        );
    }

    #[test]
    fn encodes_then_decodes_without_padding() {
        let pcm = pcm_from(&[0_u8, 64, 128, 255], 22050);
        let bytes = encode_dmx(&pcm, false).expect("mono, small rate");
        assert_eq!(bytes.len(), 8 + 4, "header plus four samples");
        assert_eq!(
            u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]),
            4_u32,
            "count is the sample count"
        );
        let back = decode_dmx(&bytes).expect("the lump we just wrote");
        assert_eq!(back, pcm, "lossless round trip inside 8-bit audio");
    }

    #[test]
    fn encodes_then_decodes_with_padding() {
        let pcm = pcm_from(&[10_u8, 200, 3], 11025);
        let bytes = encode_dmx(&pcm, true).expect("mono, small rate");
        assert_eq!(
            u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]),
            35_u32,
            "count grew by the 32 pad bytes"
        );
        assert_eq!(
            &bytes[8..24],
            &[10_u8; 16],
            "leading pad repeats the first sample"
        );
        assert_eq!(
            &bytes[27..43],
            &[3_u8; 16],
            "trailing pad repeats the last sample"
        );
        let back = decode_dmx(&bytes).expect("the lump we just wrote");
        assert_eq!(back, pcm, "padding round-trips away");
    }

    #[test]
    fn stereo_input_is_an_invalid_argument() {
        let stereo = Pcm {
            sample_rate: 22050,
            channels: 2,
            samples: vec![0, 0],
        };
        let CyberError::InvalidArgument(message) =
            encode_dmx(&stereo, false).expect_err("DMX cannot hold stereo")
        else {
            panic!("expected invalid_argument for stereo input");
        };
        assert!(
            message.contains("to_mono"),
            "message points at the fix: {message}"
        );
    }

    #[test]
    fn oversized_rate_and_empty_input_are_invalid() {
        let fast = Pcm {
            sample_rate: 0x0001_0000_u32,
            channels: 1,
            samples: vec![0],
        };
        assert_eq!(
            encode_dmx(&fast, false)
                .expect_err("rate over 65535")
                .code(),
            "invalid_argument",
            "16-bit rate field"
        );
        let empty = Pcm {
            sample_rate: 11025,
            channels: 1,
            samples: Vec::new(),
        };
        assert_eq!(
            encode_dmx(&empty, true).expect_err("nothing to pad").code(),
            "invalid_argument",
            "empty sound"
        );
    }
}
