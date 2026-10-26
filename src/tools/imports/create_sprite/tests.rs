//! Unit tests of `create_sprite_from_image`: [`request`] checks the three
//! naming forms, the up-front argument validation, the cleanup and height
//! rules and the wire form; [`build`] the picture pipeline, the one-edit
//! commit, the rotation warnings and the preview name.
//!
//! [`args`], [`by_name`], [`by_sprite`], [`names_of`] and [`request`]
//! build the calls the topics share, [`processed`], [`white_with_red`],
//! [`playpal`], [`state`] and [`fetched`] the pictures they run, and
//! [`code`] and [`name`] read the answers back.

mod build;
mod request;

use crate::assets::fetch::Fetched;
use crate::error::CyberError;
use crate::gfx::codec::encode_png;
use crate::gfx::image::RgbaImage;
use crate::gfx::ops::OpsReport;
use crate::session::Session;
use crate::state::AppState;
use crate::tools::imports::create_sprite::SpriteRequest;
use crate::tools::imports::processed::{ProcessParams, Processed};
use crate::wad::archive::Lump;
use crate::wad::name::LumpName;

use super::{CreateSpriteParams, parse_create, parse_sprite_names};

/// The error code of a [`CyberError`], for compact asserts.
pub(super) fn code(error: &CyberError) -> &str {
    error.code()
}

/// A parsed test name.
pub(super) fn name(text: &str) -> LumpName {
    LumpName::parse(text).expect("test names are valid")
}

/// Parameters over one `source` with nothing else set.
pub(super) fn args() -> CreateSpriteParams {
    CreateSpriteParams {
        source: "/x/y.png".to_owned(),
        name: None,
        sprite: None,
        frame: None,
        frames: None,
        rotation: None,
        mirror_rotation: None,
        remove_background: None,
        background_tolerance: None,
        background_color: None,
        trim: None,
        height: None,
        offsets: None,
        replace: false,
        shared: ProcessParams {
            ops: Vec::new(),
            dither: None,
            palette: None,
        },
    }
}

/// Arguments over one whole `name`.
pub(super) fn by_name(text: &str) -> CreateSpriteParams {
    let mut call = args();
    call.name = Some(text.to_owned());
    call
}

/// Arguments over `sprite` with its optional parts.
pub(super) fn by_sprite(text: &str) -> CreateSpriteParams {
    let mut call = args();
    call.sprite = Some(text.to_owned());
    call
}

/// The lump names one set of arguments resolves to, in order.
pub(super) fn names_of(call: &CreateSpriteParams) -> Vec<String> {
    parse_sprite_names(call)
        .expect("test namings are valid")
        .iter()
        .map(|naming| naming.name.to_string())
        .collect()
}

/// A processed picture of a fixture colour over the given shape.
pub(super) fn processed(width: u32, height: u32) -> Processed {
    let count = usize::try_from(width)
        .expect("test width")
        .checked_mul(usize::try_from(height).expect("test height"))
        .expect("test pixels fit");
    let pixels: Vec<u8> = (0..count).flat_map(|_| [20_u8, 40, 60, 255]).collect();
    let rgba = RgbaImage::new(width, height, pixels).expect("test shape is valid");
    Processed {
        rgba,
        report: OpsReport::default(),
    }
}

/// The request over the current arguments.
pub(super) fn request(call: &CreateSpriteParams) -> SpriteRequest {
    parse_create(call).expect("test requests are valid")
}

/// A request carrying an explicit `height`.
pub(super) fn request_for_height(height: Option<u32>) -> SpriteRequest {
    let mut call = by_name("TROOA1");
    call.height = height;
    request(&call)
}

/// The refusal one automatic cleanup step reports when the picture is left
/// with no content: the typed [`CyberError::Emptied`] marker
/// [`trim`](crate::gfx::ops::alpha::trim) raises, carrying `text`.
pub(super) fn trim_error(text: &str) -> CyberError {
    CyberError::Emptied(Box::new(CyberError::invalid(text)))
}

/// Any other step failure: a plain `invalid_argument` the automatic rule
/// has nothing to reword.
pub(super) fn step_error(text: &str) -> CyberError {
    CyberError::invalid(text)
}

/// The `white_with_red` raster's pixel at `(x, y)`: red inside the
/// `10 x 20` block, white outside.
pub(super) fn figure_pixel(x: u32, y: u32) -> [u8; 4] {
    let in_block = (10_u32..20_u32).contains(&x) && (20_u32..40_u32).contains(&y);
    if in_block {
        [255_u8, 0, 0, 255]
    } else {
        [255_u8, 255, 255, 255]
    }
}

/// A `40 x 60` raster: white around a solid `10 x 20` red block.
pub(super) fn white_with_red() -> RgbaImage {
    let pixels: Vec<u8> = (0_u32..60_u32)
        .flat_map(|y| (0_u32..40_u32).map(move |x| figure_pixel(x, y)))
        .flatten()
        .collect();
    RgbaImage::new(40_u32, 60_u32, pixels).expect("40x60 is valid")
}

/// The fixture palette bytes: `(i, 255 - i, i)`.
pub(super) fn playpal() -> Vec<u8> {
    (0_u32..512_u32)
        .map(|position| {
            u8::try_from(position.checked_rem(256_u32).expect("non-zero"))
                .expect("remainders fit u8")
        })
        .flat_map(|index| [index, 255_u8.wrapping_sub(index), index])
        .collect()
}

/// Appends a `PLAYPAL` lump to a session's working archive.
fn seed_playpal(session: &mut Session) -> Result<(), CyberError> {
    session.edit("seed", |wad| {
        wad.lumps.push(Lump::new(name("PLAYPAL"), playpal()));
        Ok(())
    })
}

/// A state whose session carries a `PLAYPAL`.
pub(super) fn state() -> AppState {
    let state = AppState::default();
    state
        .with_session(seed_playpal)
        .expect("the seed edit runs");
    state
}

/// A fetched test picture over `image`.
pub(super) fn fetched(image: &RgbaImage) -> Fetched {
    Fetched {
        bytes: encode_png(image, None).expect("test PNG encodes"),
        filename: "art.png".to_owned(),
        mime: Some("image/png".to_owned()),
    }
}
