//! Format-agnostic sound file decoding: [`decode_any`] turns the bytes of
//! a `WAV`, `OGG`, `MP3` or `FLAC` file into interleaved `i16` [`Pcm`], and
//! [`sniff_format`] names the container from the file's first bytes.
//!
//! `RIFF`/`WAVE` files the in-house [`crate::audio::wav::decode_wav`] can
//! read never touch `symphonia`; everything else (and `WAV`s it rejects) is
//! probed and decoded with the pure-Rust `symphonia` stack. All float-to-
//! integer sample conversion happens inside `symphonia`; nothing in this
//! module performs float arithmetic.

use std::io;

use symphonia::core::audio::AudioSpec;
use symphonia::core::codecs::CodecParameters;
use symphonia::core::codecs::audio::{AudioCodecParameters, AudioDecoder, AudioDecoderOptions};
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatOptions, FormatReader, TrackType};
use symphonia::core::io::{MediaSourceStream, MediaSourceStreamOptions};
use symphonia::core::meta::MetadataOptions;
use symphonia::core::packet::Packet;

use crate::audio::pcm::Pcm;
use crate::audio::wav::decode_wav;
use crate::error::CyberError;

/// Longest sound [`decode_any`] returns: ten minutes, in milliseconds.
const MAX_DECODED_MS: u64 = 600_000;
/// `RIFF` form-type field offset in a `WAV` file.
const RIFF_FORM_OFFSET: usize = 8;

/// Identifies a sound file from its first bytes.
///
/// Returns the container name (`"wav"`, `"ogg"`, `"flac"` or `"mp3"`) for a
/// matching magic, `None` for anything else. `MP3` is reported for both an
/// `ID3` tag and a bare `MPEG` frame sync.
#[must_use]
#[inline]
pub fn sniff_format(bytes: &[u8]) -> Option<&'static str> {
    if is_wave(bytes) {
        return Some("wav");
    }
    if bytes.starts_with(b"OggS") {
        return Some("ogg");
    }
    if bytes.starts_with(b"fLaC") {
        return Some("flac");
    }
    if bytes.starts_with(b"ID3") || is_mpeg_sync(bytes) {
        return Some("mp3");
    }
    None
}

/// Whether `bytes` start with a `RIFF` header carrying the `WAVE` form type.
fn is_wave(bytes: &[u8]) -> bool {
    let form_end = RIFF_FORM_OFFSET.checked_add(4).unwrap_or(0);
    bytes.starts_with(b"RIFF") && bytes.get(RIFF_FORM_OFFSET..form_end) == Some(b"WAVE")
}

/// Whether `bytes` open with an `MPEG` audio frame sync (11 set bits).
fn is_mpeg_sync(bytes: &[u8]) -> bool {
    bytes
        .first_chunk::<2>()
        .is_some_and(|&[first, second]| first == 0xFF_u8 && second & 0xE0_u8 == 0xE0_u8)
}

/// Decodes the bytes of any supported sound file into interleaved `i16`
/// [`Pcm`].
///
/// `RIFF`/`WAVE` files go through the in-house [`decode_wav`] first; when
/// that refuses the file (float samples, exotic layout) or the bytes are not
/// `WAV` at all, `symphonia` probes the container and decodes its default
/// audio track. `hint_ext` is an optional lowercase file extension that
/// speeds up the probe; it never overrides what the bytes say.
///
/// # Errors
///
/// [`CyberError::Format`] for unrecognizable or corrupt data, for files
/// that carry no decodable audio, and for a decoded stream with a 0 Hz
/// sample rate; [`CyberError::Unsupported`] for a recognized stream whose
/// codec this build cannot decode; [`CyberError::LimitExceeded`] for audio
/// longer than ten minutes.
#[inline]
pub fn decode_any(bytes: &[u8], hint_ext: Option<&str>) -> Result<Pcm, CyberError> {
    let pcm = if is_wave(bytes) {
        decode_wave(bytes, hint_ext)?
    } else {
        decode_probed(bytes, hint_ext)?
    };
    check_decoded_length(&pcm)?;
    Ok(pcm)
}

/// The in-house `WAV` reader with the `symphonia` stack as its fallback; a
/// fallback failure reports both complaints.
fn decode_wave(bytes: &[u8], hint_ext: Option<&str>) -> Result<Pcm, CyberError> {
    match decode_wav(bytes) {
        Ok(pcm) => Ok(pcm),
        Err(in_house) => decode_probed(bytes, hint_ext).map_err(|probe| {
            CyberError::Format(format!("{probe}; the in-house WAV reader said: {in_house}"))
        }),
    }
}

/// The `(rate, channels)` shape of a decoded buffer.
#[derive(Clone, Copy)]
struct Shape {
    /// Samples per second.
    sample_rate: u32,
    /// Interleaved channels per frame.
    channels: u16,
}

/// What draining the packet stream produced: interleaved `i16` samples and
/// the shape of the first buffer that held frames.
struct Drained {
    /// Interleaved samples, frame-major.
    samples: Vec<i16>,
    /// Shape of the first non-empty buffer, `None` when nothing decoded.
    shape: Option<Shape>,
}

/// Probes the container with `symphonia` and decodes its default audio
/// track. Bytes no supported container sniffs from are refused before the
/// probe ever scans them.
fn decode_probed(bytes: &[u8], hint_ext: Option<&str>) -> Result<Pcm, CyberError> {
    let container = sniff_format(bytes).ok_or_else(|| {
        CyberError::Format(
            "cannot identify the sound file: supported containers are WAV (RIFF), OGG, FLAC and \
             MP3"
            .to_owned(),
        )
    })?;
    let mut reader = open_stream(bytes, hint_ext, container)?;
    let (track_id, params) = audio_track(&*reader)?;
    let mut decoder = make_decoder(&params)?;
    let drained = drain_packets(reader.as_mut(), track_id, decoder.as_mut())?;
    assemble(drained)
}

/// Runs the `symphonia` probe over `bytes`; `hint_ext` is context only.
fn open_stream<'bytes>(
    bytes: &'bytes [u8],
    hint_ext: Option<&str>,
    container: &str,
) -> Result<Box<dyn FormatReader + 'bytes>, CyberError> {
    let mut hint = Hint::new();
    if let Some(ext) = hint_ext {
        hint.with_extension(ext);
    }
    let source = Box::new(io::Cursor::new(bytes));
    let stream = MediaSourceStream::new(source, MediaSourceStreamOptions::default());
    symphonia::default::get_probe()
        .probe(
            &hint,
            stream,
            FormatOptions::default(),
            MetadataOptions::default(),
        )
        .map_err(|error| {
            CyberError::Format(format!("the {container} file could not be opened: {error}"))
        })
}

/// The default audio track's id and codec parameters.
fn audio_track(reader: &dyn FormatReader) -> Result<(u32, AudioCodecParameters), CyberError> {
    let track = reader
        .default_track(TrackType::Audio)
        .ok_or_else(|| CyberError::Format("the media carries no audio track".to_owned()))?;
    let params = match track.codec_params.clone() {
        Some(CodecParameters::Audio(params)) => params,
        Some(_) => {
            return Err(CyberError::Unsupported(
                "the default track is not an audio track".to_owned(),
            ));
        }
        None => {
            return Err(CyberError::Format(
                "the default audio track carries no codec parameters; re-export the file"
                    .to_owned(),
            ));
        }
    };
    Ok((track.id, params))
}

/// Instantiates the registered decoder for `params`.
fn make_decoder(params: &AudioCodecParameters) -> Result<Box<dyn AudioDecoder>, CyberError> {
    symphonia::default::get_codecs()
        .make_audio_decoder(params, &AudioDecoderOptions::default())
        .map_err(|error| symphonia_error(&error))
}

/// Decodes every packet of `track_id`, appending interleaved `i16` samples
/// to a buffer and keeping the shape of the first buffer with frames.
fn drain_packets(
    reader: &mut dyn FormatReader,
    track_id: u32,
    decoder: &mut dyn AudioDecoder,
) -> Result<Drained, CyberError> {
    let mut drained = Drained {
        samples: Vec::new(),
        shape: None,
    };
    while let Some(packet) = next_audio_packet(reader, track_id)? {
        drained.shape = absorb_packet(decoder, &packet, &mut drained.samples)?.or(drained.shape);
    }
    Ok(drained)
}

/// The next packet belonging to `track_id`; `None` at end of stream. A
/// mid-stream track change (`ResetRequired`) ends the iteration too.
fn next_audio_packet(
    reader: &mut dyn FormatReader,
    track_id: u32,
) -> Result<Option<Packet>, CyberError> {
    loop {
        match reader.next_packet() {
            Ok(Some(packet)) if packet.track_id == track_id => return Ok(Some(packet)),
            Ok(Some(_)) => {}
            Ok(None) | Err(symphonia::core::errors::Error::ResetRequired) => return Ok(None),
            Err(error) => return Err(symphonia_error(&error)),
        }
    }
}

/// Decodes one packet into `out`, reporting the buffer's shape when it held
/// frames. Packets a decoder rejects as merely damaged are skipped, exactly
/// as the engine's own loader would.
fn absorb_packet(
    decoder: &mut dyn AudioDecoder,
    packet: &Packet,
    out: &mut Vec<i16>,
) -> Result<Option<Shape>, CyberError> {
    let buffer = match decoder.decode(packet) {
        Ok(buffer) => buffer,
        Err(error) if is_damaged(&error) => return Ok(None),
        Err(error) => return Err(symphonia_error(&error)),
    };
    if buffer.frames() == 0 {
        return Ok(None);
    }
    let shape = shape_of(buffer.spec());
    let mut chunk = Vec::new();
    buffer.copy_to_vec_interleaved::<i16>(&mut chunk);
    out.extend_from_slice(&chunk);
    Ok(Some(shape))
}

/// Whether a decode failure only condemns one packet, not the stream.
const fn is_damaged(error: &symphonia::core::errors::Error) -> bool {
    matches!(
        error,
        symphonia::core::errors::Error::DecodeError(_) | symphonia::core::errors::Error::IoError(_)
    )
}

/// The rate and channel count of a buffer specification.
fn shape_of(spec: &AudioSpec) -> Shape {
    let channels = u16::try_from(spec.channels().count()).unwrap_or(1_u16);
    Shape {
        sample_rate: spec.rate(),
        channels,
    }
}

/// Builds the [`Pcm`] from the decoded samples, refusing an empty stream.
fn assemble(drained: Drained) -> Result<Pcm, CyberError> {
    let Drained { samples, shape } = drained;
    if samples.is_empty() {
        return Err(CyberError::Format(
            "the sound file holds no decodable audio; re-export it from the source".to_owned(),
        ));
    }
    let Shape {
        sample_rate,
        channels,
    } = shape.ok_or_else(|| {
        CyberError::Internal("decoding produced samples without an audio specification".to_owned())
    })?;
    Ok(Pcm {
        sample_rate,
        channels,
        samples,
    })
}

/// Refuses a zero sample rate — `Pcm::duration_ms` reports `0` for it, so
/// it would slip past the cap — and audio longer than ten minutes.
fn check_decoded_length(pcm: &Pcm) -> Result<(), CyberError> {
    if pcm.sample_rate == 0 {
        return Err(CyberError::Format(
            "the decoded sound reports a sample rate of 0 Hz, so its length cannot be checked \
             against the decode limit; re-export the file with a positive rate"
                .to_owned(),
        ));
    }
    let longest = pcm.duration_ms();
    if longest > MAX_DECODED_MS {
        return Err(CyberError::LimitExceeded(format!(
            "the sound is {longest} ms long, the decode limit is {MAX_DECODED_MS} ms (10 \
             minutes); trim the file"
        )));
    }
    Ok(())
}

/// Maps a `symphonia` error onto the domain error codes.
fn symphonia_error(error: &symphonia::core::errors::Error) -> CyberError {
    let text = error.to_string();
    if matches!(error, symphonia::core::errors::Error::Unsupported(_)) {
        return CyberError::Unsupported(format!(
            "symphonia: {text}; convert the file to WAV, OGG, FLAC or MP3"
        ));
    }
    CyberError::Format(format!("cannot decode the sound file: {text}"))
}

#[cfg(test)]
mod tests {
    use super::{decode_any, is_mpeg_sync, sniff_format};
    use crate::audio::pcm::Pcm;
    use crate::audio::wav::{decode_wav, encode_wav};

    /// The bytes of a float-`WAV` file the in-house reader refuses.
    fn float_wav(rate: u32, samples: &[f32]) -> Vec<u8> {
        let mut fmt = Vec::new();
        fmt.extend_from_slice(&3_u16.to_le_bytes()); // IEEE float tag
        fmt.extend_from_slice(&1_u16.to_le_bytes()); // mono
        fmt.extend_from_slice(&rate.to_le_bytes());
        let byte_rate = rate.checked_mul(4).expect("test rates fit");
        fmt.extend_from_slice(&byte_rate.to_le_bytes());
        fmt.extend_from_slice(&4_u16.to_le_bytes()); // block align
        fmt.extend_from_slice(&32_u16.to_le_bytes()); // bits
        let data: Vec<u8> = samples.iter().copied().flat_map(f32::to_le_bytes).collect();
        let body = data.len();
        let riff_size = 4_usize
            .checked_add(fmt.len())
            .and_then(|n| n.checked_add(8))
            .and_then(|n| n.checked_add(body))
            .and_then(|n| n.checked_add(8))
            .expect("test sizes fit");
        let mut out: Vec<u8> = Vec::new();
        out.extend_from_slice(b"RIFF");
        let size = u32::try_from(riff_size).expect("test sizes fit");
        out.extend_from_slice(&size.to_le_bytes());
        out.extend_from_slice(b"WAVE");
        out.extend_from_slice(b"fmt ");
        let fmt_len = u32::try_from(fmt.len()).expect("test sizes fit");
        out.extend_from_slice(&fmt_len.to_le_bytes());
        out.extend_from_slice(&fmt);
        out.extend_from_slice(b"data");
        let data_len = u32::try_from(body).expect("test sizes fit");
        out.extend_from_slice(&data_len.to_le_bytes());
        out.extend_from_slice(&data);
        out
    }

    /// `value` written big-endian, without the denied `to_be_bytes`.
    fn be16(value: u16) -> [u8; 2] {
        let mut bytes = value.to_le_bytes();
        bytes.reverse();
        bytes
    }

    /// `value` written big-endian, without the denied `to_be_bytes`.
    fn be64(value: u64) -> [u8; 8] {
        let mut bytes = value.to_le_bytes();
        bytes.reverse();
        bytes
    }

    /// A frame-less `FLAC` stream: "fLaC", one last-marker `STREAMINFO`
    /// block and no frames after it.
    fn empty_flac(rate: u32) -> Vec<u8> {
        let mut info: Vec<u8> = Vec::new();
        info.extend_from_slice(&be16(4096)); // min block size
        info.extend_from_slice(&be16(4096)); // max block size
        info.extend_from_slice(&[0_u8; 3]); // min frame size: unknown
        info.extend_from_slice(&[0_u8; 3]); // max frame size: unknown
        // sample rate << 44 | (channels − 1) << 41 | (bits − 1) << 36 | total samples
        let bits = 15_u64.checked_shl(36).expect("test shift fits");
        let packed = u64::from(rate)
            .checked_shl(44)
            .expect("test rate fits")
            .checked_add(bits)
            .expect("test fields fit");
        info.extend_from_slice(&be64(packed));
        info.extend_from_slice(&[0_u8; 16]); // stream MD5
        let mut out: Vec<u8> = Vec::new();
        out.extend_from_slice(b"fLaC");
        out.push(0x80_u8); // last metadata block, type 0 (STREAMINFO)
        let len = u32::try_from(info.len()).expect("test size fits");
        let mut size = len.to_le_bytes();
        size.reverse();
        out.extend_from_slice(size.get(1..).expect("four bytes")); // 24-bit body size
        out.extend_from_slice(&info);
        out
    }

    /// A mono test [`Pcm`] of `frames` identical samples at 22050 Hz.
    fn tone(frames: usize) -> Pcm {
        Pcm {
            sample_rate: 22050,
            channels: 1,
            samples: vec![1000_i16; frames],
        }
    }

    #[test]
    fn sniff_names_every_supported_container() {
        let pcm = tone(4);
        let wav = encode_wav(&pcm, 16).expect("test wav");
        assert_eq!(sniff_format(&wav), Some("wav"), "RIFF/WAVE");
        assert_eq!(sniff_format(b"OggS....."), Some("ogg"), "ogg capture");
        assert_eq!(sniff_format(b"fLaC....."), Some("flac"), "flac stream");
        assert_eq!(sniff_format(b"ID3....."), Some("mp3"), "id3 tag");
        assert_eq!(
            sniff_format(&[0xFF_u8, 0xFB, 0, 0]),
            Some("mp3"),
            "frame sync"
        );
        assert_eq!(sniff_format(b"NONSENSE...."), None, "unknown bytes");
        assert_eq!(sniff_format(b""), None, "empty input");
        assert_eq!(sniff_format(b"RIFF......"), None, "RIFF without WAVE");
        assert!(is_mpeg_sync(&[0xFF_u8, 0xE0]), "the lowest sync head");
        assert!(!is_mpeg_sync(&[0xFF_u8, 0x1F]), "the highest non-sync head");
    }

    #[test]
    fn riff_wav_decodes_through_the_in_house_reader() {
        let pcm = tone(100);
        let file = encode_wav(&pcm, 16).expect("test wav");
        let back = decode_any(&file, Some("wav")).expect("a plain PCM WAV");
        assert_eq!(back, pcm, "byte-identical round trip, no symphonia");
        let no_hint = decode_any(&file, None).expect("the hint only steers the probe");
        assert_eq!(no_hint, pcm, "no hint, same result");
    }

    #[test]
    fn float_wav_falls_through_to_symphonia() {
        let file = float_wav(22050, &[0.0_f32, 0.5, -0.5, 1.0]);
        let error = decode_wav(&file).expect_err("the in-house reader refuses float");
        assert_eq!(error.code(), "unsupported", "float is unsupported in-house");
        let pcm = decode_any(&file, Some("wav")).expect("symphonia reads float WAV");
        assert_eq!(pcm.sample_rate, 22050, "rate from the container");
        assert_eq!(pcm.channels, 1, "mono from the container");
        assert_eq!(pcm.samples.len(), 4, "one i16 per float sample");
        let converted: [i16; 4] = *pcm.samples.first_chunk().expect("four samples");
        let [zero, half, minus_half, full] = converted;
        assert!(
            i32::from(zero).abs() <= 1_i32,
            "silence stays silence, got {zero}"
        );
        let expected = 0x4000_i32;
        assert!(
            i32::from(half)
                .checked_sub(expected)
                .expect("i16 in i32 range")
                .abs()
                <= 2_i32,
            "half scale, got {half}"
        );
        assert!(
            i32::from(minus_half)
                .checked_add(expected)
                .expect("i16 in i32 range")
                .abs()
                <= 2_i32,
            "negative half, got {minus_half}"
        );
        assert!(
            full > 32000_i16,
            "full scale reaches the i16 top, got {full}"
        );
    }

    #[test]
    fn garbage_is_a_format_error_whatever_the_hint_says() {
        let error = decode_any(b"total nonsense, no container here", Some("mp3"))
            .expect_err("nothing to decode");
        assert_eq!(error.code(), "format_error", "unrecognized bytes");
        assert!(
            error.to_string().contains("cannot identify"),
            "the message names the supported containers: {error}"
        );
    }

    #[test]
    fn a_truncated_wav_reports_both_readers() {
        let mut wav_like = encode_wav(&tone(4), 16).expect("test wav");
        wav_like.truncate(14);
        let broken =
            decode_any(&wav_like, Some("wav")).expect_err("no reader finishes this header");
        assert_eq!(broken.code(), "format_error", "corrupt WAV code");
        assert!(
            broken.to_string().contains("the in-house WAV reader said"),
            "the fallback keeps the in-house complaint: {broken}"
        );
        let short = decode_any(b"RIFF", Some("wav")).expect_err("four bytes are no header");
        assert_eq!(short.code(), "format_error", "stub RIFF code");
    }

    #[test]
    fn a_frameless_flac_stream_decodes_nothing() {
        let file = empty_flac(8000);
        assert_eq!(sniff_format(&file), Some("flac"), "the container sniffs");
        let error = decode_any(&file, Some("flac")).expect_err("no frames to decode");
        assert_eq!(error.code(), "format_error", "empty FLAC code");
    }

    #[test]
    fn audio_past_ten_minutes_is_refused() {
        let pcm = Pcm {
            sample_rate: 1000,
            channels: 1,
            samples: vec![0_i16; 601_000],
        };
        let file = encode_wav(&pcm, 16).expect("test wav");
        let error = decode_any(&file, Some("wav")).expect_err("601 s is over the cap");
        assert_eq!(error.code(), "limit_exceeded", "decode length cap");
        assert!(
            error.to_string().contains("10 minutes"),
            "the message names the cap: {error}"
        );
    }

    #[test]
    fn a_zero_rate_pcm_cannot_dodge_the_length_check() {
        // duration_ms is 0 for a zero rate, so without the check this
        // would sail past the ten-minute cap.
        let pcm = Pcm {
            sample_rate: 0,
            channels: 1,
            samples: vec![0_i16; 700_000],
        };
        let error = super::check_decoded_length(&pcm).expect_err("0 Hz is not measurable");
        assert_eq!(error.code(), "format_error", "zero rate code");
        assert!(
            error.to_string().contains("0 Hz"),
            "the message names the rate: {error}"
        );
    }

    #[test]
    fn a_zero_rate_wav_never_decodes() {
        let mut fmt = Vec::new();
        fmt.extend_from_slice(&1_u16.to_le_bytes()); // integer PCM
        fmt.extend_from_slice(&1_u16.to_le_bytes()); // mono
        fmt.extend_from_slice(&0_u32.to_le_bytes()); // rate 0
        fmt.extend_from_slice(&0_u32.to_le_bytes()); // byte rate
        fmt.extend_from_slice(&2_u16.to_le_bytes()); // block align
        fmt.extend_from_slice(&16_u16.to_le_bytes()); // bits
        let data = [0_u8; 8];
        let mut out: Vec<u8> = Vec::new();
        out.extend_from_slice(b"RIFF");
        let riff_size = fmt
            .len()
            .checked_add(data.len())
            .and_then(|n| n.checked_add(20))
            .expect("test size fits");
        let size = u32::try_from(riff_size).expect("test size fits u32");
        out.extend_from_slice(&size.to_le_bytes());
        out.extend_from_slice(b"WAVE");
        out.extend_from_slice(b"fmt ");
        out.extend_from_slice(
            &u32::try_from(fmt.len())
                .expect("test size fits")
                .to_le_bytes(),
        );
        out.extend_from_slice(&fmt);
        out.extend_from_slice(b"data");
        out.extend_from_slice(
            &u32::try_from(data.len())
                .expect("test size fits")
                .to_le_bytes(),
        );
        out.extend_from_slice(&data);
        let error = decode_any(&out, Some("wav")).expect_err("a 0 Hz header cannot decode");
        assert_eq!(error.code(), "format_error", "zero rate wav code");
    }
}
