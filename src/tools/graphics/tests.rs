//! Shared builders for the `graphics` tests: the parsed name, page and
//! error-code shorthand, the `Doom` header and `PNG` lump bytes the fixture
//! archive is filled with, its rows and the option and argument shapes the
//! picture and thing calls are made with.
//!
//! [`list`] checks the paging helper, the last-wins rule, the three listings
//! and the sprite grouping helpers; [`picture`] checks the image options,
//! the sprite and thing selectors, the render and offset helpers, the
//! fetched file's name and the thing definitions and preparation.

mod list;
mod picture;

use alloc::collections::BTreeSet;
use std::path::PathBuf;

use crate::error::CyberError;
use crate::gfx::codec::encode_png;
use crate::gfx::image::RgbaImage;
use crate::session::{Layer, LayerRole, Scope, Session};
use crate::tools::graphics::Page;
use crate::tools::graphics::get::GetSpriteParams;
use crate::tools::graphics::list::{is_patch_candidate, pnames_of};
use crate::tools::graphics::picture::{ImageParams, Rendered};
use crate::tools::graphics::thing_sprite::{GetThingSpriteParams, ThingRequest};
use crate::tools::lumps::rows::{LumpRow, build_rows};
use crate::wad::archive::{Lump, Wad, WadKind};
use crate::wad::name::LumpName;
use crate::wad::textures::write_pnames;

/// A valid parsed name for test data.
pub(super) fn name(text: &str) -> LumpName {
    LumpName::parse(text).expect("test names are valid")
}

/// A page for the listing calls.
pub(super) fn page(offset: usize, limit: usize) -> Page {
    Page { offset, limit }
}

/// The error code of a [`CyberError`], for compact asserts.
pub(super) fn code(error: &CyberError) -> &str {
    error.code()
}

/// A `Doom` patch header: dimensions and offsets, no columns.
pub(super) fn header(width: u16, height: u16, left: i16, top: i16) -> Vec<u8> {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&width.to_le_bytes());
    bytes.extend_from_slice(&height.to_le_bytes());
    bytes.extend_from_slice(&left.to_le_bytes());
    bytes.extend_from_slice(&top.to_le_bytes());
    bytes
}

/// A `PNG` lump of the given shape with optional `grAb` offsets.
pub(super) fn png(width: u32, height: u32, offsets: Option<(i32, i32)>) -> Vec<u8> {
    let pixels = usize::try_from(width)
        .expect("test width")
        .checked_mul(usize::try_from(height).expect("test height"))
        .expect("test pixels fit")
        .checked_mul(4)
        .expect("test pixel bytes fit");
    let img = RgbaImage::new(width, height, vec![0x40_u8; pixels]).expect("test shapes fit");
    encode_png(&img, offsets).expect("test PNGs encode")
}

/// The fixture archive: two raw flats and a `PNG` flat (with a repeated
/// `FLOOR1` to exercise last-wins), two region patches, a `PNG` patch
/// global named by `PNAMES`, and a sprites region with the mirrored
/// grammar plus one malformed name.
pub(super) fn fixture() -> Session {
    let mut session = Session::default();
    session
        .set_iwad(Layer::new(
            PathBuf::from("gfx.wad"),
            LayerRole::Iwad,
            Wad {
                kind: WadKind::Iwad,
                lumps: vec![
                    Lump::marker(name("F_START")),
                    Lump::new(name("FLOOR1"), vec![1_u8; 4_096]),
                    Lump::new(name("FLOOR1"), vec![3_u8; 8_192]),
                    Lump::new(name("PNGFLAT"), png(64, 64, None)),
                    Lump::new(name("ODDFLET"), vec![7_u8; 100]),
                    Lump::marker(name("F_END")),
                    Lump::marker(name("P_START")),
                    Lump::new(name("WALLP1"), header(16, 16, -2, -14)),
                    Lump::new(name("JUNKP1"), vec![b'x'; 40]),
                    Lump::marker(name("P_END")),
                    Lump::marker(name("S_START")),
                    Lump::new(name("TROOA1"), header(8, 8, 4, 4)),
                    Lump::new(name("TROOA2A8"), header(8, 8, 4, 4)),
                    Lump::new(name("PLAYA0"), header(16, 16, 0, 0)),
                    Lump::new(name("BADNAME"), vec![b'?'; 5]),
                    Lump::marker(name("S_END")),
                    Lump::new(name("PNGPAT"), png(4, 2, Some((3_i32, -4_i32)))),
                    Lump::new(
                        name("PNAMES"),
                        write_pnames(&[
                            "WALLP1".to_owned(),
                            "PNGPAT".to_owned(),
                            "TROOA1".to_owned(),
                        ])
                        .expect("test names are valid"),
                    ),
                ],
            },
        ))
        .expect("no batch is open");
    session
}

/// The fixture rows of the merged view.
pub(super) fn rows(session: &Session) -> Vec<LumpRow> {
    build_rows(session, Scope::Merged).expect("the fixture lists")
}

/// The row named `text` of the fixture session.
pub(super) fn row_named<'rows>(rows: &'rows [LumpRow], text: &str) -> &'rows LumpRow {
    rows.iter()
        .find(|row| row.name == name(text))
        .unwrap_or_else(|| panic!("no fixture row {text}"))
}

/// The candidate flags of the fixture's patch listings.
pub(super) fn candidates() -> BTreeSet<String> {
    let session = fixture();
    let pnames = pnames_of(&session).expect("PNAMES parses");
    rows(&session)
        .iter()
        .filter(|row| is_patch_candidate(row, &pnames))
        .map(|row| row.name.to_string())
        .collect()
}

/// The shared picture options of a test call.
pub(super) fn params(
    palette: Option<usize>,
    format: Option<&str>,
    scale: Option<u32>,
) -> ImageParams {
    ImageParams {
        palette,
        format: format.map(str::to_owned),
        scale,
    }
}

/// The `get_sprite` arguments naming one lump, one sprite, or both.
pub(super) fn sprite_params(name: Option<&str>, sprite: Option<&str>) -> GetSpriteParams {
    GetSpriteParams {
        name: name.map(str::to_owned),
        sprite: sprite.map(str::to_owned),
        frame: None,
        rotation: None,
        common: ImageParams::default(),
    }
}

/// A one-pixel render with optional drawing offsets.
pub(super) fn rendered(offsets: Option<(i32, i32)>) -> Rendered {
    let image = RgbaImage::new(1, 1, vec![0_u8; 4]).expect("1x1 is a valid shape");
    match offsets {
        Some((left, top)) => Rendered {
            image,
            left_offset: Some(left),
            top_offset: Some(top),
        },
        None => Rendered {
            image,
            left_offset: None,
            top_offset: None,
        },
    }
}

/// The `get_thing_sprite` arguments for one type and selection.
pub(super) fn thing_params(
    kind: u32,
    frame: Option<&str>,
    rotation: Option<u8>,
) -> GetThingSpriteParams {
    GetThingSpriteParams {
        r#type: kind,
        frame: frame.map(str::to_owned),
        rotation,
        common: ImageParams::default(),
    }
}

/// A direct `ThingRequest` for the session-side helpers.
pub(super) fn thing_selection(kind: u32, frame: Option<char>, rotation: u8) -> ThingRequest {
    ThingRequest {
        kind,
        frame,
        rotation,
    }
}
