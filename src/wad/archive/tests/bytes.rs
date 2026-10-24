//! The bytes topic: what each kind's magic serializes to, the constructors
//! and defaults of a [`Wad`], the canonical and deduplicating layouts, and
//! the overlapping layouts `parse` accepts.

use serde_json::json;

use super::{build_file, duplicated, name, raw_entry, sample, written_entries};
use crate::wad::archive::{HEADER_LEN, Lump, Wad, WadKind};

#[test]
fn kind_magic_and_serialization() {
    assert_eq!(*WadKind::Iwad.magic(), *b"IWAD", "IWAD bytes");
    assert_eq!(*WadKind::Pwad.magic(), *b"PWAD", "PWAD bytes");
    assert_eq!(
        serde_json::to_value(WadKind::Iwad).expect("serializes"),
        json!("IWAD"),
        "IWAD serializes as the magic"
    );
    assert_eq!(
        serde_json::to_value(WadKind::Pwad).expect("serializes"),
        json!("PWAD"),
        "PWAD serializes as the magic"
    );
}

#[test]
fn lump_constructors_and_size() {
    let filled = Lump::new(name("SPLY3"), vec![b'a'; 4]);
    assert_eq!(filled.size(), 4, "payload size");
    assert_eq!(filled.data.as_ref(), b"aaaa", "payload bytes");
    let marker = Lump::marker(name("F_END"));
    assert_eq!(marker.size(), 0, "markers are empty");
    assert!(marker.data.is_empty(), "markers hold no bytes");
    assert_eq!(marker.name, name("f_end"), "names are case-folded");
}

#[test]
fn default_is_an_empty_pwad() {
    let wad = Wad::default();
    assert_eq!(wad.kind, WadKind::Pwad, "default kind");
    assert!(wad.is_empty(), "default has no lumps");
    assert_eq!(wad.len(), 0, "default length");
}

#[test]
fn round_trips_through_bytes_for_both_kinds() {
    for kind in [WadKind::Iwad, WadKind::Pwad] {
        let wad = sample(kind);
        let encoded = wad.to_bytes().expect("a small WAD serializes");
        assert_eq!(&encoded[..4], kind.magic(), "magic is first");
        let parsed = Wad::parse(&encoded).expect("the encoding parses back");
        assert_eq!(parsed, wad, "round trip is exact for {kind:?}");
    }
}

#[test]
fn empty_wad_round_trips_as_a_bare_header() {
    let encoded = Wad::default().to_bytes().expect("empty serializes");
    assert_eq!(encoded.len(), HEADER_LEN, "just the header");
    assert_eq!(
        Wad::parse(&encoded).expect("bare header parses"),
        Wad::default(),
        "empty round trip"
    );
}

#[test]
fn to_bytes_lays_data_then_the_directory_in_order() {
    let wad = sample(WadKind::Pwad);
    let encoded = wad.to_bytes().expect("serializes");
    let entries = written_entries(&encoded, wad.len());
    let mut expected = HEADER_LEN;
    for (index, lump) in wad.lumps.iter().enumerate() {
        let (filepos, size) = entries[index];
        let start = usize::try_from(filepos).expect("test files are small");
        let width = usize::try_from(size).expect("test files are small");
        assert_eq!(
            start, expected,
            "entry {index} records the running position"
        );
        assert_eq!(width, lump.size(), "entry {index} records the size");
        let end = start.checked_add(width).expect("test files are small");
        assert_eq!(
            &encoded[start..end],
            lump.data.as_ref(),
            "entry {index} payload sits at its recorded offset"
        );
        expected = end;
    }
    let directory_at = i32::from_le_bytes(encoded[8..12].try_into().expect("four bytes"));
    assert_eq!(
        usize::try_from(directory_at).expect("test files are small"),
        expected,
        "the header names the directory start"
    );
}

#[test]
fn zero_size_lumps_get_the_current_position() {
    let wad = Wad {
        kind: WadKind::Pwad,
        lumps: vec![
            Lump::marker(name("A_START")),
            Lump::new(name("DATA"), vec![b'x'; 3]),
            Lump::marker(name("A_END")),
        ],
    };
    let offsets = wad.data_offsets().expect("small WAD offsets");
    assert_eq!(offsets[0], 12, "marker before the data gets 12");
    assert_eq!(offsets[1], 12, "the payload follows at 12");
    assert_eq!(offsets[2], 15, "the end marker sits after the payload");
    let encoded = wad.to_bytes().expect("serializes");
    let entries = written_entries(&encoded, offsets.len());
    for (index, offset) in offsets.iter().enumerate() {
        let stored = u32::try_from(entries[index].0).expect("test files are small");
        assert_eq!(stored, *offset, "entry {index} was written as planned");
    }
}

#[test]
fn to_bytes_dedup_round_trips_through_parse_to_an_equal_wad() {
    let wad = duplicated();
    let (bytes, saved) = wad.to_bytes_dedup().expect("a small WAD serializes");
    assert_eq!(
        Wad::parse(&bytes).expect("shared offsets parse back"),
        wad,
        "the round trip is exact even though entries share data ranges"
    );
    assert_eq!(&bytes[..4], b"PWAD", "the magic is written as usual");
    assert_eq!(saved, 18, "the two later copies of the 9-byte payload");
}

#[test]
fn to_bytes_dedup_is_smaller_with_duplicates_and_identical_without() {
    let wad = duplicated();
    let plain = wad.to_bytes().expect("serializes");
    let (deduped, saved) = wad.to_bytes_dedup().expect("serializes");
    assert!(
        deduped.len() < plain.len(),
        "sharing the payload shrinks the file: {} vs {saved} saved",
        deduped.len()
    );
    assert_eq!(
        plain.len().saturating_sub(deduped.len()),
        saved,
        "the reported saving is the size difference"
    );
    let unique = sample(WadKind::Pwad);
    let (unique_dedup, unique_saved) = unique.to_bytes_dedup().expect("serializes");
    assert_eq!(
        unique_dedup,
        unique.to_bytes().expect("serializes"),
        "no duplicates means the plain layout byte for byte"
    );
    assert_eq!(unique_saved, 0_usize, "and nothing is saved");
}

#[test]
fn to_bytes_dedup_gives_the_twins_one_shared_offset() {
    let wad = duplicated();
    let (bytes, _saved) = wad.to_bytes_dedup().expect("serializes");
    let entries = written_entries(&bytes, wad.len());
    assert_eq!(
        entries[0].0, entries[2].0,
        "the twin shares the first offset"
    );
    assert_eq!(entries[0].0, entries[4].0, "and so does the third copy");
    assert_ne!(
        entries[0].0, entries[3].0,
        "the distinct payload sits elsewhere"
    );
    assert_eq!(
        entries[2].1, 9_i32,
        "the size field still names the whole payload"
    );
    let start = usize::try_from(entries[0].0).expect("test files are small");
    let end = start.checked_add(9_usize).expect("test files are small");
    assert_eq!(
        &bytes[start..end],
        b"same-same",
        "at the first copy's bytes"
    );
}

#[test]
fn data_offsets_match_the_written_directory() {
    let wad = sample(WadKind::Iwad);
    let offsets = wad.data_offsets().expect("offsets");
    let encoded = wad.to_bytes().expect("serializes");
    let entries = written_entries(&encoded, offsets.len());
    for (index, (entry, offset)) in entries.iter().zip(offsets.iter()).enumerate() {
        let stored = u32::try_from(entry.0).expect("test files are small");
        assert_eq!(stored, *offset, "offset {index} matches the plan");
    }
}

#[test]
fn parse_accepts_overlapping_and_shared_ranges() {
    let mut directory = Vec::new();
    directory.extend(raw_entry(12, 4, *b"FIRST\0\0\0"));
    directory.extend(raw_entry(14, 4, *b"SECOND\0\0"));
    directory.extend(raw_entry(12, 4, *b"FIRST\0\0\0"));
    directory.extend(raw_entry(18, 0, *b"-1\0\0\0\0\0\0"));
    let file = build_file(WadKind::Pwad, 4, 20, &[b"ABCDEFGH", &directory]);
    let wad = Wad::parse(&file).expect("overlaps are legal");
    assert_eq!(wad.lumps[0].data.as_ref(), b"ABCD", "first window");
    assert_eq!(wad.lumps[1].data.as_ref(), b"CDEF", "overlapping window");
    assert_eq!(wad.lumps[2].data.as_ref(), b"ABCD", "shared offset window");
    assert_eq!(wad.lumps[2].name, wad.lumps[0].name, "same name too");
    assert!(wad.lumps[3].data.is_empty(), "garbage position, zero size");
    assert_eq!(wad.lumps[3].name.raw()[..2], *b"-1", "odd bytes kept");
}

#[test]
fn parse_accepts_a_negative_filepos_on_an_empty_lump() {
    let directory = raw_entry(-1_234_567, 0, *b"MAP01\0\0\0");
    let file = build_file(WadKind::Iwad, 1, 12, &[&directory]);
    let wad = Wad::parse(&file).expect("markers may hold any filepos");
    assert_eq!(wad.lumps[0], Lump::marker(name("MAP01")), "empty marker");
}

#[test]
fn parse_accepts_a_directory_before_the_data() {
    let directory = raw_entry(28, 4, *b"TAIL\0\0\0\0");
    let file = build_file(WadKind::Pwad, 1, 12, &[&directory, b"DATA"]);
    let wad = Wad::parse(&file).expect("the directory may sit before payloads");
    assert_eq!(
        wad.lumps[0].data.as_ref(),
        b"DATA",
        "payload written after the directory"
    );
}
