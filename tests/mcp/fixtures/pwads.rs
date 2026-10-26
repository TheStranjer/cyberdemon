//! `PWAD` fixtures.
//!
//! The override, redundant, map-format, custom-definition and empty-map
//! archives plus the not-a-`WAD` file.

use super::graphics::playpal;
use super::{ENDOOM_BYTES, FLAT_BYTES, lump, map_block, marker, parsed, two_room_map, write_wad};
use cyberdemon::wad::map::write_map;
use cyberdemon::wad::{Lump, MapFormat, Wad, WadKind};
use std::fs;
use std::path::{Path, PathBuf};

/// A `PWAD` overriding `FLOOR1`, adding a `DEHACKED` global and a two-room
/// `MAP02` block.///
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

/// The two-room map written through the model dispatcher in `format` under
/// the header `text`, carrying `other` in place of the Doom node lumps.
///
/// # Panics
///
/// Panics if the fixture map cannot be serialized.
#[must_use]
#[inline]
pub fn map_block_in(text: &str, format: MapFormat, other: Vec<Lump>) -> Vec<Lump> {
    let mut map = two_room_map();
    map.name = parsed(text);
    map.format = format;
    map.udmf_namespace = (format == MapFormat::Udmf).then(|| "zdoom".to_owned());
    map.other = other;
    write_map(&map).expect("the fixture map serializes")
}

/// A `PWAD` overriding nothing: its `PLAYPAL` and `ENDOOM` are byte-identical
/// copies of the `IWAD` fixture's, for the `optimize_wad` redundancy tests.
///
/// # Panics
///
/// Panics if the fixture cannot be serialized or written.
#[must_use]
#[inline]
pub fn pwad_redundant(dir: &Path) -> PathBuf {
    let wad = Wad {
        kind: WadKind::Pwad,
        lumps: vec![
            lump("PLAYPAL", playpal()),
            lump("ENDOOM", vec![b' '; ENDOOM_BYTES]),
        ],
    };
    write_wad(dir, "pwad-redundant.wad", &wad)
}

/// The map-format fixture's `MAP03`: the two-room map as `UDMF` in the
/// `zdoom` namespace, a non-empty `ZNODES` in place of the binary nodes,
/// followed by its `GL` node run.
fn udmf_map03() -> Vec<Lump> {
    let mut lumps = map_block_in(
        "MAP03",
        MapFormat::Udmf,
        vec![lump("ZNODES", vec![9_u8; 8])],
    );
    lumps.push(marker("GL_MAP03"));
    lumps.push(lump("GL_VERT", vec![1_u8; 8]));
    lumps.push(lump("GL_NODES", vec![2_u8; 8]));
    lumps
}

/// The map-format fixture's `MAP04`: the two-room map as binary `Hexen`
/// with a non-empty `NODES` (the writer adds the `BEHAVIOR` that makes the
/// block Hexen).
fn hexen_map04() -> Vec<Lump> {
    map_block_in(
        "MAP04",
        MapFormat::Hexen,
        vec![
            marker("SEGS"),
            marker("SSECTORS"),
            lump("NODES", vec![3_u8; 8]),
            marker("REJECT"),
            marker("BLOCKMAP"),
        ],
    )
}

/// A `PWAD` carrying the map-format fixtures: a `UDMF` `MAP03` (`zdoom`
/// namespace, non-empty `ZNODES`, attached `GL_MAP03` run) and a binary
/// `Hexen` `MAP04` (non-empty `NODES`, generated empty `BEHAVIOR`).
///
/// # Panics
///
/// Panics if the fixture cannot be serialized or written.
#[must_use]
#[inline]
pub fn pwad_map_formats(dir: &Path) -> PathBuf {
    let mut lumps = udmf_map03();
    lumps.extend(hexen_map04());
    let wad = Wad {
        kind: WadKind::Pwad,
        lumps,
    };
    write_wad(dir, "pwad-formats.wad", &wad)
}

/// The DEHACKED text of the custom-definitions fixture: the Imp (mobj 12)
/// moves to 20000 and gets a 10-unit radius (655360 in 16.16 fixed). The
/// `#` comment lines inside the block must not close it.
const DEHACKED_CUSTOM: &str = "# the Imp gets a new number\nThing 12 (Evil Imp)\n\
                               # and a tighter radius\nID # = 20000\nWidth = 655360\n";

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

/// A `PWAD` whose only map is a wholly empty `Doom` block: every member
/// lump present but zero-length, so the map has no vertices for a render
/// to frame.
///
/// # Panics
///
/// Panics if the fixture cannot be serialized or written.
#[must_use]
#[inline]
pub fn pwad_empty_map(dir: &Path) -> PathBuf {
    let lumps = vec![
        marker("MAP01"),
        marker("THINGS"),
        marker("LINEDEFS"),
        marker("SIDEDEFS"),
        marker("VERTEXES"),
        marker("SECTORS"),
        marker("SEGS"),
        marker("SSECTORS"),
        marker("NODES"),
        marker("REJECT"),
        marker("BLOCKMAP"),
    ];
    let wad = Wad {
        kind: WadKind::Pwad,
        lumps,
    };
    write_wad(dir, "pwad-empty-map.wad", &wad)
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
