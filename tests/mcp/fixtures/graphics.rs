//! Graphics fixtures.
//!
//! The palette, patch, sprite and texture lump bytes the `IWAD` fixtures
//! are dressed with, plus the texture-heavy view `IWAD`. The shared
//! primitives come from `super` and the archive writer from
//! [`super::iwads`].

use super::iwads::{blocked_view_map, grate_map, two_room_textured_map, write_iwad};
use super::{FLAT_BYTES, lump, marker, parsed};
use cyberdemon::gfx::codec::encode_png;
use cyberdemon::gfx::image::RgbaImage;
use cyberdemon::wad::Lump;
use cyberdemon::wad::map::doom::write_doom_map;
use cyberdemon::wad::textures::{
    PatchRef, TextureDef, TextureLayout, TextureLump, write_pnames, write_texture_lump,
};
use std::path::{Path, PathBuf};

/// 14 `PLAYPAL` palettes of 768 bytes; palette zero's colour `i` is
/// `(i, 255 - i, i)` and the other 13 are copies of it.
#[must_use]
#[inline]
pub fn playpal() -> Vec<u8> {
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
/// `fullbright` renders with).
///
/// Every later table steps each index eight places up the palette, so lit
/// modes read darker against the fixture palette's brightness-decreasing
/// colour ramp.
#[must_use]
#[inline]
pub fn colormap() -> Vec<u8> {
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
#[must_use]
#[inline]
pub fn patch(width: usize, height: usize, fill: u8) -> Vec<u8> {
    patch_with(width, height, fill, (0_i16, 0_i16))
}

/// The same hand-built `Doom` patch with `offsets` in its header: what
/// `get_patch` reports and writes into a `grAb` chunk.
///
/// # Panics
///
/// Panics if the fixture patch fields do not fit.
#[must_use]
#[inline]
pub fn patch_with(width: usize, height: usize, fill: u8, offsets: (i16, i16)) -> Vec<u8> {
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

/// One hand-built `Doom` patch post: `topdelta`, length, pad, the pixel
/// bytes and the trailing pad.
fn post(top: u8, pixels: &[u8]) -> Vec<u8> {
    let length = u8::try_from(pixels.len()).expect("test posts stay within one post");
    let mut bytes = vec![top, length, 0_u8];
    bytes.extend_from_slice(pixels);
    bytes.push(0_u8);
    bytes
}

/// The `GRATEP` fixture patch: `16x16` of uniform palette index 70 with
/// a `2x2` transparent hole at texels `(7,7)` and `(8,8)`. The hole
/// columns carry two posts whose gap leaves rows 7 and 8 uncovered, and
/// pixels no post covers decode transparent, exactly like the codec's
/// own hand-built tests encode clear texels.
fn grate_patch() -> Vec<u8> {
    let side = 16_usize;
    let mut columns: Vec<Vec<u8>> = Vec::new();
    for x in 0..side {
        let mut column = Vec::new();
        if (7..9).contains(&x) {
            column.extend_from_slice(&post(0, &[70_u8; 7]));
            column.extend_from_slice(&post(9, &[70_u8; 7]));
        } else {
            column.extend_from_slice(&post(0, &[70_u8; 16]));
        }
        column.push(0xFF_u8);
        columns.push(column);
    }
    let directory_end = 8_usize
        .checked_add(columns.len().checked_mul(4).expect("test width fits"))
        .expect("test directory fits");
    let mut bytes = Vec::new();
    for field in [side, side] {
        let stored = u16::try_from(field).expect("test patch fields fit");
        bytes.extend_from_slice(&stored.to_le_bytes());
    }
    bytes.extend_from_slice(&0_i16.to_le_bytes());
    bytes.extend_from_slice(&0_i16.to_le_bytes());
    let mut offset = directory_end;
    for column in &columns {
        bytes.extend_from_slice(
            &i32::try_from(offset)
                .expect("test offsets fit")
                .to_le_bytes(),
        );
        offset = offset.checked_add(column.len()).expect("test offsets fit");
    }
    for column in &columns {
        bytes.extend_from_slice(column);
    }
    bytes
}

/// The `columns` by `rows` grid of `PNAMES` index `patch_index`, tiled with
/// `step` origins `(x, y)`, in row-major record order.
///
/// # Panics
///
/// Panics if a tile origin does not fit an `i16`.
#[must_use]
#[inline]
pub fn tile(patch_index: u16, columns: usize, rows: usize, step: (i16, i16)) -> Vec<PatchRef> {
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
/// indexed plane.
///
/// Its colour is `PLAYPAL` index 64 of the fixture palette and every
/// pixel is opaque.
///
/// # Panics
///
/// Panics if the fixture `PNG` cannot be encoded.
#[must_use]
#[inline]
pub fn png64() -> Vec<u8> {
    let side = 64_usize;
    let pixels: Vec<[u8; 4]> =
        vec![[64_u8, 191, 64, 255]; side.checked_mul(side).expect("64x64 fits")];
    let img = RgbaImage::new(64, 64, pixels.concat()).expect("64x64 is a valid shape");
    encode_png(&img, None).expect("the fixture PNG encodes")
}

/// The fixture sprites region: one `TROO` frame spread over the naming
/// grammar (`A1` plain, `A2A8`/`A3A7`/`A4A6` mirrored pairs, `A5` plain),
/// an all-angles `PLAYA0`, and the malformed `BADNAME`.
///
/// All are small `Doom` patches, so every sprite lump is also readable
/// patch data.
#[must_use]
#[inline]
pub fn sprite_lumps() -> Vec<Lump> {
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
///
/// # Panics
///
/// Panics if the fixture patch names are invalid.
#[must_use]
#[inline]
pub fn pnames() -> Vec<u8> {
    write_pnames(&["WALLP1".to_owned(), "STEPP1".to_owned()])
        .expect("fixture patch names are valid")
}

/// The two wall textures the fixture `TEXTURE1` lumps define: `WALL1`
/// (64x128, `WALLP1` tiled) and `STEP1` (64x32, `STEPP1` tiled), the
/// textures [`two_room_map`] dresses with.
fn wall_textures() -> Vec<TextureDef> {
    vec![
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
    ]
}

/// The fixture `TEXTURE1`: the wall textures [`two_room_map`] dresses with.
///
/// # Panics
///
/// Panics if the fixture texture lump cannot be serialized.
#[must_use]
#[inline]
pub fn texture1() -> Vec<u8> {
    let lump = TextureLump {
        layout: TextureLayout::Doom,
        textures: wall_textures(),
    };
    write_texture_lump(&lump).expect("the fixture texture lump serializes")
}

/// The view fixture's `PNAMES`: the two wall patches plus the `GRATEP`
/// masked-middle patch, in index order. The count-sensitive tests read
/// the plain fixture's [`pnames`], so the view iwad keeps its own copy.
fn view_pnames() -> Vec<u8> {
    write_pnames(&[
        "WALLP1".to_owned(),
        "STEPP1".to_owned(),
        "GRATEP".to_owned(),
    ])
    .expect("fixture patch names are valid")
}

/// The view fixture's `TEXTURE1`: the two wall textures plus `GRATE`
/// (`16x16`, one `GRATEP` at the origin), the masked middle
/// [`grate_map`] dresses its divider line with.
fn view_texture1() -> Vec<u8> {
    let mut textures = wall_textures();
    textures.push(TextureDef {
        name: "GRATE".to_owned(),
        flags_raw: 0,
        width: 16,
        height: 16,
        patches: vec![PatchRef {
            patch: 2,
            x: 0,
            y: 0,
        }],
    });
    let lump = TextureLump {
        layout: TextureLayout::Doom,
        textures,
    };
    write_texture_lump(&lump).expect("the view fixture texture lump serializes")
}

/// The `CHECKER` fixture flat: `64x64`, left half palette index 10, right
/// half index 20.
fn checker_flat() -> Vec<u8> {
    let mut data = Vec::new();
    for _row in 0..64_usize {
        data.extend_from_slice(&[10_u8; 32]);
        data.extend_from_slice(&[20_u8; 32]);
    }
    data
}

/// The `SKY1` fixture sky: a `128x128` flat of uniform index 200.
fn sky_texture() -> Vec<u8> {
    vec![200_u8; 0x4000]
}

/// An `IWAD` for the textured-view tests: `MAP01` as
/// [`two_room_textured_map`], `MAP02` as [`grate_map`] and `MAP03` as
/// [`blocked_view_map`], plus the pictures they draw with.
///
/// The flat region holds `FLOOR1`, the two-tone `CHECKER`, the `F_SKY1`
/// stand-in and the uniform `SKY1` sky; the patch region and texture
/// records dress the walls like the plain fixture `IWAD` does and add
/// the `GRATEP` patch and `GRATE` texture the masked-middle view test
/// reads, in this fixture's own `PNAMES`/`TEXTURE1` copies. A sprites
/// region of its own carries a single `TROOA1` (`8x16` of palette index
/// 50, offsets `4`/`16`) for the view tests' imp; no other sprite name
/// exists here, which is what makes `POSS` and `PLAY` report as missing.
///
/// # Panics
///
/// Panics if the fixture cannot be serialized or written.
#[must_use]
#[inline]
pub fn iwad_view_textures(dir: &Path) -> PathBuf {
    let mut lumps = vec![lump("PLAYPAL", playpal()), lump("COLORMAP", colormap())];
    let mut map = two_room_textured_map();
    map.name = parsed("MAP01");
    lumps.extend(write_doom_map(&map).expect("the fixture map serializes"));
    lumps.extend(write_doom_map(&grate_map()).expect("the grate map serializes"));
    lumps.extend(write_doom_map(&blocked_view_map()).expect("the blocked map serializes"));
    lumps.push(marker("F_START"));
    lumps.push(lump("FLOOR1", vec![1_u8; FLAT_BYTES]));
    lumps.push(lump("CHECKER", checker_flat()));
    lumps.push(lump("F_SKY1", vec![66_u8; FLAT_BYTES]));
    lumps.push(lump("SKY1", sky_texture()));
    lumps.push(marker("F_END"));
    lumps.push(marker("P_START"));
    lumps.push(lump(
        "WALLP1",
        patch_with(16, 16, 100_u8, (-2_i16, -14_i16)),
    ));
    lumps.push(lump("STEPP1", patch(16, 8, 150_u8)));
    lumps.push(lump("GRATEP", grate_patch()));
    lumps.push(marker("P_END"));
    lumps.push(marker("S_START"));
    lumps.push(lump("TROOA1", patch_with(8, 16, 50_u8, (4_i16, 16_i16))));
    lumps.push(marker("S_END"));
    lumps.push(lump("PNAMES", view_pnames()));
    lumps.push(lump("TEXTURE1", view_texture1()));
    write_iwad(dir, "iwad-view-textures.wad", lumps)
}

/// The [`iwad_stock`] `PNAMES`: the wall patch and the sky patch, in index
/// order. `DOOM II` names its sky patches `RSKY#`, one step away from the
/// texture name the engine looks up.
///
/// # Panics
///
/// Panics if the fixture patch names are invalid.
#[must_use]
#[inline]
pub fn stock_pnames() -> Vec<u8> {
    write_pnames(&["WALLP1".to_owned(), "RSKY1".to_owned()]).expect("fixture patch names are valid")
}

/// The [`iwad_stock`] `TEXTURE1`: the `WALL1` the halls wear plus `SKY1`,
/// a `128x128` record of two `RSKY1` tiles. Nothing in the archive is a
/// lump named `SKY1`, exactly as in the stock `IWAD`.
///
/// # Panics
///
/// Panics if the stock fixture texture lump cannot be serialized.
#[must_use]
#[inline]
pub fn stock_texture1() -> Vec<u8> {
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
                name: "SKY1".to_owned(),
                flags_raw: 0,
                width: 128,
                height: 128,
                patches: vec![
                    PatchRef {
                        patch: 1,
                        x: 0,
                        y: 0,
                    },
                    PatchRef {
                        patch: 1,
                        x: 64,
                        y: 0,
                    },
                ],
            },
        ],
    };
    write_texture_lump(&lump).expect("the stock fixture texture lump serializes")
}
