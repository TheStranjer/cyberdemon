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

use crate::error::CyberError;
use crate::music::midi_read::{MidiEvent, MidiFile, parse_midi};
use crate::music::midi_write::{MAX_VLQ_VALUE, write_vlq};
use crate::music::mus2mid::{
    CHANNEL_COUNT, DEFAULT_VOLUME, MIDI_PERCUSSION_CHANNEL, MUS_PERCUSSION_CHANNEL, ZERO_VALUE,
    midi_control, system_control,
};
use crate::wad::bytes::Writer;

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

/// Builds the `MUS` event stream: channel mapping, volumes and delays.
struct Score {
    /// The score bytes written so far.
    bytes: Vec<u8>,
    /// The `MUS` channel each `MIDI` channel plays on, once it has played.
    mapped: [Option<u8>; CHANNEL_COUNT],
    /// The next melodic `MUS` channel to hand out.
    next_melodic: u8,
    /// The velocity each `MUS` channel last played at.
    volumes: [u8; CHANNEL_COUNT],
    /// The key, patch and percussion numbers the score calls for.
    instruments: [bool; VALUE_COUNT],
    /// `MUS` tick of the open group, and the offset of its last descriptor.
    open: Option<(u64, usize)>,
    /// Kinds of event the stream cannot carry, counted per kind.
    dropped: Dropped,
}

/// Counts of the event kinds the `MUS` stream cannot carry.
#[derive(Debug, Default)]
struct Dropped {
    /// Control changes with no `MUS` counterpart.
    control: u32,
    /// Polyphonic key pressure events.
    key_pressure: u32,
    /// Channel pressure events.
    channel_pressure: u32,
}

impl Score {
    /// An empty stream with nothing mapped and every channel at 127.
    const fn new() -> Self {
        Self {
            bytes: Vec::new(),
            mapped: [None; CHANNEL_COUNT],
            next_melodic: 0_u8,
            volumes: [DEFAULT_VOLUME; CHANNEL_COUNT],
            instruments: [false; VALUE_COUNT],
            open: None,
            dropped: Dropped {
                control: 0_u32,
                key_pressure: 0_u32,
                channel_pressure: 0_u32,
            },
        }
    }

    /// Translates one placed event, opening a group or adding to one.
    fn event(&mut self, placed: &Placed) -> Result<(), CyberError> {
        match placed.event {
            MidiEvent::NoteOn { .. } => self.note_on(placed),
            MidiEvent::NoteOff { channel, note } => {
                self.write(placed, channel, Body::release(note))
            }
            MidiEvent::PitchBend { channel, value } => {
                self.write(placed, channel, Body::bend(bend_byte(value)))
            }
            MidiEvent::ProgramChange { channel, program } => {
                self.write(placed, channel, Body::control(PROGRAM_CONTROLLER, program))
            }
            MidiEvent::ControlChange { .. } => self.control_change(placed),
            MidiEvent::KeyPressure { .. } => {
                self.dropped.key_pressure = self.dropped.key_pressure.saturating_add(1_u32);
                Ok(())
            }
            MidiEvent::ChannelPressure { .. } => {
                self.dropped.channel_pressure = self.dropped.channel_pressure.saturating_add(1_u32);
                Ok(())
            }
            MidiEvent::SetTempo { .. } | MidiEvent::EndOfTrack => Ok(()),
        }
    }

    /// A note on at velocity 0 releases; any other velocity plays.
    fn note_on(&mut self, placed: &Placed) -> Result<(), CyberError> {
        let MidiEvent::NoteOn {
            channel,
            note,
            velocity,
        } = placed.event
        else {
            return Ok(());
        };
        if velocity == ZERO_VALUE {
            return self.write(placed, channel, Body::release(note));
        }
        self.write(placed, channel, Body::note(note, velocity))
    }

    /// Writes a mapped control change, or counts one with no counterpart.
    fn control_change(&mut self, placed: &Placed) -> Result<(), CyberError> {
        let MidiEvent::ControlChange {
            channel,
            number,
            value,
        } = placed.event
        else {
            return Ok(());
        };
        if let Some(body) = control_body(number, value) {
            return self.write(placed, channel, body);
        }
        self.dropped.control = self.dropped.control.saturating_add(1_u32);
        Ok(())
    }

    /// Finishes the stream: close the last group at `end_tick`, mark the end.
    fn finish(&mut self, end_tick: Option<u64>) -> Result<(), CyberError> {
        if let Some(tick) = end_tick {
            self.close(tick)?;
        }
        self.bytes.push(descriptor(KIND_SCORE_END, ZERO_VALUE));
        Ok(())
    }

    /// The `MUS` instrument table: patches and keys used, ascending.
    fn instrument_table(&self) -> Vec<u8> {
        (0_usize..VALUE_COUNT)
            .filter(|index| self.instruments.get(*index).copied().unwrap_or(false))
            .filter_map(|index| u8::try_from(index).ok())
            .collect()
    }

    /// The number of melodic `MUS` channels the stream used.
    const fn primary(&self) -> u8 {
        self.next_melodic
    }

    /// One line per kind of event the stream dropped, in fixed order.
    fn warnings(&self) -> Vec<String> {
        let mut lines = Vec::new();
        if self.dropped.control > 0_u32 {
            lines.push(format!(
                "dropped {} control changes with no MUS counterpart",
                self.dropped.control
            ));
        }
        if self.dropped.key_pressure > 0_u32 {
            lines.push(format!(
                "dropped {} key pressure events",
                self.dropped.key_pressure
            ));
        }
        if self.dropped.channel_pressure > 0_u32 {
            lines.push(format!(
                "dropped {} channel pressure events",
                self.dropped.channel_pressure
            ));
        }
        lines
    }

    /// Maps the channel, resolves a note's volume and appends the event.
    fn write(&mut self, placed: &Placed, midi: u8, mut body: Body) -> Result<(), CyberError> {
        let mus = self.channel(midi)?;
        if body.kind == KIND_NOTE {
            self.apply_volume(mus, &mut body);
        }
        if body.kind == KIND_CONTROL
            && body.data1 == PROGRAM_CONTROLLER
            && mus != MUS_PERCUSSION_CHANNEL
        {
            self.use_instrument(usize::from(body.data2.unwrap_or(ZERO_VALUE)));
        }
        self.close_if_later(placed.tick)?;
        let position = self.bytes.len();
        self.bytes.push(descriptor(body.kind, mus));
        self.bytes.push(body.data1);
        if let Some(value) = body.data2 {
            self.bytes.push(value);
        }
        self.open = Some((placed.tick, position));
        Ok(())
    }

    /// Turns a note body's velocity into the key and volume bytes it needs.
    fn apply_volume(&mut self, mus: u8, body: &mut Body) {
        let velocity = body.data2.unwrap_or(DEFAULT_VOLUME).min(MAX_VALUE);
        let key = body.data1 & MAX_VALUE;
        let changed = self.set_volume(mus, velocity);
        self.mark_instrument(mus, key);
        body.data1 = if changed { key | VOLUME_FLAG } else { key };
        body.data2 = changed.then_some(velocity);
    }

    /// The `MUS` channel a `MIDI` channel plays on, mapping it when needed.
    fn channel(&mut self, midi: u8) -> Result<u8, CyberError> {
        if midi == MIDI_PERCUSSION_CHANNEL {
            return Ok(MUS_PERCUSSION_CHANNEL);
        }
        if let Some(mapped) = self.mapped.get(usize::from(midi)).copied().flatten() {
            return Ok(mapped);
        }
        if self.next_melodic >= MELODIC_CHANNELS {
            return Err(CyberError::LimitExceeded(format!(
                "the score uses more than {MELODIC_CHANNELS} melodic MIDI channels; a MUS score has room for {MELODIC_CHANNELS}"
            )));
        }
        let mapped = self.next_melodic;
        self.next_melodic = mapped.saturating_add(1_u8);
        if let Some(slot) = self.mapped.get_mut(usize::from(midi)) {
            *slot = Some(mapped);
        }
        Ok(mapped)
    }

    /// Remembers a percussion key, and nothing on melodic channels.
    fn mark_instrument(&mut self, mus: u8, note: u8) {
        if mus == MUS_PERCUSSION_CHANNEL {
            let number = usize::from(note)
                .saturating_add(PERCUSSION_INSTRUMENT_BASE)
                .min(VALUE_COUNT.saturating_sub(1_usize));
            self.use_instrument(number);
        }
    }

    /// Marks one instrument number as used.
    fn use_instrument(&mut self, index: usize) {
        if let Some(slot) = self.instruments.get_mut(index) {
            *slot = true;
        }
    }

    /// Remembers a channel's velocity; true when it moves from the old one.
    fn set_volume(&mut self, mus: u8, volume: u8) -> bool {
        let old = self
            .volumes
            .get(usize::from(mus))
            .copied()
            .unwrap_or(DEFAULT_VOLUME);
        if let Some(slot) = self.volumes.get_mut(usize::from(mus)) {
            *slot = volume;
        }
        volume != old
    }

    /// Closes the open group when a later tick arrives; otherwise not.
    fn close_if_later(&mut self, tick: u64) -> Result<(), CyberError> {
        let Some((open, _)) = self.open else {
            return Ok(());
        };
        if tick > open {
            self.close(tick)
        } else {
            Ok(())
        }
    }

    /// Sets the last bit on the open group and writes the delay to `tick`.
    fn close(&mut self, tick: u64) -> Result<(), CyberError> {
        let Some((open, descriptor_at)) = self.open.take() else {
            return Ok(());
        };
        if tick <= open {
            return Ok(());
        }
        let delay = u32::try_from(tick.saturating_sub(open)).unwrap_or(u32::MAX);
        if delay > MAX_VLQ_VALUE {
            return Err(CyberError::LimitExceeded(format!(
                "a {delay}-tick delay does not fit the four bytes of a MUS quantity; shorten the gap between events"
            )));
        }
        if let Some(byte) = self.bytes.get_mut(descriptor_at) {
            *byte |= LAST_EVENT_FLAG;
        }
        write_vlq(&mut self.bytes, delay);
        Ok(())
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
mod tests {
    use super::{MusConversion, Score, midi_to_mus};
    use crate::music::midi_write::{put_be_u16, put_be_u32, read_vlq, write_vlq};
    use crate::music::mus2mid::mus_to_midi;
    use crate::wad::bytes::{Reader, Writer};

    /// One decoded `MUS` event, with the delay that closed its group.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    struct Ev {
        kind: u8,
        channel: u8,
        data1: u8,
        data2: Option<u8>,
        delay: Option<u32>,
    }

    /// A chunk: its id, big-endian body length, then the body.
    fn chunk(id: [u8; 4], body: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&id);
        put_be_u32(
            &mut out,
            u32::try_from(body.len()).expect("hand-built chunks are short"),
        );
        out.extend_from_slice(body);
        out
    }

    /// A complete type-0 file with one track closing at delta 0.
    fn smf(body: &[u8]) -> Vec<u8> {
        let mut track = body.to_vec();
        track.extend_from_slice(&[0x00, 0xFF, 0x2F, 0x00]);
        let mut header = Vec::new();
        put_be_u16(&mut header, 0_u16);
        put_be_u16(&mut header, 1_u16);
        put_be_u16(&mut header, 70_u16);
        let mut out = chunk(*b"MThd", &header);
        out.extend_from_slice(&chunk(*b"MTrk", &track));
        out
    }

    /// The conversion a hand-built track body produces.
    fn convert(body: &[u8]) -> MusConversion {
        midi_to_mus(&smf(body)).expect("a hand-built file converts")
    }

    /// The events of a converted lump, score end included.
    fn decode(mus: &[u8]) -> Vec<Ev> {
        assert!(mus.starts_with(b"MUS\x1A"), "the output is a MUS lump");
        let score_len = usize::from(word(mus, 4));
        let score_start = usize::from(word(mus, 6));
        let end = score_start.saturating_add(score_len);
        let mut reader = Reader::new(mus.get(score_start..end).expect("the declared score"));
        let mut out = Vec::new();
        while reader.remaining() > 0 {
            let descriptor = reader.read_u8().expect("a whole descriptor");
            let kind = descriptor.checked_shr(4).unwrap_or(0) & 0x07;
            let (data1, data2) = event_data(&mut reader, kind);
            let last = descriptor & 0x80 != 0;
            let delay = last.then(|| read_vlq(&mut reader).expect("a delay"));
            out.push(Ev {
                kind,
                channel: descriptor & 0x0F,
                data1,
                data2,
                delay,
            });
        }
        out
    }

    /// The little-endian word at `offset`, for reading a header back.
    fn word(mus: &[u8], offset: usize) -> u16 {
        let low = mus.get(offset).copied().expect("a header word");
        let high = mus
            .get(offset.saturating_add(1))
            .copied()
            .expect("a header word");
        u16::from_le_bytes([low, high])
    }

    /// The data bytes a descriptor of `kind` introduces.
    fn event_data(reader: &mut Reader<'_>, kind: u8) -> (u8, Option<u8>) {
        match kind {
            0 | 2 | 3 => (reader.read_u8().expect("one data byte"), None),
            1 => note_data(reader),
            4 => (
                reader.read_u8().expect("a controller number"),
                Some(reader.read_u8().expect("a controller value")),
            ),
            _ => (0, None),
        }
    }

    /// A key byte with its optional volume byte.
    fn note_data(reader: &mut Reader<'_>) -> (u8, Option<u8>) {
        let key = reader.read_u8().expect("a key byte");
        (
            key & 0x7F,
            (key & 0x80 != 0).then(|| reader.read_u8().expect("a volume byte")),
        )
    }

    /// The score bytes a list of events encodes to.
    fn encode(events: &[Ev]) -> Vec<u8> {
        let mut score: Vec<u8> = Vec::new();
        for event in events {
            encode_event(*event, &mut score);
        }
        score
    }

    /// Append one event: descriptor, data bytes and closing delay.
    fn encode_event(event: Ev, out: &mut Vec<u8>) {
        out.push(event_descriptor(event));
        event_body(event, out);
        if let Some(delay) = event.delay {
            write_vlq(out, delay);
        }
    }

    /// The descriptor byte of an event, last bit per its delay.
    fn event_descriptor(event: Ev) -> u8 {
        let marked = u8::from(event.delay.is_some())
            .checked_mul(0x80)
            .unwrap_or(0);
        event
            .kind
            .checked_shl(4)
            .unwrap_or(0)
            .checked_add(event.channel)
            .unwrap_or(0)
            | marked
    }

    /// Appends the data bytes an event kind carries.
    fn event_body(event: Ev, out: &mut Vec<u8>) {
        match event.kind {
            0 | 2 | 3 => out.push(event.data1),
            1 => match event.data2 {
                Some(volume) => {
                    out.push(event.data1 | 0x80);
                    out.push(volume);
                }
                None => out.push(event.data1),
            },
            4 => {
                out.push(event.data1);
                out.push(event.data2.unwrap_or(0));
            }
            _ => {}
        }
    }

    /// A complete lump: header claiming these score bytes at offset 16.
    fn lump(score: &[u8]) -> Vec<u8> {
        let mut lump = Writer::new();
        lump.put_bytes(b"MUS\x1A");
        lump.put_u16(u16::try_from(score.len()).expect("a hand-built score fits"));
        lump.put_u16(16_u16);
        lump.put_u16(0_u16);
        lump.put_u16(0_u16);
        lump.put_u16(0_u16);
        lump.put_u16(0_u16);
        let mut bytes = lump.into_inner();
        bytes.extend_from_slice(score);
        bytes
    }

    /// The header's `score_start`, primary-channel and instrument-count words.
    fn header(mus: &[u8]) -> (u16, u16, u16) {
        (word(mus, 6), word(mus, 8), word(mus, 12))
    }

    #[test]
    fn mus_to_midi_to_mus_reproduces_the_score() {
        let score = [
            Ev {
                kind: 0,
                channel: 0,
                data1: 0x30,
                data2: None,
                delay: None,
            },
            Ev {
                kind: 1,
                channel: 0,
                data1: 0x3C,
                data2: Some(64),
                delay: Some(35),
            },
            Ev {
                kind: 4,
                channel: 0,
                data1: 7,
                data2: Some(100),
                delay: None,
            },
            Ev {
                kind: 3,
                channel: 1,
                data1: 12,
                data2: None,
                delay: Some(70),
            },
            Ev {
                kind: 2,
                channel: 1,
                data1: 128,
                data2: None,
                delay: Some(140),
            },
            Ev {
                kind: 6,
                channel: 0,
                data1: 0,
                data2: None,
                delay: None,
            },
        ];
        let midi = mus_to_midi(&lump(&encode(&score))).expect("the hand-built score converts");
        let back = midi_to_mus(&midi).expect("and converts back");
        assert_eq!(decode(&back.mus), score, "the same events, delays and all");
        assert!(
            back.warnings.is_empty(),
            "a mapped score warns about nothing"
        );
    }

    #[test]
    fn a_tempo_change_rescales_the_later_delays() {
        // 500000 then 250000 microseconds per quarter at tick 70: events
        // before the change move 1 MUS tick per MIDI tick, after it 2 MIDI
        // ticks per MUS tick, so the 140-tick release lands at MUS 105.
        let conversion = convert(&[
            0x00, 0xFF, 0x51, 0x03, 0x07, 0xA1, 0x20, // 500000
            0x00, 0x90, 0x3C, 0x7F, // play at MIDI tick 0
            0x46, 0xFF, 0x51, 0x03, 0x03, 0xD2, 0x50, // 250000 at tick 70
            0x00, 0x90, 0x3E, 0x7F, // play at MIDI tick 70
            0x46, 0x80, 0x3C, 0x00, // release at MIDI tick 140
        ]);
        assert_eq!(
            decode(&conversion.mus),
            [
                Ev {
                    kind: 1,
                    channel: 0,
                    data1: 0x3C,
                    data2: None,
                    delay: Some(70)
                },
                Ev {
                    kind: 1,
                    channel: 0,
                    data1: 0x3E,
                    data2: None,
                    delay: Some(35)
                },
                Ev {
                    kind: 0,
                    channel: 0,
                    data1: 0x3C,
                    data2: None,
                    delay: None
                },
                Ev {
                    kind: 6,
                    channel: 0,
                    data1: 0,
                    data2: None,
                    delay: None
                },
            ],
            "70, then half speed: 35 for the next 70 ticks"
        );
    }

    #[test]
    fn a_velocity_zero_note_on_releases() {
        let conversion = convert(&[0x00, 0x90, 0x3C, 0x00]);
        assert_eq!(
            decode(&conversion.mus),
            [
                Ev {
                    kind: 0,
                    channel: 0,
                    data1: 0x3C,
                    data2: None,
                    delay: None
                },
                Ev {
                    kind: 6,
                    channel: 0,
                    data1: 0,
                    data2: None,
                    delay: None
                },
            ],
            "type 0, no volume byte"
        );
    }

    #[test]
    fn percussion_lands_on_channel_fifteen_and_lists_its_keys() {
        let conversion = convert(&[
            0x00, 0x99, 0x30, 0x7F, // drum key 48 on
            0x00, 0x89, 0x30, 0x00, // and off
        ]);
        assert_eq!(
            decode(&conversion.mus),
            [
                Ev {
                    kind: 1,
                    channel: 15,
                    data1: 0x30,
                    data2: None,
                    delay: None
                },
                Ev {
                    kind: 0,
                    channel: 15,
                    data1: 0x30,
                    data2: None,
                    delay: None
                },
                Ev {
                    kind: 6,
                    channel: 0,
                    data1: 0,
                    data2: None,
                    delay: None
                },
            ],
            "MIDI 9 is MUS 15"
        );
        let (start, primary, count) = header(&conversion.mus);
        assert_eq!(
            (start, primary, count),
            (18, 0, 1),
            "one instrument, no melodic channel"
        );
        assert_eq!(&conversion.mus[16..18], &[148, 0], "key 48 plus 100");
    }

    #[test]
    fn the_instrument_table_lists_melodic_programs_ascending() {
        let conversion = convert(&[
            0x00, 0xC0, 0x05, // program 5 on MIDI 0
            0x00, 0xC0, 0x05, // again, deduplicated
            0x00, 0xC1, 0x64, // program 100 on MIDI 1
            0x00, 0xC9, 0x07, // a program on the percussion channel: not listed
        ]);
        let (start, primary, count) = header(&conversion.mus);
        assert_eq!(
            (start, primary, count),
            (20, 2, 2),
            "two instruments, two parts"
        );
        assert_eq!(
            &conversion.mus[16..20],
            &[5, 0, 100, 0],
            "ascending, no duplicate"
        );
        let events = decode(&conversion.mus);
        assert_eq!(
            events[3].channel, 15,
            "the drum program still writes MUS 15"
        );
    }

    #[test]
    fn dropped_events_are_counted_one_warning_per_kind() {
        let conversion = convert(&[
            0x00, 0xB0, 0x02, 0x09, // unmapped CC
            0x00, 0xB0, 0x07, 0x64, // mapped CC: kept
            0x00, 0xB0, 0x02, 0x0A, // unmapped CC again
            0x00, 0xA3, 0x3C, 0x40, // key pressure
            0x00, 0xD0, 0x50, // channel pressure
            0x00, 0x90, 0x3C, 0x7F,
        ]);
        assert_eq!(
            conversion.warnings,
            [
                "dropped 2 control changes with no MUS counterpart",
                "dropped 1 key pressure events",
                "dropped 1 channel pressure events",
            ],
            "one line per kind, with counts"
        );
        let events = decode(&conversion.mus);
        assert_eq!(
            events,
            [
                Ev {
                    kind: 4,
                    channel: 0,
                    data1: 3,
                    data2: Some(100),
                    delay: None
                },
                Ev {
                    kind: 1,
                    channel: 0,
                    data1: 0x3C,
                    data2: None,
                    delay: None
                },
                Ev {
                    kind: 6,
                    channel: 0,
                    data1: 0,
                    data2: None,
                    delay: None
                },
            ],
            "the mapped CC became MUS controller 3; the rest vanished"
        );
    }

    #[test]
    fn a_sixteenth_melodic_channel_exceeds_the_limit() {
        // MIDI has only sixteen channels and 9 is percussion, so real files
        // cannot ask for a sixteenth melodic channel; the pool guard is
        // exercised directly.
        let mut score = Score::new();
        for (index, midi) in [0_u8, 1, 2, 3, 4, 5, 6, 7, 8, 10, 11, 12, 13, 14, 15]
            .into_iter()
            .enumerate()
        {
            let expected = u8::try_from(index).expect("fifteen channels fit a u8");
            assert_eq!(
                score.channel(midi).expect("a pool slot"),
                expected,
                "channel {midi}"
            );
        }
        assert_eq!(
            score.channel(3).expect("already mapped"),
            3,
            "the map is remembered"
        );
        assert_eq!(
            score.channel(9).expect("percussion"),
            15,
            "MIDI 9 is always MUS 15"
        );
        let error = score.channel(16).expect_err("the pool is full");
        assert_eq!(error.code(), "limit_exceeded", "sixteenth melodic channel");
    }

    #[test]
    fn a_delay_past_twentyeight_bits_exceeds_the_limit() {
        // At 4 000 000 microseconds per quarter, every MIDI tick is eight
        // MUS ticks, so one 28-bit delta asks for eight times that.
        let error = midi_to_mus(&smf(&[
            0x00, 0xFF, 0x51, 0x03, 0x3D, 0x09, 0x00, // 4000000
            0x00, 0x90, 0x3C, 0x7F, 0xFF, 0xFF, 0xFF, 0x7F, 0x80, 0x3C,
            0x00, // release 268435455 ticks on
        ]))
        .expect_err("a two-billion-tick gap");
        assert_eq!(error.code(), "limit_exceeded", "oversized delay code");
        assert!(error.to_string().contains("delay"), "names it: {error}");
    }

    #[test]
    fn a_score_past_the_header_limit_exceeds_it() {
        let mut body: Vec<u8> = Vec::new();
        for _ in 0_usize..40_000 {
            body.extend_from_slice(&[0x00, 0x90, 0x3C, 0x40]);
        }
        let error = midi_to_mus(&smf(&body)).expect_err("eighty thousand bytes");
        assert_eq!(error.code(), "limit_exceeded", "oversized score code");
        assert!(
            error.to_string().contains("65535"),
            "names the limit: {error}"
        );
    }

    #[test]
    fn parse_errors_pass_through() {
        let mut header = Vec::new();
        put_be_u16(&mut header, 2_u16); // format 2
        put_be_u16(&mut header, 0_u16);
        put_be_u16(&mut header, 70_u16);
        let unsupported = midi_to_mus(&chunk(*b"MThd", &header)).expect_err("format 2");
        assert_eq!(unsupported.code(), "unsupported", "format 2 code");
        let truncated =
            midi_to_mus(&chunk(*b"MThd", &[0, 0, 0, 1, 0, 46])).expect_err("a header alone");
        assert_eq!(truncated.code(), "format_error", "missing track code");
    }

    #[test]
    fn merged_tracks_keep_time_order_in_the_score() {
        let mut header = Vec::new();
        put_be_u16(&mut header, 1_u16); // format 1
        put_be_u16(&mut header, 2_u16);
        put_be_u16(&mut header, 70_u16);
        let mut out = chunk(*b"MThd", &header);
        for track in [
            &[0x46_u8, 0x90, 0x3C, 0x7F][..], // plays at tick 70
            &[0x00_u8, 0x91, 0x40, 0x7F][..], // plays at tick 0
        ] {
            let mut body = track.to_vec();
            body.extend_from_slice(&[0x00, 0xFF, 0x2F, 0x00]);
            out.extend_from_slice(&chunk(*b"MTrk", &body));
        }
        let conversion = midi_to_mus(&out).expect("a format-1 file converts");
        assert_eq!(
            decode(&conversion.mus),
            [
                Ev {
                    kind: 1,
                    channel: 0,
                    data1: 0x40,
                    data2: None,
                    delay: Some(70)
                },
                Ev {
                    kind: 1,
                    channel: 1,
                    data1: 0x3C,
                    data2: None,
                    delay: None
                },
                Ev {
                    kind: 6,
                    channel: 0,
                    data1: 0,
                    data2: None,
                    delay: None
                },
            ],
            "the second track plays first; MUS channels follow first use"
        );
    }

    #[test]
    fn empty_and_all_dropped_scores_are_just_an_end_marker() {
        assert_eq!(
            decode(&convert(&[]).mus),
            [Ev {
                kind: 6,
                channel: 0,
                data1: 0,
                data2: None,
                delay: None
            }],
            "an empty track"
        );
        let dropped = convert(&[0x00, 0xD0, 0x50]); // channel pressure only
        assert_eq!(
            decode(&dropped.mus),
            [Ev {
                kind: 6,
                channel: 0,
                data1: 0,
                data2: None,
                delay: None
            }],
            "nothing survived"
        );
        assert_eq!(dropped.warnings.len(), 1, "and that was reported");
    }
}
