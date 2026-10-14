//! WAD fixtures written to disk with the library itself.
//!
//! Shared by the integration tests: every helper takes the scratch
//! directory (keep the [`tempfile::TempDir`] alive for as long as the paths
//! are used) and returns the path it wrote.

use std::fs;
use std::path::{Path, PathBuf};

use cyberdemon::wad::archive::write_file_atomic;
use cyberdemon::wad::map::doom::write_doom_map;
use cyberdemon::wad::map::{Linedef, Map, Sector, Sidedef, Thing, Vertex};
use cyberdemon::wad::{Lump, LumpName, MapFormat, Wad, WadKind};

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
    Vertex { x, y }
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

/// A map block: the two-room map encoded as binary `Doom` lumps under the
/// header `text`.
fn map_block(text: &str) -> Vec<Lump> {
    let mut map = two_room_map();
    map.name = parsed(text);
    write_doom_map(&map).expect("the fixture map serializes")
}

/// 14 `PLAYPAL` palettes of 768 bytes; palette zero's colour `i` is
/// `(i, 255 - i, i)` and the other 13 are copies of it.
fn playpal() -> Vec<u8> {
    let mut first = Vec::new();
    for index in 0_u8..=255 {
        first.extend_from_slice(&[index, 255_u8.wrapping_sub(index), index]);
    }
    let mut palettes = Vec::new();
    for _palette in 0_u8..14 {
        palettes.extend_from_slice(&first);
    }
    palettes
}

/// 34 `COLORMAP` tables of 256 bytes, every table the identity map.
fn colormap() -> Vec<u8> {
    let mut tables = Vec::new();
    for _table in 0_u8..34 {
        for index in 0_u8..=255 {
            tables.push(index);
        }
    }
    tables
}

/// Serialize `wad` into `file` inside `dir` and return the path.
fn write_wad(dir: &Path, file: &str, wad: &Wad) -> PathBuf {
    let path = dir.join(file);
    let bytes = wad.to_bytes().expect("fixtures serialize");
    write_file_atomic(&path, &bytes).expect("fixtures write");
    path
}

/// The directory of the `iwad` fixtures: palettes, one two-room `MAP01`
/// block, one flat region with `FLOOR1` (and a second, byte-3 copy when
/// `duplicate_flat` is set) and an `ENDOOM`.
fn iwad_lumps(duplicate_flat: bool) -> Vec<Lump> {
    let mut lumps = vec![lump("PLAYPAL", playpal()), lump("COLORMAP", colormap())];
    lumps.extend(map_block("MAP01"));
    lumps.push(marker("F_START"));
    lumps.push(lump("FLOOR1", vec![1_u8; FLAT_BYTES]));
    if duplicate_flat {
        lumps.push(lump("FLOOR1", vec![3_u8; FLAT_BYTES]));
    }
    lumps.push(marker("F_END"));
    lumps.push(lump("ENDOOM", vec![b' '; ENDOOM_BYTES]));
    lumps
}

/// Write `lumps` as a `Wad` of the given `file` in `dir`.
fn write_iwad(dir: &Path, file: &str, lumps: Vec<Lump>) -> PathBuf {
    let wad = Wad {
        kind: WadKind::Iwad,
        lumps,
    };
    write_wad(dir, file, &wad)
}

/// A minimal `DOOM II` style `IWAD`: palettes, one two-room `MAP01` block,
/// one flat region with `FLOOR1` and an `ENDOOM`.
///
/// # Panics
///
/// Panics if the fixture cannot be serialized or written.
#[must_use]
#[inline]
pub fn iwad(dir: &Path) -> PathBuf {
    write_iwad(dir, "iwad-doom2.wad", iwad_lumps(false))
}

/// The same `IWAD` with a second `FLOOR1` (filled with byte 3) after the
/// first in the flats region, for the occurrence-selection tests.
///
/// # Panics
///
/// Panics if the fixture cannot be serialized or written.
#[must_use]
#[inline]
pub fn iwad_duplicated(dir: &Path) -> PathBuf {
    write_iwad(dir, "iwad-floors.wad", iwad_lumps(true))
}

/// The lump directory of the Heretic `IWAD` fixture: palettes, the `TITLE`
/// and `MUS_E1M1` signature lumps Heretic detection looks for, and two
/// episode map blocks.
fn heretic_lumps() -> Vec<Lump> {
    let mut lumps = vec![
        lump("PLAYPAL", playpal()),
        lump("COLORMAP", colormap()),
        lump("TITLE", vec![b'H'; 64]),
        lump("MUS_E1M1", vec![0_u8; 16]),
    ];
    lumps.extend(map_block("E1M1"));
    lumps.extend(map_block("E2M1"));
    lumps
}

/// A minimal `Heretic` style `IWAD`: the same palettes as the `DOOM II`
/// fixture, the `E1M1` and `E2M1` episode blocks, and the `TITLE` plus
/// `MUS_E1M1` lumps that make a session detect Heretic.
///
/// # Panics
///
/// Panics if the fixture cannot be serialized or written.
#[must_use]
#[inline]
pub fn iwad_heretic(dir: &Path) -> PathBuf {
    write_iwad(dir, "iwad-heretic.wad", heretic_lumps())
}

/// The lump directory of the Hexen `IWAD` fixture: palettes, the `TITLE`
/// and `WINNOWR` signature lumps Hexen detection looks for, and one `MAP01`
/// block.
fn hexen_lumps() -> Vec<Lump> {
    let mut lumps = vec![
        lump("PLAYPAL", playpal()),
        lump("COLORMAP", colormap()),
        lump("TITLE", vec![b'H'; 64]),
        lump("WINNOWR", vec![b'W'; 64]),
    ];
    lumps.extend(map_block("MAP01"));
    lumps
}

/// A minimal `Hexen` style `IWAD`: the same palettes as the `DOOM II`
/// fixture, one `MAP01` block, and the `TITLE` plus `WINNOWR` lumps that
/// make a session detect Hexen.
///
/// # Panics
///
/// Panics if the fixture cannot be serialized or written.
#[must_use]
#[inline]
pub fn iwad_hexen(dir: &Path) -> PathBuf {
    write_iwad(dir, "iwad-hexen.wad", hexen_lumps())
}

/// A `PWAD` overriding `FLOOR1`, adding a `DEHACKED` global and a two-room
/// `MAP02` block.
///
/// # Panics
///
/// Panics if the fixture cannot be serialized or written.
#[must_use]
#[inline]
pub fn pwad(dir: &Path) -> PathBuf {
    let mut lumps = vec![
        marker("FF_START"),
        lump("FLOOR1", vec![2_u8; FLAT_BYTES]),
        marker("FF_END"),
        lump("DEHACKED", b"Thing 1 (Troop)\n".to_vec()),
    ];
    lumps.extend(map_block("MAP02"));
    let wad = Wad {
        kind: WadKind::Pwad,
        lumps,
    };
    write_wad(dir, "pwad-extra.wad", &wad)
}

/// The DEHACKED text of the custom-definitions fixture: the Imp (mobj 12)
/// moves to 20000 and gets a 10-unit radius (655360 in 16.16 fixed).
const DEHACKED_CUSTOM: &str = "Thing 12 (Evil Imp)\nID # = 20000\nWidth = 655360\n";

/// The DECORATE text: a plain actor, a parent and a child that inherits
/// the parent's sprite and radius; the comment and string braces must not
/// confuse the parser.
const DECORATE_CUSTOM: &str = "// a comment with an open brace {\n\
                               actor PlasmaLamp 20010 {\n  Radius 24\n  States {\n    \
                               Spawn: PLSM A 10 A_Look(\"} closed\")\n  }\n}\n\
                               actor LampBase : Actor {\n  radius 16.5\n  States {\n    \
                               Spawn: LAMP B 5\n  }\n}\n\
                               actor DerivedLamp : LampBase 20011 { }\n";

/// The ZSCRIPT text includes the `ZEXTRA` lump for the real class.
const ZSCRIPT_CUSTOM: &str = "#include \"ZEXTRA\"\n";

/// The included class text.
const ZEXTRA_CUSTOM: &str = "class Torch : Actor { Default { Radius 10; } States { Spawn: TORC \
                             D 3 } }\n";

/// The MAPINFO text with unrelated blocks around the `DoomEdNums` block.
const MAPINFO_CUSTOM: &str = "GameInfo { sky1 = SKY1 }\nDoomEdNums {\n  20012 = LampBase\n  \
                              20013 = NotAClass\n}\n";

/// The ZMAPINFO text maps the included `ZScript` class.
const ZMAPINFO_CUSTOM: &str = "DoomEdNums {\n  12345 = Torch\n}\n";

/// A `PWAD` carrying the custom-definitions fixture: a `DEHACKED` move of
/// the Imp, `DECORATE` actors with inheritance, a `ZSCRIPT` that includes
/// `ZEXTRA`, and `MAPINFO`/`ZMAPINFO` `DoomEdNums` blocks.
///
/// # Panics
///
/// Panics if the fixture cannot be serialized or written.
#[must_use]
#[inline]
pub fn pwad_custom_defs(dir: &Path) -> PathBuf {
    let wad = Wad {
        kind: WadKind::Pwad,
        lumps: vec![
            lump("DEHACKED", DEHACKED_CUSTOM.as_bytes().to_vec()),
            lump("DECORATE", DECORATE_CUSTOM.as_bytes().to_vec()),
            lump("ZSCRIPT", ZSCRIPT_CUSTOM.as_bytes().to_vec()),
            lump("ZEXTRA", ZEXTRA_CUSTOM.as_bytes().to_vec()),
            lump("MAPINFO", MAPINFO_CUSTOM.as_bytes().to_vec()),
            lump("ZMAPINFO", ZMAPINFO_CUSTOM.as_bytes().to_vec()),
        ],
    };
    write_wad(dir, "pwad-custom.wad", &wad)
}

/// A file that is not a `WAD` at all, for `format_error` paths.
///
/// # Panics
///
/// Panics if the file cannot be written.
#[must_use]
#[inline]
pub fn not_a_wad(dir: &Path) -> PathBuf {
    let path = dir.join("notes.txt");
    fs::write(&path, b"this is a text file, not a WAD").expect("fixtures write");
    path
}
