//! Unit tests of the nodes group: the readers' parsing and reporting, the
//! three builders' edits and the whole-set plan `finalize` shares.
//!
//! [`name`], [`marker`] and [`lump`] build lumps, [`seg_bytes`],
//! [`node_bytes`], [`vanilla_map`] and [`xnod_bytes`] the smallest
//! decodable runs, [`two_sectors`], [`block_session`], [`build_session`],
//! [`session_over`], [`l_room_model`], [`l_room_session`],
//! [`sidedless_session`] and [`udmf_room_session`] the sessions every topic
//! edits, [`request`] a shared call and [`working_map`], [`lump_names`],
//! [`build_params`] the reads of a finished edit.
//! [`inspect`] checks the three readers, [`build_tables`] the two side-lump
//! builders, [`build_nodes`] the tree's classic and extended layouts,
//! [`build_gl`] the `glBSP` run and [`build_all`] the combined whole-set
//! build.

mod build_all;
mod build_gl;
mod build_nodes;
mod build_tables;
mod inspect;

use std::path::PathBuf;

use crate::session::{Layer, LayerRole, Scope, Session};
use crate::tools::nodes::{BuildNodesParams, Detail, Request};
use crate::wad::archive::{Lump, Wad, WadKind};
use crate::wad::bytes::Writer;
use crate::wad::classify::{MapFormat, analyze};
use crate::wad::map::{Linedef, Map, Sector, Sidedef, Vertex, read_map, write_map};
use crate::wad::name::LumpName;

/// A valid parsed name for test data.
pub(super) fn name(text: &str) -> LumpName {
    LumpName::parse(text).expect("test names are valid")
}

/// An empty marker lump named `text`.
pub(super) fn marker(text: &str) -> Lump {
    Lump::marker(name(text))
}

/// A lump named `text` holding `payload`.
pub(super) fn lump(text: &str, payload: &[u8]) -> Lump {
    Lump::new(name(text), payload.to_vec())
}

/// One 12-byte vanilla seg record.
pub(super) fn seg_bytes() -> Vec<u8> {
    let mut w = Writer::new();
    w.put_u16(0);
    w.put_u16(1);
    w.put_i16(0);
    w.put_u16(0);
    w.put_i16(0);
    w.put_i16(0);
    w.into_inner()
}

/// One 28-byte vanilla node record with both children on subsector 0.
pub(super) fn node_bytes() -> Vec<u8> {
    let mut w = Writer::new();
    w.put_i16(0);
    w.put_i16(0);
    w.put_i16(1);
    w.put_i16(0);
    for _field in 0_usize..8_usize {
        w.put_i16(0);
    }
    w.put_u16(0x8000);
    w.put_u16(0x8000);
    w.into_inner()
}

/// The smallest complete vanilla run: one seg, one subsector, one node.
pub(super) fn vanilla_map() -> Map {
    Map {
        other: vec![
            lump("SEGS", &seg_bytes()),
            lump("SSECTORS", &[1, 0, 0, 0]),
            lump("NODES", &node_bytes()),
        ],
        ..Map::default()
    }
}

/// An `XNOD` extended run: no added vertices, one subsector of one
/// seg, one node with subsector children.
pub(super) fn xnod_bytes() -> Vec<u8> {
    let mut w = Writer::new();
    w.put_bytes(b"XNOD");
    w.put_u32(2);
    w.put_u32(0);
    w.put_u32(1);
    w.put_u32(1);
    w.put_u32(1);
    w.put_u32(0);
    w.put_u32(1);
    w.put_u16(0);
    w.put_u8(1);
    w.put_u32(1);
    for _field in 0_u32..4_u32 {
        w.put_i16(0);
    }
    for _field in 0_usize..8_usize {
        w.put_i16(0);
    }
    w.put_u32(0x8000_0000);
    w.put_u32(0x8000_0000);
    w.into_inner()
}

/// Two empty `SECTORS` records for a decodable fixture block.
pub(super) fn two_sectors() -> Vec<u8> {
    let mut sectors = Writer::new();
    for _sector in 0_u32..2_u32 {
        sectors.put_i16(0);
        sectors.put_i16(128);
        sectors.put_bytes(b"FLOOR4\0\0");
        sectors.put_bytes(b"FLOOR4\0\0");
        sectors.put_i16(160);
        sectors.put_u16(0);
        sectors.put_u16(0);
    }
    sectors.into_inner()
}

/// A merged-scope request for `map` at `detail`.
pub(super) fn request(map: Option<&str>, scope: Scope, detail: Detail) -> Request {
    Request {
        map: map.map(str::to_owned),
        scope,
        detail,
    }
}

/// A decodable `MAP01` block carrying `other` as its node lumps.
pub(super) fn block_session(other: Vec<Lump>) -> Session {
    let mut lumps = vec![
        marker("MAP01"),
        marker("THINGS"),
        marker("LINEDEFS"),
        marker("SIDEDEFS"),
        marker("VERTEXES"),
        lump("SECTORS", &two_sectors()),
    ];
    lumps.extend(other);
    session_over(lumps)
}

/// A decodable `MAP01` with real geometry: vertices `(0, 0)` and
/// `(200, 0)`, one one-sided line between them (its sidedef
/// reference dangles), two sectors, and a `REJECT` placeholder but
/// no `BLOCKMAP` lump.
pub(super) fn build_session() -> Session {
    let mut verts = Writer::new();
    for (x, y) in [(0_i16, 0_i16), (200_i16, 0_i16)] {
        verts.put_i16(x);
        verts.put_i16(y);
    }
    let mut line = Writer::new();
    line.put_u16(0);
    line.put_u16(1);
    line.put_u16(1);
    line.put_u16(0);
    line.put_u16(0);
    line.put_u16(0);
    line.put_u16(0xFFFF);
    session_over(vec![
        marker("MAP01"),
        marker("THINGS"),
        lump("LINEDEFS", &line.into_inner()),
        marker("SIDEDEFS"),
        lump("VERTEXES", &verts.into_inner()),
        lump("SECTORS", &two_sectors()),
        marker("SEGS"),
        marker("SSECTORS"),
        marker("NODES"),
        marker("REJECT"),
    ])
}

/// A base layer holding `lumps`.
pub(super) fn base(lumps: Vec<Lump>) -> Layer {
    Layer::new(
        PathBuf::from("base.wad"),
        LayerRole::Iwad,
        Wad {
            kind: WadKind::Iwad,
            lumps,
        },
    )
}

/// A session over a base layer of `lumps`.
pub(super) fn session_over(lumps: Vec<Lump>) -> Session {
    let mut session = Session::default();
    session.set_iwad(base(lumps)).expect("no batch is open");
    session
}

/// A single-sector L-shaped room as the map model, ready to encode.
pub(super) fn l_room_model() -> Map {
    let ring: [(i32, i32); 6] = [
        (0, 256),
        (128, 256),
        (128, 128),
        (256, 128),
        (256, 0),
        (0, 0),
    ];
    let mut map = Map {
        name: name("MAP01"),
        ..Map::default()
    };
    map.sectors.push(Sector::default());
    map.sidedefs.push(Sidedef {
        sector: 0,
        ..Sidedef::default()
    });
    let mut indices: Vec<usize> = Vec::new();
    for &(x, y) in &ring {
        map.vertices.push(Vertex {
            x,
            y,
            ..Default::default()
        });
        indices.push(map.vertices.len().checked_sub(1).expect("just pushed"));
    }
    let back = indices.iter().skip(1).chain(indices.iter().take(1));
    for (&from, &to) in indices.iter().zip(back) {
        map.linedefs.push(Linedef {
            v1: from,
            v2: to,
            flags: 1,
            front: Some(0),
            ..Default::default()
        });
    }
    map
}

/// A session holding one decodable L-shaped room as `MAP01`.
pub(super) fn l_room_session() -> Session {
    let lumps = write_map(&l_room_model()).expect("the L fixture encodes");
    session_over(lumps)
}

/// A session holding the L-shaped room with every line's sides cut: a
/// map that yields no segs.
pub(super) fn sidedless_session() -> Session {
    let mut map = l_room_model();
    for line in &mut map.linedefs {
        line.front = None;
        line.back = None;
    }
    session_over(write_map(&map).expect("the sidedless fixture encodes"))
}

/// `build_nodes` arguments from optional pieces.
pub(super) fn build_params(
    map: Option<&str>,
    variant: Option<&str>,
    split_cost: Option<i64>,
) -> BuildNodesParams {
    BuildNodesParams {
        map: map.map(str::to_owned),
        variant: variant.map(str::to_owned),
        split_cost,
        compress: None,
    }
}

/// The working archive's decoded `MAP01` block.
pub(super) fn working_map(session: &Session) -> Map {
    let analysis = analyze(&session.working().lumps);
    let block = analysis
        .maps
        .iter()
        .rev()
        .find(|block| block.name == name("MAP01"))
        .expect("the block was copied in");
    read_map(&session.working().lumps, block).expect("the edited block decodes")
}

/// A session holding the L-shaped room as a `UDMF` `MAP01`.
pub(super) fn udmf_room_session() -> Session {
    let mut map = l_room_model();
    map.format = MapFormat::Udmf;
    map.udmf_namespace = Some("zdoom".to_owned());
    session_over(write_map(&map).expect("the UDMF fixture encodes"))
}

/// The directory names of an archive, in order.
pub(super) fn lump_names(session: &Session) -> Vec<String> {
    session
        .working()
        .lumps
        .iter()
        .map(|lump| lump.name.to_string())
        .collect()
}
