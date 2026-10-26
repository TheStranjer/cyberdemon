//! Conversion tests: the round trip from a hand-built `MUS` score through
//! `MIDI` and back, the tempo, channel and instrument mapping of a
//! converted track, the limits the score refuses and the warnings it
//! reports for what `MUS` cannot carry.

use super::{Ev, chunk, convert, decode, encode, header, lump, smf};
use crate::music::consts::{SMF_HEADER_ID, SMF_TRACK_ID};
use crate::music::mid2mus::{Score, midi_to_mus};
use crate::music::mus2mid::mus_to_midi;
use crate::wad::bytes::Writer;

/// A header body declaring `format` and `tracks` tracks at 70 ticks per
/// quarter, big-endian as `SMF` stores the three fields.
fn header_body(format: u16, tracks: u16) -> Vec<u8> {
    let mut body = Writer::new();
    body.put_u16_be(format);
    body.put_u16_be(tracks);
    body.put_u16_be(70_u16);
    body.into_inner()
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
    let unsupported = midi_to_mus(&chunk(SMF_HEADER_ID, &header_body(2, 0))).expect_err("format 2");
    assert_eq!(unsupported.code(), "unsupported", "format 2 code");
    let truncated =
        midi_to_mus(&chunk(*b"MThd", &[0, 0, 0, 1, 0, 46])).expect_err("a header alone");
    assert_eq!(truncated.code(), "format_error", "missing track code");
}

#[test]
fn merged_tracks_keep_time_order_in_the_score() {
    let mut out = chunk(SMF_HEADER_ID, &header_body(1, 2));
    for track in [
        &[0x46_u8, 0x90, 0x3C, 0x7F][..], // plays at tick 70
        &[0x00_u8, 0x91, 0x40, 0x7F][..], // plays at tick 0
    ] {
        let mut body = track.to_vec();
        body.extend_from_slice(&[0x00, 0xFF, 0x2F, 0x00]);
        out.extend_from_slice(&chunk(SMF_TRACK_ID, &body));
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
