//! The kind topic: the fixed names and the exact-size `ENDOOM`, the
//! `colormaps` namespace, the image and audio magics, the `DMX` and `DP`
//! sound rules and the graphics namespaces beating those weak tests while a
//! strong signature still overrides them, the namespace-implied graphic
//! kinds, [`is_doom_patch`]'s accepted and rejected headers, the text,
//! binary and empty fallbacks, the wire names of a whole analysis and the
//! empty directory.

use serde_json::json;

use super::{junk, kind_at, lump, marker, namespace_at};
use crate::wad::archive::Lump;
use crate::wad::classify::{
    LumpInfo, LumpKind, MapBlock, MapFormat, Namespace, analyze, is_doom_patch,
};

#[test]
fn fixed_names_get_their_kinds() {
    let cases: [(&str, LumpKind); 16] = [
        ("PLAYPAL", LumpKind::Palette),
        ("COLORMAP", LumpKind::Colormap),
        ("PNAMES", LumpKind::Pnames),
        ("TEXTURE1", LumpKind::TextureDefs),
        ("TEXTURE2", LumpKind::TextureDefs),
        ("DEHACKED", LumpKind::Dehacked),
        ("DECORATE", LumpKind::Decorate),
        ("ZSCRIPT", LumpKind::Zscript),
        ("MAPINFO", LumpKind::Mapinfo),
        ("ZMAPINFO", LumpKind::Mapinfo),
        ("UMAPINFO", LumpKind::Mapinfo),
        ("EMAPINFO", LumpKind::Mapinfo),
        ("DEMO1", LumpKind::Demo),
        ("DEMO2", LumpKind::Demo),
        ("DEMO3", LumpKind::Demo),
        ("DEMO4", LumpKind::Demo),
    ];
    let lumps: Vec<Lump> = cases.iter().map(|case| lump(case.0, b"hi")).collect();
    let analysis = analyze(&lumps);
    for (at, case) in cases.iter().enumerate() {
        assert_eq!(kind_at(&analysis, at), case.1, "kind of {}", case.0);
    }
}

#[test]
fn endoom_needs_the_exact_size() {
    let full = vec![lump("ENDOOM", &vec![0xA0_u8; 4000])];
    assert_eq!(kind_at(&analyze(&full), 0), LumpKind::Endoom, "4000 bytes");
    let short = vec![lump("ENDOOM", &vec![0xA0_u8; 3999])];
    assert_eq!(
        kind_at(&analyze(&short), 0),
        LumpKind::Unknown,
        "3999 bytes is not ENDOOM"
    );
}

#[test]
fn colormaps_namespace_lumps_are_colormaps() {
    let lumps = vec![marker("C_START"), lump("GAMMA2", &junk()), marker("C_END")];
    let analysis = analyze(&lumps);
    assert_eq!(
        kind_at(&analysis, 1),
        LumpKind::Colormap,
        "any lump in the namespace"
    );
    assert_eq!(
        kind_at(&analysis, 0),
        LumpKind::Marker,
        "the markers stay markers"
    );
    assert_eq!(
        namespace_at(&analysis, 1),
        Some(Namespace::Colormaps),
        "namespace set"
    );
}

/// One magic-detection case: name, payload, expected kind.
type MagicCase = (&'static str, Vec<u8>, LumpKind);

#[test]
fn content_magics_get_their_kinds() {
    let cases: Vec<MagicCase> = vec![
        ("PNGD", b"\x89PNG\r\n\x1a\n\x00\x01".to_vec(), LumpKind::Png),
        ("JPEGD", vec![0xFF, 0xD8, 0xFF, 0x00], LumpKind::Jpeg),
        ("GIFD", b"GIF89a".to_vec(), LumpKind::Gif),
        ("MUSD", b"MUS\x1Arest".to_vec(), LumpKind::Mus),
        ("MIDID", b"MThd\x00\x06".to_vec(), LumpKind::Midi),
        ("WAVD", b"RIFF$\0\0\0WAVEfmt".to_vec(), LumpKind::Wav),
        ("OGGD", b"OggS\x00".to_vec(), LumpKind::Ogg),
        ("FLACD", b"fLaC\x00".to_vec(), LumpKind::Flac),
        ("MP3D", b"ID3\x04\x00".to_vec(), LumpKind::Mp3),
        ("MP3E", vec![0xFF, 0xFB, 0x10], LumpKind::Mp3),
    ];
    let lumps: Vec<Lump> = cases.iter().map(|case| lump(case.0, &case.1)).collect();
    let analysis = analyze(&lumps);
    for (at, case) in cases.iter().enumerate() {
        assert_eq!(kind_at(&analysis, at), case.2, "magic of {}", case.0);
    }
    let riff_only = vec![lump("RIFFX", b"RIFF\x24\x00\x00\x00AIFFdata")];
    assert_eq!(
        kind_at(&analyze(&riff_only), 0),
        LumpKind::Unknown,
        "a RIFF that is not WAVE does not match"
    );
}

#[test]
fn doom_sound_and_pc_speaker_detection() {
    let lumps = vec![
        lump("DSSNARE", &[3, 0, 0x80, 0x2B, 4, 0, 0, 0, 1, 2, 3, 4]),
        lump("DPSONG", &[0, 0, 0x91, 0x8A]),
        lump("DSBAD", &[3, 0, 0, 0, 0x10, 0x27, 0, 0, 1]),
        lump("DPBAD", &[1, 0, 0x91, 0x8A]),
    ];
    let analysis = analyze(&lumps);
    assert_eq!(
        kind_at(&analysis, 0),
        LumpKind::DoomSound,
        "format 3 with a fitting count"
    );
    assert_eq!(
        kind_at(&analysis, 1),
        LumpKind::PcSpeaker,
        "DP name, zero head"
    );
    assert_eq!(
        kind_at(&analysis, 2),
        LumpKind::Unknown,
        "count past the end"
    );
    assert_eq!(
        kind_at(&analysis, 3),
        LumpKind::Unknown,
        "DP name but nonzero head"
    );
}

#[test]
fn namespaces_imply_the_graphic_kinds() {
    let cases = [
        ("F_START", "F_END", Namespace::Flats, LumpKind::Flat),
        ("S_START", "S_END", Namespace::Sprites, LumpKind::Sprite),
        ("P_START", "P_END", Namespace::Patches, LumpKind::Patch),
    ];
    for (start, end, ns, expected) in cases {
        let lumps = vec![marker(start), lump("MEMBER", &junk()), marker(end)];
        let analysis = analyze(&lumps);
        assert_eq!(
            kind_at(&analysis, 1),
            expected,
            "a loose lump inside {start}"
        );
        assert_eq!(namespace_at(&analysis, 1), Some(ns), "namespace of {start}");
    }
}

/// A 4096-byte flat whose first two bytes read as an `MPEG` frame sync:
/// palette index 255 followed by 229.
fn frame_sync_flat() -> Vec<u8> {
    let mut data = vec![0xAA_u8; 4096];
    data[0] = 0xFF;
    data[1] = 0xE5;
    data
}

/// A dark flat payload that reads as a `DMX` sound: format word `3` and a
/// sample count that fits the lump.
fn dmx_shaped_flat() -> Vec<u8> {
    vec![3_u8, 0, 0, 0, 4, 0, 0, 0, 200, 210, 220, 230]
}

/// A graphics payload that reads as `DP` PC speaker data: a zero first word.
fn speaker_shaped_flat() -> Vec<u8> {
    vec![0_u8, 0, 0x91, 0x8A]
}

/// One `F_START`/`F_END` region holding a single lump.
fn flats_lump(name: &str, data: &[u8]) -> Vec<Lump> {
    vec![marker("F_START"), lump(name, data), marker("F_END")]
}

#[test]
fn the_flats_namespace_beats_the_frame_sync() {
    let data = frame_sync_flat();
    let inside = analyze(&flats_lump("FLAT01", &data));
    assert_eq!(kind_at(&inside, 1), LumpKind::Flat, "the namespace wins");
    assert_eq!(
        namespace_at(&inside, 1),
        Some(Namespace::Flats),
        "the namespace is still reported"
    );
    let outside = analyze(&[lump("MP3ISH", &data)]);
    assert_eq!(
        kind_at(&outside, 0),
        LumpKind::Mp3,
        "the same bytes outside a namespace"
    );
}

#[test]
fn the_flats_namespace_beats_the_weak_sound_tests() {
    let dmx = dmx_shaped_flat();
    assert_eq!(
        kind_at(&analyze(&flats_lump("FLAT02", &dmx)), 1),
        LumpKind::Flat,
        "a dark flat is not a `DMX` sound"
    );
    assert_eq!(
        kind_at(&analyze(&[lump("DSISH", &dmx)]), 0),
        LumpKind::DoomSound,
        "the same bytes outside a namespace"
    );
    let speaker = speaker_shaped_flat();
    assert_eq!(
        kind_at(&analyze(&flats_lump("DPFLAT", &speaker)), 1),
        LumpKind::Flat,
        "a `DP` name in the flats namespace is not PC speaker data"
    );
    assert_eq!(
        kind_at(&analyze(&[lump("DPFLAT", &speaker)]), 0),
        LumpKind::PcSpeaker,
        "the same lump outside a namespace"
    );
}

#[test]
fn every_graphics_namespace_refuses_the_weak_magics() {
    let data = frame_sync_flat();
    let cases = [
        ("F_START", "F_END", LumpKind::Flat),
        ("S_START", "S_END", LumpKind::Sprite),
        ("P_START", "P_END", LumpKind::Patch),
    ];
    for case in cases {
        let lumps = vec![marker(case.0), lump("MEMBER", &data), marker(case.1)];
        assert_eq!(
            kind_at(&analyze(&lumps), 1),
            case.2,
            "a frame-sync payload inside {}",
            case.0
        );
    }
}

#[test]
fn a_strong_magic_still_wins_inside_the_flats_namespace() {
    let cases: Vec<MagicCase> = vec![
        ("PNGF", b"\x89PNG\r\n\x1a\n\x00\x01".to_vec(), LumpKind::Png),
        ("JPEF", vec![0xFF_u8, 0xD8, 0xFF, 0x00], LumpKind::Jpeg),
        ("GIFF", b"GIF87a".to_vec(), LumpKind::Gif),
        ("WAVF", b"RIFF$\0\0\0WAVEfmt".to_vec(), LumpKind::Wav),
        ("MIDF", b"MThd\x00\x06".to_vec(), LumpKind::Midi),
        ("MUSF", b"MUS\x1Arest".to_vec(), LumpKind::Mus),
        ("OGGF", b"OggS\x00".to_vec(), LumpKind::Ogg),
        ("FLCF", b"fLaC\x00".to_vec(), LumpKind::Flac),
        ("ID3F", b"ID3\x04\x00".to_vec(), LumpKind::Mp3),
    ];
    for case in &cases {
        let inside = analyze(&flats_lump(case.0, &case.1));
        assert_eq!(kind_at(&inside, 1), case.2, "container of {}", case.0);
        assert_eq!(
            namespace_at(&inside, 1),
            Some(Namespace::Flats),
            "{} is still reported in the flats namespace",
            case.0
        );
    }
}

/// A patch payload: header, `width` column offsets and `tail` spare
/// bytes behind the table.
fn patch_bytes(width: u16, height: u16, offsets: &[u32], tail: usize) -> Vec<u8> {
    let mut data = Vec::new();
    data.extend_from_slice(&width.to_le_bytes());
    data.extend_from_slice(&height.to_le_bytes());
    data.extend_from_slice(&0_i16.to_le_bytes());
    data.extend_from_slice(&0_i16.to_le_bytes());
    for offset in offsets {
        data.extend_from_slice(&offset.to_le_bytes());
    }
    data.append(&mut vec![0xAA_u8; tail]);
    data
}

#[test]
fn is_doom_patch_accepts_well_formed_headers() {
    assert!(
        is_doom_patch(&patch_bytes(1, 1, &[12], 1)),
        "one column pointing past the table"
    );
    assert!(
        is_doom_patch(&patch_bytes(2, 3, &[16, 16], 4)),
        "both columns at the table end"
    );
    let loose = vec![lump("TWOD", &patch_bytes(2, 2, &[16, 19], 5))];
    assert_eq!(
        kind_at(&analyze(&loose), 0),
        LumpKind::Patch,
        "the pass uses the check too"
    );
}

#[test]
fn is_doom_patch_rejects_bad_headers() {
    assert!(!is_doom_patch(&patch_bytes(0, 1, &[12], 1)), "zero width");
    assert!(
        !is_doom_patch(&patch_bytes(4097, 1, &[1], 1)),
        "width past the cap"
    );
    assert!(!is_doom_patch(&patch_bytes(1, 0, &[12], 1)), "zero height");
    assert!(
        !is_doom_patch(&patch_bytes(1, 4097, &[1], 1)),
        "height past the cap"
    );
    assert!(
        !is_doom_patch(&patch_bytes(2, 1, &[16], 0)),
        "table truncated"
    );
    assert!(
        !is_doom_patch(&patch_bytes(1, 1, &[8], 4)),
        "offset inside the table"
    );
    assert!(
        !is_doom_patch(&patch_bytes(1, 1, &[13], 0)),
        "offset at the very end"
    );
    assert!(!is_doom_patch(&[1_u8, 0, 1]), "shorter than the header");
    assert!(!is_doom_patch(&[]), "empty data");
}

#[test]
fn text_binary_and_empty_fallbacks() {
    let lumps = vec![
        lump("HELLO", b"hello world\r\n\t!"),
        lump("BINARY", &junk()),
        marker("NOTHING"),
    ];
    let analysis = analyze(&lumps);
    assert_eq!(kind_at(&analysis, 0), LumpKind::Text, "printable payload");
    assert_eq!(
        kind_at(&analysis, 1),
        LumpKind::Unknown,
        "high bytes are not text"
    );
    assert_eq!(kind_at(&analysis, 2), LumpKind::Empty, "empty non-marker");
    assert_eq!(
        analysis.lumps.first().expect("first").size,
        15,
        "size reported"
    );
    assert_eq!(
        analysis.lumps.get(2).expect("third").index,
        2,
        "index reported"
    );
}

#[test]
fn serialized_names_use_the_contract_casing() {
    let lumps = vec![marker("MAP01"), lump("THINGS", &junk()), lump("X", &[])];
    let analysis = analyze(&lumps);
    let value = serde_json::to_value(&analysis).expect("an Analysis serializes");
    assert_eq!(
        serde_json::to_value(analysis.lumps.first().expect("header info")).expect("serializes"),
        json!({
            "index": 0_u64,
            "name": "MAP01",
            "size": 0_u64,
            "kind": "map_marker",
            "namespace": null,
            "map": "MAP01",
        }),
        "header entry"
    );
    assert_eq!(
        serde_json::to_value(analysis.lumps.get(2).expect("loose")).expect("serializes"),
        json!({
            "index": 2_u64,
            "name": "X",
            "size": 0_u64,
            "kind": "empty",
            "namespace": null,
            "map": null,
        }),
        "loose empty entry"
    );
    assert_eq!(
        value["maps"][0]["format"],
        json!("doom"),
        "format is lowercase"
    );
    assert_eq!(value["maps"][0]["gl"], json!(null), "absent GL is null");
    assert_eq!(
        serde_json::to_value(MapFormat::Udmf).expect("serializes"),
        json!("udmf"),
        "udmf"
    );
    for (kind, text) in [
        (LumpKind::DoomSound, "doom_sound"),
        (LumpKind::PcSpeaker, "pc_speaker"),
        (LumpKind::TextureDefs, "texture_defs"),
        (LumpKind::MapData, "map_data"),
    ] {
        assert_eq!(
            serde_json::to_value(kind).expect("serializes"),
            json!(text),
            "serialized kind"
        );
    }
}

#[test]
fn empty_directory_analyzes_cleanly() {
    let analysis = analyze(&[]);
    assert!(analysis.lumps.is_empty(), "no entries");
    assert!(analysis.maps.is_empty(), "no maps");
    assert!(analysis.warnings.is_empty(), "no warnings");
    let empty_maps: Vec<MapBlock> = Vec::new();
    assert!(empty_maps.is_empty(), "type anchor for MapBlock");
    let first_info: Option<&LumpInfo> = analysis.lumps.first();
    assert!(first_info.is_none(), "type anchor for LumpInfo");
}

#[test]
fn map_formats_parse_from_their_wire_names() {
    for (format, text) in [
        (MapFormat::Doom, "doom"),
        (MapFormat::Hexen, "hexen"),
        (MapFormat::Udmf, "udmf"),
    ] {
        assert_eq!(format.as_str(), text, "wire name");
        assert_eq!(MapFormat::parse(text), Some(format), "lowercase");
        assert_eq!(
            MapFormat::parse(&text.to_ascii_uppercase()),
            Some(format),
            "case-insensitive"
        );
    }
    assert_eq!(MapFormat::parse("zdoom"), None, "not a format name");
    assert_eq!(MapFormat::parse(""), None, "empty is not a format");
}
