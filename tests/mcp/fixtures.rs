//! WAD fixtures written to disk with the library itself.
//!
//! Shared by the integration tests: every helper takes the scratch
//! directory (keep the [`tempfile::TempDir`] alive for as long as the paths
//! are used) and returns the path it wrote.

use std::fs;
use std::path::{Path, PathBuf};

use cyberdemon::gfx::codec::encode_png;
use cyberdemon::gfx::image::RgbaImage;
use cyberdemon::wad::archive::write_file_atomic;
use cyberdemon::wad::bytes::Writer;
use cyberdemon::wad::map::doom::write_doom_map;
use cyberdemon::wad::map::write_map;
use cyberdemon::wad::map::{Linedef, Map, Sector, Sidedef, Thing, Vertex};
use cyberdemon::wad::textures::{
    PatchRef, TextureDef, TextureLayout, TextureLump, write_pnames, write_texture_lump,
};
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

/// 34 `COLORMAP` tables of 256 bytes: table 0 the identity (what
/// `fullbright` renders with) and every later table stepping each index
/// eight places up the palette, so lit modes read darker against the
/// fixture palette's brightness-decreasing colour ramp.
fn colormap() -> Vec<u8> {
    let mut tables = Vec::new();
    for table in 0_u8..34 {
        for index in 0_u8..=255 {
            tables.push(index.saturating_add(table.saturating_mul(8_u8)));
        }
    }
    tables
}

/// A hand-built `Doom` patch: `width` by `height`, zero offsets, one
/// full-height post per column filled with `fill`. Encoded here rather
/// than through a graphics tool so the texture fixtures stand on their
/// own.
fn patch(width: usize, height: usize, fill: u8) -> Vec<u8> {
    patch_with(width, height, fill, (0_i16, 0_i16))
}

/// The same hand-built `Doom` patch with `offsets` in its header: what
/// `get_patch` reports and writes into a `grAb` chunk.
fn patch_with(width: usize, height: usize, fill: u8, offsets: (i16, i16)) -> Vec<u8> {
    let (left, top) = offsets;
    let column_len = height.checked_add(5_usize).expect("test column fits");
    let directory_end = 8_usize
        .checked_add(width.checked_mul(4).expect("test width fits"))
        .expect("test directory fits");
    let mut bytes = Vec::new();
    for field in [width, height] {
        let stored = u16::try_from(field).expect("test patch fields fit");
        bytes.extend_from_slice(&stored.to_le_bytes());
    }
    bytes.extend_from_slice(&left.to_le_bytes());
    bytes.extend_from_slice(&top.to_le_bytes());
    for column in 0..width {
        let step = column.checked_mul(column_len).expect("test offsets fit");
        let offset = directory_end.checked_add(step).expect("test offsets fit");
        bytes.extend_from_slice(
            &i32::try_from(offset)
                .expect("test offsets fit")
                .to_le_bytes(),
        );
    }
    let post_len = u8::try_from(height).expect("test patches are at most 255 tall");
    for _column in 0..width {
        bytes.push(0_u8);
        bytes.push(post_len);
        bytes.push(0_u8);
        bytes.extend_from_slice(&vec![fill; height]);
        bytes.push(0_u8);
        bytes.push(0xFF_u8);
    }
    bytes
}

/// The `columns` by `rows` grid of `PNAMES` index `patch_index`, tiled with
/// `step` origins `(x, y)`, in row-major record order.
fn tile(patch_index: u16, columns: usize, rows: usize, step: (i16, i16)) -> Vec<PatchRef> {
    let mut placements = Vec::new();
    for row in 0..rows {
        for column in 0..columns {
            placements.push(PatchRef {
                patch: patch_index,
                x: i16::try_from(column)
                    .expect("test grids fit")
                    .checked_mul(step.0)
                    .expect("test origins fit"),
                y: i16::try_from(row)
                    .expect("test grids fit")
                    .checked_mul(step.1)
                    .expect("test origins fit"),
            });
        }
    }
    placements
}

/// A 64x64 `RGBA` `PNG`, encoded through the library codec, for the
/// `PNGFLAT` fixture flat: a flat whose bytes are `PNG` rather than a raw
/// indexed plane. Its colour is `PLAYPAL` index 64 of the fixture palette
/// and every pixel is opaque.
fn png64() -> Vec<u8> {
    let side = 64_usize;
    let pixels: Vec<[u8; 4]> =
        vec![[64_u8, 191, 64, 255]; side.checked_mul(side).expect("64x64 fits")];
    let img = RgbaImage::new(64, 64, pixels.concat()).expect("64x64 is a valid shape");
    encode_png(&img, None).expect("the fixture PNG encodes")
}

/// The fixture sprites region: one `TROO` frame spread over the naming
/// grammar (`A1` plain, `A2A8`/`A3A7`/`A4A6` mirrored pairs, `A5` plain),
/// an all-angles `PLAYA0`, and the malformed `BADNAME`, all as small
/// `Doom` patches so every sprite lump is also readable patch data.
fn sprite_lumps() -> Vec<Lump> {
    vec![
        marker("S_START"),
        lump("TROOA1", patch(8, 8, 0x33_u8)),
        lump("TROOA2A8", patch(8, 8, 0x33_u8)),
        lump("TROOA3A7", patch(8, 8, 0x33_u8)),
        lump("TROOA4A6", patch(8, 8, 0x33_u8)),
        lump("TROOA5", patch(8, 8, 0x33_u8)),
        lump("PLAYA0", patch(16, 16, 0x44_u8)),
        lump("BADNAME", vec![b'?'; 5]),
        marker("S_END"),
    ]
}

/// The fixture `PNAMES`: the two patch lumps, in index order.
fn pnames() -> Vec<u8> {
    write_pnames(&["WALLP1".to_owned(), "STEPP1".to_owned()])
        .expect("fixture patch names are valid")
}

/// The fixture `TEXTURE1`: `WALL1` (64x128, `WALLP1` tiled) and `STEP1`
/// (64x32, `STEPP1` tiled), the textures [`two_room_map`] dresses with.
fn texture1() -> Vec<u8> {
    let lump = TextureLump {
        layout: TextureLayout::Doom,
        textures: vec![
            TextureDef {
                name: "WALL1".to_owned(),
                flags_raw: 0,
                width: 64,
                height: 128,
                patches: tile(0, 4, 8, (16_i16, 16_i16)),
            },
            TextureDef {
                name: "STEP1".to_owned(),
                flags_raw: 0,
                width: 64,
                height: 32,
                patches: tile(1, 4, 4, (16_i16, 8_i16)),
            },
        ],
    };
    write_texture_lump(&lump).expect("the fixture texture lump serializes")
}

/// Serialize `wad` into `file` inside `dir` and return the path.
fn write_wad(dir: &Path, file: &str, wad: &Wad) -> PathBuf {
    let path = dir.join(file);
    let bytes = wad.to_bytes().expect("fixtures serialize");
    write_file_atomic(&path, &bytes).expect("fixtures write");
    path
}

/// The directory of the `IWAD` fixtures: palettes, one two-room `MAP01`
/// block, one flat region with `FLOOR1` (and a second, byte-3 copy when
/// `duplicate_flat` is set) plus the `PNG` flat `PNGFLAT`, an `ENDOOM`,
/// the texture fixtures: a patches region with `WALLP1` and `STEPP1`
/// plus the `PNAMES` and `TEXTURE1` lumps describing `WALL1` and `STEP1`,
/// and a sprites region (`TROO` and `PLAY` frames plus one malformed
/// name).
fn iwad_lumps(duplicate_flat: bool) -> Vec<Lump> {
    let mut lumps = vec![lump("PLAYPAL", playpal()), lump("COLORMAP", colormap())];
    lumps.extend(map_block("MAP01"));
    lumps.push(marker("F_START"));
    lumps.push(lump("FLOOR1", vec![1_u8; FLAT_BYTES]));
    if duplicate_flat {
        lumps.push(lump("FLOOR1", vec![3_u8; FLAT_BYTES]));
    }
    lumps.push(lump("PNGFLAT", png64()));
    lumps.push(marker("F_END"));
    lumps.push(lump("ENDOOM", vec![b' '; ENDOOM_BYTES]));
    lumps.push(marker("P_START"));
    lumps.push(lump(
        "WALLP1",
        patch_with(16, 16, 100_u8, (-2_i16, -14_i16)),
    ));
    lumps.push(lump("STEPP1", patch(16, 8, 150_u8)));
    lumps.push(marker("P_END"));
    lumps.extend(sprite_lumps());
    lumps.push(lump("PNAMES", pnames()));
    lumps.push(lump("TEXTURE1", texture1()));
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

/// A minimal `DOOM II` style `IWAD`: palettes, one two-room `MAP01`
/// block, and a sprites region.
///
/// It also carries one flat region with `FLOOR1` and the `PNG` flat
/// `PNGFLAT`, an `ENDOOM`, and the two-room map's textures (`WALLP1`/
/// `STEPP1` patches, `PNAMES`, `TEXTURE1`).
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

/// An `IWAD` whose only map is [`two_room_map`] with sidedef 4's sector
/// index broken to 99: a dangling reference the map assembler must report
/// as a problem instead of failing on.
///
/// # Panics
///
/// Panics if the fixture cannot be serialized or written.
#[must_use]
#[inline]
pub fn iwad_broken_map(dir: &Path) -> PathBuf {
    let mut map = two_room_map();
    map.sidedefs
        .get_mut(4)
        .expect("the fixture map has a sidedef 4")
        .sector = 99;
    let wad = Wad {
        kind: WadKind::Iwad,
        lumps: write_doom_map(&map).expect("the fixture map serializes"),
    };
    write_wad(dir, "iwad-broken.wad", &wad)
}

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
fn map_block_in(text: &str, format: MapFormat, other: Vec<Lump>) -> Vec<Lump> {
    let mut map = two_room_map();
    map.name = parsed(text);
    map.format = format;
    map.udmf_namespace = (format == MapFormat::Udmf).then(|| "zdoom".to_owned());
    map.other = other;
    write_map(&map).expect("the fixture map serializes")
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
