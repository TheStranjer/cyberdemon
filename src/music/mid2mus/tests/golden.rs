//! The byte-exact goldens of both conversion directions: the `MUS` lump and
//! the `Standard MIDI File` each one produces are pinned here in full, so a
//! change to the shared `MUS`/`MIDI` vocabulary or the `SMF` byte writers is
//! caught as a changed byte, not as a passing structural check.
//!
//! The two fixtures are a closed pair: [`MUS_GOLDEN`] converts to
//! [`MIDI_GOLDEN`] and back, and [`MIDI_GOLDEN`] converts to [`MUS_GOLDEN`]
//! and back, both byte for byte. The score is annotated below; the events
//! cover a program change, a mapped controller, a volume-carrying note and
//! its release, a pitch bend, a valueless system event closed by a delay no
//! single byte counts, and percussion on `MUS` channel 15, which is why the
//! instrument table holds patch 5 and the percussion number 148.

use crate::music::mid2mus::midi_to_mus;
use crate::music::mus2mid::mus_to_midi;

/// The `MUS` lump whose score is:
///
/// ```text
/// 40 00 05         controller 0 = 5        -> program change, patch 5
/// 40 03 64         controller 3 = 100      -> CC 7 (volume) = 100
/// 10 BC 60         note 0x3C, volume 96    -> note on, velocity 96
/// 80 3C 02         release 0x3C, delay 2   -> note off, next event 2 ticks on
/// 21 40            bend 64 on channel 1    -> pitch bend 0x1000
/// B1 0B 81 00      system 11, delay 128    -> CC 123, next event 128 ticks on
/// 1F B0 40         drum 0x30, volume 64    -> note on, MIDI channel 9
/// 8F 30 01         drum release, delay 1   -> note off, the score ends 1 tick on
/// 60               score end
/// ```
const MUS_GOLDEN: [u8; 45] = [
    0x4D, 0x55, 0x53, 0x1A, 0x19, 0x00, 0x14, 0x00, 0x02, 0x00, 0x00, 0x00, 0x02, 0x00, 0x00, 0x00,
    0x05, 0x00, 0x94, 0x00, 0x40, 0x00, 0x05, 0x40, 0x03, 0x64, 0x10, 0xBC, 0x60, 0x80, 0x3C, 0x02,
    0x21, 0x40, 0xB1, 0x0B, 0x81, 0x00, 0x1F, 0xB0, 0x40, 0x8F, 0x30, 0x01, 0x60,
];

/// The type-0 `Standard MIDI File` [`MUS_GOLDEN`]'s score renders as: the
/// `MThd` chunk declaring one track at 70 ticks per quarter note, the `MTrk`
/// chunk of 43 bytes, its tempo meta event at 500 000 microseconds, the
/// translated events and the end of track.
const MIDI_GOLDEN: [u8; 65] = [
    0x4D, 0x54, 0x68, 0x64, 0x00, 0x00, 0x00, 0x06, 0x00, 0x00, 0x00, 0x01, 0x00, 0x46, 0x4D, 0x54,
    0x72, 0x6B, 0x00, 0x00, 0x00, 0x2B, 0x00, 0xFF, 0x51, 0x03, 0x07, 0xA1, 0x20, 0x00, 0xC0, 0x05,
    0x00, 0xB0, 0x07, 0x64, 0x00, 0x90, 0x3C, 0x60, 0x00, 0x80, 0x3C, 0x00, 0x02, 0xE1, 0x00, 0x20,
    0x00, 0xB1, 0x7B, 0x00, 0x81, 0x00, 0x99, 0x30, 0x40, 0x00, 0x89, 0x30, 0x00, 0x01, 0xFF, 0x2F,
    0x00,
];

#[test]
fn mus_score_renders_the_golden_midi_bytes() {
    assert_eq!(
        mus_to_midi(&MUS_GOLDEN).expect("the golden MUS converts"),
        MIDI_GOLDEN,
        "every event, delta and chunk field of the translation"
    );
}

#[test]
fn midi_file_rebuilds_the_golden_mus_bytes() {
    let conversion = midi_to_mus(&MIDI_GOLDEN).expect("the golden MIDI converts");
    assert_eq!(
        conversion.mus, MUS_GOLDEN,
        "the same lump: header, instrument table and score"
    );
    assert!(
        conversion.warnings.is_empty(),
        "a mapped score warns about nothing: {:?}",
        conversion.warnings
    );
}

#[test]
fn the_goldens_round_trip_through_both_converters() {
    let midi = mus_to_midi(&MUS_GOLDEN).expect("MUS to MIDI");
    let back = midi_to_mus(&midi).expect("MIDI back to MUS");
    assert_eq!(back.mus, MUS_GOLDEN, "MUS to MIDI to MUS is the identity");
    let again = mus_to_midi(&back.mus).expect("MIDI to MUS to MIDI");
    assert_eq!(again, midi, "and back again, byte for byte");
}
