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
//! The chunk ids, meta status bytes and tempo field the writer lays down are
//! the shared `SMF` vocabulary of [`crate::music`]'s `consts` module, which
//! [`crate::music::midi_read`] parses against, and the big-endian widths go
//! through [`Writer`]'s `_be` methods: the one place the crate spells that
//! byte order.

use crate::error::CyberError;
use crate::music::consts::{
    MIDI_META_END_OF_TRACK, MIDI_META_STATUS, MIDI_META_TEMPO, MIDI_TEMPO_DATA_LEN,
    SMF_HEADER_BODY_LEN, SMF_HEADER_ID, SMF_TRACK_ID, TEMPO_SHIFTS,
};
use crate::wad::bytes::{Reader, Writer, usize_to_u32};

/// Bytes before a chunk payload: the id and its 32-bit length.
const CHUNK_PREFIX_LEN: usize = 8;
/// Bytes of the whole header chunk: id, length and body.
const HEADER_CHUNK_LEN: usize = 14;
/// Format code of a type-0 file: one track holds the whole score.
const FORMAT_TYPE0: u16 = 0_u16;
/// A type-0 file holds exactly one track.
const TYPE0_TRACKS: u16 = 1_u16;
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

    /// Appends a tempo meta event: `microseconds` per quarter note, the
    /// three-byte big-endian value `SMF` stores.
    #[inline]
    pub fn put_tempo(&mut self, delta: u32, microseconds: u32) {
        write_vlq(&mut self.body, delta);
        self.body.push(MIDI_META_STATUS);
        self.body.push(MIDI_META_TEMPO);
        self.body.push(MIDI_TEMPO_DATA_LEN);
        self.body.extend_from_slice(&tempo_bytes(microseconds));
    }

    /// Appends the end-of-track meta event at `delta`.
    fn put_end_of_track(&mut self, delta: u32) {
        write_vlq(&mut self.body, delta);
        self.body.push(MIDI_META_STATUS);
        self.body.push(MIDI_META_END_OF_TRACK);
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
        let mut out = Writer::with_capacity(capacity);
        out.put_bytes(&SMF_HEADER_ID);
        out.put_u32_be(SMF_HEADER_BODY_LEN);
        out.put_u16_be(FORMAT_TYPE0);
        out.put_u16_be(TYPE0_TRACKS);
        out.put_u16_be(division);
        out.put_bytes(&SMF_TRACK_ID);
        out.put_u32_be(length);
        out.put_bytes(&body);
        Ok(out.into_inner())
    }
}

/// `microseconds` as the three big-endian bytes a tempo meta event stores.
fn tempo_bytes(microseconds: u32) -> [u8; 3] {
    TEMPO_SHIFTS.map(|shift| byte_at(microseconds, shift))
}

#[cfg(test)]
mod tests {
    use super::{MidiWriter, digit_at, overlong_vlq, read_vlq, tempo_bytes, vlq_len, write_vlq};
    use crate::music::midi_read::parse_midi;
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
        let mut body: Vec<u8> = TEMPO.to_vec();
        body.extend_from_slice(&[0x04, 0x90, 0x3C, 0x7F]);
        body.extend_from_slice(&[0x81, 0x00, 0xFF, 0x2F, 0x00]);
        assert_eq!(body.len(), 16, "hand-counted track length");
        let mut expected: Vec<u8> = Vec::new();
        expected.extend_from_slice(b"MThd");
        expected.extend_from_slice(&[0x00, 0x00, 0x00, 0x06, 0x00, 0x00, 0x00, 0x01, 0x00, 0x46]);
        expected.extend_from_slice(b"MTrk");
        expected.extend_from_slice(&[0x00, 0x00, 0x00, 0x10]);
        expected.extend_from_slice(&body);
        let rendered = midi.into_type0(70, 128).expect("a short track");
        assert_eq!(
            rendered, expected,
            "the whole type-0 file, with the end of track queued 128 ticks out"
        );
    }

    #[test]
    fn a_rendered_file_parses_back_with_the_shared_reader() {
        let mut midi = MidiWriter::new();
        midi.put_tempo(0, 500_000);
        midi.put_event(10, 0x91, &[0x3C, 0x40]);
        let file = parse_midi(
            &midi
                .into_type0(70, 5)
                .expect("a short track renders a whole file"),
        )
        .expect("what the writer renders, the reader reads");
        assert_eq!(
            file.division, 70,
            "the big-endian division field round trips"
        );
        assert_eq!(file.events.len(), 3, "tempo, note and end of track");
    }
}
