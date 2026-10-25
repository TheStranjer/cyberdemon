//! Unit tests of `create_flat_from_image`: [`request`] checks the
//! up-front argument validation, the square sides, the role rule and the
//! warnings those arguments produce, beside the documented wire form;
//! [`build`] the picture pipeline, the collision rule, the one-edit
//! commit, the theme override and the whole run.
//!
//! [`args`] and [`picture`] build the calls and rasters the topics share,
//! [`code`] and [`name`] read the answers back.

mod build;
mod request;

use crate::error::CyberError;
use crate::gfx::codec::encode_png;
use crate::gfx::image::RgbaImage;
use crate::tools::imports::create_flat::CreateFlatParams;
use crate::tools::imports::processed::ProcessParams;
use crate::wad::name::LumpName;

/// The error code of a [`CyberError`], for compact asserts.
pub(super) fn code(error: &CyberError) -> &str {
    error.code()
}

/// A parsed test name.
pub(super) fn name(text: &str) -> LumpName {
    LumpName::parse(text).expect("test names are valid")
}

/// Parameters over `source`/`name` with everything else default.
pub(super) fn args() -> CreateFlatParams {
    CreateFlatParams {
        source: "/x/y.png".to_owned(),
        name: "MYFLAT".to_owned(),
        fit: None,
        size: None,
        tileable: true,
        assign_role: None,
        replace: false,
        shared: ProcessParams {
            ops: Vec::new(),
            dither: None,
            palette: None,
        },
    }
}

/// A `width` x `height` opaque test picture of ramp colours.
fn picture(width: u32, height: u32) -> RgbaImage {
    let count = usize::try_from(width)
        .expect("test width")
        .checked_mul(usize::try_from(height).expect("test height"))
        .expect("test pixels fit");
    let pixels: Vec<u8> = (0..count)
        .flat_map(|pixel| {
            let slot = u8::try_from(pixel.checked_rem(256_usize).expect("non-zero"))
                .expect("remainders fit u8");
            [slot, 255_u8.wrapping_sub(slot), slot, 255_u8]
        })
        .collect();
    RgbaImage::new(width, height, pixels).expect("test shape is valid")
}

/// The PNG bytes of [`picture`], what every fetch and process reads.
pub(super) fn png(width: u32, height: u32) -> Vec<u8> {
    encode_png(&picture(width, height), None).expect("test PNG encodes")
}
