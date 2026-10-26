//! The `MUS` event stream: the packed descriptor and data bytes of every
//! event, with the channel mapping, volumes, instruments and delays that
//! make a merged timeline into one.
//!
//! [`Score`] holds the whole state of a conversion: the `MUS` channel each
//! `MIDI` channel plays on, the velocity each of them last played at, the
//! instrument numbers the score calls for, the tick and descriptor offset
//! of the open group and the [`Dropped`] counts of the events the format
//! cannot carry. [`Placed`](super::Placed) events arrive in stream order;
//! each one either joins the group already open or closes it with the `VLQ`
//! delay to its own tick. The parent keeps the tick timeline and assembles
//! the header and instrument table around the bytes [`midi_to_mus`] writes.
//!
//! [`midi_to_mus`]: super::midi_to_mus

use super::{
    Body, PERCUSSION_INSTRUMENT_BASE, Placed, VALUE_COUNT, bend_byte, control_body, descriptor,
};
use crate::error::CyberError;
use crate::music::consts::{
    CHANNEL_COUNT, DEFAULT_VOLUME, EVENT_CONTROL, EVENT_NOTE, EVENT_SCORE_END, LAST_EVENT_FLAG,
    MAX_VALUE, MELODIC_CHANNELS, MIDI_PERCUSSION_CHANNEL, MUS_PERCUSSION_CHANNEL,
    PROGRAM_CONTROLLER, VOLUME_FLAG, ZERO_VALUE,
};
use crate::music::midi_read::MidiEvent;
use crate::music::midi_write::{MAX_VLQ_VALUE, write_vlq};

/// Builds the `MUS` event stream: channel mapping, volumes and delays.
pub(super) struct Score {
    /// The score bytes written so far.
    pub(super) bytes: Vec<u8>,
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
    pub(super) const fn new() -> Self {
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
    pub(super) fn event(&mut self, placed: &Placed) -> Result<(), CyberError> {
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
    pub(super) fn finish(&mut self, end_tick: Option<u64>) -> Result<(), CyberError> {
        if let Some(tick) = end_tick {
            self.close(tick)?;
        }
        self.bytes.push(descriptor(EVENT_SCORE_END, ZERO_VALUE));
        Ok(())
    }

    /// The `MUS` instrument table: patches and keys used, ascending.
    pub(super) fn instrument_table(&self) -> Vec<u8> {
        (0_usize..VALUE_COUNT)
            .filter(|index| self.instruments.get(*index).copied().unwrap_or(false))
            .filter_map(|index| u8::try_from(index).ok())
            .collect()
    }

    /// The number of melodic `MUS` channels the stream used.
    pub(super) const fn primary(&self) -> u8 {
        self.next_melodic
    }

    /// One line per kind of event the stream dropped, in fixed order.
    pub(super) fn warnings(&self) -> Vec<String> {
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
        if body.kind == EVENT_NOTE {
            self.apply_volume(mus, &mut body);
        }
        if body.kind == EVENT_CONTROL
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
    pub(super) fn channel(&mut self, midi: u8) -> Result<u8, CyberError> {
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
