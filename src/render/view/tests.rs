//! Shared builders for the column caster's tests: the raw-16.16 and
//! fraction makers, the fixture [`Edge`] builders and the solid-image,
//! palette and `COLORMAP`-table makers every topic builds on.
//!
//! [`math`] measures the exact maths: what a ray pierces, how fractions
//! order, what a camera projects, and which pitches are legal.
//! [`texturing`] reads the mapping helpers: flat and sky texels, pegged
//! texture tops, texture `u`, wrapped columns, texture keys and wall
//! directions. [`box_render`] casts a whole box map and checks, pixel by
//! pixel, where its walls, floor, ceiling, sky and background land.
//! [`masked`] follows the masked middles a two-sided divider collects and
//! how the render module's transparent pass paints them.

mod box_render;
mod masked;
mod math;
mod texturing;

use super::walk::Edge;
use super::{DIRECTION_SCALE, Fraction, ONE_16};
use crate::fixed::Angle;
use crate::gfx::image::IndexedImage;
use crate::gfx::palette::Palette;
use crate::render::light::WallDir;

/// `value` scaled into raw 16.16, for expected positions.
pub(super) fn raw(value: i64) -> i128 {
    i128::from(value).saturating_mul(ONE_16)
}

/// A raw test fraction by value (denominators are positive).
pub(super) fn frac(n: i128, d: i128) -> Fraction {
    Fraction::new(n, d).expect("test denominators are non-zero")
}

/// A unit-length east-pointing column direction.
pub(super) fn east() -> (i128, i128) {
    (DIRECTION_SCALE, 0_i128)
}

/// A prepared horizontal edge from `y0` `span` units east of `x0`.
pub(super) fn horizontal(x0: i64, y0: i64, span: i64) -> Edge {
    Edge {
        line: 0,
        x0,
        y0,
        ex: span,
        ey: 0,
        len16: raw(span),
        dir: WallDir::EastWest,
        front: 0,
        back: Some(1),
        upper_unpegged: false,
        lower_unpegged: false,
    }
}

/// The `2^32`-scaled unit direction of `angle`, for sky tests.
pub(super) fn unit(angle: Angle) -> (i128, i128) {
    (
        i128::from(angle.cos().0).saturating_mul(ONE_16),
        i128::from(angle.sin().0).saturating_mul(ONE_16),
    )
}

/// A solid indexed image of `index`.
pub(super) fn solid(width: u32, height: u32, index: u8) -> IndexedImage {
    let count = usize::try_from(width.saturating_mul(height)).expect("test sizes fit");
    IndexedImage::new(width, height, vec![index; count], vec![true; count])
        .expect("test images build")
}

/// The fixture palette: colour `i` is `(i, 255 - i, i)`.
pub(super) fn fixture_palette() -> Palette {
    let mut colors = [[0_u8; 3]; 256];
    for (index, color) in colors.iter_mut().enumerate() {
        let byte = u8::try_from(index).expect("the palette covers exactly the u8 range");
        *color = [byte, 255_u8.wrapping_sub(byte), byte];
    }
    Palette::from(colors)
}

/// The identity lighting table.
pub(super) fn identity_table() -> [u8; 256] {
    let mut table = [0_u8; 256];
    for (index, entry) in table.iter_mut().enumerate() {
        *entry = u8::try_from(index).expect("the table covers exactly the u8 range");
    }
    table
}
