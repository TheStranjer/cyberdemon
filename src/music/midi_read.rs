//! `Standard MIDI File` parsing: the reader that [`MidiFile`] describes.
//!
//! [`parse_midi`] walks a type-0 or type-1 `SMF` and merges every `MTrk` track
//! into one list of [`TimedEvent`]s at absolute ticks, honouring running
//! status. `SysEx` and any meta event other than tempo and end of track are
//! consumed and dropped; [`crate::music::midi_write::read_vlq`] reads the
//! variable-length quantities and the shared `consts` module names every
//! chunk id, status byte and meta type this walks over. `SMF` fields are
//! big-endian, so they come off [`crate::wad::bytes::Reader`]'s `_be`
//! methods. A non-zero `MidiFile::division` is required, so the tick to time
//! conversion downstream never divides by zero.

use crate::error::CyberError;
use crate::music::consts::{
    CHANNEL_MASK, MIDI_CHANNEL_PRESSURE, MIDI_CONTROL_CHANGE, MIDI_KEY_PRESSURE,
    MIDI_META_END_OF_TRACK, MIDI_META_STATUS, MIDI_META_TEMPO, MIDI_NOTE_OFF, MIDI_NOTE_ON,
    MIDI_PROGRAM_CHANGE, SMF_HEADER_BODY_LEN, SMF_HEADER_ID, SMF_TRACK_ID, TEMPO_SHIFTS,
};
use crate::music::midi_write::read_vlq;
use crate::wad::bytes::{Reader, u32_to_usize};

/// Format code of a type-1 file: one conductor track and one per part.
const FORMAT_TYPE1: u16 = 1_u16;
/// Set on a division whose high bit picks the `SMPTE` frame encoding.
const SMPTE_FLAG: u16 = 0x8000_u16;
/// High bit that introduces any `MIDI` status byte.
const STATUS_BIT: u8 = 0x80_u8;
/// Status bytes from here on are system messages, not channel voices.
const SYSTEM_STATUS: u8 = 0xF0_u8;
/// The message kind bits of a channel status byte.
const KIND_MASK: u8 = 0xF0_u8;
/// The low seven bits of a data byte.
const SEVEN_BITS: u8 = 0x7F_u8;
/// Shift of the high digit of a 14-bit pitch bend value.
const BEND_HIGH_SHIFT: u32 = 7_u32;
/// Bytes a tempo payload holds, counted from the shared shifts that pick
/// them out.
const TEMPO_DATA_LEN: usize = TEMPO_SHIFTS.len();

/// One `SMF` event with its absolute position in the merged file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TimedEvent {
    /// Ticks since the start of the file, in the header's division.
    pub tick: u32,
    /// The event itself.
    pub event: MidiEvent,
}

/// A parsed channel or kept meta event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MidiEvent {
    /// `0x90`: start `note` on `channel`; velocity 0 means note off.
    NoteOn {
        /// The `MIDI` channel, 0 through 15.
        channel: u8,
        /// The key, 0 through 127.
        note: u8,
        /// How hard the key was struck, 0 through 127.
        velocity: u8,
    },
    /// `0x80`: stop `note` on `channel`.
    NoteOff {
        /// The `MIDI` channel, 0 through 15.
        channel: u8,
        /// The key, 0 through 127.
        note: u8,
    },
    /// `0xA0`: polyphonic key pressure, kept so it can be reported dropped.
    KeyPressure {
        /// The `MIDI` channel, 0 through 15.
        channel: u8,
        /// The key the pressure applies to.
        key: u8,
        /// The pressure value.
        pressure: u8,
    },
    /// `0xB0`: change `number` to `value` on `channel`.
    ControlChange {
        /// The `MIDI` channel, 0 through 15.
        channel: u8,
        /// The controller number.
        number: u8,
        /// The controller value.
        value: u8,
    },
    /// `0xC0`: select `program` on `channel`.
    ProgramChange {
        /// The `MIDI` channel, 0 through 15.
        channel: u8,
        /// The patch number.
        program: u8,
    },
    /// `0xD0`: channel pressure, kept so it can be reported dropped.
    ChannelPressure {
        /// The `MIDI` channel, 0 through 15.
        channel: u8,
        /// The pressure value.
        pressure: u8,
    },
    /// `0xE0`: bend `channel` to the 14-bit `value`, centre 8192.
    PitchBend {
        /// The `MIDI` channel, 0 through 15.
        channel: u8,
        /// The 14-bit bend value, least significant digit first on the wire.
        value: u16,
    },
    /// `FF 51`: set the tempo to `microseconds` per quarter note.
    SetTempo {
        /// Microseconds per quarter note.
        microseconds: u32,
    },
    /// `FF 2F`: the end of the track this event was read from.
    EndOfTrack,
}

/// A parsed file: the division all tracks share and their merged events.
#[derive(Debug, Clone)]
pub struct MidiFile {
    /// Ticks per quarter note; always non-zero, never `SMPTE`.
    pub division: u16,
    /// Every event of every track, ordered by `tick`; ties keep the track
    /// order in which they were read, because the merge is a stable sort.
    pub events: Vec<TimedEvent>,
}

/// Reads a type-0 or type-1 `Standard MIDI File` and merges its tracks.
///
/// # Errors
///
/// [`CyberError::Format`] when the header is missing, short or cut off, a
/// chunk id is wrong, an event or quantity is truncated, or a tick overflow
/// is hit. [`CyberError::Unsupported`] for format 2 and for a `SMPTE`
/// division.
#[inline]
pub fn parse_midi(bytes: &[u8]) -> Result<MidiFile, CyberError> {
    let mut reader = Reader::new(bytes);
    let (track_count, division) = read_header(&mut reader)?;
    let mut events = Vec::new();
    read_tracks(&mut reader, track_count, &mut events)?;
    events.sort_by_key(|event| event.tick);
    Ok(MidiFile { division, events })
}

/// Checks the `MThd` chunk and returns the declared track count and division.
fn read_header(reader: &mut Reader<'_>) -> Result<(u16, u16), CyberError> {
    let id = reader.read_array::<4>()?;
    if id != SMF_HEADER_ID {
        return Err(CyberError::Format(
            "not a Standard MIDI File: the data does not start with 'MThd'; export the score as a .mid file".to_owned(),
        ));
    }
    let body_len = reader.read_u32_be()?;
    if body_len < SMF_HEADER_BODY_LEN {
        return Err(CyberError::Format(format!(
            "MIDI header body is {body_len} bytes, shorter than the {SMF_HEADER_BODY_LEN} bytes it needs"
        )));
    }
    let format = reader.read_u16_be()?;
    if format > FORMAT_TYPE1 {
        return Err(CyberError::Unsupported(format!(
            "MIDI format {format} is not supported; convert the file to format 0 or 1 first"
        )));
    }
    let track_count = reader.read_u16_be()?;
    let division = reader.read_u16_be()?;
    if division & SMPTE_FLAG != 0 {
        return Err(CyberError::Unsupported(
            "SMPTE time division is not supported; re-export the file with ticks per quarter note"
                .to_owned(),
        ));
    }
    if division == 0_u16 {
        return Err(CyberError::Format(
            "MIDI division is 0 ticks per quarter note; no time can be measured with it".to_owned(),
        ));
    }
    let extra = usize::try_from(body_len.saturating_sub(SMF_HEADER_BODY_LEN)).unwrap_or(usize::MAX);
    reader.read_bytes(extra)?;
    Ok((track_count, division))
}

/// Reads `track_count` `MTrk` chunks, appending each track's events.
fn read_tracks(
    reader: &mut Reader<'_>,
    track_count: u16,
    events: &mut Vec<TimedEvent>,
) -> Result<(), CyberError> {
    for _ in 0_u16..track_count {
        let body = read_track_chunk(reader)?;
        read_track(body, events)?;
    }
    Ok(())
}

/// Consumes one `MTrk` header and returns its declared body.
fn read_track_chunk<'reader>(reader: &mut Reader<'reader>) -> Result<&'reader [u8], CyberError> {
    let id = reader.read_array::<4>()?;
    if id != SMF_TRACK_ID {
        return Err(CyberError::Format(
            "expected an 'MTrk' chunk; the file is truncated or not a Standard MIDI File"
                .to_owned(),
        ));
    }
    let body_len = reader.read_u32_be()?;
    let count = u32_to_usize(body_len)?;
    reader.read_bytes(count)
}

/// Turns one track body into absolute-tick events, honouring running status.
fn read_track(body: &[u8], events: &mut Vec<TimedEvent>) -> Result<(), CyberError> {
    let mut reader = Reader::new(body);
    let mut tick = 0_u32;
    let mut running: Option<u8> = None;
    while reader.remaining() > 0 {
        let delta = read_vlq(&mut reader)?;
        let Some(at) = tick.checked_add(delta) else {
            return Err(CyberError::LimitExceeded(format!(
                "a track's delta times pass the largest tick a u32 counts at offset {}",
                reader.pos()
            )));
        };
        tick = at;
        let Some(event) = read_event(&mut reader, &mut running)? else {
            continue;
        };
        events.push(TimedEvent { tick, event });
        if matches!(event, MidiEvent::EndOfTrack) {
            break;
        }
    }
    Ok(())
}

/// Reads the event at the cursor; `None` when it was consumed and skipped.
fn read_event(
    reader: &mut Reader<'_>,
    running: &mut Option<u8>,
) -> Result<Option<MidiEvent>, CyberError> {
    let byte = reader.read_u8()?;
    if byte >= SYSTEM_STATUS {
        return system_event(reader, byte, running);
    }
    if byte >= STATUS_BIT {
        *running = Some(byte);
        return Ok(Some(channel_event(reader, byte, None)?));
    }
    match *running {
        Some(status) => Ok(Some(channel_event(reader, status, Some(byte))?)),
        None => Err(CyberError::Format(format!(
            "data byte 0x{byte:02X} at offset {} without a status or running status",
            reader.pos().saturating_sub(1)
        ))),
    }
}

/// Reads a channel message, reusing `status` with `first` as its data byte.
fn channel_event(
    reader: &mut Reader<'_>,
    status: u8,
    first: Option<u8>,
) -> Result<MidiEvent, CyberError> {
    let channel = status & CHANNEL_MASK;
    let data1 = match first {
        Some(byte) => byte,
        None => reader.read_u8()?,
    };
    match status & KIND_MASK {
        MIDI_NOTE_OFF => {
            reader.read_u8()?;
            Ok(MidiEvent::NoteOff {
                channel,
                note: data1 & SEVEN_BITS,
            })
        }
        MIDI_NOTE_ON => {
            let velocity = reader.read_u8()?;
            Ok(MidiEvent::NoteOn {
                channel,
                note: data1 & SEVEN_BITS,
                velocity: velocity & SEVEN_BITS,
            })
        }
        MIDI_KEY_PRESSURE => {
            let pressure = reader.read_u8()?;
            Ok(MidiEvent::KeyPressure {
                channel,
                key: data1 & SEVEN_BITS,
                pressure: pressure & SEVEN_BITS,
            })
        }
        MIDI_CONTROL_CHANGE => {
            let value = reader.read_u8()?;
            Ok(MidiEvent::ControlChange {
                channel,
                number: data1 & SEVEN_BITS,
                value: value & SEVEN_BITS,
            })
        }
        MIDI_PROGRAM_CHANGE => Ok(MidiEvent::ProgramChange {
            channel,
            program: data1 & SEVEN_BITS,
        }),
        MIDI_CHANNEL_PRESSURE => Ok(MidiEvent::ChannelPressure {
            channel,
            pressure: data1 & SEVEN_BITS,
        }),
        _ => {
            let data2 = reader.read_u8()?;
            let value = bend_value(data1, data2);
            Ok(MidiEvent::PitchBend { channel, value })
        }
    }
}

/// The 14-bit pitch bend from its two 7-bit digits, least significant first.
fn bend_value(data1: u8, data2: u8) -> u16 {
    let low = u16::from(data1 & SEVEN_BITS);
    let high = u16::from(data2 & SEVEN_BITS)
        .checked_shl(BEND_HIGH_SHIFT)
        .unwrap_or(0_u16);
    low.checked_add(high).unwrap_or(0_u16)
}

/// Reads a meta or `SysEx` event; system messages end the running status.
fn system_event(
    reader: &mut Reader<'_>,
    status: u8,
    running: &mut Option<u8>,
) -> Result<Option<MidiEvent>, CyberError> {
    *running = None;
    match status {
        MIDI_META_STATUS => meta_event(reader),
        0xF0_u8 | 0xF7_u8 => {
            let length = usize::try_from(read_vlq(reader)?).unwrap_or(usize::MAX);
            reader.read_bytes(length)?;
            Ok(None)
        }
        _ => Err(CyberError::Format(format!(
            "status byte 0x{status:02X} at offset {} cannot appear in a Standard MIDI File",
            reader.pos().saturating_sub(1)
        ))),
    }
}

/// Reads a meta event; only tempo and end of track are kept.
fn meta_event(reader: &mut Reader<'_>) -> Result<Option<MidiEvent>, CyberError> {
    let kind = reader.read_u8()?;
    let length = usize::try_from(read_vlq(reader)?).unwrap_or(usize::MAX);
    let data = reader.read_bytes(length)?;
    match kind {
        MIDI_META_TEMPO => {
            let bytes: [u8; TEMPO_DATA_LEN] = data.try_into().map_err(|error| {
                CyberError::Format(format!(
                    "tempo meta event carries {length} bytes, not the {TEMPO_DATA_LEN} it needs: {error}"
                ))
            })?;
            Ok(Some(MidiEvent::SetTempo {
                microseconds: tempo_microseconds(bytes),
            }))
        }
        MIDI_META_END_OF_TRACK => Ok(Some(MidiEvent::EndOfTrack)),
        _ => Ok(None),
    }
}

/// The 24-bit big-endian microseconds a tempo payload stores.
fn tempo_microseconds(bytes: [u8; TEMPO_DATA_LEN]) -> u32 {
    TEMPO_SHIFTS
        .iter()
        .zip(bytes.iter())
        .fold(0_u32, |value, (shift, byte)| {
            let placed = u32::from(*byte).checked_shl(*shift).unwrap_or(0_u32);
            value.checked_add(placed).unwrap_or(value)
        })
}

#[cfg(test)]
mod tests {
    use super::{MidiEvent, parse_midi};
    use crate::wad::bytes::Writer;

    /// A chunk: its id, big-endian body length, then the body.
    fn chunk(id: [u8; 4], body: &[u8]) -> Vec<u8> {
        let mut out = Writer::new();
        out.put_bytes(&id);
        out.put_u32_be(u32::try_from(body.len()).expect("hand-built chunks are short"));
        out.put_bytes(body);
        out.into_inner()
    }

    /// A complete file: a header for `format` and these track bodies.
    fn smf(format: u16, division: u16, tracks: &[&[u8]]) -> Vec<u8> {
        let mut header = Writer::new();
        header.put_u16_be(format);
        header.put_u16_be(u16::try_from(tracks.len()).expect("a few tracks"));
        header.put_u16_be(division);
        let mut out = chunk(*b"MThd", &header.into_inner());
        for track in tracks {
            out.extend_from_slice(&chunk(*b"MTrk", track));
        }
        out
    }

    /// An end-of-track meta event at `delta`, as raw bytes.
    fn end(delta: u8) -> Vec<u8> {
        vec![delta, 0xFF, 0x2F, 0x00]
    }

    /// Appends an end of track at delta 0 and parses the whole file.
    fn parsed(track_body: &[u8]) -> Vec<MidiEvent> {
        let mut body = track_body.to_vec();
        body.extend_from_slice(&end(0));
        let file = parse_midi(&smf(0, 70, &[&body])).expect("a hand-built file parses");
        assert_eq!(file.division, 70, "the division reads back");
        file.events.iter().map(|item| item.event).collect()
    }

    /// The parsed events of a format-1 file with these tracks.
    fn merged(tracks: &[&[u8]]) -> Vec<(u32, MidiEvent)> {
        let mut owned: Vec<Vec<u8>> = Vec::new();
        for track in tracks {
            let mut body = (*track).to_vec();
            body.extend_from_slice(&end(0));
            owned.push(body);
        }
        let refs: Vec<&[u8]> = owned.iter().map(Vec::as_slice).collect();
        let file = parse_midi(&smf(1, 70, &refs)).expect("a hand-built file parses");
        file.events
            .iter()
            .map(|item| (item.tick, item.event))
            .collect()
    }

    #[test]
    fn channel_events_keep_ticks_status_and_data() {
        use MidiEvent::{ControlChange, NoteOff, NoteOn, PitchBend, ProgramChange};
        let mut body: Vec<u8> = vec![
            0x00, 0x90, 0x3C, 0x7F, // note on at tick 0
            0x46, 0x80, 0x3C, 0x00, // note off 70 ticks later
            0x00, 0xC0, 0x05, // program change, one data byte
            0x00, 0xE0, 0x00, 0x40, // pitch bend, centre 8192
            0x00, 0xB0, 0x07, 0x64, // control change
        ];
        body.extend_from_slice(&end(0));
        let file = parse_midi(&smf(0, 70, &[&body])).expect("hand-built file");
        let ticks: Vec<u32> = file.events.iter().map(|item| item.tick).collect();
        assert_eq!(
            ticks,
            [0_u32, 70, 70, 70, 70, 70],
            "absolute ticks accumulate"
        );
        let kinds: Vec<MidiEvent> = file.events.iter().map(|item| item.event).collect();
        assert_eq!(
            kinds,
            [
                NoteOn {
                    channel: 0,
                    note: 0x3C,
                    velocity: 0x7F
                },
                NoteOff {
                    channel: 0,
                    note: 0x3C
                },
                ProgramChange {
                    channel: 0,
                    program: 5
                },
                PitchBend {
                    channel: 0,
                    value: 8192
                },
                ControlChange {
                    channel: 0,
                    number: 7,
                    value: 100
                },
                MidiEvent::EndOfTrack,
            ],
            "every message decodes"
        );
    }

    #[test]
    fn running_status_repeats_the_last_one() {
        use MidiEvent::{NoteOn, ProgramChange};
        let events = parsed(&[
            0x00, 0x90, 0x3C, 0x40, // explicit status
            0x00, 0x40, 0x50, // running status: note on again
            0x00, 0xC0, 0x05, // one-byte message
            0x00, 0x07, // running program change takes one byte
        ]);
        assert_eq!(
            events[..4],
            [
                NoteOn {
                    channel: 0,
                    note: 0x3C,
                    velocity: 0x40
                },
                NoteOn {
                    channel: 0,
                    note: 0x40,
                    velocity: 0x50
                },
                ProgramChange {
                    channel: 0,
                    program: 5
                },
                ProgramChange {
                    channel: 0,
                    program: 7
                },
            ],
            "a data byte reuses the running status; a one-byte message stops early"
        );
    }

    #[test]
    fn meta_and_sysex_are_kept_or_skipped() {
        let events = parsed(&[
            0x00, 0xFF, 0x51, 0x03, 0x07, 0xA1, 0x20, // tempo 500000
            0x00, 0xFF, 0x01, 0x02, b'h', b'i', // text meta: skipped
            0x00, 0xF0, 0x03, 0xF7, 0x7E, 0x00, // SysEx: skipped
            0x00, 0x90, 0x3C, 0x7F, // survives the skipped events
        ]);
        assert_eq!(
            events,
            [
                MidiEvent::SetTempo {
                    microseconds: 500_000
                },
                MidiEvent::NoteOn {
                    channel: 0,
                    note: 0x3C,
                    velocity: 0x7F
                },
                MidiEvent::EndOfTrack,
            ],
            "tempo is kept, text and SysEx vanish"
        );
    }

    #[test]
    fn tracks_merge_by_tick_with_ties_in_track_order() {
        use MidiEvent::{EndOfTrack, NoteOn};
        let events = merged(&[
            &[0x46, 0x91, 0x40, 0x7F], // track 1: note on at tick 70
            &[0x00, 0x90, 0x3C, 0x7F], // track 0: note on at tick 0
        ]);
        // Note: the events come out in tick order regardless of chunk order.
        let ticks: Vec<u32> = events.iter().map(|pair| pair.0).collect();
        assert_eq!(ticks, [0_u32, 0, 70, 70], "sorted by absolute tick");
        assert_eq!(
            events[0].1,
            NoteOn {
                channel: 0,
                note: 0x3C,
                velocity: 0x7F
            },
            "the second chunk still plays first"
        );
        let tie = merged(&[
            &[0x00, 0x90, 0x3C, 0x7F], // first track's event at tick 0
            &[0x00, 0x90, 0x40, 0x7F], // second track's event at tick 0
        ]);
        assert_eq!(
            tie[0].1,
            NoteOn {
                channel: 0,
                note: 0x3C,
                velocity: 0x7F
            },
            "ties keep track order (stable merge)"
        );
        assert_eq!(
            tie[2].1,
            NoteOn {
                channel: 0,
                note: 0x40,
                velocity: 0x7F
            },
            "the second track's tie follows the first's"
        );
        assert!(matches!(tie[1].1, EndOfTrack), "end markers survive");
    }

    #[test]
    fn format_two_and_smpte_are_unsupported() {
        let format_two = parse_midi(&smf(2, 70, &[])).expect_err("format 2 never converts");
        assert_eq!(format_two.code(), "unsupported", "format code");
        assert!(
            format_two.to_string().contains("format 2"),
            "names it: {format_two}"
        );
        let smpte = parse_midi(&smf(0, 0xE77F, &[])).expect_err("SMPTE never converts");
        assert_eq!(smpte.code(), "unsupported", "division code");
        assert!(smpte.to_string().contains("SMPTE"), "names it: {smpte}");
    }

    #[test]
    fn bad_header_fields_are_format_errors() {
        let not_midi = parse_midi(b"RIFFabcd").expect_err("no MThd");
        assert_eq!(not_midi.code(), "format_error", "magic code");
        let short = parse_midi(&chunk(*b"MThd", &[0, 0])).expect_err("a 2-byte body");
        assert_eq!(short.code(), "format_error", "body length code");
        let zero = parse_midi(&smf(0, 0, &[])).expect_err("zero division");
        assert_eq!(zero.code(), "format_error", "zero division code");
        assert!(zero.to_string().contains("division"), "names it: {zero}");
    }

    #[test]
    fn wrong_chunk_ids_and_truncation_are_format_errors() {
        let mut bytes = chunk(*b"MThd", &[0, 0, 0, 1, 0, 46]);
        bytes.extend_from_slice(&chunk(*b"Mtxt", &[0x00, 0xFF, 0x2F, 0x00]));
        let bad_id = parse_midi(&bytes).expect_err("no MTrk");
        assert_eq!(bad_id.code(), "format_error", "chunk id code");
        assert!(bad_id.to_string().contains("MTrk"), "names it: {bad_id}");
        // Declares one track, none follows.
        let missing = parse_midi(&chunk(*b"MThd", &[0, 0, 0, 1, 0, 46])).expect_err("no track");
        assert_eq!(missing.code(), "format_error", "missing track code");
        // A track whose event runs past its declared body.
        let cut = parse_midi(&smf(0, 70, &[&[0x00, 0x90, 0x3C]])).expect_err("two bytes short");
        assert_eq!(cut.code(), "format_error", "truncated event code");
        // A five-byte delta quantity.
        let long_vlq =
            parse_midi(&smf(0, 70, &[&[0xFF, 0xFF, 0xFF, 0xFF, 0xFF]])).expect_err("five bytes");
        assert_eq!(long_vlq.code(), "format_error", "overlong quantity code");
    }

    #[test]
    fn stray_statuses_and_a_short_tempo_are_format_errors() {
        // A data byte with no running status.
        let no_status = parse_midi(&smf(0, 70, &[&[0x3C, 0x00]]))
            .expect_err("a data byte cannot start an event");
        assert_eq!(no_status.code(), "format_error", "no-status code");
        // A real-time status byte, which SMF forbids.
        let realtime = parse_midi(&smf(0, 70, &[&[0x00, 0xF8]])).expect_err("no clock in a file");
        assert_eq!(realtime.code(), "format_error", "real-time code");
        // A tempo meta of the wrong width.
        let bad_tempo = parse_midi(&smf(
            0,
            70,
            &[&[0x00, 0xFF, 0x51, 0x02, 0x07, 0xA1, 0x00, 0xFF, 0x2F, 0x00]],
        ))
        .expect_err("two bytes hold no tempo");
        assert_eq!(bad_tempo.code(), "format_error", "tempo width code");
    }

    #[test]
    fn tick_overflow_past_u32_is_a_limit_error() {
        let mut body: Vec<u8> = Vec::new();
        for _ in 0_usize..17 {
            body.extend_from_slice(&[0xFF, 0xFF, 0xFF, 0x7F]); // delta +268435455
            body.extend_from_slice(&[0x90, 0x3C, 0x7F]); // one note on per delta
        }
        let overflow = parse_midi(&smf(0, 70, &[&body])).expect_err("past u32::MAX");
        assert_eq!(overflow.code(), "limit_exceeded", "tick overflow code");
    }

    #[test]
    fn pressures_are_kept_as_events() {
        use MidiEvent::{ChannelPressure, EndOfTrack, KeyPressure};
        let events = parsed(&[0x00, 0xA0, 0x3C, 0x40, 0x00, 0xD0, 0x50]);
        assert_eq!(
            events,
            [
                KeyPressure {
                    channel: 0,
                    key: 0x3C,
                    pressure: 0x40
                },
                ChannelPressure {
                    channel: 0,
                    pressure: 0x50
                },
                EndOfTrack,
            ],
            "both pressure kinds survive for the converter to drop"
        );
    }

    #[test]
    fn a_long_header_body_is_skipped_and_tracks_follow() {
        let mut header = Vec::new();
        header.extend_from_slice(&[0x00, 0x00]); // format 0
        header.extend_from_slice(&[0x00, 0x01]); // one track
        header.extend_from_slice(&[0x00, 0x46]); // division 70
        header.extend_from_slice(&[0xAA, 0xBB]); // two extra bytes to skip
        let mut bytes = chunk(*b"MThd", &header);
        bytes.extend_from_slice(&chunk(*b"MTrk", &[0x00, 0xFF, 0x2F, 0x00]));
        let file = parse_midi(&bytes).expect("an oversized header still parses");
        assert_eq!(file.division, 70, "fields keep their offsets");
        assert_eq!(file.events.len(), 1, "just the end of track");
    }
}
