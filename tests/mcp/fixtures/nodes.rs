//! Node-lump fixtures.
//!
//! The hand-built `SEGS`/`SSECTORS`/`NODES`, `XNOD` and `BLOCKMAP` bytes
//! and the `PWAD`s whose map blocks carry them.

use super::pwads::map_block_in;
use super::{lump, marker, parsed, room, side, spawn, vertex, write_wad};
use cyberdemon::wad::bytes::Writer;
use cyberdemon::wad::map::write_map;
use cyberdemon::wad::map::{Linedef, Map};
use cyberdemon::wad::{Lump, MapFormat, Wad, WadKind};
use std::path::{Path, PathBuf};

/// One vanilla `SEGS` record.
fn seg_record(w: &mut Writer, fields: (u16, u16, i16, u16, i16, i16)) {
    w.put_u16(fields.0);
    w.put_u16(fields.1);
    w.put_i16(fields.2);
    w.put_u16(fields.3);
    w.put_i16(fields.4);
    w.put_i16(fields.5);
}

/// The nodes fixture's vanilla lumps: four segs, two subsectors of two
/// segs each and one root node whose children are both leaves.
fn vanilla_node_lumps() -> Vec<Lump> {
    let mut segs = Writer::new();
    for record in [
        (0_u16, 1_u16, 0_i16, 0_u16, 0_i16, 0_i16),
        (1, 2, -1, 1, 1, 64),
        (2, 3, 0, 2, 0, 0),
        (3, 0, -0x4000, 3, 1, 32),
    ] {
        seg_record(&mut segs, record);
    }
    let mut ssectors = Writer::new();
    ssectors.put_u16(2);
    ssectors.put_u16(0);
    ssectors.put_u16(2);
    ssectors.put_u16(2);
    let mut nodes = Writer::new();
    nodes.put_i16(0);
    nodes.put_i16(0);
    nodes.put_i16(256);
    nodes.put_i16(0);
    for _field in 0_usize..8_usize {
        nodes.put_i16(256);
    }
    nodes.put_u16(0x8001);
    nodes.put_u16(0x8000);
    vec![
        lump("SEGS", segs.into_inner()),
        lump("SSECTORS", ssectors.into_inner()),
        lump("NODES", nodes.into_inner()),
    ]
}

/// The nodes fixture's `BLOCKMAP`: a `1x2` grid whose first cell holds
/// linedef 3 and whose second is empty, origins `-128`/`-256`.
fn fixture_blockmap() -> Vec<u8> {
    let mut w = Writer::new();
    w.put_i16(-128);
    w.put_i16(-256);
    w.put_u16(1);
    w.put_u16(2);
    w.put_u16(6);
    w.put_u16(9);
    w.put_u16(0);
    w.put_u16(3);
    w.put_u16(0xFFFF);
    w.put_u16(0);
    w.put_u16(0xFFFF);
    w.into_inner()
}

/// An `XNOD` extended node lump: no added vertices beyond one, two
/// subsectors of one seg, two segs reaching the added vertex and one
/// node with subsector children.
fn xnod_lump() -> Vec<u8> {
    let mut w = Writer::new();
    w.put_bytes(b"XNOD");
    w.put_u32(6);
    w.put_u32(1);
    w.put_i32(0x0001_0000);
    w.put_i32(-0x0004_0000);
    w.put_u32(2);
    w.put_u32(1);
    w.put_u32(1);
    w.put_u32(2);
    for record in [(6_u32, 0_u32, 0_u16, 0_u8), (6, 1, 1, 1)] {
        w.put_u32(record.0);
        w.put_u32(record.1);
        w.put_u16(record.2);
        w.put_u8(record.3);
    }
    w.put_u32(1);
    w.put_i16(0);
    w.put_i16(0);
    w.put_i16(64);
    w.put_i16(0);
    for _field in 0_usize..8_usize {
        w.put_i16(256);
    }
    w.put_u32(0x8000_0000);
    w.put_u32(0x8000_0001);
    w.into_inner()
}

/// The nodes fixture's `MAP05`: the two-room map with the hand-built
/// [`vanilla_node_lumps`], a two-sector reject matrix and
/// [`fixture_blockmap`].
fn nodes_map05() -> Vec<Lump> {
    let mut other = vanilla_node_lumps();
    other.push(lump("REJECT", vec![0b0000_1010_u8]));
    other.push(lump("BLOCKMAP", fixture_blockmap()));
    map_block_in("MAP05", MapFormat::Doom, other)
}

/// The nodes fixture's `MAP06`: the two-room map with a `NODES` lump of a
/// broken record size and a `BLOCKMAP` whose offsets leave the lump, for
/// the `format_error` paths.
fn malformed_map06() -> Vec<Lump> {
    map_block_in(
        "MAP06",
        MapFormat::Doom,
        vec![
            marker("SEGS"),
            marker("SSECTORS"),
            lump("NODES", b"1234567".to_vec()),
            lump("REJECT", vec![0_u8; 4]),
            lump("BLOCKMAP", b"12345678".to_vec()),
        ],
    )
}

/// The nodes fixture's `MAP07`: the two-room map as `UDMF` whose
/// `ZNODES` carries valid `XNOD` data.
fn znodes_map07() -> Vec<Lump> {
    map_block_in("MAP07", MapFormat::Udmf, vec![lump("ZNODES", xnod_lump())])
}

/// A `PWAD` carrying the nodes-inspection fixtures.
///
/// `MAP05` is a binary `Doom` block with hand-built valid
/// `SEGS`/`SSECTORS`/`NODES`, a reject matrix, a blockmap and an attached
/// `GL` run; `MAP06` carries malformed `NODES` and `BLOCKMAP`; the `UDMF`
/// `MAP07` holds a valid `XNOD` in `ZNODES`.
///
/// # Panics
///
/// Panics if the fixture cannot be serialized or written.
#[must_use]
#[inline]
pub fn pwad_nodes(dir: &Path) -> PathBuf {
    let mut lumps = nodes_map05();
    lumps.push(marker("GL_MAP05"));
    lumps.push(lump("GL_VERT", b"gNd2\x01\x02".to_vec()));
    lumps.push(lump("GL_SEGS", vec![7_u8; 4]));
    lumps.push(lump("GL_PVS", vec![8_u8; 2]));
    lumps.extend(malformed_map06());
    lumps.extend(znodes_map07());
    let wad = Wad {
        kind: WadKind::Pwad,
        lumps,
    };
    write_wad(dir, "pwad-nodes.wad", &wad)
}

/// The `MAP05` of the conversion fixture: a `Doom` family `UDMF` block whose
/// `TEXTMAP` carries fractional coordinates, unmodelled fields and a
/// `BEHAVIOR` lump — everything a binary `Doom` record cannot store.
fn lossy_udmf_map05() -> Vec<Lump> {
    let text = concat!(
        "namespace = \"doom\";",
        "vertex { x = 0.4; y = 0; }",
        "vertex { x = 128.5; y = 0; }",
        "vertex { x = 128; y = 128.25; }",
        "vertex { x = 0; y = 128; }",
        "sidedef { sector = 0; offsetx = 4.5; scalex_top = 2; }",
        "sector { texturefloor = \"FLOOR1\"; textureceiling = \"CEIL1\";",
        " heightceiling = 128.5; lightlevel = 180.5; }",
        "linedef { v1 = 0; v2 = 1; sidefront = 0; alpha = 0.5; }",
        "linedef { v1 = 1; v2 = 2; sidefront = 0; }",
        "linedef { v1 = 2; v2 = 3; sidefront = 0; }",
        "linedef { v1 = 3; v2 = 0; sidefront = 0; }",
        "thing { x = 64.5; y = 64; type = 1; height = 8.5; comment = \"start\"; }",
    );
    vec![
        marker("MAP05"),
        lump("TEXTMAP", text.as_bytes().to_vec()),
        lump("BEHAVIOR", b"ACS\0x".to_vec()),
        marker("ENDMAP"),
    ]
}

/// The `MAP06` of the conversion fixture: a small binary `Hexen` block with
/// one `Line_SetIdentification` line (special 121 carrying id 7 in `args[0]`).
fn hexen_identification_map06() -> Vec<Lump> {
    let mut line = Linedef {
        v1: 0,
        v2: 1,
        flags: 1,
        front: Some(0),
        ..Default::default()
    };
    line.special = 121_i32;
    line.args = [7_i32, 0_i32, 0_i32, 0_i32, 0_i32];
    let map = Map {
        name: parsed("MAP06"),
        format: MapFormat::Hexen,
        udmf_namespace: None,
        things: vec![spawn(1, 0, 0, 0)],
        vertices: vec![vertex(0, 0), vertex(128, 0), vertex(128, 128)],
        linedefs: vec![
            line,
            Linedef {
                v1: 1,
                v2: 2,
                flags: 1,
                front: Some(0),
                ..Default::default()
            },
            Linedef {
                v1: 2,
                v2: 0,
                flags: 1,
                front: Some(0),
                ..Default::default()
            },
        ],
        sidedefs: vec![side("-", "-", "WALL1", 0)],
        sectors: vec![room(0, 128, 160, 0)],
        other: Vec::new(),
    };
    write_map(&map).expect("the Hexen conversion fixture serializes")
}

/// A `PWAD` carrying the lossy-conversion fixtures.
///
/// The `UDMF` `Doom` family `MAP05` (fractional coordinates, unmodelled
/// fields, a `BEHAVIOR` lump) and the binary `Hexen` `MAP06` (one
/// `Line_SetIdentification` line, a generated `BEHAVIOR`).
///
/// # Panics
///
/// Panics if the fixture cannot be serialized or written.
#[must_use]
#[inline]
pub fn pwad_conversions(dir: &Path) -> PathBuf {
    let mut lumps = lossy_udmf_map05();
    lumps.extend(hexen_identification_map06());
    let wad = Wad {
        kind: WadKind::Pwad,
        lumps,
    };
    write_wad(dir, "pwad-conversions.wad", &wad)
}
