//! Canonical `RIFF`/`WAVE` reading and writing: `fmt `/`data` chunk
//! walking, `8`/`16`/`24`/`32`-bit integer `PCM` input (reduced to
//! `i16`), and `8`/`16`-bit output.

use crate::audio::pcm::Pcm;
use crate::audio::u8_unsigned_to_i16;
use crate::error::CyberError;
use crate::wad::bytes::{Reader, Writer, u16_at, u32_at, u32_to_usize, usize_to_u32};

/// RIFF file header: magic, size, form type.
const RIFF_HEADER_LEN: usize = 12;
/// One chunk header: four id bytes and a little-endian `u32` size.
const CHUNK_HEADER_LEN: usize = 8;
/// Size of the canonical `fmt ` chunk body this encoder writes.
const FMT_CHUNK_SIZE: u32 = 16_u32;
/// `RIFF` size field overhead: the form type plus the two chunks.
const RIFF_SIZE_OVERHEAD: u32 = 36_u32;
/// Offset of the format tag inside the `fmt ` body.
const FMT_TAG_OFFSET: usize = 0;
/// Offset of the channel count inside the `fmt ` body.
const FMT_CHANNELS_OFFSET: usize = 2;
/// Offset of the sample rate inside the `fmt ` body.
const FMT_RATE_OFFSET: usize = 4;
/// Offset of the sample width inside the `fmt ` body.
const FMT_BITS_OFFSET: usize = 14;
/// Offset of the sub-format `GUID` inside an extensible `fmt ` body.
const FMT_SUBFORMAT_OFFSET: usize = 24;
/// The shortest `fmt ` body that holds a `PCM` format record.
const FMT_MIN_LEN: usize = 16;
/// Format tag for integer `PCM`.
const WAVE_FORMAT_PCM: u16 = 1_u16;
/// Format tag for `IEEE` float data, which this decoder does not read.
const WAVE_FORMAT_FLOAT: u16 = 3_u16;
/// Format tag for `WAVE_FORMAT_EXTENSIBLE`.
const WAVE_FORMAT_EXTENSIBLE: u16 = 0xFFFE_u16;
/// Largest channel count the format allows.
const MAX_CHANNELS: u16 = 8_u16;
/// Shift from a `32`-bit sample to its top `16` bits.
const I32_TO_I16_SHIFT: u32 = 16_u32;

/// The parts of a `fmt ` body this decoder keeps.
struct FormatSpec {
    /// Sample rate from the chunk.
    sample_rate: u32,
    /// Channel count, validated to be within `1..=8`.
    channels: u16,
    /// Bits per sample: `8`, `16`, `24` or `32`.
    bits: u16,
}

/// The chunk bodies a walk of the file found.
struct Chunks<'data> {
    /// Body of the last `fmt ` chunk, when the file has one.
    format: Option<&'data [u8]>,
    /// Body of the last `data` chunk, when the file has one.
    data: Option<&'data [u8]>,
}

/// Decodes a `RIFF`/`WAVE` file into a [`Pcm`] of interleaved `i16`
/// samples.
///
/// `8`-bit data is unsigned, `16`/`24`/`32`-bit data is signed
/// little-endian reduced to its most significant `16` bits. Unknown
/// chunks are skipped, and odd-sized chunks carry their pad byte.
///
/// # Errors
///
/// [`CyberError::Format`] when the magic, the `fmt ` or `data` chunk is
/// missing, a chunk is truncated, or the channel count, rate or sample
/// width is outside what this decoder accepts;
/// [`CyberError::Unsupported`] for float or non-`PCM` data.
#[inline]
pub fn decode_wav(bytes: &[u8]) -> Result<Pcm, CyberError> {
    check_riff(bytes)?;
    let chunks = scan_chunks(bytes)?;
    let format = parse_format(
        chunks
            .format
            .ok_or_else(|| CyberError::Format("WAV file has no 'fmt ' chunk".to_owned()))?,
    )?;
    let data = chunks
        .data
        .ok_or_else(|| CyberError::Format("WAV file has no 'data' chunk".to_owned()))?;
    let samples = decode_samples(data, &format)?;
    Ok(Pcm {
        sample_rate: format.sample_rate,
        channels: format.channels,
        samples,
    })
}

/// Encodes a [`Pcm`] as a canonical `RIFF`/`WAVE` file: a `44`-byte
/// header (`fmt ` of size `16`) followed by the sample data.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] when `bits` is not `8` or `16`, the
/// channel count leaves `1..=8`, or the rate is zero;
/// [`CyberError::LimitExceeded`] when the data does not fit the `32`-bit
/// chunk sizes.
#[inline]
pub fn encode_wav(pcm: &Pcm, bits: u16) -> Result<Vec<u8>, CyberError> {
    let width = match bits {
        8_u16 => 1_usize,
        16_u16 => 2_usize,
        other => {
            return Err(CyberError::InvalidArgument(format!(
                "WAV encoding supports 8 or 16 bits, got {other}; pass bits = 8 or 16"
            )));
        }
    };
    if !(1_u16..=MAX_CHANNELS).contains(&pcm.channels) || pcm.sample_rate == 0 {
        return Err(CyberError::InvalidArgument(format!(
            "WAV needs channels in 1..={MAX_CHANNELS} and a positive rate, got {} at {} Hz; downmix with Pcm::to_mono() or resample",
            pcm.channels, pcm.sample_rate
        )));
    }
    let data = encode_data(pcm, width);
    let data_len = usize_to_u32(data.len(), "WAV data")?;
    let riff_size = data_len.checked_add(RIFF_SIZE_OVERHEAD).ok_or_else(|| {
        CyberError::LimitExceeded("WAV data is larger than the 4 GiB RIFF container".to_owned())
    })?;
    let block_align = u16::try_from(
        usize::from(pcm.channels)
            .checked_mul(width)
            .ok_or_else(|| {
                CyberError::LimitExceeded("WAV block alignment overflowed".to_owned())
            })?,
    )
    .map_err(|error| CyberError::LimitExceeded(format!("WAV block alignment: {error}")))?;
    let byte_rate = byte_rate(pcm.sample_rate, pcm.channels, width)?;
    let mut writer = Writer::new();
    writer.put_bytes(b"RIFF");
    writer.put_u32(riff_size);
    writer.put_bytes(b"WAVE");
    writer.put_bytes(b"fmt ");
    writer.put_u32(FMT_CHUNK_SIZE);
    writer.put_u16(WAVE_FORMAT_PCM);
    writer.put_u16(pcm.channels);
    writer.put_u32(pcm.sample_rate);
    writer.put_u32(byte_rate);
    writer.put_u16(block_align);
    writer.put_u16(bits);
    writer.put_bytes(b"data");
    writer.put_u32(data_len);
    writer.put_bytes(&data);
    Ok(writer.into_inner())
}

/// The data bytes for the requested sample width.
fn encode_data(pcm: &Pcm, width: usize) -> Vec<u8> {
    if width == 1 {
        return pcm.to_u8_unsigned();
    }
    pcm.samples
        .iter()
        .flat_map(|sample| sample.to_le_bytes())
        .collect()
}

/// `rate × channels × width`, the byte rate field, as a `u32`.
fn byte_rate(rate: u32, channels: u16, width: usize) -> Result<u32, CyberError> {
    let scaled = u64::from(rate)
        .checked_mul(u64::from(u32::from(channels)))
        .and_then(|value| value.checked_mul(u64::try_from(width).unwrap_or(1)))
        .ok_or_else(|| {
            CyberError::LimitExceeded(format!(
                "WAV byte rate for {rate} Hz x {channels} ch overflows"
            ))
        })?;
    u32::try_from(scaled).map_err(|error| {
        CyberError::LimitExceeded(format!("WAV byte rate does not fit 32 bits: {error}"))
    })
}

/// Validates the outermost `RIFF...WAVE` header.
fn check_riff(bytes: &[u8]) -> Result<(), CyberError> {
    let mut reader = Reader::new(bytes);
    let magic = reader.read_array::<4>()?;
    if magic != *b"RIFF" {
        return Err(CyberError::Format(
            "not a WAV file: the RIFF magic is missing".to_owned(),
        ));
    }
    reader.read_u32()?;
    let form = reader.read_array::<4>()?;
    if form != *b"WAVE" {
        return Err(CyberError::Format(
            "not a WAV file: the RIFF form type is not 'WAVE'".to_owned(),
        ));
    }
    Ok(())
}

/// Walks the chunk list after the file header, word-aligned.
fn scan_chunks(bytes: &[u8]) -> Result<Chunks<'_>, CyberError> {
    let mut reader = Reader::at(bytes, RIFF_HEADER_LEN);
    let mut chunks = Chunks {
        format: None,
        data: None,
    };
    while reader.remaining() >= CHUNK_HEADER_LEN {
        let id = reader.read_array::<4>()?;
        let size = u32_to_usize(reader.read_u32()?)?;
        let body = reader.read_bytes(size)?;
        if id == *b"fmt " {
            chunks.format = Some(body);
        } else if id == *b"data" {
            chunks.data = Some(body);
        } else {
            // Unknown chunks (LIST, fact, ...) are skipped by their size.
        }
        if size.checked_rem(2) == Some(1) && reader.remaining() >= 1 {
            reader.read_u8()?;
        }
    }
    Ok(chunks)
}

/// Parses and validates a `fmt ` body.
fn parse_format(body: &[u8]) -> Result<FormatSpec, CyberError> {
    if body.len() < FMT_MIN_LEN {
        return Err(CyberError::Format(format!(
            "'fmt ' chunk is {} bytes, a PCM format record needs at least {FMT_MIN_LEN}",
            body.len()
        )));
    }
    let tag = u16_at(body, FMT_TAG_OFFSET)?;
    check_tag(tag, body)?;
    let channels = u16_at(body, FMT_CHANNELS_OFFSET)?;
    if !(1_u16..=MAX_CHANNELS).contains(&channels) {
        return Err(CyberError::Format(format!(
            "WAV channel count {channels} is outside 1..={MAX_CHANNELS}; convert the file to at most 8 channels"
        )));
    }
    let sample_rate = u32_at(body, FMT_RATE_OFFSET)?;
    if sample_rate == 0 {
        return Err(CyberError::Format(
            "WAV sample rate is zero; convert the file with a valid rate".to_owned(),
        ));
    }
    let bits = u16_at(body, FMT_BITS_OFFSET)?;
    if bits_to_bytes(bits).is_none() {
        return Err(CyberError::Format(format!(
            "WAV sample width {bits} bits is unsupported; only 8, 16, 24 and 32 bit integer PCM decode"
        )));
    }
    Ok(FormatSpec {
        sample_rate,
        channels,
        bits,
    })
}

/// Accepts integer `PCM`, plain or extensible; rejects everything else.
fn check_tag(tag: u16, body: &[u8]) -> Result<(), CyberError> {
    if tag == WAVE_FORMAT_PCM {
        return Ok(());
    }
    if tag == WAVE_FORMAT_FLOAT {
        return Err(CyberError::Unsupported(
            "IEEE float WAV input is not supported; convert the file to integer PCM".to_owned(),
        ));
    }
    if tag == WAVE_FORMAT_EXTENSIBLE {
        let is_pcm = body
            .get(FMT_SUBFORMAT_OFFSET..FMT_SUBFORMAT_OFFSET.saturating_add(2))
            .is_some_and(|guid| guid == [0x01_u8, 0x00_u8]);
        return if is_pcm {
            Ok(())
        } else {
            Err(CyberError::Unsupported(
                "extensible WAV only supports the PCM sub-format GUID".to_owned(),
            ))
        };
    }
    Err(CyberError::Unsupported(format!(
        "WAV format tag {tag} is not supported; only integer PCM (1) and extensible PCM (65534) decode"
    )))
}

/// The byte width of a supported sample depth, `None` otherwise.
const fn bits_to_bytes(bits: u16) -> Option<usize> {
    match bits {
        8_u16 => Some(1_usize),
        16_u16 => Some(2_usize),
        24_u16 => Some(3_usize),
        32_u16 => Some(4_usize),
        _ => None,
    }
}

/// Turns the `data` body into interleaved `i16` samples.
fn decode_samples(data: &[u8], format: &FormatSpec) -> Result<Vec<i16>, CyberError> {
    let width = bits_to_bytes(format.bits).ok_or_else(|| {
        CyberError::Format(format!(
            "WAV sample width {} bits is unsupported",
            format.bits
        ))
    })?;
    let stride = usize::from(format.channels)
        .checked_mul(width)
        .ok_or_else(|| {
            CyberError::Format("WAV block alignment does not fit memory addressing".to_owned())
        })?;
    if stride == 0 || data.len().checked_rem(stride) != Some(0) {
        return Err(CyberError::Format(format!(
            "WAV data chunk is {} bytes, not a whole number of {stride}-byte frames; the file is truncated",
            data.len()
        )));
    }
    let capacity = data.len().checked_div(width).unwrap_or(0);
    let mut samples = Vec::with_capacity(capacity);
    match width {
        1_usize => samples.extend(data.iter().copied().map(u8_unsigned_to_i16)),
        2_usize => samples.extend(data.chunks_exact(2).map(le_i16)),
        3_usize => samples.extend(data.chunks_exact(3).map(le_i16_from_24)),
        4_usize => samples.extend(data.chunks_exact(4).map(le_i16_from_32)),
        other => {
            return Err(CyberError::Format(format!(
                "WAV sample width {other} bytes is unsupported"
            )));
        }
    }
    Ok(samples)
}

/// One little-endian `i16` sample.
fn le_i16(chunk: &[u8]) -> i16 {
    let mut le = [0_u8; 2];
    le.copy_from_slice(chunk);
    i16::from_le_bytes(le)
}

/// The top 16 bits of a little-endian `i24` sample.
fn le_i16_from_24(chunk: &[u8]) -> i16 {
    let mut le = [0_u8; 2];
    le.copy_from_slice(chunk.get(1..3).unwrap_or(&[0_u8, 0_u8]));
    i16::from_le_bytes(le)
}

/// The top 16 bits of a little-endian `i32` sample.
fn le_i16_from_32(chunk: &[u8]) -> i16 {
    let mut le = [0_u8; 4];
    le.copy_from_slice(chunk);
    let value = i32::from_le_bytes(le)
        .checked_shr(I32_TO_I16_SHIFT)
        .unwrap_or(0_i32);
    i16::try_from(value).unwrap_or(if value >= 0 { i16::MAX } else { i16::MIN })
}

#[cfg(test)]
mod tests {
    use super::{decode_wav, encode_wav};
    use crate::audio::pcm::Pcm;

    /// A `fmt ` body for the given shape.
    fn fmt_body(tag: u16, channels: u16, rate: u32, bits: u16) -> Vec<u8> {
        let width = bits.checked_div(8_u16).expect("test widths");
        let block = channels.checked_mul(width).expect("test block align");
        let byte_rate = rate.checked_mul(u32::from(block)).expect("test byte rate");
        let mut body = Vec::new();
        body.extend_from_slice(&tag.to_le_bytes());
        body.extend_from_slice(&channels.to_le_bytes());
        body.extend_from_slice(&rate.to_le_bytes());
        body.extend_from_slice(&byte_rate.to_le_bytes());
        body.extend_from_slice(&block.to_le_bytes());
        body.extend_from_slice(&bits.to_le_bytes());
        body
    }

    /// A `fmt ` body for extensible PCM: the base record, `cbSize`,
    /// valid bits, the four-byte channel mask and the PCM sub-format
    /// GUID, forty bytes in all.
    fn extensible_body(channels: u16, rate: u32, bits: u16) -> Vec<u8> {
        let mut body = fmt_body(0xFFFE_u16, channels, rate, bits);
        body.extend_from_slice(&22_u16.to_le_bytes());
        body.extend_from_slice(&bits.to_le_bytes());
        body.extend_from_slice(&0_u32.to_le_bytes());
        let mut guid = [0_u8; 16];
        guid[0] = 1_u8;
        body.extend_from_slice(&guid);
        body
    }

    /// One chunk of a test file: its id and payload bytes.
    type ChunkSpec<'chunk> = (&'chunk [u8], &'chunk [u8]);

    /// Assembles a RIFF/WAVE file from `(id, body)` chunk specs.
    fn wav_file(chunks: &[ChunkSpec<'_>]) -> Vec<u8> {
        let mut body = Vec::new();
        for &(id, payload) in chunks {
            body.extend_from_slice(id);
            body.extend_from_slice(&chunk_size(payload));
            body.extend_from_slice(payload);
            body.extend_from_slice(pad_for(payload));
        }
        let mut out = Vec::new();
        out.extend_from_slice(b"RIFF");
        let total = 4_usize.checked_add(body.len()).expect("test size");
        out.extend_from_slice(
            &u32::try_from(total)
                .expect("test size fits u32")
                .to_le_bytes(),
        );
        out.extend_from_slice(b"WAVE");
        out.extend_from_slice(&body);
        out
    }

    /// The little-endian chunk size field for a payload.
    fn chunk_size(payload: &[u8]) -> [u8; 4] {
        u32::try_from(payload.len())
            .expect("test chunks fit u32")
            .to_le_bytes()
    }

    /// The pad byte an odd-sized payload is followed by.
    fn pad_for(payload: &[u8]) -> &'static [u8] {
        if payload.len().checked_rem(2_usize) == Some(1) {
            &[0_u8]
        } else {
            &[]
        }
    }

    /// A file with one `fmt ` chunk and the given `data` bytes.
    fn wav_data(fmt: &[u8], data: &[u8]) -> Vec<u8> {
        wav_file(&[(b"fmt ", fmt), (b"data", data)])
    }

    #[test]
    fn decodes_8_bit_mono() {
        let file = wav_data(&fmt_body(1, 1, 11025, 8), &[0_u8, 128, 255]);
        let pcm = decode_wav(&file).expect("a canonical 8-bit mono WAV");
        assert_eq!(pcm.sample_rate, 11025, "rate from fmt");
        assert_eq!(pcm.channels, 1, "channels from fmt");
        assert_eq!(
            pcm.samples,
            vec![i16::MIN, 0, 0x7F00_i16],
            "unsigned 8-bit input"
        );
    }

    #[test]
    fn decodes_16_bit_stereo() {
        let mut data = Vec::new();
        for sample in [100_i16, -100, i16::MAX, i16::MIN] {
            data.extend_from_slice(&sample.to_le_bytes());
        }
        let file = wav_data(&fmt_body(1, 2, 44100, 16), &data);
        let pcm = decode_wav(&file).expect("a canonical 16-bit stereo WAV");
        assert_eq!(pcm.channels, 2, "stereo survives");
        assert_eq!(pcm.frames(), 2, "four samples, two frames");
        assert_eq!(
            pcm.samples,
            vec![100, -100, i16::MAX, i16::MIN],
            "signed 16-bit input"
        );
    }

    #[test]
    fn decodes_24_and_32_bit_to_their_top_16_bits() {
        let file_24 = wav_data(
            &fmt_body(1, 1, 48000, 24),
            &[0x00_u8, 0x00, 0x80, 0x00, 0xFF, 0x7F],
        );
        let pcm_24 = decode_wav(&file_24).expect("24-bit input");
        assert_eq!(
            pcm_24.samples,
            vec![i16::MIN, i16::MAX],
            "top 16 bits of each i24"
        );
        let file_32 = wav_data(
            &fmt_body(1, 1, 48000, 32),
            &[0x00_u8, 0x00, 0x00, 0x80, 0x00, 0x00, 0xFF, 0x7F],
        );
        let pcm_32 = decode_wav(&file_32).expect("32-bit input");
        assert_eq!(
            pcm_32.samples,
            vec![i16::MIN, i16::MAX],
            "top 16 bits of each i32"
        );
    }

    #[test]
    fn decodes_extensible_pcm() {
        let file = wav_data(&extensible_body(1, 22050, 16), &12345_i16.to_le_bytes());
        let pcm = decode_wav(&file).expect("an extensible PCM WAV");
        assert_eq!(pcm.sample_rate, 22050, "rate from the extended fmt");
        assert_eq!(pcm.samples, vec![12345], "data decoded normally");
    }

    #[test]
    fn skips_odd_sized_chunks_before_data() {
        let file = wav_file(&[
            (b"fmt ", &fmt_body(1, 1, 8000, 16)),
            (b"NOTE", b"abc"),
            (b"data", &7_i16.to_le_bytes()),
        ]);
        let pcm = decode_wav(&file).expect("the pad byte after a 3-byte chunk is skipped");
        assert_eq!(pcm.samples, vec![7], "data chunk found after the odd chunk");
    }

    #[test]
    fn float_input_is_unsupported() {
        let file = wav_data(&fmt_body(3, 2, 48000, 32), &[0_u8; 8]);
        let error = decode_wav(&file).expect_err("IEEE float is rejected");
        assert_eq!(error.code(), "unsupported", "float code");
        assert!(
            error.to_string().contains("float"),
            "float message: {error}"
        );
    }

    #[test]
    fn truncated_data_is_a_format_error() {
        let mut file = wav_file(&[(b"fmt ", &fmt_body(1, 1, 8000, 16))]);
        file.extend_from_slice(b"data");
        file.extend_from_slice(&8_u32.to_le_bytes());
        file.extend_from_slice(&[0_u8, 0, 0]);
        let error = decode_wav(&file).expect_err("three bytes cannot hold four samples");
        assert_eq!(error.code(), "format_error", "truncated data code");
    }

    #[test]
    fn missing_chunks_are_format_errors() {
        let no_data = wav_file(&[(b"fmt ", &fmt_body(1, 1, 8000, 16))]);
        assert_eq!(
            decode_wav(&no_data).expect_err("no data chunk").code(),
            "format_error",
            "missing data code"
        );
        let no_fmt = wav_file(&[(b"data", &0_i16.to_le_bytes())]);
        assert_eq!(
            decode_wav(&no_fmt).expect_err("no fmt chunk").code(),
            "format_error",
            "missing fmt code"
        );
        assert_eq!(
            decode_wav(b"NONSENSE.....").expect_err("not RIFF").code(),
            "format_error",
            "bad magic code"
        );
    }

    #[test]
    fn encodes_then_decodes_at_16_bits() {
        let pcm = Pcm {
            sample_rate: 44100,
            channels: 2,
            samples: vec![0, -1, 12345, -22528],
        };
        let file = encode_wav(&pcm, 16).expect("stereo, 16-bit");
        assert_eq!(file.len(), 44 + 8, "canonical 44-byte header plus data");
        assert_eq!(
            decode_wav(&file).expect("our own file"),
            pcm,
            "16-bit round trip"
        );
    }

    #[test]
    fn encodes_then_decodes_at_8_bits() {
        let pcm = Pcm::from_u8_unsigned(&[0_u8, 128, 255], 11025);
        let file = encode_wav(&pcm, 8).expect("mono, 8-bit");
        assert_eq!(file.len(), 44 + 3, "header plus three bytes");
        assert_eq!(
            decode_wav(&file).expect("our own file"),
            pcm,
            "8-bit round trip"
        );
    }

    #[test]
    fn encode_rejects_unencodable_shapes() {
        let pcm = Pcm {
            sample_rate: 8000,
            channels: 1,
            samples: vec![0],
        };
        assert_eq!(
            encode_wav(&pcm, 24).expect_err("24-bit output").code(),
            "invalid_argument",
            "bits outside 8 and 16"
        );
        let wide = Pcm {
            sample_rate: 8000,
            channels: 9,
            samples: vec![0; 9],
        };
        assert_eq!(
            encode_wav(&wide, 16).expect_err("9 channels").code(),
            "invalid_argument",
            "channels outside 1..=8"
        );
        let silent_rate = Pcm {
            sample_rate: 0,
            channels: 1,
            samples: vec![0],
        };
        assert_eq!(
            encode_wav(&silent_rate, 16).expect_err("zero rate").code(),
            "invalid_argument",
            "rate must be positive"
        );
    }
}
