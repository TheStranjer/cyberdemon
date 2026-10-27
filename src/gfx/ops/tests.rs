//! Unit tests of the image-operation framework, all drawing their pictures
//! from the shared [`quad`] fixture.
//!
//! [`colour`] the hex forms, the integer luminance and the rounding and
//! clamping helpers; [`dispatch`] runs op lists through
//! [`apply_ops`](crate::gfx::ops::apply_ops), every variant reaching its
//! own module there, the size caps and the parameter errors; [`json`] the
//! documented `JSON` shapes, their serde defaults and each new op's road
//! through the dispatch.

mod colour;
mod dispatch;
mod json;

use crate::gfx::image::RgbaImage;

/// A 2x2 image: red, green, blue, white - all opaque.
pub(super) fn quad() -> RgbaImage {
    RgbaImage::new(
        2,
        2,
        vec![
            255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 255, 255,
        ],
    )
    .expect("2x2 RGBA8 is valid")
}
