//! The map topic: the binary member run and its `Doom` format, `BEHAVIOR`
//! making a block `Hexen`, `UDMF` blocks with and without an `ENDMAP`,
//! back-to-back maps, the three `GL` attachment cases and a `THINGS` run
//! without a header being no map at all.

use super::{junk, kind_at, lump, marker, name};
use crate::wad::classify::{Analysis, LumpKind, MapFormat, analyze};
use crate::wad::name::LumpName;

/// The map the analysis gave the lump at `index`.
fn map_at(analysis: &Analysis, index: usize) -> Option<LumpName> {
    analysis.lumps.get(index).and_then(|entry| entry.map)
}

#[test]
fn binary_map_block_is_doom_format() {
    let lumps = vec![
        marker("MAP01"),
        lump("THINGS", &junk()),
        lump("LINEDEFS", &junk()),
        lump("SIDEDEFS", &junk()),
        lump("VERTEXES", &junk()),
        lump("SEGS", &junk()),
        lump("SSECTORS", &junk()),
        lump("NODES", &junk()),
        lump("SECTORS", &junk()),
        lump("REJECT", &junk()),
        lump("BLOCKMAP", &junk()),
        lump("PLAYPAL", b"palettes"),
    ];
    let analysis = analyze(&lumps);
    assert_eq!(analysis.maps.len(), 1, "one map: {:?}", analysis.maps);
    let block = analysis.maps.first().expect("the block");
    assert_eq!(block.name, name("MAP01"), "the header names the map");
    assert_eq!(
        (block.header, block.end),
        (0, 11),
        "the run spans the members"
    );
    assert_eq!(block.format, MapFormat::Doom, "no BEHAVIOR means Doom");
    assert_eq!(block.gl, None, "no GL block");
    assert_eq!(kind_at(&analysis, 0), LumpKind::MapMarker, "header kind");
    assert_eq!(kind_at(&analysis, 1), LumpKind::MapData, "member kind");
    assert_eq!(
        map_at(&analysis, 0),
        Some(name("MAP01")),
        "header belongs to the map"
    );
    assert_eq!(
        map_at(&analysis, 10),
        Some(name("MAP01")),
        "last member too"
    );
    assert_eq!(
        kind_at(&analysis, 11),
        LumpKind::Palette,
        "the first non-member ends it"
    );
    assert_eq!(map_at(&analysis, 11), None, "and is not map data");
    assert!(
        analysis.warnings.is_empty(),
        "clean block: {:?}",
        analysis.warnings
    );
}

#[test]
fn behavior_lump_marks_the_hexen_format() {
    let lumps = vec![
        marker("E1M1"),
        lump("THINGS", &junk()),
        lump("SECTORS", &junk()),
        lump("BEHAVIOR", &junk()),
        lump("SPECIALS", &junk()),
    ];
    let analysis = analyze(&lumps);
    let block = analysis.maps.first().expect("one map");
    assert_eq!(block.format, MapFormat::Hexen, "BEHAVIOR means Hexen");
    assert_eq!(block.end, 4, "SPECIALS is not a binary member");
    assert_eq!(
        kind_at(&analysis, 3),
        LumpKind::MapData,
        "BEHAVIOR is map data"
    );
}

#[test]
fn udmf_block_spans_textmap_through_endmap() {
    let lumps = vec![
        marker("MAP01"),
        lump("TEXTMAP", b"map \"Doom\";"),
        lump("ZNODES", &junk()),
        lump("BEHAVIOR", &junk()),
        lump("DIALOGUE", &junk()),
        lump("REJECT", &junk()),
        lump("BLOCKMAP", &junk()),
        marker("ENDMAP"),
        lump("FLATA", &junk()),
    ];
    let analysis = analyze(&lumps);
    let block = analysis.maps.first().expect("one map");
    assert_eq!(block.format, MapFormat::Udmf, "TEXTMAP wins over BEHAVIOR");
    assert_eq!((block.header, block.end), (0, 8), "ENDMAP is included");
    assert_eq!(
        kind_at(&analysis, 2),
        LumpKind::MapData,
        "unlisted lumps are members"
    );
    assert_eq!(
        kind_at(&analysis, 7),
        LumpKind::MapData,
        "ENDMAP is map data too"
    );
    assert_eq!(map_at(&analysis, 8), None, "the run ended at ENDMAP");
    assert!(
        analysis.warnings.is_empty(),
        "clean UDMF: {:?}",
        analysis.warnings
    );
}

#[test]
fn udmf_without_endmap_runs_on_and_warns() {
    let lumps = vec![
        marker("E1M1"),
        lump("TEXTMAP", b"x"),
        lump("ZNODES", &junk()),
    ];
    let analysis = analyze(&lumps);
    let block = analysis.maps.first().expect("one map");
    assert_eq!(block.end, 3, "the block runs to the directory end");
    assert_eq!(analysis.warnings.len(), 1, "missing ENDMAP warns");
    assert!(
        analysis.warnings[0].contains("ENDMAP"),
        "the warning names ENDMAP: {}",
        analysis.warnings[0]
    );
}

#[test]
fn two_maps_back_to_back() {
    let lumps = vec![
        marker("MAP01"),
        lump("THINGS", &junk()),
        lump("SECTORS", &junk()),
        marker("MAP02"),
        lump("THINGS", &junk()),
        lump("SECTORS", &junk()),
    ];
    let analysis = analyze(&lumps);
    assert_eq!(analysis.maps.len(), 2, "both maps found");
    let (first, second) = (
        analysis.maps.first().expect("first map"),
        analysis.maps.get(1).expect("second map"),
    );
    assert_eq!((first.header, first.end), (0, 3), "first block");
    assert_eq!((second.header, second.end), (3, 6), "second block");
    assert_eq!(map_at(&analysis, 4), Some(name("MAP02")), "second THINGS");
    assert_eq!(kind_at(&analysis, 3), LumpKind::MapMarker, "second header");
}

#[test]
fn gl_block_attaches_by_name() {
    let mut lumps = vec![
        marker("MAP01"),
        lump("THINGS", &junk()),
        lump("SECTORS", &junk()),
    ];
    lumps.extend([
        marker("GL_MAP01"),
        lump("GL_VERT", &junk()),
        lump("GL_SEGS", &junk()),
        lump("GL_SSECT", &junk()),
        lump("GL_NODES", &junk()),
        lump("GL_PVS", &junk()),
        lump("PLAYPAL", b"palettes"),
    ]);
    let analysis = analyze(&lumps);
    let block = analysis.maps.first().expect("one map");
    assert_eq!(block.gl, Some((3, 9)), "GL block attached to MAP01");
    assert_eq!(
        kind_at(&analysis, 3),
        LumpKind::MapData,
        "GL header is map data"
    );
    assert_eq!(
        map_at(&analysis, 8),
        Some(name("MAP01")),
        "GL_PVS belongs to the map"
    );
    assert_eq!(map_at(&analysis, 9), None, "PLAYPAL does not");
    assert!(
        analysis.warnings.is_empty(),
        "clean GL: {:?}",
        analysis.warnings
    );
}

#[test]
fn gl_level_attaches_to_the_preceding_map() {
    let lumps = vec![
        marker("E1M1"),
        lump("THINGS", &junk()),
        marker("GL_LEVEL"),
        lump("GL_VERT", &junk()),
        lump("GL_NODES", &junk()),
        lump("SECTORS", &junk()),
    ];
    let analysis = analyze(&lumps);
    let block = analysis.maps.first().expect("one map");
    assert_eq!(block.gl, Some((2, 5)), "GL_LEVEL took the preceding map");
    assert_eq!(
        kind_at(&analysis, 4),
        LumpKind::MapData,
        "GL_NODES is map data"
    );
    assert_eq!(
        kind_at(&analysis, 5),
        LumpKind::Unknown,
        "SECTORS after the run is loose"
    );
}

#[test]
fn gl_block_without_a_matching_map_is_ignored() {
    let lumps = vec![
        marker("GL_MAP09"),
        lump("GL_VERT", &junk()),
        lump("GL_SEGS", &junk()),
        lump("END", &junk()),
    ];
    let analysis = analyze(&lumps);
    assert!(
        analysis.maps.is_empty(),
        "nothing to attach to: {:?}",
        analysis.maps
    );
    assert_eq!(
        kind_at(&analysis, 0),
        LumpKind::Empty,
        "the empty GL header falls back"
    );
    assert_eq!(
        kind_at(&analysis, 1),
        LumpKind::Unknown,
        "a loose GL lump is not map data"
    );
    assert_eq!(
        map_at(&analysis, 1),
        None,
        "no map ownership without a block"
    );
}

#[test]
fn things_without_a_header_is_not_a_map() {
    let lumps = vec![lump("THINGS", &junk()), lump("LINEDEFS", &junk())];
    let analysis = analyze(&lumps);
    assert!(analysis.maps.is_empty(), "no preceding header means no map");
    assert_eq!(
        kind_at(&analysis, 0),
        LumpKind::Unknown,
        "THINGS at index 0 is a loose lump"
    );
    assert_eq!(map_at(&analysis, 0), None, "and owns no map");
}
