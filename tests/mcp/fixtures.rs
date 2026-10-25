//! WAD fixtures written to disk with the library itself.
//!
//! Shared by the integration tests: every helper takes the scratch
//! directory (keep the [`tempfile::TempDir`] alive for as long as the paths
//! are used) and returns the path it wrote.
//!
//! The builders split by subject: `fixtures/graphics` dresses the archives,
//! `fixtures/iwads` and `fixtures/pwads` write whole archives,
//! `fixtures/nodes` carries the node-lump maps and `fixtures/features` the
//! door wardrobe. The primitives and archive writer below are shared by
//! all of them; every moved builder is re-exported here so the call sites
//! keep reading `fixtures::<name>`.

use cyberdemon::wad::archive::write_file_atomic;
use cyberdemon::wad::map::doom::write_doom_map;
use cyberdemon::wad::map::{Linedef, Map, Sector, Sidedef, Thing, Vertex};
use cyberdemon::wad::{Lump, LumpName, MapFormat, Wad};
use std::path::{Path, PathBuf};

/// Bytes in one flat this repo's fixtures use.
const FLAT_BYTES: usize = 4096;

/// Bytes in an `ENDOOM` screen.
const ENDOOM_BYTES: usize = 4000;

/// A lump named `text` carrying `data`.
fn lump(text: &str, data: Vec<u8>) -> Lump {
    Lump::new(parsed(text), data)
}

/// An empty marker lump named `text`.
fn marker(text: &str) -> Lump {
    Lump::marker(parsed(text))
}

/// A parsed fixture name.
fn parsed(text: &str) -> LumpName {
    LumpName::parse(text).expect("fixture names are valid")
}

/// A vertex at `x`/`y`.
fn vertex(x: i32, y: i32) -> Vertex {
    Vertex {
        x,
        y,
        ..Default::default()
    }
}

/// A fully visible spawn of `kind` at `x`/`y` facing `angle`.
fn spawn(kind: i32, x: i32, y: i32, angle: i32) -> Thing {
    Thing {
        kind,
        x,
        y,
        angle,
        flags: 7,
        ..Default::default()
    }
}

/// A blocking one-sided line from `v1` to `v2` with sidedef `front`.
fn solid_line(v1: usize, v2: usize, front: usize) -> Linedef {
    Linedef {
        v1,
        v2,
        flags: 1,
        front: Some(front),
        ..Default::default()
    }
}

/// The two-sided step line between the rooms: flags 4, sidedefs 2/3.
fn step_line() -> Linedef {
    Linedef {
        v1: 4,
        v2: 1,
        flags: 4,
        front: Some(2),
        back: Some(3),
        ..Default::default()
    }
}

/// A sidedef with the given names over `sector`.
fn side(upper: &str, lower: &str, middle: &str, sector: usize) -> Sidedef {
    Sidedef {
        upper: upper.to_owned(),
        lower: lower.to_owned(),
        middle: middle.to_owned(),
        sector,
        ..Default::default()
    }
}

/// A `FLOOR1`-floored sector with the given heights, light and tag.
fn room(floor_height: i32, ceiling_height: i32, light: i32, tag: i32) -> Sector {
    Sector {
        floor_height,
        ceiling_height,
        floor_flat: "FLOOR1".to_owned(),
        ceiling_flat: "FLOOR1".to_owned(),
        light,
        tag,
        ..Default::default()
    }
}

/// The empty generated node lumps a built map carries in [`Map::other`].
fn node_lumps() -> Vec<Lump> {
    ["SEGS", "SSECTORS", "NODES", "REJECT", "BLOCKMAP"]
        .into_iter()
        .map(marker)
        .collect()
}

/// A two-room `Doom` map: a hall (sector 0, light 160) and a raised back
/// room (sector 1, floor 32, light 192, tag 1) split by a two-sided step
/// line, with a player start and an imp.
///
/// Reused by the fixture archives and by later map tickets.
#[must_use]
#[inline]
pub fn two_room_map() -> Map {
    Map {
        name: parsed("MAP01"),
        format: MapFormat::Doom,
        things: vec![spawn(1, 64, 128, 0), spawn(3001, 384, 128, 180)],
        vertices: vec![
            vertex(0, 0),
            vertex(256, 0),
            vertex(512, 0),
            vertex(512, 256),
            vertex(256, 256),
            vertex(0, 256),
        ],
        linedefs: vec![
            solid_line(0, 5, 0),
            solid_line(5, 4, 1),
            step_line(),
            solid_line(1, 0, 4),
            solid_line(4, 3, 5),
            solid_line(3, 2, 6),
            solid_line(2, 1, 7),
        ],
        sidedefs: vec![
            side("-", "-", "WALL1", 0),
            side("-", "-", "WALL1", 0),
            side("-", "STEP1", "-", 0),
            side("-", "-", "-", 1),
            side("-", "-", "WALL1", 0),
            side("-", "-", "WALL1", 1),
            side("-", "-", "WALL1", 1),
            side("-", "-", "WALL1", 1),
        ],
        sectors: vec![room(0, 128, 160, 0), room(32, 128, 192, 1)],
        other: node_lumps(),
        ..Default::default()
    }
}

/// A single-sector L-shaped room: the concave corner makes a node build
/// cut the bottom wall at `(128, 0)`, one point no vertex sits on, so
/// `build_nodes` appends exactly one split vertex there.
#[must_use]
#[inline]
pub fn l_room_map() -> Map {
    let ring: [(i32, i32); 6] = [
        (0, 256),
        (128, 256),
        (128, 128),
        (256, 128),
        (256, 0),
        (0, 0),
    ];
    let mut map = Map {
        name: parsed("MAP01"),
        format: MapFormat::Doom,
        things: vec![spawn(1, 64, 128, 0)],
        sectors: vec![room(0, 128, 180, 0)],
        sidedefs: vec![side("-", "-", "WALL1", 0)],
        other: node_lumps(),
        ..Default::default()
    };
    let mut indices: Vec<usize> = Vec::new();
    for (index, &(x, y)) in ring.iter().enumerate() {
        map.vertices.push(vertex(x, y));
        indices.push(index);
    }
    let back = indices.iter().skip(1).chain(indices.iter().take(1));
    for (&from, &to) in indices.iter().zip(back) {
        map.linedefs.push(solid_line(from, to, 0));
    }
    map
}

/// A map block: the two-room map encoded as binary `Doom` lumps under the
/// header `text`.
fn map_block(text: &str) -> Vec<Lump> {
    let mut map = two_room_map();
    map.name = parsed(text);
    write_doom_map(&map).expect("the fixture map serializes")
}

/// Serialize `wad` into `file` inside `dir` and return the path.
fn write_wad(dir: &Path, file: &str, wad: &Wad) -> PathBuf {
    let path = dir.join(file);
    let bytes = wad.to_bytes().expect("fixtures serialize");
    write_file_atomic(&path, &bytes, true).expect("fixtures write");
    path
}

/// The `features` fixtures: the door wardrobe and its `IWAD`.
#[path = "fixtures/features.rs"]
pub mod features;
/// The graphics builders: palettes, patch and flat bytes, sprites, texture
/// lumps and the texture-heavy view `IWAD`.
#[path = "fixtures/graphics.rs"]
pub mod graphics;
/// The `IWAD` archives: the minimal `DOOM II`, stock-shaped, duplicated-
/// flat, Heretic, Hexen, L-room and broken-map fixtures.
#[path = "fixtures/iwads.rs"]
pub mod iwads;
/// The node-lump fixtures: hand-built `SEGS`/`SSECTORS`/`NODES`, `XNOD`
/// and `BLOCKMAP` bytes and the `PWAD`s that carry them.
#[path = "fixtures/nodes.rs"]
pub mod nodes;
/// The `PWAD` archives: overrides, redundant globals, map formats, custom
/// definitions, empty maps and the not-a-`WAD` file.
#[path = "fixtures/pwads.rs"]
pub mod pwads;

pub use features::iwad_features;
pub use graphics::iwad_view_textures;
pub use iwads::{
    blocked_view_map, grate_map, iwad, iwad_broken_map, iwad_duplicated, iwad_heretic, iwad_hexen,
    iwad_l_room, iwad_stock, two_room_textured_map,
};
pub use nodes::{pwad_conversions, pwad_nodes};
pub use pwads::{
    not_a_wad, pwad, pwad_custom_defs, pwad_empty_map, pwad_map_formats, pwad_redundant,
};
