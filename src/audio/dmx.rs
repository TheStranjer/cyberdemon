//! `Doom` `DS*` sound lumps: the `DMX` format-3 encoding of unsigned
//! `8`-bit mono audio, with the vanilla leading/trailing pad bytes.
//!
//! [`decode_dmx`] strips the `16 + 16` padding the engine never plays;
//! [`encode_dmx`] can re-add it.

use crate::audio::pcm::Pcm;
use crate::error::CyberError;
use crate::wad::bytes::{Reader, Writer, u32_to_usize, usize_to_u32};

/// The `DMX` format tag of `Doom` `8`-bit unsigned sound data.
const DMX_FORMAT: u16 = 3_u16;
/// The same tag read back from bytes written big-endian by some `Mac`
/// tools; the sample rate is byte-swapped in those lumps too.
const DMX_FORMAT_SWAPPED: u16 = 0x0300_u16;
/// Header size: format, sample rate and sample count.
const DMX_HEADER_LEN: usize = 8;
/// Pad bytes the vanilla engine keeps at each end of the sample data.
const DMX_PADDING: usize = 16;
/// Silence in unsigned `8`-bit storage, the fallback pad byte.
const DMX_SILENCE: u8 = 128_u8;

/// Decodes a `DMX` sound lump into a mono 16-bit [`Pcm`].
///
/// A `sample_count` above `32` is treated as carrying the vanilla `16`
/// leading and `16` trailing pad bytes and both ends are stripped; short
/// lumps are kept whole.
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
    let raw_rate = reader.read_u16()?;
    let sample_rate = if raw_format == DMX_FORMAT_SWAPPED {
        raw_rate.swap_bytes()
    } else {
        raw_rate
    };
    let count = u32_to_usize(reader.read_u32()?)?;
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

/// Drops the vanilla `16 + 16` pad bytes when the sample data is longer
/// than they are; short lumps are returned untouched.
fn strip_padding(body: &[u8]) -> &[u8] {
    let pad_total = DMX_PADDING.saturating_mul(2);
    if body.len() <= pad_total {
        return body;
    }
    let end = body.len().saturating_sub(DMX_PADDING);
    body.get(DMX_PADDING..end).unwrap_or(body)
}

/// Encodes a mono [`Pcm`] as a `DMX` sound lump.
///
/// When `pad` is set the vanilla `16` pad bytes are written at each end,
/// filled with the first and last sample; `sample_count` grows with them,
/// exactly as the engine's own converter does.
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
    use super::{decode_dmx, encode_dmx};
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
        let mut samples = vec![0_u8; 16];
        samples.extend_from_slice(&payload);
        samples.extend_from_slice(&[255_u8; 16]);
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

    #[test]
    fn byte_swapped_magic_swaps_the_rate_too() {
        // A big-endian writer puts 00 03 in the format field (u16 LE read:
        // 0x0300) and 56 22 in the rate field, which is 0x2256 read back.
        let bytes = lump(0x0300, 0x2256, &[128_u8, 129_u8]);
        let pcm = decode_dmx(&bytes).expect("the swapped magic is accepted");
        assert_eq!(pcm.sample_rate, 22050, "the swapped rate is restored");
        assert_eq!(pcm.frames(), 2, "samples come from the same header");
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
