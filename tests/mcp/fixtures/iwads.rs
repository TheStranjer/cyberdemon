//! `IWAD` fixtures.
//!
//! The minimal `DOOM II`, stock-shaped, duplicated-flat, Heretic, Hexen,
//! L-room, broken-map and textured-view archives, written with the
//! graphics lump builders from [`super::graphics`].

use super::graphics::{
    colormap, patch, patch_with, playpal, pnames, png64, sprite_lumps, stock_pnames,
    stock_texture1, texture1,
};
use super::{
    ENDOOM_BYTES, FLAT_BYTES, l_room_map, lump, map_block, marker, node_lumps, parsed, room, side,
    solid_line, spawn, two_room_map, vertex, write_wad,
};
use cyberdemon::wad::map::doom::write_doom_map;
use cyberdemon::wad::map::{Linedef, Map, Sector, Thing};
use cyberdemon::wad::{Lump, MapFormat, Wad, WadKind};
use std::path::{Path, PathBuf};

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
#[must_use]
#[inline]
pub fn write_iwad(dir: &Path, file: &str, lumps: Vec<Lump>) -> PathBuf {
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

/// The two-room map with its rendering upgrades: room 0's floor on the
/// two-tone `CHECKER` flat and room 1's ceiling on `F_SKY1`, for the
/// textured-plane and sky view tests.
#[must_use]
#[inline]
pub fn two_room_textured_map() -> Map {
    let mut map = two_room_map();
    map.sectors[0].floor_flat = "CHECKER".to_owned();
    map.sectors[1].ceiling_flat = "F_SKY1".to_owned();
    map
}

/// The masked-middle fixture map: two equal-height triangular rooms
/// split by a diagonal, see-through `GRATE` divider.
///
/// The hall (sector 0, light 160) holds the fixture camera at
/// `(64, 128)` facing east, straight at the divider line whose faced
/// side carries the `GRATE` middle with its `2x2` transparent hole. The
/// diagonal is neither north-south nor east-west, so `Doom`'s fake
/// contrast leaves the texels' palette indices untouched even under
/// full bright. Behind the divider lies the back room (sector 1, light
/// 192): its `FLOOR1` ceiling flat shows through the hole and its
/// `WALL1` east wall, one contrast step darker, bands below.
#[must_use]
#[inline]
pub fn grate_map() -> Map {
    Map {
        name: parsed("MAP02"),
        format: MapFormat::Doom,
        things: vec![spawn(1, 64, 128, 0)],
        vertices: vec![
            vertex(0, 0),
            vertex(0, 256),
            vertex(512, 0),
            vertex(512, 256),
        ],
        linedefs: vec![
            solid_line(0, 1, 0),
            Linedef {
                v1: 1,
                v2: 2,
                flags: 4,
                front: Some(1),
                back: Some(2),
                ..Default::default()
            },
            solid_line(2, 0, 3),
            solid_line(1, 3, 4),
            solid_line(3, 2, 5),
        ],
        sidedefs: vec![
            side("-", "-", "WALL1", 0),
            side("-", "-", "GRATE", 0),
            side("-", "-", "-", 1),
            side("-", "-", "WALL1", 0),
            side("-", "-", "WALL1", 1),
            side("-", "-", "WALL1", 1),
        ],
        sectors: vec![room(0, 128, 160, 0), room(0, 128, 192, 0)],
        other: node_lumps(),
        ..Default::default()
    }
}

/// The masked-middle fixture map with the sprite pass's occluder and
/// mouthpieces: [`two_room_textured_map`] as `MAP03`, sealed by walls.
///
/// Its two-sided step line becomes a pair of one-sided `WALL1` walls
/// closing the hall at `x = 256`, and two `POSS` former humans (sprite
/// lumps this fixture `IWAD` does not carry) stand in the back room
/// behind those walls.
///
/// The fixture camera's whole east sight line stops at the sealed step
/// line, so the `MAP01` imp and both `POSS` things dress sprite items the
/// depth test drops; the wall itself paints where the sprite would show.
///
/// # Panics
///
/// Panics if the fixture map lacks its step line's sidedefs.
#[must_use]
#[inline]
pub fn blocked_view_map() -> Map {
    let mut map = two_room_textured_map();
    map.name = parsed("MAP03");
    map.things.push(spawn(3004, 384, 64, 180));
    map.things.push(spawn(3004, 384, 192, 180));
    let hall_side = map
        .sidedefs
        .get_mut(2)
        .expect("the fixture map has a sidedef 2");
    *hall_side = side("-", "-", "WALL1", 0);
    let room_side = map
        .sidedefs
        .get_mut(3)
        .expect("the fixture map has a sidedef 3");
    *room_side = side("-", "-", "WALL1", 1);
    let seal = map
        .linedefs
        .get_mut(2)
        .expect("the fixture map has a linedef 2");
    seal.flags = 1;
    seal.back = None;
    map.linedefs.push(solid_line(1, 4, 3));
    map
}

/// One-sector `512x512` hall: `FLOOR1` floor, `ceiling` overhead, light
/// 192, `WALL1` all round and `things` standing in it. The body of the
/// [`iwad_stock`] maps, whose camera is always the centre.
fn hall_map(text: &str, ceiling: &str, things: Vec<Thing>) -> Map {
    let ring: [(i32, i32); 4] = [(0, 0), (0, 512), (512, 512), (512, 0)];
    let mut map = Map {
        name: parsed(text),
        format: MapFormat::Doom,
        things,
        sectors: vec![Sector {
            floor_height: 0,
            ceiling_height: 128,
            floor_flat: "FLOOR1".to_owned(),
            ceiling_flat: ceiling.to_owned(),
            light: 192,
            tag: 0,
            ..Default::default()
        }],
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

/// The hall's monster ring: a player start on the centre and one `TROO`
/// imp (type `3001`, facing due east) at each of the eight bearings `128`
/// units around it, so a camera on the centre looks up a different one of
/// the engine's eight rotation buckets for every one of them, the pair
/// names `A2A8`/`A3A7`/`A4A6` included.
fn imp_ring() -> Vec<Thing> {
    let spots: [(i32, i32); 8] = [
        (384, 256),
        (346, 346),
        (256, 384),
        (166, 346),
        (128, 256),
        (166, 166),
        (256, 128),
        (346, 166),
    ];
    let mut marks = vec![spawn(1, 256, 256, 0)];
    marks.extend(spots.iter().map(|&(x, y)| spawn(3001, x, y, 0)));
    marks
}

/// The [`iwad_stock`] `MAP03` things: [`imp_ring`] plus two `POSS` former
/// humans to the north-east and the south-east, the sprite this fixture
/// genuinely does without, so the two of them are exactly one problem line
/// while the ring of imps stays silent.
fn poss_pair_things() -> Vec<Thing> {
    let mut marks = imp_ring();
    marks.push(spawn(3004, 384, 346, 0));
    marks.push(spawn(3004, 384, 166, 0));
    marks
}

/// A `DOOM II`-shaped `IWAD`: the pictures a stock archive really carries,
/// in the shapes it really carries them in.
///
/// `SKY1` is a `TEXTURE1` record over the `RSKY1` patch with no lump of
/// that name anywhere, and the `TROO` standing frame is spread over the
/// five names an `IWAD` writes (`TROOA1`, `TROOA2A8`, `TROOA3A7`,
/// `TROOA4A6`, `TROOA5`, each `8x16` of palette index `50`, so rotations
/// `2`, `3` and `4` exist only as the first pair of a mirrored name).
/// Three halls of [`hall_map`]: `MAP01` under a `FLOOR1` ceiling ringed by
/// [`imp_ring`], `MAP02` the same hall under `F_SKY1`, and `MAP03` the ring
/// plus two `POSS` former humans, the sprite this fixture genuinely does
/// without.
///
/// # Panics
///
/// Panics if the fixture cannot be serialized or written.
#[must_use]
#[inline]
pub fn iwad_stock(dir: &Path) -> PathBuf {
    let mut lumps = vec![lump("PLAYPAL", playpal()), lump("COLORMAP", colormap())];
    for (text, ceiling, things) in [
        ("MAP01", "FLOOR1", imp_ring()),
        ("MAP02", "F_SKY1", vec![spawn(1, 256, 256, 0)]),
        ("MAP03", "FLOOR1", poss_pair_things()),
    ] {
        let hall = hall_map(text, ceiling, things);
        lumps.extend(write_doom_map(&hall).expect("the stock fixture map serializes"));
    }
    lumps.push(marker("F_START"));
    lumps.push(lump("FLOOR1", vec![1_u8; FLAT_BYTES]));
    lumps.push(lump("F_SKY1", vec![66_u8; FLAT_BYTES]));
    lumps.push(marker("F_END"));
    lumps.push(marker("P_START"));
    lumps.push(lump(
        "WALLP1",
        patch_with(16, 16, 100_u8, (-2_i16, -14_i16)),
    ));
    lumps.push(lump("RSKY1", patch(64, 128, 200_u8)));
    lumps.push(marker("P_END"));
    lumps.push(marker("S_START"));
    for text in ["TROOA1", "TROOA2A8", "TROOA3A7", "TROOA4A6", "TROOA5"] {
        lumps.push(lump(text, patch_with(8, 16, 50_u8, (4_i16, 16_i16))));
    }
    lumps.push(marker("S_END"));
    lumps.push(lump("PNAMES", stock_pnames()));
    lumps.push(lump("TEXTURE1", stock_texture1()));
    write_iwad(dir, "iwad-stock.wad", lumps)
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

/// A minimal `IWAD` whose only map is the concave [`l_room_map`]: a node
/// build cuts its bottom wall, appending one split vertex, for the
/// `build_nodes` tests.
///
/// # Panics
///
/// Panics if the fixture cannot be serialized or written.
#[must_use]
#[inline]
pub fn iwad_l_room(dir: &Path) -> PathBuf {
    let wad = Wad {
        kind: WadKind::Iwad,
        lumps: write_doom_map(&l_room_map()).expect("the L fixture serializes"),
    };
    write_wad(dir, "iwad-lroom.wad", &wad)
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
