//! `Standard MIDI File` data converted to `Doom` `MUS` music lumps.
//!
//! [`midi_to_mus`] parses the file with [`crate::music::midi_read`], merges
//! its tracks into one absolute-tick timeline, converts ticks to `MUS`
//! ticks piecewise across tempo changes (`MUS` runs at 140 ticks per second)
//! and encodes the result as a `MUS` header, instrument table and packed
//! event stream. The `consts` module's `MUS` and `MIDI` tables drive the
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
//! helpers, the `convert` topic the conversion checks and the `golden` topic
//! that pins both directions down to the byte.

mod score;

use crate::error::CyberError;
use crate::music::consts::{
    BEND_SHIFT, CHANNEL_MASK, EVENT_BEND, EVENT_CONTROL, EVENT_NOTE, EVENT_RELEASE, EVENT_SYSTEM,
    EVENT_TYPE_SHIFT, MICROSECOND_HALF, MICROSECONDS_PER_SECOND, MUS_HEADER_LEN,
    MUS_INSTRUMENT_BYTES, MUS_MAGIC, MUS_MAX_SCORE_LEN, MUS_TEMPO_MICROSECONDS,
    MUS_TICKS_PER_SECOND, ZERO_VALUE, mus_control, mus_system,
};
use crate::music::midi_read::{MidiEvent, MidiFile, parse_midi};
use crate::wad::bytes::Writer;
use score::Score;

/// Number of distinct key, controller and patch values one byte counts.
const VALUE_COUNT: usize = 256_usize;
/// Offset a percussion key becomes its `MUS` instrument number at.
const PERCUSSION_INSTRUMENT_BASE: usize = 100_usize;

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
    if score_len > MUS_MAX_SCORE_LEN {
        return Err(CyberError::LimitExceeded(format!(
            "the score is {score_len} bytes, longer than the {MUS_MAX_SCORE_LEN} a MUS header counts; use fewer events"
        )));
    }
    let table_len = instruments
        .len()
        .checked_mul(MUS_INSTRUMENT_BYTES)
        .unwrap_or(0_usize);
    let start = MUS_HEADER_LEN
        .checked_add(table_len)
        .unwrap_or(MUS_HEADER_LEN);
    let mut out = Writer::new();
    out.put_bytes(&MUS_MAGIC);
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
    let mut tempo = u128::from(MUS_TEMPO_MICROSECONDS);
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
        .saturating_mul(u128::from(MUS_TICKS_PER_SECOND))
        .saturating_add(MICROSECOND_HALF);
    let ticks = rounded
        .checked_div(MICROSECONDS_PER_SECOND)
        .unwrap_or(u128::MAX);
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
            kind: EVENT_NOTE,
            data1: note,
            data2: Some(velocity),
        }
    }

    /// A note stop.
    const fn release(note: u8) -> Self {
        Self {
            kind: EVENT_RELEASE,
            data1: note,
            data2: None,
        }
    }

    /// A pitch bend, `value14` already reduced to one byte.
    const fn bend(value: u8) -> Self {
        Self {
            kind: EVENT_BEND,
            data1: value,
            data2: None,
        }
    }

    /// A controller change of `number` to `value`.
    const fn control(number: u8, value: u8) -> Self {
        Self {
            kind: EVENT_CONTROL,
            data1: number,
            data2: Some(value),
        }
    }

    /// A valueless system controller `number`.
    const fn system(number: u8) -> Self {
        Self {
            kind: EVENT_SYSTEM,
            data1: number,
            data2: None,
        }
    }
}

/// A descriptor byte: the event type above the channel, last bit clear.
fn descriptor(kind: u8, channel: u8) -> u8 {
    kind.checked_shl(EVENT_TYPE_SHIFT)
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
    let reduced = value.checked_shr(BEND_SHIFT).unwrap_or(0_u16);
    u8::try_from(reduced).unwrap_or(u8::MAX)
}

#[cfg(test)]
mod tests;
