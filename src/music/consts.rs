//! The `MUS` and `MIDI` vocabulary both converters read from.
//!
//! The `MUS` lump layout, the packed event stream's bit fields and type
//! codes, the channel and controller tables, the tempo the format runs at and
//! the chunk and status bytes of a `Standard MIDI File` are each spelled once
//! here: [`crate::music::mus2mid`] decodes these values and
//! [`crate::music::mid2mus`] encodes them, so neither keeps a literal of its
//! own. The controller tables are the one pair of functions here: they answer
//! in both directions, `midi_control` mapping a `MUS` controller number to
//! the `MIDI` `CC` it selects and `mus_control` searching the same table
//! backwards for the writer.

// ---------------------------------------------------------------------------
// The MUS lump layout
// ---------------------------------------------------------------------------

/// The magic every `MUS` lump starts with: `MUS` and `0x1A`.
pub(crate) const MUS_MAGIC: [u8; 4] = *b"MUS\x1A";
/// Bytes of the fixed `MUS` header, ahead of the instrument table.
pub(crate) const MUS_HEADER_LEN: usize = 16;
/// Offset of the score length field, in bytes.
pub(crate) const MUS_SCORE_LEN_OFFSET: usize = 4;
/// Offset of the score start field, in bytes.
pub(crate) const MUS_SCORE_START_OFFSET: usize = 6;
/// Header bytes each instrument table entry costs.
pub(crate) const MUS_INSTRUMENT_BYTES: usize = 2;
/// The largest value the header's score length field counts.
pub(crate) const MUS_MAX_SCORE_LEN: usize = 0xFFFF_usize;

/// The layout's own shape, settled at compile time: the score length word
/// comes first and both score words sit inside the fixed header.
const _: () = assert!(
    MUS_SCORE_LEN_OFFSET < MUS_SCORE_START_OFFSET && MUS_SCORE_START_OFFSET < MUS_HEADER_LEN,
    "the MUS score words must sit, in order, inside the fixed header"
);

// ---------------------------------------------------------------------------
// The packed event stream
// ---------------------------------------------------------------------------

/// Set on the last event of a group; a delay in ticks follows its data.
pub(crate) const LAST_EVENT_FLAG: u8 = 0x80_u8;
/// The event type bits of a descriptor byte.
pub(crate) const EVENT_TYPE_MASK: u8 = 0x70_u8;
/// How far the type bits sit above the channel bits.
pub(crate) const EVENT_TYPE_SHIFT: u32 = 4_u32;
/// The channel number bits, the low nibble of both a `MUS` descriptor byte
/// and a `MIDI` channel status byte.
pub(crate) const CHANNEL_MASK: u8 = 0x0F_u8;
/// Set on a key byte when a volume byte follows it.
pub(crate) const VOLUME_FLAG: u8 = 0x80_u8;
/// The largest key, controller, patch or volume value one `MUS` byte carries;
/// its bits double as the mask that reads a key back.
pub(crate) const MAX_VALUE: u8 = 0x7F_u8;
/// Value of the `MUS` system events, which carry none of their own, and the
/// velocity a note off is written at.
pub(crate) const ZERO_VALUE: u8 = 0_u8;
/// Velocity a channel plays at until a note gives it one.
pub(crate) const DEFAULT_VOLUME: u8 = 127_u8;
/// Bits a `MUS` pitch value is shifted up to become a 14-bit `MIDI` bend, and
/// the same shift back down when a bend is written.
pub(crate) const BEND_SHIFT: u32 = 6_u32;
/// The `MUS` controller number that means "change program".
pub(crate) const PROGRAM_CONTROLLER: u8 = 0_u8;

/// `MUS` event type 0: stop a note.
pub(crate) const EVENT_RELEASE: u8 = 0_u8;
/// `MUS` event type 1: start a note, with an optional volume byte.
pub(crate) const EVENT_NOTE: u8 = 1_u8;
/// `MUS` event type 2: pitch bend, 128 at centre.
pub(crate) const EVENT_BEND: u8 = 2_u8;
/// `MUS` event type 3: a system controller, which carries no value.
pub(crate) const EVENT_SYSTEM: u8 = 3_u8;
/// `MUS` event type 4: a controller value, or a program change at number 0.
pub(crate) const EVENT_CONTROL: u8 = 4_u8;
/// `MUS` event type 5: end of measure, no data and no output.
pub(crate) const EVENT_MEASURE_END: u8 = 5_u8;
/// `MUS` event type 6: end of score.
pub(crate) const EVENT_SCORE_END: u8 = 6_u8;

// ---------------------------------------------------------------------------
// Channels
// ---------------------------------------------------------------------------

/// Number of `MUS` channels.
pub(crate) const CHANNEL_COUNT: usize = 16;
/// The `MUS` channel that carries percussion.
pub(crate) const MUS_PERCUSSION_CHANNEL: u8 = 15_u8;
/// The `MUS` channels melodic parts may take; 15 is percussion.
pub(crate) const MELODIC_CHANNELS: u8 = MUS_PERCUSSION_CHANNEL;
/// The `MIDI` channel percussion plays on.
pub(crate) const MIDI_PERCUSSION_CHANNEL: u8 = 9_u8;
/// The `MIDI` channels handed to non-percussion `MUS` channels, in order of
/// first use; 9 is reserved for percussion, so the pool skips it.
pub(crate) const MIDI_CHANNEL_POOL: [u8; 15] =
    [0_u8, 1, 2, 3, 4, 5, 6, 7, 8, 10, 11, 12, 13, 14, 15];
/// The last pool entry, kept for a score that needs a sixteenth channel.
pub(crate) const MIDI_CHANNEL_LAST: u8 = 15_u8;

// ---------------------------------------------------------------------------
// The controller tables
// ---------------------------------------------------------------------------

/// The first `MUS` controller number that maps to a `MIDI` `CC`; 0 is a
/// program change.
pub(crate) const FIRST_MUS_CONTROL: u8 = 1_u8;
/// The last `MUS` controller number.
pub(crate) const LAST_MUS_CONTROL: u8 = 9_u8;
/// The first `MUS` system event number.
pub(crate) const FIRST_MUS_SYSTEM: u8 = 10_u8;
/// The last `MUS` system event number.
pub(crate) const LAST_MUS_SYSTEM: u8 = 14_u8;

/// The `MIDI` controller a `MUS` controller event selects, or `None` for a
/// number with no counterpart. Number 0 is a program change, not a
/// controller, and is handled by the caller.
pub(crate) const fn midi_control(number: u8) -> Option<u8> {
    match number {
        1_u8 => Some(0_u8),
        2_u8 => Some(1_u8),
        3_u8 => Some(7_u8),
        4_u8 => Some(10_u8),
        5_u8 => Some(11_u8),
        6_u8 => Some(91_u8),
        7_u8 => Some(93_u8),
        8_u8 => Some(64_u8),
        9_u8 => Some(67_u8),
        _ => None,
    }
}

/// The `MIDI` controller a `MUS` system event selects, by `MUS` number.
pub(crate) const fn system_control(number: u8) -> Option<u8> {
    match number {
        10_u8 => Some(120_u8),
        11_u8 => Some(123_u8),
        12_u8 => Some(126_u8),
        13_u8 => Some(127_u8),
        14_u8 => Some(121_u8),
        _ => None,
    }
}

/// The `MUS` controller number that selects a mapped `MIDI` `CC`, the
/// backwards form of [`midi_control`].
pub(crate) fn mus_control(controller: u8) -> Option<u8> {
    (FIRST_MUS_CONTROL..=LAST_MUS_CONTROL).find(|number| midi_control(*number) == Some(controller))
}

/// The `MUS` system event number that selects a mapped `MIDI` `CC`, the
/// backwards form of [`system_control`].
pub(crate) fn mus_system(control: u8) -> Option<u8> {
    (FIRST_MUS_SYSTEM..=LAST_MUS_SYSTEM).find(|number| system_control(*number) == Some(control))
}

// ---------------------------------------------------------------------------
// Tempo
// ---------------------------------------------------------------------------

/// Ticks a `MUS` score runs at, per second.
pub(crate) const MUS_TICKS_PER_SECOND: u32 = 140_u32;
/// Ticks per quarter note of the files a `MUS` conversion writes: 70 at
/// [`MUS_TEMPO_MICROSECONDS`] per quarter is [`MUS_TICKS_PER_SECOND`] ticks
/// per second, so a `MUS` delay is already a `MIDI` delta time.
pub(crate) const MUS_TICKS_PER_QUARTER: u16 = 70_u16;
/// 500 000 microseconds per quarter note: the tempo a `MUS` score runs at and
/// the default a `MIDI` file without a tempo meta event falls back to.
pub(crate) const MUS_TEMPO_MICROSECONDS: u32 = 500_000_u32;
/// Microseconds in a second.
pub(crate) const MICROSECONDS_PER_SECOND: u128 = 1_000_000_u128;
/// Half of [`MICROSECONDS_PER_SECOND`], the round-up offset of a rounded
/// quotient.
pub(crate) const MICROSECOND_HALF: u128 = 500_000_u128;

// ---------------------------------------------------------------------------
// Standard MIDI Files
// ---------------------------------------------------------------------------

/// Chunk id of the `SMF` header chunk.
pub(crate) const SMF_HEADER_ID: [u8; 4] = *b"MThd";
/// Chunk id of a track chunk.
pub(crate) const SMF_TRACK_ID: [u8; 4] = *b"MTrk";
/// Body length of the `SMF` header chunk: format, track count, division.
pub(crate) const SMF_HEADER_BODY_LEN: u32 = 6_u32;

/// Status byte of a `MIDI` note off event.
pub(crate) const MIDI_NOTE_OFF: u8 = 0x80_u8;
/// Status byte of a `MIDI` note on event.
pub(crate) const MIDI_NOTE_ON: u8 = 0x90_u8;
/// Status byte of a polyphonic key pressure event.
pub(crate) const MIDI_KEY_PRESSURE: u8 = 0xA0_u8;
/// Status byte of a control change event.
pub(crate) const MIDI_CONTROL_CHANGE: u8 = 0xB0_u8;
/// Status byte of a program change event: one data byte.
pub(crate) const MIDI_PROGRAM_CHANGE: u8 = 0xC0_u8;
/// Status byte of a channel pressure event: one data byte.
pub(crate) const MIDI_CHANNEL_PRESSURE: u8 = 0xD0_u8;
/// Status byte of a pitch bend event.
pub(crate) const MIDI_PITCH_BEND: u8 = 0xE0_u8;
/// Status byte that introduces a meta event.
pub(crate) const MIDI_META_STATUS: u8 = 0xFF_u8;
/// Meta type of a tempo setting.
pub(crate) const MIDI_META_TEMPO: u8 = 0x51_u8;
/// Meta type of the end of track.
pub(crate) const MIDI_META_END_OF_TRACK: u8 = 0x2F_u8;
/// Payload length of a tempo meta event: the 24 bits of microseconds it
/// stores, as the byte a meta header counts.
pub(crate) const MIDI_TEMPO_DATA_LEN: u8 = 3_u8;
/// Shifts that pick those three bytes of a 24-bit big-endian tempo, top
/// first.
pub(crate) const TEMPO_SHIFTS: [u32; 3] = [16_u32, 8_u32, 0_u32];

#[cfg(test)]
mod tests {
    use super::{
        BEND_SHIFT, CHANNEL_COUNT, CHANNEL_MASK, DEFAULT_VOLUME, EVENT_BEND, EVENT_CONTROL,
        EVENT_MEASURE_END, EVENT_NOTE, EVENT_RELEASE, EVENT_SCORE_END, EVENT_SYSTEM,
        EVENT_TYPE_MASK, EVENT_TYPE_SHIFT, FIRST_MUS_CONTROL, FIRST_MUS_SYSTEM, LAST_MUS_CONTROL,
        LAST_MUS_SYSTEM, MAX_VALUE, MELODIC_CHANNELS, MICROSECOND_HALF, MICROSECONDS_PER_SECOND,
        MIDI_CHANNEL_LAST, MIDI_CHANNEL_POOL, MIDI_PERCUSSION_CHANNEL, MIDI_TEMPO_DATA_LEN,
        MUS_HEADER_LEN, MUS_INSTRUMENT_BYTES, MUS_MAGIC, MUS_MAX_SCORE_LEN, MUS_PERCUSSION_CHANNEL,
        MUS_SCORE_LEN_OFFSET, MUS_TEMPO_MICROSECONDS, MUS_TICKS_PER_QUARTER, MUS_TICKS_PER_SECOND,
        PROGRAM_CONTROLLER, TEMPO_SHIFTS, VOLUME_FLAG, midi_control, mus_control, mus_system,
        system_control,
    };

    /// Every `MUS` controller number, the range [`midi_control`] answers over.
    const CONTROLS: [u8; 9] = [1_u8, 2, 3, 4, 5, 6, 7, 8, 9];
    /// Every `MUS` system event number, the range [`system_control`] answers
    /// over.
    const SYSTEMS: [u8; 5] = [10_u8, 11, 12, 13, 14];

    #[test]
    fn the_lump_layout_measures_the_header_it_describes() {
        assert_eq!(MUS_MAGIC, *b"MUS\x1A", "the magic is MUS and 0x1A");
        assert_eq!(
            MUS_MAGIC.len(),
            MUS_SCORE_LEN_OFFSET,
            "a u16 follows the magic"
        );
        assert_eq!(MUS_INSTRUMENT_BYTES, 2, "each table entry is one u16");
        assert_eq!(
            MUS_INSTRUMENT_BYTES.checked_mul(8_usize).unwrap_or(0),
            MUS_HEADER_LEN,
            "the fixed header is eight u16 words"
        );
        assert_eq!(
            MUS_MAX_SCORE_LEN,
            usize::from(u16::MAX),
            "the score length field is a u16"
        );
    }

    #[test]
    fn the_descriptor_bits_split_one_byte_three_ways() {
        assert_eq!(
            u16::from(EVENT_TYPE_MASK) & u16::from(CHANNEL_MASK),
            0,
            "the type and channel fields do not overlap"
        );
        assert_eq!(
            EVENT_TYPE_MASK,
            0x07_u8.checked_shl(EVENT_TYPE_SHIFT).unwrap_or(0),
            "the mask covers exactly the seven type bits"
        );
        assert_eq!(
            u16::from(EVENT_TYPE_MASK) & u16::from(VOLUME_FLAG),
            0,
            "the last-event flag sits above the type bits"
        );
        assert_eq!(BEND_SHIFT, 6, "a MUS pitch is six bits short of a bend");
        assert_eq!(
            DEFAULT_VOLUME, MAX_VALUE,
            "the default velocity is a full data byte"
        );
    }

    #[test]
    fn the_event_type_codes_are_the_three_bits_of_a_descriptor() {
        let codes = [
            EVENT_RELEASE,
            EVENT_NOTE,
            EVENT_BEND,
            EVENT_SYSTEM,
            EVENT_CONTROL,
            EVENT_MEASURE_END,
            EVENT_SCORE_END,
        ];
        for (expected, code) in codes.iter().enumerate() {
            let number = u8::try_from(expected).unwrap_or(0);
            assert_eq!(*code, number, "event type {number}");
            let descriptor = number.checked_shl(EVENT_TYPE_SHIFT).unwrap_or(0);
            assert_eq!(
                (descriptor & EVENT_TYPE_MASK)
                    .checked_shr(EVENT_TYPE_SHIFT)
                    .unwrap_or(0),
                number,
                "type {number} is read back out of its descriptor"
            );
        }
    }

    #[test]
    fn the_channel_tables_agree_about_percussion_and_the_pool() {
        assert_eq!(
            usize::from(MUS_PERCUSSION_CHANNEL).checked_add(1),
            Some(CHANNEL_COUNT),
            "the percussion channel is the last of the sixteen"
        );
        assert_eq!(
            MELODIC_CHANNELS, MUS_PERCUSSION_CHANNEL,
            "every channel below the percussion one is melodic"
        );
        assert_eq!(
            MIDI_CHANNEL_POOL.len(),
            usize::from(MELODIC_CHANNELS),
            "the pool has one entry per melodic channel"
        );
        assert!(
            !MIDI_CHANNEL_POOL.contains(&MIDI_PERCUSSION_CHANNEL),
            "the pool keeps MIDI 9 for drums"
        );
        assert_eq!(
            MIDI_CHANNEL_POOL.last().copied(),
            Some(MIDI_CHANNEL_LAST),
            "the overflow channel is the pool's last entry"
        );
    }

    #[test]
    fn the_controller_tables_are_inverses_within_their_ranges() {
        for number in CONTROLS {
            let controller = midi_control(number).expect("every mapped control number answers");
            assert_eq!(
                mus_control(controller),
                Some(number),
                "CC {controller} comes back from MUS control {number}"
            );
        }
        for number in SYSTEMS {
            let control = system_control(number).expect("every system number maps to a CC");
            assert_eq!(
                mus_system(control),
                Some(number),
                "CC {control} comes back from MUS system {number}"
            );
        }
    }

    #[test]
    fn unmapped_numbers_and_controllers_answer_none() {
        assert_eq!(
            midi_control(PROGRAM_CONTROLLER),
            None,
            "number 0 is a program change, not a controller"
        );
        assert_eq!(
            mus_control(PROGRAM_CONTROLLER),
            Some(FIRST_MUS_CONTROL),
            "CC 0 is the bank control"
        );
        for number in [15_u8, 99, 145] {
            assert_eq!(midi_control(number), None, "{number} maps nothing");
            assert_eq!(system_control(number), None, "{number} maps nothing");
        }
        assert_eq!(
            mus_control(65),
            None,
            "an unmapped CC has no control number"
        );
        assert_eq!(mus_system(7), None, "an unmapped CC has no system number");
        assert_eq!(
            mus_control(120),
            None,
            "a system controller is not searched among the controls"
        );
        assert_eq!(
            mus_system(7),
            None,
            "a volume change is not searched among the system events"
        );
    }

    #[test]
    fn the_ranges_hold_the_mapped_numbers() {
        for number in CONTROLS {
            assert!(
                (FIRST_MUS_CONTROL..=LAST_MUS_CONTROL).contains(&number),
                "{number} is inside the control range"
            );
        }
        for number in SYSTEMS {
            assert!(
                (FIRST_MUS_SYSTEM..=LAST_MUS_SYSTEM).contains(&number),
                "{number} is inside the system range"
            );
        }
    }

    #[test]
    fn the_written_tempo_and_division_reproduce_the_tick_rate() {
        // The pair a converted score is written with has to run at the rate
        // MUS scores count: 1 000 000 microseconds over the quarter note's
        // length in microseconds, times the ticks per quarter.
        let ticks = MICROSECONDS_PER_SECOND
            .saturating_mul(u128::from(MUS_TICKS_PER_QUARTER))
            .checked_div(u128::from(MUS_TEMPO_MICROSECONDS))
            .expect("a nonzero tempo");
        assert_eq!(
            ticks,
            u128::from(MUS_TICKS_PER_SECOND),
            "70 ticks at 500000us is 140 ticks per second"
        );
        assert_eq!(
            MICROSECOND_HALF.checked_mul(2_u128),
            Some(MICROSECONDS_PER_SECOND),
            "the rounding offset is half a second"
        );
    }

    #[test]
    fn the_tempo_field_is_a_big_endian_twenty_four_bit_number() {
        assert_eq!(
            usize::from(MIDI_TEMPO_DATA_LEN),
            TEMPO_SHIFTS.len(),
            "the length byte counts exactly the shifts"
        );
        assert_eq!(TEMPO_SHIFTS, [16_u32, 8_u32, 0_u32], "top byte first");
        let bytes = TEMPO_SHIFTS.map(|shift| {
            u8::try_from(500_000_u32.checked_shr(shift).unwrap_or(0) & 0xFF_u32).unwrap_or(0)
        });
        assert_eq!(bytes, [0x07_u8, 0xA1, 0x20], "500000 microseconds");
    }
}
