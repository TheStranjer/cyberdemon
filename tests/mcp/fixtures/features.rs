//! Fixtures for the `features` tools.
//!
//! The door wardrobe patches, textures and the `IWAD` that carries them.

use super::graphics::{colormap, patch, patch_with, playpal, tile};
use super::iwads::write_iwad;
use super::{ENDOOM_BYTES, FLAT_BYTES, lump, map_block, marker};
use cyberdemon::wad::textures::{
    TextureDef, TextureLayout, TextureLump, write_pnames, write_texture_lump,
};
use std::path::{Path, PathBuf};

/// The features fixture's `PNAMES`: the two wall patches plus the door
/// patch, in index order.
fn pnames_features() -> Vec<u8> {
    write_pnames(&[
        "WALLP1".to_owned(),
        "STEPP1".to_owned(),
        "DOORP1".to_owned(),
        "SW1P1".to_owned(),
    ])
    .expect("fixture patch names are valid")
}

/// The 64x128 door face `DOOR1`, tiling the `DOORP1` patch.
fn door_texture() -> TextureDef {
    TextureDef {
        name: "DOOR1".to_owned(),
        flags_raw: 0,
        width: 64,
        height: 128,
        patches: tile(2, 4, 8, (16_i16, 16_i16)),
    }
}

/// A 16x128 track texture `name` tiling one column of the `DOORP1` patch.
fn track_texture(name: &str) -> TextureDef {
    TextureDef {
        name: name.to_owned(),
        flags_raw: 0,
        width: 16,
        height: 128,
        patches: tile(2, 1, 8, (16_i16, 16_i16)),
    }
}

/// A 32x128 switch panel `name` tiling the `SW1P1` patch.
fn switch_texture(name: &str) -> TextureDef {
    TextureDef {
        name: name.to_owned(),
        flags_raw: 0,
        width: 32,
        height: 128,
        patches: tile(3, 2, 8, (16_i16, 16_i16)),
    }
}

/// The features fixture's `TEXTURE1`: the two-room map's `WALL1`/`STEP1`
/// plus the door wardrobe the `features` tools dress with (`DOOR1` the
/// face, `DOORTRAK` the plain track, `DOORRED`/`DOORBLU`/`DOORYEL` the
/// key tracks).
fn texture1_features() -> Vec<u8> {
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
            door_texture(),
            track_texture("DOORTRAK"),
            track_texture("DOORRED"),
            track_texture("DOORBLU"),
            track_texture("DOORYEL"),
            switch_texture("SW1STRTN"),
            switch_texture("SW1EXIT"),
        ],
    };
    write_texture_lump(&lump).expect("the fixture texture lump serializes")
}

/// A minimal `DOOM II` style `IWAD` for the `features` tools: the
/// textured two-room `MAP01` plus the whole door wardrobe.
///
/// Beyond the palettes it carries the flats `FLOOR1` and `FLAT10` (what
/// the theme's `door_floor` role wants), the patches
/// `WALLP1`/`STEPP1`/`DOORP1`, and a `TEXTURE1` covering every door role
/// (`DOOR1`, `DOORTRAK`, `DOORRED`, `DOORBLU`, `DOORYEL`, `WALL1`,
/// `STEP1`).
///
/// # Panics
///
/// Panics if the fixture cannot be serialized or written.
#[must_use]
#[inline]
pub fn iwad_features(dir: &Path) -> PathBuf {
    let mut lumps = vec![lump("PLAYPAL", playpal()), lump("COLORMAP", colormap())];
    lumps.extend(map_block("MAP01"));
    lumps.push(marker("F_START"));
    lumps.push(lump("FLOOR1", vec![1_u8; FLAT_BYTES]));
    lumps.push(lump("FLAT10", vec![10_u8; FLAT_BYTES]));
    lumps.push(marker("F_END"));
    lumps.push(lump("ENDOOM", vec![b' '; ENDOOM_BYTES]));
    lumps.push(marker("P_START"));
    lumps.push(lump(
        "WALLP1",
        patch_with(16, 16, 100_u8, (-2_i16, -14_i16)),
    ));
    lumps.push(lump("STEPP1", patch(16, 8, 150_u8)));
    lumps.push(lump("DOORP1", patch(16, 16, 170_u8)));
    lumps.push(lump("SW1P1", patch(32, 16, 200_u8)));
    lumps.push(marker("P_END"));
    lumps.push(lump("PNAMES", pnames_features()));
    lumps.push(lump("TEXTURE1", texture1_features()));
    write_iwad(dir, "iwad-features.wad", lumps)
}
