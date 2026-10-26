//! Shared fixtures and helpers for the resolver's tests: the fixture
//! session with its flats, patches and sprite lumps, the lump and picture
//! builders and the texture-definition builders the topic modules build
//! on.

mod pictures;
mod textures;

use std::path::PathBuf;

use super::Request;
use crate::error::CyberError;
use crate::gfx::codec::encode_png;
use crate::gfx::image::{IndexedImage, RgbaImage};
use crate::gfx::patch::{Patch, encode_patch};
use crate::session::{Layer, LayerRole, Session};
use crate::wad::archive::{Lump, Wad, WadKind};
use crate::wad::name::LumpName;
use crate::wad::textures::{
    PatchRef, TextureDef, TextureLayout, TextureLump, write_pnames, write_texture_lump,
};

/// A parsed fixture name.
pub(super) fn name(text: &str) -> LumpName {
    LumpName::parse(text).expect("test names are valid")
}

/// A lump named `text` carrying `data`.
pub(super) fn lump(text: &str, data: Vec<u8>) -> Lump {
    Lump::new(name(text), data)
}

/// The error code of a [`CyberError`], for compact asserts.
pub(super) fn code(error: &CyberError) -> &str {
    error.code()
}

/// One opaque `RGBA` pixel of `color`.
pub(super) fn pixel(color: [u8; 3]) -> [u8; 4] {
    let [red, green, blue] = color;
    [red, green, blue, 255_u8]
}

/// A `width`x`height` `PNG` of one colour with optional `grAb` offsets.
pub(super) fn png(width: u32, height: u32, color: [u8; 3], offsets: Option<(i32, i32)>) -> Vec<u8> {
    flat_png(width, height, pixel(color), offsets)
}

/// A `width`x`height` `PNG` repeating one pixel, with optional `grAb`.
pub(super) fn flat_png(
    width: u32,
    height: u32,
    one: [u8; 4],
    offsets: Option<(i32, i32)>,
) -> Vec<u8> {
    let count = usize::try_from(width)
        .expect("test width")
        .checked_mul(usize::try_from(height).expect("test height"))
        .expect("test pixels fit");
    let mut pixels = Vec::with_capacity(count.saturating_mul(4));
    for _pixel in 0..count {
        pixels.extend_from_slice(&one);
    }
    let img = RgbaImage::new(width, height, pixels).expect("test shape is valid");
    encode_png(&img, offsets).expect("test PNGs encode")
}

/// One `PLAYPAL` entry: colour `i` is `(i, 255 - i, i)`, 14 copies.
pub(super) fn playpal() -> Vec<u8> {
    let mut entry = Vec::new();
    for index in 0_u8..=255 {
        entry.extend_from_slice(&[index, 255_u8.wrapping_sub(index), index]);
    }
    let mut palettes = Vec::new();
    for _palette in 0_u8..14 {
        palettes.extend_from_slice(&entry);
    }
    palettes
}

/// A `Doom` patch of a shape and offsets, every pixel `fill`.
pub(super) fn doom_patch(width: u32, height: u32, offsets: (i16, i16), fill: u8) -> Vec<u8> {
    let columns = usize::try_from(width).expect("test width");
    let total = columns
        .checked_mul(usize::try_from(height).expect("test height"))
        .expect("test pixels fit");
    let image = IndexedImage::new(width, height, vec![fill; total], vec![true; total])
        .expect("test shapes are valid");
    encode_patch(&Patch {
        left_offset: offsets.0,
        top_offset: offsets.1,
        image,
    })
    .expect("test patches encode")
}

/// A `Doom` patch `width` by 2 whose only opaque column is `column`.
pub(super) fn stripe(width: u32, column: u32, offsets: (i16, i16)) -> Vec<u8> {
    let columns = usize::try_from(width).expect("test width");
    let total = columns.checked_mul(2).expect("test pixels fit");
    let mut indices = vec![0_u8; total];
    let mut opaque = vec![false; total];
    for row in 0_usize..2 {
        let slot = row
            .checked_mul(columns)
            .and_then(|start| start.checked_add(usize::try_from(column).expect("test column")))
            .expect("test index fits");
        indices[slot] = 5_u8;
        opaque[slot] = true;
    }
    let image = IndexedImage::new(width, 2, indices, opaque).expect("test shape is valid");
    encode_patch(&Patch {
        left_offset: offsets.0,
        top_offset: offsets.1,
        image,
    })
    .expect("test patches encode")
}

/// Appends `extra` to the fixture session as one working-archive edit.
pub(super) fn with_extra(extra: Vec<Lump>) -> Session {
    let mut built = session();
    built
        .edit("add extras", |wad| {
            wad.lumps.extend(extra);
            Ok(())
        })
        .expect("plain append");
    built
}

/// The fixture session: a loose flat, two `FLOOR1` copies, a `PNG`
/// flat, a region patch and a loose `PNG` patch, sprite-region lumps
/// covering the direct, mirrored and all-angles grammar, and one lump
/// that is no picture at all.
pub(super) fn session() -> Session {
    let mut built = Session::default();
    built
        .set_iwad(Layer::new(
            PathBuf::from("gfx.wad"),
            LayerRole::Iwad,
            Wad {
                kind: WadKind::Iwad,
                lumps: vec![
                    lump("PLAYPAL", playpal()),
                    lump("LOOSE1", vec![9_u8; 4_096]),
                    lump("FLOOR1", vec![1_u8; 4_096]),
                    lump("PNGFLAT", png(2, 2, [64, 191, 64], None)),
                    lump("FLOOR1", vec![2_u8; 4_096]),
                    lump("WALLP1", doom_patch(4, 4, (-2, -14), 0x11)),
                    lump(
                        "PNGPAT",
                        png(3, 2, [0x11, 0x22, 0x33], Some((3_i32, -4_i32))),
                    ),
                    Lump::marker(name("S_START")),
                    lump("TROOA1", doom_patch(8, 8, (4, 0), 0x33)),
                    lump("TROOA2A8", stripe(4, 0, (1, 2))),
                    lump("PLAYA0", doom_patch(16, 16, (0, 0), 0x44)),
                    Lump::marker(name("S_END")),
                    lump("JUNK", vec![b'x'; 40]),
                ],
            },
        ))
        .expect("no batch is open");
    built
}

/// The first index of a decoded image, for "which bytes won" checks.
pub(super) fn first_index(image: &IndexedImage) -> u8 {
    *image.indices.first().expect("a decoded image holds pixels")
}

/// The request for one fixture rotation.
pub(super) fn request(sprite: &str, frame: char, rotation: u8) -> Request {
    Request::new(sprite, frame, rotation).expect("test requests are valid")
}

/// A `Doom`-layout `TEXTURE1`/`TEXTURE2` lump holding `textures`.
pub(super) fn texture_bytes(textures: Vec<TextureDef>) -> Vec<u8> {
    write_texture_lump(&TextureLump {
        layout: TextureLayout::Doom,
        textures,
    })
    .expect("test texture lumps serialize")
}

/// A texture definition named `text` placing `patches`.
pub(super) fn definition(
    text: &str,
    width: u16,
    height: u16,
    patches: Vec<PatchRef>,
) -> TextureDef {
    TextureDef {
        name: text.to_owned(),
        flags_raw: 0,
        width,
        height,
        patches,
    }
}

/// A patch placement at `x`/`y`.
pub(super) fn place(patch: u16, x: i16, y: i16) -> PatchRef {
    PatchRef { patch, x, y }
}

/// A `PNAMES` lump naming `texts`, in index order.
pub(super) fn pnames_lump(texts: &[&str]) -> Lump {
    let names: Vec<String> = texts.iter().map(|text| (*text).to_owned()).collect();
    lump(
        "PNAMES",
        write_pnames(&names).expect("test patch names are valid"),
    )
}
