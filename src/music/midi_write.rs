//! A small `Standard MIDI File` writer: one track of delta-prefixed events
//! and the type-0 rendering that wraps them.
//!
//! [`MidiWriter`] accumulates a track body exactly as `SMF` stores it: every
//! event is preceded by its delta time as a variable-length quantity and
//! running status is never used, so each event carries its own status byte.
//! [`MidiWriter::into_type0`] closes the track and prepends the file header.
//! [`write_vlq`] and [`read_vlq`] are the shared `VLQ` helpers; `Doom`'s
//! `MUS` delays use the same encoding, so [`crate::music::mus2mid`] reads
//! with the latter.
//!
//! `SMF` is big-endian and the crate denies `to_be_bytes`, so multi-byte
//! values are written by reversing their little-endian byte arrays.

use crate::error::CyberError;
use crate::wad::bytes::{Reader, usize_to_u32};

/// Chunk id of the `SMF` header chunk.
const HEADER_ID: [u8; 4] = *b"MThd";
/// Chunk id of a track chunk.
const TRACK_ID: [u8; 4] = *b"MTrk";
/// Body length of the `SMF` header chunk: format, track count, division.
const HEADER_BODY_LEN: u32 = 6_u32;
/// Bytes before a chunk payload: the id and its 32-bit length.
const CHUNK_PREFIX_LEN: usize = 8;
/// Bytes of the whole header chunk: id, length and body.
const HEADER_CHUNK_LEN: usize = 14;
/// Format code of a type-0 file: one track holds the whole score.
const FORMAT_TYPE0: u16 = 0_u16;
/// A type-0 file holds exactly one track.
const TYPE0_TRACKS: u16 = 1_u16;
/// Status byte that introduces a meta event.
const META_STATUS: u8 = 0xFF_u8;
/// Meta type of a tempo setting.
const META_TEMPO: u8 = 0x51_u8;
/// Meta type of the end of track.
const META_END_OF_TRACK: u8 = 0x2F_u8;
/// Payload length of a tempo meta event: 24 bits of microseconds.
const TEMPO_DATA_LEN: u8 = 3_u8;
/// Payload length of an end-of-track meta event: none.
const END_OF_TRACK_DATA_LEN: u8 = 0_u8;
/// Bits per `VLQ` digit.
const VLQ_BITS: u32 = 7_u32;
/// The value bits of one `VLQ` digit.
const VLQ_DIGIT: u8 = 0x7F_u8;
/// Set on every `VLQ` byte but the last.
const VLQ_MORE: u8 = 0x80_u8;
/// Most bytes a `VLQ` occupies, which caps it at 28 bits.
const VLQ_MAX_BYTES: usize = 4;
/// The largest value a `VLQ` carries: the 28 bits its four digits hold.
pub const MAX_VLQ_VALUE: u32 = 0x0FFF_FFFF_u32;
/// The low eight bits of a wider value.
const LOW_BYTE: u32 = 0xFF_u32;
/// Shifts that pick the three bytes of a 24-bit big-endian value, top first.
const U24_SHIFTS: [u32; 3] = [16_u32, 8_u32, 0_u32];

/// The byte of `value` starting at bit `shift`.
fn byte_at(value: u32, shift: u32) -> u8 {
    let bits = value.checked_shr(shift).unwrap_or(0) & LOW_BYTE;
    u8::try_from(bits).unwrap_or(0_u8)
}

/// The `VLQ` digit of `value` starting at bit `shift`.
fn digit_at(value: u32, shift: u32) -> u8 {
    let bits = value.checked_shr(shift).unwrap_or(0) & u32::from(VLQ_DIGIT);
    u8::try_from(bits).unwrap_or(0_u8)
}

/// Appends `value` as a big-endian `u16` by reversing its little-endian
/// bytes.
pub(crate) fn put_be_u16(out: &mut Vec<u8>, value: u16) {
    let mut word = value.to_le_bytes();
    word.reverse();
    out.extend_from_slice(&word);
}

/// Appends `value` as a big-endian `u32`.
pub(crate) fn put_be_u32(out: &mut Vec<u8>, value: u32) {
    let mut word = value.to_le_bytes();
    word.reverse();
    out.extend_from_slice(&word);
}

/// How many bytes `value` needs as a `VLQ`, at most [`VLQ_MAX_BYTES`].
const fn vlq_len(value: u32) -> usize {
    match value {
        0_u32..=0x7F_u32 => 1_usize,
        0x80_u32..=0x3FFF_u32 => 2_usize,
        0x4000_u32..=0x1F_FFFF_u32 => 3_usize,
        _ => VLQ_MAX_BYTES,
    }
}

/// The shift of the most significant `VLQ` digit of `value`.
fn vlq_top_shift(value: u32) -> u32 {
    let digits = vlq_len(value)
        .checked_sub(1)
        .and_then(|index| u32::try_from(index).ok())
        .unwrap_or(0_u32);
    digits.checked_mul(VLQ_BITS).unwrap_or(0_u32)
}

/// Appends `value` as a variable-length quantity: seven bits per byte, high
/// bit set on every byte but the last. A value above the 28-bit maximum is
/// written from its low 28 bits.
#[inline]
pub fn write_vlq(out: &mut Vec<u8>, value: u32) {
    let mut shift = vlq_top_shift(value);
    loop {
        let digit = digit_at(value, shift);
        if shift == 0 {
            out.push(digit);
            return;
        }
        out.push(digit | VLQ_MORE);
        shift = shift.saturating_sub(VLQ_BITS);
    }
}

/// Reads a variable-length quantity from `reader`, the counterpart of
/// [`write_vlq`].
///
/// # Errors
///
/// [`CyberError::Format`] when the data ends mid-quantity or when a fifth
/// byte still carries a continuation bit.
#[inline]
pub fn read_vlq(reader: &mut Reader<'_>) -> Result<u32, CyberError> {
    let mut value = 0_u32;
    for _ in 0_usize..VLQ_MAX_BYTES {
        let byte = reader.read_u8()?;
        value = value
            .checked_shl(VLQ_BITS)
            .and_then(|shifted| shifted.checked_add(u32::from(byte & VLQ_DIGIT)))
            .ok_or_else(overlong_vlq)?;
        if byte & VLQ_MORE == 0 {
            return Ok(value);
        }
    }
    Err(overlong_vlq())
}

/// The failure for a quantity that never ends its continuation bytes.
fn overlong_vlq() -> CyberError {
    CyberError::Format(format!(
        "variable-length quantity spans more than {VLQ_MAX_BYTES} bytes; the input is not a MUS or MIDI stream"
    ))
}

/// Builds the track body of a type-0 `Standard MIDI File`.
///
/// Events are appended with the delta time that precedes them; the writer
/// adds no events of its own except the end-of-track meta event that
/// [`MidiWriter::into_type0`] writes.
#[derive(Debug, Default)]
pub struct MidiWriter {
    /// The track body bytes written so far, without an end of track.
    body: Vec<u8>,
}

impl MidiWriter {
    /// A writer with an empty track body.
    #[must_use]
    #[inline]
    pub fn new() -> Self {
        Self::default()
    }

    /// Appends a channel event: the delta as a `VLQ`, then `status` and its
    /// `data` bytes. Running status is never used, so `status` is written
    /// even when it repeats the previous event's.
    #[inline]
    pub fn put_event(&mut self, delta: u32, status: u8, data: &[u8]) {
        write_vlq(&mut self.body, delta);
        self.body.push(status);
        self.body.extend_from_slice(data);
    }

    /// Appends a meta event: the delta as a `VLQ`, `FF`, `kind`, the payload
    /// length as a `VLQ`, then the payload.
    ///
    /// # Errors
    ///
    /// [`CyberError::LimitExceeded`] when the payload is longer than 32 bits
    /// can count.
    #[inline]
    pub fn put_meta(&mut self, delta: u32, kind: u8, data: &[u8]) -> Result<(), CyberError> {
        let length = usize_to_u32(data.len(), "MIDI meta event")?;
        write_vlq(&mut self.body, delta);
        self.body.push(META_STATUS);
        self.body.push(kind);
        write_vlq(&mut self.body, length);
        self.body.extend_from_slice(data);
        Ok(())
    }

    /// Appends a tempo meta event: `microseconds` per quarter note, the
    /// three-byte big-endian value `SMF` stores.
    #[inline]
    pub fn put_tempo(&mut self, delta: u32, microseconds: u32) {
        write_vlq(&mut self.body, delta);
        self.body.push(META_STATUS);
        self.body.push(META_TEMPO);
        self.body.push(TEMPO_DATA_LEN);
        self.body.extend_from_slice(&tempo_bytes(microseconds));
    }

    /// Appends the end-of-track meta event at `delta`.
    fn put_end_of_track(&mut self, delta: u32) {
        write_vlq(&mut self.body, delta);
        self.body.push(META_STATUS);
        self.body.push(META_END_OF_TRACK);
        self.body.push(END_OF_TRACK_DATA_LEN);
    }

    /// Renders a complete type-0 file: a header declaring one track at
    /// `division` ticks per quarter note, then this track body closed by an
    /// end-of-track meta event at `final_delta`.
    ///
    /// # Errors
    ///
    /// [`CyberError::LimitExceeded`] when the track body is longer than 32
    /// bits can count, which no `SMF` reader accepts.
    #[inline]
    pub fn into_type0(mut self, division: u16, final_delta: u32) -> Result<Vec<u8>, CyberError> {
        self.put_end_of_track(final_delta);
        let body = self.body;
        let length = usize_to_u32(body.len(), "MIDI track")?;
        let capacity = HEADER_CHUNK_LEN
            .checked_add(CHUNK_PREFIX_LEN)
            .and_then(|head| head.checked_add(body.len()))
            .unwrap_or(0_usize);
        let mut out = Vec::with_capacity(capacity);
        out.extend_from_slice(&HEADER_ID);
        put_be_u32(&mut out, HEADER_BODY_LEN);
        put_be_u16(&mut out, FORMAT_TYPE0);
        put_be_u16(&mut out, TYPE0_TRACKS);
        put_be_u16(&mut out, division);
        out.extend_from_slice(&TRACK_ID);
        put_be_u32(&mut out, length);
        out.extend_from_slice(&body);
        Ok(out)
    }
}

/// `microseconds` as the three big-endian bytes a tempo meta event stores.
fn tempo_bytes(microseconds: u32) -> [u8; 3] {
    U24_SHIFTS.map(|shift| byte_at(microseconds, shift))
}

#[cfg(test)]
mod tests {
    use super::{
        MidiWriter, U24_SHIFTS, digit_at, overlong_vlq, put_be_u16, put_be_u32, read_vlq,
        tempo_bytes, vlq_len, write_vlq,
    };
    use crate::wad::bytes::Reader;

    #[test]
    fn vlq_encodes_the_documented_boundaries() {
        let cases: [(u32, [u8; 4]); 6] = [
            (0, [0x00, 0x00, 0x00, 0x00]),
            (127, [0x7F, 0x00, 0x00, 0x00]),
            (128, [0x81, 0x00, 0x00, 0x00]),
            (0x3FFF, [0xFF, 0x7F, 0x00, 0x00]),
            (0x4000, [0x81, 0x80, 0x00, 0x00]),
            (0x0FFF_FFFF, [0xFF, 0xFF, 0xFF, 0x7F]),
        ];
        for (value, expected) in cases {
            let mut bytes = Vec::new();
            write_vlq(&mut bytes, value);
            let width = vlq_len(value);
            assert_eq!(&bytes[..width], &expected[..width], "encoding of {value}");
            assert_eq!(bytes.len(), width, "length of {value}");
        }
    }

    #[test]
    fn vlq_round_trips_every_boundary() {
        for value in [0_u32, 127, 128, 0x3FFF, 0x4000, 0x0FFF_FFFF] {
            let mut bytes = Vec::new();
            write_vlq(&mut bytes, value);
            let mut reader = Reader::new(&bytes);
            assert_eq!(
                read_vlq(&mut reader).expect("a written quantity reads back"),
                value,
                "round trip of {value}"
            );
            assert_eq!(reader.pos(), bytes.len(), "{value} read in full");
        }
    }

    #[test]
    fn vlq_above_28_bits_keeps_its_low_digits() {
        let mut bytes = Vec::new();
        write_vlq(&mut bytes, 0x1FFF_FFFF);
        assert_eq!(
            bytes,
            [0xFF, 0xFF, 0xFF, 0x7F],
            "truncated to the 28-bit maximum"
        );
    }

    #[test]
    fn vlq_reads_multibyte_quantities() {
        let mut reader = Reader::new(&[0x81_u8, 0x80, 0x00, 0x7F]);
        assert_eq!(
            read_vlq(&mut reader).expect("three bytes"),
            0x4000_u32,
            "81 80 00 is 16384"
        );
        assert_eq!(reader.pos(), 3, "the fourth byte is left alone");
    }

    #[test]
    fn vlq_errors_are_format_failures() {
        let mut empty = Reader::new(&[]);
        let truncated = read_vlq(&mut empty).expect_err("no data to read");
        assert_eq!(truncated.code(), "format_error", "truncated code");

        let mut overlong = Reader::new(&[0xFF_u8, 0xFF, 0xFF, 0xFF, 0xFF]);
        let error = read_vlq(&mut overlong).expect_err("five continuation bytes");
        assert_eq!(error.code(), "format_error", "overlong code");
        assert_eq!(
            error.to_string(),
            overlong_vlq().to_string(),
            "overlong message"
        );
    }

    #[test]
    fn big_endian_writers_reverse_the_little_endian_bytes() {
        let mut words = Vec::new();
        put_be_u16(&mut words, 0x0046_u16);
        put_be_u32(&mut words, 0x0000_0006_u32);
        assert_eq!(
            words,
            vec![0x00, 0x46, 0x00, 0x00, 0x00, 0x06],
            "SMF chunk fields"
        );
    }

    #[test]
    fn tempo_bytes_are_the_documented_three_bytes_big_endian() {
        assert_eq!(
            tempo_bytes(500_000),
            [0x07, 0xA1, 0x20],
            "500000 microseconds"
        );
        assert_eq!(tempo_bytes(0), [0x00, 0x00, 0x00], "zero tempo");
    }

    #[test]
    fn digit_at_masks_seven_bits_per_shift() {
        assert_eq!(digit_at(0x3FC0_u32, 0), 0x40_u8, "low digit");
        assert_eq!(digit_at(0x3FC0_u32, 7), 0x7F_u8, "high digit");
    }

    #[test]
    fn writer_renders_a_type_zero_file() {
        /// The delta-`0` tempo event a converted score starts with.
        const TEMPO: [u8; 7] = [0x00, 0xFF, 0x51, 0x03, 0x07, 0xA1, 0x20];

        let mut midi = MidiWriter::new();
        midi.put_tempo(0, 500_000);
        midi.put_event(4, 0x90, &[0x3C, 0x7F]);
        midi.put_meta(0, 0x58, &[4, 2, 24, 8])
            .expect("a four byte meta payload");
        let mut body: Vec<u8> = TEMPO.to_vec();
        body.extend_from_slice(&[0x04, 0x90, 0x3C, 0x7F]);
        body.extend_from_slice(&[0x00, 0xFF, 0x58, 0x04, 0x04, 0x02, 0x18, 0x08]);
        body.extend_from_slice(&[0x81, 0x00, 0xFF, 0x2F, 0x00]);
        assert_eq!(body.len(), 24, "hand-counted track length");
        let mut expected: Vec<u8> = Vec::new();
        expected.extend_from_slice(b"MThd");
        expected.extend_from_slice(&[0x00, 0x00, 0x00, 0x06, 0x00, 0x00, 0x00, 0x01, 0x00, 0x46]);
        expected.extend_from_slice(b"MTrk");
        expected.extend_from_slice(&[0x00, 0x00, 0x00, 0x18]);
        expected.extend_from_slice(&body);
        assert_eq!(
            midi.into_type0(70, 128).expect("a short track"),
            expected,
            "the whole type-0 file, with the end of track queued 128 ticks out"
        );
    }

    #[test]
    fn u24_shifts_read_a_big_endian_field_top_first() {
        assert_eq!(U24_SHIFTS, [16_u32, 8_u32, 0_u32], "byte order");
    }
}
