//! `Standard MIDI File` data converted to `Doom` `MUS` music lumps.
//!
//! [`midi_to_mus`] parses the file with [`crate::music::midi_read`], merges
//! its tracks into one absolute-tick timeline, converts ticks to `MUS`
//! ticks piecewise across tempo changes (`MUS` runs at 140 ticks per second)
//! and encodes the result as a `MUS` header, instrument table and packed
//! event stream. The [`crate::music::mus2mid`] controller tables drive the
//! translation backwards, and the [`crate::music::midi_write`] `VLQ` helpers
//! write the delays.
//!
//! The mapping is deterministic, the mirror image of the `MUS` to `MIDI`
//! conversion: `MIDI` channel 9 becomes the percussion channel 15 and every
//! other channel takes the next free `MUS` channel in order of first use.
//! Choices the score cannot express are reported, not guessed: controllers,
//! key pressure and channel pressure that have no `MUS` event are dropped
//! and counted in [`MusConversion::warnings`].
//!
//! The packed event stream itself, with that channel mapping, the note
//! volumes and the group delays, is the private `score` module's `Score`;
//! this file keeps the timeline in front of it and the header, instrument
//! table and byte layout around it. The `tests` module holds the shared
//! helpers and the `convert` topic the conversion checks.

mod score;

use crate::error::CyberError;
use crate::music::midi_read::{MidiEvent, MidiFile, parse_midi};
use crate::music::mus2mid::{ZERO_VALUE, midi_control, system_control};
use crate::wad::bytes::Writer;
use score::Score;

/// Bytes of the fixed `MUS` header, ahead of the instrument table.
const HEADER_LEN: usize = 16;
/// The magic every `MUS` lump starts with.
const MAGIC: [u8; 4] = *b"MUS\x1A";
/// Header bytes each instrument table entry costs.
const INSTRUMENT_BYTES: usize = 2_usize;
/// The largest value the header's score length field counts.
const MAX_SCORE_LEN: usize = 0xFFFF_usize;

/// Ticks a `MUS` score runs at, per second.
const MUS_TICKS_PER_SECOND: u128 = 140_u128;
/// Microseconds in a second.
const MICROSECONDS: u128 = 1_000_000_u128;
/// Half of [`MICROSECONDS`], the round-up offset of a rounded quotient.
const MICROSECOND_HALF: u128 = 500_000_u128;
/// 500 000 microseconds per quarter note: the tempo a file omits.
const DEFAULT_TEMPO: u32 = 500_000_u32;
/// The `MUS` channels melodic parts may take; 15 is percussion.
const MELODIC_CHANNELS: u8 = 15_u8;
/// The first `MUS` controller number; 0 is a program change.
const FIRST_CONTROL: u8 = 1_u8;
/// The last `MUS` controller number.
const LAST_CONTROL: u8 = 9_u8;
/// The first `MUS` system event number.
const FIRST_SYSTEM: u8 = 10_u8;
/// The last `MUS` system event number.
const LAST_SYSTEM: u8 = 14_u8;

/// Set on the last event of a group; a delay in ticks follows its data.
const LAST_EVENT_FLAG: u8 = 0x80_u8;
/// How far the type bits sit above the channel bits.
const TYPE_SHIFT: u32 = 4_u32;
/// The channel bits of a descriptor byte.
const CHANNEL_MASK: u8 = 0x0F_u8;
/// Set on a key byte when a volume byte follows it.
const VOLUME_FLAG: u8 = 0x80_u8;
/// The largest key, controller value or patch a `MUS` byte carries.
const MAX_VALUE: u8 = 0x7F_u8;
/// Shift a 14-bit `MIDI` bend is reduced by to become a `MUS` value.
const BEND_REDUCTION: u32 = 6_u32;
/// The `MUS` controller number that means "change program".
const PROGRAM_CONTROLLER: u8 = 0_u8;
/// Number of distinct key, controller and patch values one byte counts.
const VALUE_COUNT: usize = 256_usize;
/// Offset a percussion key becomes its `MUS` instrument number at.
const PERCUSSION_INSTRUMENT_BASE: usize = 100_usize;

/// `MUS` event type 0: stop a note.
const KIND_RELEASE: u8 = 0_u8;
/// `MUS` event type 1: start a note.
const KIND_NOTE: u8 = 1_u8;
/// `MUS` event type 2: pitch bend.
const KIND_BEND: u8 = 2_u8;
/// `MUS` event type 3: a system controller.
const KIND_SYSTEM: u8 = 3_u8;
/// `MUS` event type 4: a controller value, or a program change at 0.
const KIND_CONTROL: u8 = 4_u8;
/// `MUS` event type 6: end of score.
const KIND_SCORE_END: u8 = 6_u8;

/// A `MUS` score and the count of what its stream could not carry.
#[derive(Debug, Clone)]
pub struct MusConversion {
    /// The complete `MUS` lump: header, instrument table and score.
    pub mus: Vec<u8>,
    /// One line per kind of `MIDI` event the score dropped, with its count.
    pub warnings: Vec<String>,
}

/// Converts the bytes of a `Standard MIDI File` into a `Doom` `MUS` lump.
///
/// The score is one event stream at 140 ticks per second: every event the
/// mapping can express, each tick group closed by a delay to the next, and
/// the score-end marker. A tempo meta event rebases the tick conversion
/// from its own tick on; the default is 500 000 microseconds per quarter.
///
/// # Errors
///
/// [`CyberError::Format`] and [`CyberError::Unsupported`] pass through from
/// [`crate::music::midi_read::parse_midi`].
/// [`CyberError::LimitExceeded`] when the score is longer than 65535 bytes,
/// a delay will not fit one `VLQ`, or more than fifteen melodic channels
/// are used.
#[inline]
pub fn midi_to_mus(bytes: &[u8]) -> Result<MusConversion, CyberError> {
    let file = parse_midi(bytes)?;
    let placed = place_events(&file);
    let mut score = Score::new();
    for event in placed.events {
        score.event(&event)?;
    }
    score.finish(placed.end_tick)?;
    let instruments = score.instrument_table();
    Ok(MusConversion {
        mus: assemble(&score.bytes, &instruments, score.primary())?,
        warnings: score.warnings(),
    })
}

/// The fixed header, the ascending instrument table and the score bytes.
fn assemble(score: &[u8], instruments: &[u8], primary: u8) -> Result<Vec<u8>, CyberError> {
    let score_len = score.len();
    if score_len > MAX_SCORE_LEN {
        return Err(CyberError::LimitExceeded(format!(
            "the score is {score_len} bytes, longer than the {MAX_SCORE_LEN} a MUS header counts; use fewer events"
        )));
    }
    let table_len = instruments
        .len()
        .checked_mul(INSTRUMENT_BYTES)
        .unwrap_or(0_usize);
    let start = HEADER_LEN.checked_add(table_len).unwrap_or(HEADER_LEN);
    let mut out = Writer::new();
    out.put_bytes(&MAGIC);
    out.put_u16(usize_to_u16(score_len).unwrap_or(0_u16));
    out.put_u16(usize_to_u16(start).unwrap_or(0_u16));
    out.put_u16(u16::from(primary));
    out.put_u16(0_u16);
    out.put_u16(usize_to_u16(instruments.len()).unwrap_or(u16::MAX));
    out.put_u16(0_u16);
    for patch in instruments {
        out.put_u16(u16::from(*patch));
    }
    out.put_bytes(score);
    Ok(out.into_inner())
}

/// The `u16` form of a count, `None` when it grew past one.
fn usize_to_u16(value: usize) -> Option<u16> {
    u16::try_from(value).ok()
}

/// Where the placed timeline ends: the last end-of-track marker's tick.
struct PlacedTimeline {
    /// Playable events at their `MUS` ticks, in stream order.
    events: Vec<Placed>,
    /// Tick of the last end-of-track marker, if the file had one.
    end_tick: Option<u64>,
}

/// One output event at its position in score ticks.
struct Placed {
    /// Position in the score, in `MUS` ticks.
    tick: u64,
    /// The `MIDI` event to translate.
    event: MidiEvent,
}

/// Walks the merged timeline, turning MIDI ticks into `MUS` ticks.
fn place_events(file: &MidiFile) -> PlacedTimeline {
    let division = u128::from(file.division);
    let mut segment_tick = 0_u128;
    let mut segment_us = 0_u128;
    let mut tempo = u128::from(DEFAULT_TEMPO);
    let mut events: Vec<Placed> = Vec::new();
    let mut end_tick: Option<u64> = None;
    for item in &file.events {
        let at = u128::from(item.tick);
        let elapsed = at.saturating_sub(segment_tick);
        let us = segment_us.saturating_add(microseconds(elapsed, tempo, division));
        if let MidiEvent::SetTempo { microseconds } = item.event {
            tempo = u128::from(microseconds);
            segment_tick = at;
            segment_us = us;
            continue;
        }
        let tick = mus_tick(us);
        if matches!(item.event, MidiEvent::EndOfTrack) {
            end_tick = Some(tick);
        } else {
            events.push(Placed {
                tick,
                event: item.event,
            });
        }
    }
    PlacedTimeline { events, end_tick }
}

/// Microseconds `ticks` long at `tempo` in a `division`-step file.
fn microseconds(ticks: u128, tempo: u128, division: u128) -> u128 {
    let half = division.checked_shr(1).unwrap_or(0_u128);
    let numerator = ticks.saturating_mul(tempo).saturating_add(half);
    numerator.checked_div(division).unwrap_or(u128::MAX)
}

/// Rounded `MUS` ticks for `us` microseconds, saturating at `u64::MAX`.
fn mus_tick(us: u128) -> u64 {
    let rounded = us
        .saturating_mul(MUS_TICKS_PER_SECOND)
        .saturating_add(MICROSECOND_HALF);
    let ticks = rounded.checked_div(MICROSECONDS).unwrap_or(u128::MAX);
    u64::try_from(ticks).unwrap_or(u64::MAX)
}

/// A `MUS` event waiting for its channel, volume byte and delay.
#[derive(Debug, Clone, Copy)]
struct Body {
    /// The `MUS` event type.
    kind: u8,
    /// Key, controller number or bend value.
    data1: u8,
    /// Volume or controller value, when the event carries one.
    data2: Option<u8>,
}

impl Body {
    /// A note start at `velocity`; the channel decides the volume byte.
    const fn note(note: u8, velocity: u8) -> Self {
        Self {
            kind: KIND_NOTE,
            data1: note,
            data2: Some(velocity),
        }
    }

    /// A note stop.
    const fn release(note: u8) -> Self {
        Self {
            kind: KIND_RELEASE,
            data1: note,
            data2: None,
        }
    }

    /// A pitch bend, `value14` already reduced to one byte.
    const fn bend(value: u8) -> Self {
        Self {
            kind: KIND_BEND,
            data1: value,
            data2: None,
        }
    }

    /// A controller change of `number` to `value`.
    const fn control(number: u8, value: u8) -> Self {
        Self {
            kind: KIND_CONTROL,
            data1: number,
            data2: Some(value),
        }
    }

    /// A valueless system controller `number`.
    const fn system(number: u8) -> Self {
        Self {
            kind: KIND_SYSTEM,
            data1: number,
            data2: None,
        }
    }
}

/// A descriptor byte: the event type above the channel, last bit clear.
fn descriptor(kind: u8, channel: u8) -> u8 {
    kind.checked_shl(TYPE_SHIFT)
        .unwrap_or(ZERO_VALUE)
        .checked_add(channel & CHANNEL_MASK)
        .unwrap_or(ZERO_VALUE)
}

/// The `MUS` body a control change maps to, `None` when nothing does.
fn control_body(number: u8, value: u8) -> Option<Body> {
    if let Some(system) = mus_system(number) {
        return Some(Body::system(system));
    }
    mus_control(number).map(|control| Body::control(control, value))
}

/// The `MUS` bend byte: the 14-bit `MIDI` value reduced by six bits.
fn bend_byte(value: u16) -> u8 {
    let reduced = value.checked_shr(BEND_REDUCTION).unwrap_or(0_u16);
    u8::try_from(reduced).unwrap_or(u8::MAX)
}

/// The `MUS` controller number for a mapped `MIDI` `CC`, backwards table.
fn mus_control(controller: u8) -> Option<u8> {
    (FIRST_CONTROL..=LAST_CONTROL).find(|number| midi_control(*number) == Some(controller))
}

/// The `MUS` system event number for a mapped `MIDI` `CC`, backwards table.
fn mus_system(control: u8) -> Option<u8> {
    (FIRST_SYSTEM..=LAST_SYSTEM).find(|number| system_control(*number) == Some(control))
}

#[cfg(test)]
mod tests;
