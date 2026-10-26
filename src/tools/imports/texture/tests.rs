//! Unit tests of `create_texture_from_image`: [`size`] checks the two
//! edge rules and the nearest-`VALID_WIDTHS` names they offer, the size
//! rule with its aspect-derived and fallback edges, the up-front argument
//! validation, the role word and the documented wire form; [`plan`] the
//! collision rules, the one edit that lands patch, `PNAMES` and the
//! definition and undoes them together, the automatic shape ops, the
//! theme override, the composited preview and the whole build.
//!
//! [`args`], [`sized`] and [`request_for`] build the calls the topics
//! share, [`picture`] the drawings they run, [`session`] the archive they
//! plan against, [`make_built`] the picture they commit, [`theme_state`]
//! and [`seed_palette_layer`] the session they read back and [`code`],
//! [`name`] and [`assert_chosen`] the answers.

mod plan;
mod size;

use std::path::PathBuf;

use crate::error::CyberError;
use crate::gfx::image::RgbaImage;
use crate::session::{Layer, LayerRole, Session};
use crate::state::AppState;
use crate::themes::Role;
use crate::tools::imports::processed::ProcessParams;
use crate::wad::archive::{Lump, Wad, WadKind};
use crate::wad::name::LumpName;
use crate::wad::textures::{
    PatchRef, TextureDef, TextureLayout, TextureLump, write_pnames, write_texture_lump,
};

use super::build::Built;
use super::size::choose_size;
use super::{CreateRequest, CreateTextureParams, TextureSizeParams, parse_create};

/// The error code of a [`CyberError`], for compact asserts.
pub(super) fn code(error: &CyberError) -> &str {
    error.code()
}

/// A parsed test name.
pub(super) fn name(text: &str) -> LumpName {
    LumpName::parse(text).expect("test names are valid")
}

/// A `width` x `height` opaque test picture.
pub(super) fn picture(width: u32, height: u32) -> RgbaImage {
    let count = usize::try_from(width)
        .expect("test width")
        .checked_mul(usize::try_from(height).expect("test height"))
        .expect("test pixels fit");
    let pixels: Vec<u8> = (0..count).flat_map(|_| [20_u8, 40, 60, 255]).collect();
    RgbaImage::new(width, height, pixels).expect("test shape is valid")
}

/// Parameters over `source`/`name` with the given size fields.
pub(super) fn args(width: Option<u32>, height: Option<u32>) -> CreateTextureParams {
    CreateTextureParams {
        source: "/x/y.png".to_owned(),
        name: "MYTEX".to_owned(),
        patch_name: None,
        target: None,
        assign_role: None,
        replace: false,
        size: TextureSizeParams {
            width,
            height,
            fit: None,
            tileable: false,
        },
        shared: ProcessParams {
            ops: Vec::new(),
            dither: None,
            palette: None,
        },
    }
}

/// A request over explicit dimensions only.
pub(super) fn sized(width: Option<u32>, height: Option<u32>) -> CreateRequest {
    parse_create(&args(width, height)).expect("test requests are valid")
}

/// A request naming both halves explicitly.
pub(super) fn request_for(texture: &str, patch: &str) -> CreateRequest {
    let mut call = args(None, None);
    call.name = texture.to_owned();
    call.patch_name = Some(patch.to_owned());
    parse_create(&call).expect("test requests are valid")
}

/// Assert the size rule over a `picture` answers `(width, height)`.
pub(super) fn assert_chosen(request: &CreateRequest, image: &RgbaImage, edges: (u32, u32)) {
    let chosen = choose_size(request, image);
    assert_eq!((chosen.width, chosen.height), edges, "the chosen size");
}

/// Fixture-palette bytes: two entries of `(i, 255 - i, i)`.
pub(super) fn playpal() -> Vec<u8> {
    (0_u32..512_u32)
        .map(|position| {
            u8::try_from(position.checked_rem(256_u32).expect("non-zero"))
                .expect("remainders fit u8")
        })
        .flat_map(|index| [index, 255_u8.wrapping_sub(index), index])
        .collect()
}

/// A `PNAMES` lump listing the fixture pair.
pub(super) fn pnames_lump() -> Lump {
    Lump::new(
        name("PNAMES"),
        write_pnames(&["WALLP1".to_owned(), "STEPP1".to_owned()]).expect("test names are valid"),
    )
}

/// A `TEXTURE1` lump defining `WALL1`.
pub(super) fn texture1_lump() -> Lump {
    let lump = TextureLump {
        layout: TextureLayout::Doom,
        textures: vec![TextureDef {
            name: "WALL1".to_owned(),
            flags_raw: 0,
            width: 64,
            height: 128,
            patches: vec![PatchRef {
                patch: 0,
                x: 0,
                y: 0,
            }],
        }],
    };
    Lump::new(
        name("TEXTURE1"),
        write_texture_lump(&lump).expect("serializable"),
    )
}

/// A session on one `IWAD` layer holding the fixture texture set.
pub(super) fn session() -> Session {
    let mut session = Session::default();
    session
        .set_iwad(Layer::new(
            PathBuf::from("base.wad"),
            LayerRole::Iwad,
            Wad {
                kind: WadKind::Iwad,
                lumps: vec![
                    Lump::new(name("PLAYPAL"), playpal()),
                    pnames_lump(),
                    texture1_lump(),
                    Lump::new(name("WALLP1"), vec![b'P']),
                ],
            },
        ))
        .expect("no batch is open");
    session
}

/// A built picture with dummy bytes for the plan and commit tests.
pub(super) fn make_built(width: u32, height: u32) -> Built {
    Built {
        width,
        height,
        patch_bytes: vec![1_u8, 2, 3],
        preview_png: vec![4_u8],
        steps: Vec::new(),
        colors_used: 1,
        max_color_error: 0,
        notes: Vec::new(),
        warnings: Vec::new(),
    }
}

/// The session's override for `role` and its undo depth, as read back.
pub(super) fn theme_state(session: &Session, role: Role) -> (Option<String>, usize) {
    (
        session.texture_theme().overrides.get(&role).cloned(),
        session.undo_labels().len(),
    )
}

/// Seeds `state`'s session with a PLAYPAL-only `IWAD` layer.
pub(super) fn seed_palette_layer(state: &AppState) {
    state
        .with_session(|session| {
            session
                .set_iwad(Layer::new(
                    PathBuf::from("base.wad"),
                    LayerRole::Iwad,
                    Wad {
                        kind: WadKind::Iwad,
                        lumps: vec![Lump::new(name("PLAYPAL"), playpal())],
                    },
                ))
                .expect("no batch is open");
            Ok(())
        })
        .expect("the seed lands");
}
