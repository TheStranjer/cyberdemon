//! `Doom` `MUS` music lumps converted to `Standard MIDI Files`.
//!
//! A `MUS` score is a little-endian header, an instrument table and a stream
//! of packed events; [`mus_to_midi`] walks that stream and writes the type-0
//! `SMF` equivalent with [`MidiWriter`]. The conversion is table-driven and
//! makes no choices of its own: channel 15 becomes the percussion channel,
//! every other `MUS` channel takes the next free `MIDI` channel in order of
//! first use, and nothing but the translated events is emitted.
//!
//! `MUS` runs at 140 ticks per second. With a division of 70 ticks per
//! quarter note at 500 000 microseconds per quarter, an `MUS` delay is
//! already a `MIDI` delta time, so delays are copied through unchanged.

use crate::error::CyberError;
use crate::music::consts::{
    BEND_SHIFT, CHANNEL_COUNT, CHANNEL_MASK, DEFAULT_VOLUME, EVENT_BEND, EVENT_CONTROL,
    EVENT_MEASURE_END, EVENT_NOTE, EVENT_RELEASE, EVENT_SCORE_END, EVENT_SYSTEM, EVENT_TYPE_MASK,
    EVENT_TYPE_SHIFT, LAST_EVENT_FLAG, MAX_VALUE, MIDI_CHANNEL_LAST, MIDI_CHANNEL_POOL,
    MIDI_CONTROL_CHANGE, MIDI_NOTE_OFF, MIDI_NOTE_ON, MIDI_PERCUSSION_CHANNEL, MIDI_PITCH_BEND,
    MIDI_PROGRAM_CHANGE, MUS_HEADER_LEN, MUS_MAGIC, MUS_PERCUSSION_CHANNEL, MUS_SCORE_LEN_OFFSET,
    MUS_SCORE_START_OFFSET, MUS_TEMPO_MICROSECONDS, MUS_TICKS_PER_QUARTER, PROGRAM_CONTROLLER,
    VOLUME_FLAG, ZERO_VALUE, midi_control, system_control,
};
use crate::music::midi_write::{MidiWriter, read_vlq};
use crate::wad::bytes::{Reader, u16_at};

/// Shifts of the two 7-bit digits of a 14-bit value, least significant first.
const BEND_DIGITS: [u32; 2] = [0_u32, 7_u32];
/// Seven bits of a wider value, for splitting a 14-bit one.
const SEVEN_BITS: u32 = 0x7F_u32;

/// The event type held in bits 4..6 of a descriptor byte.
#[derive(Debug, Clone, Copy)]
pub(crate) enum EventKind {
    /// `EVENT_RELEASE`: stop a note.
    Release,
    /// `EVENT_NOTE`: start a note, with an optional volume byte.
    Note,
    /// `EVENT_BEND`: pitch bend, 128 at centre.
    Bend,
    /// `EVENT_SYSTEM`: a system controller, which carries no value.
    System,
    /// `EVENT_CONTROL`: a controller value, or a program change at number 0.
    Controller,
    /// `EVENT_MEASURE_END`: end of measure, no data and no output.
    MeasureEnd,
    /// `EVENT_SCORE_END`: end of score.
    ScoreEnd,
    /// Any other type bits: unused, no data.
    Unused,
}

impl EventKind {
    /// Decodes the three type bits of a descriptor byte.
    pub(crate) const fn from_bits(bits: u8) -> Self {
        match bits {
            EVENT_RELEASE => Self::Release,
            EVENT_NOTE => Self::Note,
            EVENT_BEND => Self::Bend,
            EVENT_SYSTEM => Self::System,
            EVENT_CONTROL => Self::Controller,
            EVENT_MEASURE_END => Self::MeasureEnd,
            EVENT_SCORE_END => Self::ScoreEnd,
            _ => Self::Unused,
        }
    }
}

/// The two 7-bit digits of a 14-bit `MIDI` value, least significant first.
fn bend_digits(value: u32) -> [u8; 2] {
    BEND_DIGITS.map(|shift| {
        let bits = value.checked_shr(shift).unwrap_or(0) & SEVEN_BITS;
        u8::try_from(bits).unwrap_or(0_u8)
    })
}

/// Converts the bytes of a `Doom` `MUS` lump into a type-0 `Standard MIDI`
/// file.
///
/// The output is a `MThd` header at 70 ticks per quarter note with one
/// `MTrk` track: a tempo meta event at time 0, the translated events, and an
/// end-of-track meta event. Delays are copied through unchanged, so the file
/// plays at the speed of the score; a delay queued by the last group of
/// events becomes the end of track's delta, keeping the total length.
///
/// The channel masks and the instrument table in the header are not needed:
/// every patch change a score makes arrives as a controller event. A `MUS`
/// channel is given a `MIDI` channel when an event on it reaches the output,
/// and no event is written for the change itself.
///
/// # Errors
///
/// [`CyberError::Format`] when the magic is missing, the header or the
/// declared score runs past the end of `bytes`, an event is cut off in the
/// middle, or a delay spans more than four bytes.
#[inline]
pub fn mus_to_midi(bytes: &[u8]) -> Result<Vec<u8>, CyberError> {
    let score = score_bytes(bytes)?;
    let mut midi = MidiWriter::new();
    midi.put_tempo(0_u32, MUS_TEMPO_MICROSECONDS);
    let mut converter = Converter::new(&mut midi);
    let mut reader = Reader::new(score);
    while reader.remaining() > 0 {
        if !converter.event(&mut reader)? {
            break;
        }
    }
    let trailing_delay = converter.pending_delay;
    midi.into_type0(MUS_TICKS_PER_QUARTER, trailing_delay)
}

/// The event stream the header declares, checked against the lump.
fn score_bytes(bytes: &[u8]) -> Result<&[u8], CyberError> {
    if !bytes.starts_with(&MUS_MAGIC) {
        return Err(CyberError::Format(
            "not a MUS file: the data does not start with 'MUS' and 0x1A; check the lump name starts with D_".to_owned(),
        ));
    }
    if bytes.len() < MUS_HEADER_LEN {
        return Err(CyberError::Format(format!(
            "MUS header is {} bytes, shorter than the {MUS_HEADER_LEN} bytes it needs; convert the whole D_* lump",
            bytes.len(),
        )));
    }
    let score_len = usize::from(u16_at(bytes, MUS_SCORE_LEN_OFFSET)?);
    let score_start = usize::from(u16_at(bytes, MUS_SCORE_START_OFFSET)?);
    // Both fields are 16-bit, so their sum always fits in a usize.
    let score_end = score_start.saturating_add(score_len);
    bytes.get(score_start..score_end).ok_or_else(|| {
        CyberError::Format(format!(
            "MUS score is {score_len} bytes at offset {score_start}, past the {} bytes of the lump; convert the whole D_* lump",
            bytes.len(),
        ))
    })
}

/// What a `MUS` controller event selects in the output.
#[derive(Debug, Clone, Copy)]
enum Control {
    /// A program change to this patch number.
    Program(u8),
    /// A control change of this `MIDI` controller to this value.
    Change {
        /// The `MIDI` controller number to change.
        controller: u8,
        /// The value to send with it.
        data: u8,
    },
    /// A `MUS` controller number with no `MIDI` counterpart.
    Ignored,
}

/// Classifies a `MUS` controller event by its number and clamped value.
const fn control_event(number: u8, value: u8) -> Control {
    if number == PROGRAM_CONTROLLER {
        return Control::Program(value);
    }
    match midi_control(number) {
        Some(controller) => Control::Change {
            controller,
            data: value,
        },
        None => Control::Ignored,
    }
}

/// State carried across the events of one score: the track being written, the
/// channel mapping, the volume each channel last played at, and the delay
/// still to be written.
struct Converter<'writer> {
    /// The track the translated events go into.
    out: &'writer mut MidiWriter,
    /// The `MIDI` channel given to each `MUS` channel, `None` until an event
    /// on it reaches the output.
    mapped: [Option<u8>; CHANNEL_COUNT],
    /// How many pool channels have been handed out.
    next_channel: usize,
    /// The volume each `MUS` channel last played at.
    volumes: [u8; CHANNEL_COUNT],
    /// Ticks to write before the next event that reaches the output.
    pending_delay: u32,
}

impl<'writer> Converter<'writer> {
    /// A converter writing its events into `out`, with no channel mapped, no
    /// volume remembered and no delay queued.
    const fn new(out: &'writer mut MidiWriter) -> Self {
        Self {
            out,
            mapped: [None; CHANNEL_COUNT],
            next_channel: 0_usize,
            volumes: [DEFAULT_VOLUME; CHANNEL_COUNT],
            pending_delay: 0_u32,
        }
    }

    /// Translates the event at the cursor, then queues the delay of a marked
    /// last event. Returns `false` once the score has ended.
    fn event(&mut self, reader: &mut Reader<'_>) -> Result<bool, CyberError> {
        let descriptor = reader.read_u8()?;
        let bits = (descriptor & EVENT_TYPE_MASK)
            .checked_shr(EVENT_TYPE_SHIFT)
            .unwrap_or(0_u8);
        let channel = descriptor & CHANNEL_MASK;
        let continued = match EventKind::from_bits(bits) {
            EventKind::Release => self.release(reader, channel)?,
            EventKind::Note => self.note(reader, channel)?,
            EventKind::Bend => self.bend(reader, channel)?,
            EventKind::System => self.system(reader, channel)?,
            EventKind::Controller => self.controller(reader, channel)?,
            EventKind::MeasureEnd | EventKind::Unused => true,
            EventKind::ScoreEnd => false,
        };
        if descriptor & LAST_EVENT_FLAG != 0 {
            self.queue(reader)?;
        }
        Ok(continued)
    }

    /// `MUS` 0: stop a note at velocity 0.
    fn release(&mut self, reader: &mut Reader<'_>, channel: u8) -> Result<bool, CyberError> {
        let note = reader.read_u8()? & MAX_VALUE;
        self.emit(channel, MIDI_NOTE_OFF, &[note, ZERO_VALUE]);
        Ok(true)
    }

    /// `MUS` 1: start a note. A volume byte follows the key when its high bit
    /// is set and becomes the volume remembered for this channel.
    fn note(&mut self, reader: &mut Reader<'_>, channel: u8) -> Result<bool, CyberError> {
        let key = reader.read_u8()?;
        let volume = if key & VOLUME_FLAG != 0 {
            let value = reader.read_u8()?.min(MAX_VALUE);
            self.set_volume(channel, value);
            value
        } else {
            self.volume(channel)
        };
        self.emit(channel, MIDI_NOTE_ON, &[key & MAX_VALUE, volume]);
        Ok(true)
    }

    /// `MUS` 2: pitch bend. The value is 128 at centre and becomes the 14-bit
    /// `MIDI` bend, sent as two 7-bit digits.
    fn bend(&mut self, reader: &mut Reader<'_>, channel: u8) -> Result<bool, CyberError> {
        let value = u32::from(reader.read_u8()?)
            .checked_shl(BEND_SHIFT)
            .unwrap_or(0_u32);
        self.emit(channel, MIDI_PITCH_BEND, &bend_digits(value));
        Ok(true)
    }

    /// `MUS` 3: a system controller, written as a valueless `MIDI` `CC`.
    fn system(&mut self, reader: &mut Reader<'_>, channel: u8) -> Result<bool, CyberError> {
        let number = reader.read_u8()?;
        if let Some(control) = system_control(number) {
            self.emit(channel, MIDI_CONTROL_CHANGE, &[control, ZERO_VALUE]);
        }
        Ok(true)
    }

    /// `MUS` 4: a program change at controller 0, another mapped controller,
    /// or nothing at all when the number has no counterpart.
    fn controller(&mut self, reader: &mut Reader<'_>, channel: u8) -> Result<bool, CyberError> {
        let number = reader.read_u8()?;
        let value = reader.read_u8()?.min(MAX_VALUE);
        match control_event(number, value) {
            Control::Program(patch) => self.emit(channel, MIDI_PROGRAM_CHANGE, &[patch]),
            Control::Change { controller, data } => {
                self.emit(channel, MIDI_CONTROL_CHANGE, &[controller, data]);
            }
            Control::Ignored => {}
        }
        Ok(true)
    }

    /// Writes one channel event for a `MUS` channel: `status` with the mapped
    /// channel in its low nibble, and the queued delay as its delta time.
    fn emit(&mut self, channel: u8, status: u8, data: &[u8]) {
        let mapped = self.midi_channel(channel);
        let delta = self.pending_delay;
        self.pending_delay = 0_u32;
        self.out.put_event(delta, status | mapped, data);
    }

    /// The `MIDI` channel a `MUS` channel plays on. Percussion is always 9;
    /// every other channel takes the next pool entry the first time one of
    /// its events is written.
    fn midi_channel(&mut self, channel: u8) -> u8 {
        if channel == MUS_PERCUSSION_CHANNEL {
            return MIDI_PERCUSSION_CHANNEL;
        }
        let index = usize::from(channel);
        if let Some(mapped) = self.mapped.get(index).copied().flatten() {
            return mapped;
        }
        let mapped = self.take_pool_channel();
        if let Some(slot) = self.mapped.get_mut(index) {
            *slot = Some(mapped);
        }
        mapped
    }

    /// The next free `MIDI` channel, or the last pool entry once all fifteen
    /// have been handed out.
    fn take_pool_channel(&mut self) -> u8 {
        let index = self.next_channel;
        self.next_channel = index.checked_add(1).unwrap_or(index);
        MIDI_CHANNEL_POOL
            .get(index)
            .copied()
            .unwrap_or(MIDI_CHANNEL_LAST)
    }

    /// Queues the delay that follows a marked last event, added to anything
    /// an event that wrote nothing already queued.
    fn queue(&mut self, reader: &mut Reader<'_>) -> Result<(), CyberError> {
        let delay = read_vlq(reader)?;
        self.pending_delay = self.pending_delay.saturating_add(delay);
        Ok(())
    }

    /// The volume a `MUS` channel last played at.
    fn volume(&self, channel: u8) -> u8 {
        self.volumes
            .get(usize::from(channel))
            .copied()
            .unwrap_or(DEFAULT_VOLUME)
    }

    /// Remembers the volume a `MUS` channel plays at.
    fn set_volume(&mut self, channel: u8, volume: u8) {
        if let Some(slot) = self.volumes.get_mut(usize::from(channel)) {
            *slot = volume;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::mus_to_midi;
    use crate::music::consts::{
        MIDI_CHANNEL_POOL, MUS_MAGIC, MUS_TICKS_PER_QUARTER, SMF_HEADER_BODY_LEN, SMF_HEADER_ID,
        SMF_TRACK_ID,
    };
    use crate::wad::bytes::Writer;

    /// The tempo meta event every track starts with.
    const TEMPO: [u8; 7] = [0x00, 0xFF, 0x51, 0x03, 0x07, 0xA1, 0x20];
    /// The end-of-track meta event, without its delta byte.
    const END: [u8; 3] = [0xFF, 0x2F, 0x00];

    /// A `MUS` lump: the 16-byte header, one instrument, `score` at 18.
    fn mus_lump(score: &[u8]) -> Vec<u8> {
        let mut lump = Writer::new();
        lump.put_bytes(&MUS_MAGIC);
        lump.put_u16(u16::try_from(score.len()).expect("a hand-built score fits in a u16"));
        lump.put_u16(18_u16);
        lump.put_u16(0x3FFF_u16);
        lump.put_u16(0_u16);
        lump.put_u16(1_u16);
        lump.put_u16(0_u16);
        lump.put_u16(0_u16);
        let mut bytes = lump.into_inner();
        bytes.extend_from_slice(score);
        bytes
    }

    /// The complete type-0 file a given track body produces.
    fn smf(body: &[u8]) -> Vec<u8> {
        let mut head = Writer::new();
        head.put_bytes(&SMF_HEADER_ID);
        head.put_u32_be(SMF_HEADER_BODY_LEN);
        head.put_u16_be(0_u16); // type 0
        head.put_u16_be(1_u16); // one track
        head.put_u16_be(MUS_TICKS_PER_QUARTER);
        head.put_bytes(&SMF_TRACK_ID);
        head.put_u32_be(u32::try_from(body.len()).expect("a hand-built track fits in a u32"));
        let mut expected = head.into_inner();
        expected.extend_from_slice(body);
        expected
    }

    /// A hand-built score converted all the way to `MIDI` bytes.
    fn converted(score: &[u8]) -> Vec<u8> {
        mus_to_midi(&mus_lump(score)).expect("a hand-built score converts")
    }

    /// The track body of a hand-built score.
    fn body(score: &[u8]) -> Vec<u8> {
        let midi = converted(score);
        midi[22..].to_vec()
    }

    /// The expected track body: tempo, `events`, end of track at `delta`.
    fn expected_body(events: &[u8], delta: u8) -> Vec<u8> {
        let mut expected: Vec<u8> = TEMPO.to_vec();
        expected.extend_from_slice(events);
        expected.extend_from_slice(&[delta]);
        expected.extend_from_slice(&END);
        expected
    }

    /// Asserts that a score stream fails to convert with a format error.
    fn assert_format_error(score: &[u8]) {
        let error = mus_to_midi(&mus_lump(score)).expect_err("a cut-off stream fails");
        assert_eq!(error.code(), "format_error", "code for {score:?}");
    }

    #[test]
    fn notes_and_their_delay_map_to_exact_bytes() {
        // Note on, note off marked last with a delay of 10, score end.
        let score = [0x10_u8, 0x3C, 0x80, 0x3C, 0x0A, 0x60];
        let events = [0x00_u8, 0x90, 0x3C, 0x7F, 0x00, 0x80, 0x3C, 0x00];
        let expected = expected_body(&events, 0x0A);
        assert_eq!(body(&score), expected, "the translated track body");
        assert_eq!(converted(&score), smf(&expected), "header and body");
    }

    #[test]
    fn volume_is_remembered_per_channel() {
        // A key byte with its high bit set carries a volume byte; the next
        // note on that channel plays at it, a new channel starts at 127.
        let score = [
            0x10_u8, 0xBC, 0x40, // note on, volume 64
            0x10, 0x3C, // note on again, remembered volume
            0x11, 0x3E, // note on a second channel, default volume
            0x60,
        ];
        let events = [
            // The volume byte belongs to this note...
            0x00_u8, 0x90, 0x3C, 0x40,
            // ...and is remembered for the next one on that channel.
            0x00, 0x90, 0x3C, 0x40,
            // A channel never used before plays at the default 127.
            0x00, 0x91, 0x3E, 0x7F,
        ];
        assert_eq!(
            body(&score),
            expected_body(&events, 0),
            "velocity 64 carries over, 127 defaults"
        );
    }

    #[test]
    fn percussion_channel_maps_to_midi_nine() {
        // MUS channel 15 is percussion and does not use the channel pool.
        let score = [
            0x1F_u8, 0x30, 0x0F, 0x30, // percussion note on and off
            0x10, 0x3C, 0x00, 0x3C, // a tonal note, still channel 0
            0x60,
        ];
        let events = [
            // MUS channel 15 is percussion: MIDI channel 9, twice.
            0x00_u8, 0x99, 0x30, 0x7F, 0x00, 0x89, 0x30, 0x00,
            // MUS channel 0 still takes the first pool entry.
            0x00, 0x90, 0x3C, 0x7F, 0x00, 0x80, 0x3C, 0x00,
        ];
        assert_eq!(
            body(&score),
            expected_body(&events, 0),
            "drums on 9, everything else in order"
        );
    }

    #[test]
    fn channels_take_pool_numbers_in_order_skipping_nine() {
        let mut score: Vec<u8> = Vec::new();
        let mut events: Vec<u8> = Vec::new();
        for channel in 0_u8..10_u8 {
            score.extend_from_slice(&[0x10_u8 | channel, 0x40 | channel]);
            let mapped = *MIDI_CHANNEL_POOL
                .get(usize::from(channel))
                .expect("ten channels fit the pool");
            events.extend_from_slice(&[0x00_u8, 0x90 | mapped, 0x40 | channel, 0x7F]);
        }
        score.push(0x60);
        assert_eq!(
            body(&score),
            expected_body(&events, 0),
            "the tenth channel lands on MIDI channel 10, past 9"
        );
    }

    #[test]
    fn pitch_bend_is_a_fourteen_bit_pair() {
        // Bottom, centre 128, and the top of the MUS range.
        let score = [0x20_u8, 0x00, 0x20, 0x80, 0x20, 0xFF, 0x60];
        let events = [
            // 0: the bottom of the bend.
            0x00_u8, 0xE0, 0x00, 0x00,
            // 128: shifted up six bits, the 14-bit centre 0x2000.
            0x00, 0xE0, 0x00, 0x40, // 255: the top, split into its two 7-bit digits.
            0x00, 0xE0, 0x40, 0x7F,
        ];
        assert_eq!(
            body(&score),
            expected_body(&events, 0),
            "a centre bend lands at 0x2000"
        );
    }

    #[test]
    fn controllers_map_and_clamp() {
        let mapping = [
            (1_u8, 0_u8),
            (2, 1),
            (3, 7),
            (4, 10),
            (5, 11),
            (6, 91),
            (7, 93),
            (8, 64),
            (9, 67),
        ];
        let mut score: Vec<u8> = Vec::new();
        let mut events: Vec<u8> = Vec::new();
        for (number, control) in mapping {
            score.extend_from_slice(&[0x40_u8, number, 0x20]);
            events.extend_from_slice(&[0x00_u8, 0xB0, control, 0x20]);
        }
        // A value above 127 clamps; an unmapped number writes nothing and
        // leaves a queued delay for the event that follows it.
        score.extend_from_slice(&[0x40_u8, 0x01, 0xC8]);
        events.extend_from_slice(&[0x00_u8, 0xB0, 0x00, 0x7F]);
        score.extend_from_slice(&[0xF0_u8, 0x05, 0x40, 0x0A, 0x20]);
        score.extend_from_slice(&[0x10_u8, 0x3C]);
        events.extend_from_slice(&[0x05_u8, 0x90, 0x3C, 0x7F]);
        score.push(0x60);
        assert_eq!(
            body(&score),
            expected_body(&events, 0),
            "bank, mod, volume, pan and friends"
        );
    }

    #[test]
    fn program_change_comes_from_controller_zero() {
        let score = [0x40_u8, 0x00, 0x05, 0x60];
        let events = [0x00_u8, 0xC0, 0x05];
        assert_eq!(body(&score), expected_body(&events, 0), "patch 5");
    }

    #[test]
    fn system_events_are_valueless_controllers() {
        let mapping = [(10_u8, 120_u8), (11, 123), (12, 126), (13, 127), (14, 121)];
        let mut score: Vec<u8> = Vec::new();
        let mut events: Vec<u8> = Vec::new();
        for (number, control) in mapping {
            score.extend_from_slice(&[0x30_u8, number]);
            events.extend_from_slice(&[0x00_u8, 0xB0, control, 0x00]);
        }
        // An unmapped system number names no controller to write.
        score.extend_from_slice(&[0x30_u8, 0x05]);
        score.push(0x60);
        assert_eq!(
            body(&score),
            expected_body(&events, 0),
            "all sounds off through reset all controllers"
        );
    }

    #[test]
    fn unused_and_measure_end_events_emit_nothing() {
        // A delay queued by an event that writes nothing still moves the
        // next event that does, and consecutive delays add up.
        let score = [0x70_u8, 0x50, 0xF0, 0x05, 0xF0, 0x03, 0x10, 0x3C, 0x60];
        let events = [0x08_u8, 0x90, 0x3C, 0x7F];
        assert_eq!(body(&score), expected_body(&events, 0), "5 then 3 is 8");
    }

    #[test]
    fn score_end_stops_even_with_bytes_left() {
        let score = [0x60_u8, 0x10, 0x3C, 0x10, 0x40, 0x99, 0x99];
        assert_eq!(
            body(&score),
            expected_body(&[], 0),
            "nothing after the score end"
        );
    }

    #[test]
    fn an_empty_score_is_tempo_and_end_of_track() {
        let score: [u8; 0] = [];
        assert_eq!(
            body(&score),
            expected_body(&[], 0),
            "a zero-length event stream"
        );
    }

    #[test]
    fn bad_magic_and_short_headers_are_format_errors() {
        let cases: [&[u8]; 3] = [&[], b"OPL\x1A", b"MUS\x1A\0\0"];
        for bytes in cases {
            let error = mus_to_midi(bytes).expect_err("no score to read");
            assert_eq!(error.code(), "format_error", "code for {bytes:?}");
        }
    }

    #[test]
    fn a_truncated_header_is_a_format_error() {
        let mut lump = Vec::new();
        lump.extend_from_slice(b"MUS\x1A");
        lump.resize(8, 0_u8);
        let error = mus_to_midi(&lump).expect_err("eight bytes hold no header");
        assert_eq!(error.code(), "format_error", "truncated header code");
        assert!(
            error.to_string().contains("header"),
            "message names the header: {error}"
        );
    }

    #[test]
    fn a_score_past_the_end_of_the_lump_is_a_format_error() {
        let mut lump = Vec::new();
        lump.extend_from_slice(b"MUS\x1A");
        // score_len 968 and score_start 16, little-endian.
        lump.extend_from_slice(&[0xC8_u8, 0x03, 0x10, 0x00]);
        lump.resize(20, 0_u8);
        let error = mus_to_midi(&lump).expect_err("968 bytes at 16 are not there");
        assert_eq!(error.code(), "format_error", "oversized score code");
        assert!(
            error.to_string().contains("past the"),
            "message says what to do: {error}"
        );
    }

    #[test]
    fn truncated_events_are_format_errors() {
        // A key event with no key byte, a controller with no value, a marked
        // last event with no delay.
        assert_format_error(&[0x10_u8]);
        assert_format_error(&[0x40, 0x01]);
        assert_format_error(&[0x80, 0x3C]);
        assert_format_error(&[0x20]);
        assert_format_error(&[0x30]);
    }

    #[test]
    fn a_five_byte_delay_is_a_format_error() {
        let error = mus_to_midi(&mus_lump(&[0xF0_u8, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF]))
            .expect_err("five continuation bytes");
        assert_eq!(error.code(), "format_error", "overlong delay code");
        assert!(
            error.to_string().contains("more than 4 bytes"),
            "message names the limit: {error}"
        );
    }

    #[test]
    fn every_tonal_channel_gets_its_own_midi_channel() {
        // All fifteen non-percussion channels plus percussion: the pool holds
        // exactly fifteen entries and MIDI 9 stays reserved for drums.
        let mut score: Vec<u8> = Vec::new();
        for channel in 0_u8..16_u8 {
            score.extend_from_slice(&[channel, 0x3C]);
        }
        score.push(0x60);
        let mut events: Vec<u8> = Vec::new();
        for index in 0_u8..15_u8 {
            let mapped = *MIDI_CHANNEL_POOL
                .get(usize::from(index))
                .expect("fifteen channels fit the pool");
            events.extend_from_slice(&[0x00_u8, 0x80 | mapped, 0x3C, 0x00]);
        }
        events.extend_from_slice(&[0x00_u8, 0x89, 0x3C, 0x00]);
        assert_eq!(
            body(&score),
            expected_body(&events, 0),
            "MUS 0..14 take the pool in order, MUS 15 takes MIDI 9"
        );
    }
}
