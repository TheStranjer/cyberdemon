//! Shared helpers for the `MUS` writer's tests: hand-built `Standard MIDI`
//! files, a reader of the lump a conversion produced and an encoder of
//! hand-written `MUS` events, so the `convert` topic can check whole
//! conversions.
//!
//! [`smf`] wraps a track body in a type-0 file, [`convert()`] runs
//! [`midi_to_mus`] over it and [`decode`] reads the score bytes back into
//! [`Ev`] rows with the delay that closed each group. [`encode`] and
//! [`lump`] go the other way, building a `MUS` lump no conversion was
//! involved in; [`header`] reads the words a test asserts on. Every
//! `#[test]` lives in the `convert` module below or, for the byte-exact
//! fixtures of both directions, in `golden`.

mod convert;
mod golden;

use super::{MusConversion, midi_to_mus};
use crate::music::consts::{
    MUS_MAGIC, MUS_SCORE_LEN_OFFSET, MUS_SCORE_START_OFFSET, SMF_HEADER_ID, SMF_TRACK_ID,
};
use crate::music::midi_write::{read_vlq, write_vlq};
use crate::wad::bytes::{Reader, Writer};

/// One decoded `MUS` event, with the delay that closed its group.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Ev {
    kind: u8,
    channel: u8,
    data1: u8,
    data2: Option<u8>,
    delay: Option<u32>,
}

/// A chunk: its id, big-endian body length, then the body.
pub(super) fn chunk(id: [u8; 4], body: &[u8]) -> Vec<u8> {
    let mut out = Writer::new();
    out.put_bytes(&id);
    out.put_u32_be(u32::try_from(body.len()).expect("hand-built chunks are short"));
    out.put_bytes(body);
    out.into_inner()
}

/// A complete type-0 file with one track closing at delta 0.
pub(super) fn smf(body: &[u8]) -> Vec<u8> {
    let mut track = body.to_vec();
    track.extend_from_slice(&[0x00, 0xFF, 0x2F, 0x00]);
    let mut header = Writer::new();
    header.put_u16_be(0_u16);
    header.put_u16_be(1_u16);
    header.put_u16_be(70_u16);
    let mut out = chunk(SMF_HEADER_ID, &header.into_inner());
    out.extend_from_slice(&chunk(SMF_TRACK_ID, &track));
    out
}

/// The conversion a hand-built track body produces.
pub(super) fn convert(body: &[u8]) -> MusConversion {
    midi_to_mus(&smf(body)).expect("a hand-built file converts")
}

/// The events of a converted lump, score end included.
pub(super) fn decode(mus: &[u8]) -> Vec<Ev> {
    assert!(mus.starts_with(&MUS_MAGIC), "the output is a MUS lump");
    let score_len = usize::from(word(mus, MUS_SCORE_LEN_OFFSET));
    let score_start = usize::from(word(mus, MUS_SCORE_START_OFFSET));
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
pub(super) fn encode(events: &[Ev]) -> Vec<u8> {
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
pub(super) fn lump(score: &[u8]) -> Vec<u8> {
    let mut lump = Writer::new();
    lump.put_bytes(&MUS_MAGIC);
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
pub(super) fn header(mus: &[u8]) -> (u16, u16, u16) {
    (
        word(mus, MUS_SCORE_START_OFFSET),
        word(mus, 8),
        word(mus, 12),
    )
}
