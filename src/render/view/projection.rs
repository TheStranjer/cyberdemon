//! The exact maths one column cast runs on: unrounded `i128` fractions
//! and the camera's projection constants.
//!
//! [`Fraction`] keeps a column ray's parameter and a hit's position along
//! its linedef exact: it normalises its denominator to a positive value
//! and never reduces, so ordering two of them stays a cross-multiplication
//! and the products and quotients that come out of them saturate. The
//! `Projection` of one camera and raster folds the camera's facing, focal
//! length and pitch into twice the focal length and the horizon row; the
//! rows, depths, plane ray parameters and sky rows the walk reads all
//! follow from those two numbers.

use core::cmp::Ordering;

use super::{HALF_16, ONE_16, ViewCamera};
use crate::fixed::{Angle, FRACUNIT, mul_div_round};

/// An exact fraction `n / d` with a positive denominator: a column ray's
/// parameter `t` and a hit's position `s` along its linedef.
///
/// Public so [`super::MaskedMiddle`] can carry a crossing's exact ray
/// parameter to the render module's other passes; the arithmetic stays the
/// column caster's own.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Fraction {
    /// Numerator.
    pub(super) n: i128,
    /// Denominator, always positive.
    pub(super) d: i128,
}

impl Fraction {
    /// Builds `num / den` from raw cross products, normalising the sign;
    /// `None` for a zero denominator (a parallel ray) or a negation that
    /// cannot be taken. Crate-visible so the render module's passes can
    /// build one for a projected item.
    pub(crate) fn new(num: i128, den: i128) -> Option<Self> {
        match den.cmp(&0_i128) {
            Ordering::Equal => None,
            Ordering::Less => Some(Self {
                n: num.checked_neg()?,
                d: den.checked_neg()?,
            }),
            Ordering::Greater => Some(Self { n: num, d: den }),
        }
    }

    /// Whether the value is strictly positive (the denominator is).
    pub(super) const fn is_positive(self) -> bool {
        self.n > 0
    }

    /// `self * m`, saturating.
    pub(super) const fn times(self, m: i128) -> i128 {
        quot(self.n.saturating_mul(m), self.d)
    }

    /// `2 * m * self`, saturating: the height step of one pixel at ray
    /// parameter `t`.
    const fn double_times(self, m: i128) -> i128 {
        quot(self.n.saturating_mul(2_i128).saturating_mul(m), self.d)
    }

    /// `m / (2 * self)` in raw 16.16, saturating: the projected height
    /// span of a world-height difference `m` at ray parameter `t`.
    const fn recip_half16(self, m: i128) -> i128 {
        quot(m.saturating_mul(HALF_16).saturating_mul(self.d), self.n)
    }

    /// The ordering of two fractions by cross-multiplication.
    pub(super) fn cmp_value(self, other: Self) -> Ordering {
        self.n
            .saturating_mul(other.d)
            .cmp(&other.n.saturating_mul(self.d))
    }
}

/// `a / b` truncated, saturating past the `i128` range and treating a
/// zero divisor as the saturated quotient of `a`'s sign.
const fn quot(a: i128, b: i128) -> i128 {
    match a.checked_div(b) {
        Some(value) => value,
        None => {
            if (a >= 0) == (b >= 0) {
                i128::MAX
            } else {
                i128::MIN
            }
        }
    }
}

/// The projection constants of one camera and raster. The transparent
/// pass in the render module's `sprites` reads rows back through
/// [`Self::height_at`] and projects sprites from [`Self::focal2`],
/// [`Self::horizon16`], [`Self::eye16`] and the raster size, so the type
/// and those methods are crate-visible; everything else stays the column
/// caster's own.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Projection {
    /// Raster width in pixels.
    width: i64,
    /// Raster height in pixels.
    height: i64,
    /// Eye height in map units.
    z: i64,
    /// Camera angle cosine, raw 16.16.
    cos: i64,
    /// Camera angle sine, raw 16.16.
    sin: i64,
    /// Twice the focal length in pixels, raw 16.16.
    pub(super) focal2: i128,
    /// The horizon row, raw 16.16.
    pub(super) horizon: i128,
}

impl Projection {
    /// Builds the projection of `camera` over a `width` by `height`
    /// raster: `focal2 = width * cot(fov/2)` in 16.16 and
    /// `horizon = height/2 + (focal2/2) * tan(pitch)`. Crate-visible so
    /// the render module's sprite pass can project an item on its own.
    pub(crate) fn new(camera: &ViewCamera, width: u32, height: u32) -> Self {
        let facing = Angle::from_degrees(i64::from(camera.angle));
        let half = Angle::from_millidegrees(i64::from(camera.fov).saturating_mul(500_i64));
        let focal2 = focal_length(i64::from(width), half);
        let tan = Angle::from_degrees(i64::from(camera.pitch))
            .tan()
            .unwrap_or_else(|| Angle::EAST.tan().unwrap_or_default());
        let lift = focal2.saturating_mul(i128::from(tan.0)).checked_shr(17);
        Self {
            width: i64::from(width),
            height: i64::from(height),
            z: i64::from(camera.z),
            cos: i64::from(facing.cos().0),
            sin: i64::from(facing.sin().0),
            focal2,
            horizon: i128::from(
                i64::from(height)
                    .checked_div(2_i64)
                    .unwrap_or_else(|| i64::from(height)),
            )
            .saturating_mul(ONE_16)
            .saturating_add(lift.unwrap_or(0)),
        }
    }

    /// The `2^32`-scaled direction of column `c`:
    /// `d = forward * focal2 + right * u` with `u = 2c + 1 - width`.
    pub(super) fn direction(&self, c: i64) -> (i128, i128) {
        let u = c
            .saturating_mul(2_i64)
            .saturating_add(1_i64)
            .saturating_sub(self.width);
        let u16 = i128::from(u).saturating_mul(ONE_16);
        let cos = i128::from(self.cos);
        let sin = i128::from(self.sin);
        (
            cos.saturating_mul(self.focal2)
                .saturating_add(sin.saturating_mul(u16)),
            sin.saturating_mul(self.focal2)
                .saturating_sub(cos.saturating_mul(u16)),
        )
    }

    /// The screen row world height `h` projects to at ray parameter `t`,
    /// floored: `horizon - (h - z) / (2t)`.
    pub(super) fn row(&self, h: i64, t: Fraction) -> i64 {
        let rise = i128::from(h).saturating_sub(i128::from(self.z));
        let raw = self.horizon.saturating_sub(t.recip_half16(rise));
        let floored = raw.div_euclid(ONE_16);
        i64::try_from(floored).unwrap_or(if raw < 0 { i64::MIN } else { i64::MAX })
    }

    /// The world height at screen `row` for ray parameter `t`, raw
    /// 16.16: `z + (horizon - row) * 2t`. The transparent pass reads a
    /// masked item's texture row through this.
    pub(crate) fn height_at(&self, row: i64, t: Fraction) -> i128 {
        let dy = self
            .horizon
            .saturating_sub(i128::from(row).saturating_mul(ONE_16));
        i128::from(self.z)
            .saturating_mul(ONE_16)
            .saturating_add(t.double_times(dy))
    }

    /// The perpendicular depth in map units, raw 16.16, at `t`.
    pub(super) const fn depth16(&self, t: Fraction) -> i128 {
        t.times(self.focal2)
    }

    /// Raster width in pixels, for a pass that projects an item into it.
    pub(crate) const fn width(&self) -> i64 {
        self.width
    }

    /// Raster height in pixels.
    pub(crate) const fn height(&self) -> i64 {
        self.height
    }

    /// Twice the focal length, raw 16.16 pixels: with a sprite's depth it
    /// gives the pass its pixels-per-map-unit scale, `focal2 / (2 * tz)`.
    pub(crate) const fn focal2(&self) -> i128 {
        self.focal2
    }

    /// The horizon row, raw 16.16 pixels.
    pub(crate) const fn horizon16(&self) -> i128 {
        self.horizon
    }

    /// Camera facing cosine, raw 16.16: the caster's column bucketing
    /// projects edge endpoints onto the camera axes with it.
    pub(super) const fn cos(&self) -> i64 {
        self.cos
    }

    /// Camera facing sine, raw 16.16, the mate of [`Self::cos`].
    pub(super) const fn sin(&self) -> i64 {
        self.sin
    }

    /// The eye height, raw 16.16 map units: what a sprite's world height
    /// is measured against.
    pub(crate) fn eye16(&self) -> i128 {
        i128::from(self.z).saturating_mul(ONE_16)
    }

    /// The ray parameter of the plane at world `height` seen at screen
    /// `row`: `t = (h - z) / (2 * (horizon - row))`, exact. `None` on the
    /// horizon row itself (zero denominator) and on rows the plane
    /// cannot own for this camera: a floor above the eye, a ceiling
    /// below it, or a plane exactly at eye height all give a
    /// non-positive `t`.
    pub(super) fn plane_t(&self, height: i64, row: i64) -> Option<Fraction> {
        let rise = i128::from(height)
            .saturating_sub(i128::from(self.z))
            .saturating_mul(ONE_16);
        let step = self
            .horizon
            .saturating_sub(i128::from(row).saturating_mul(ONE_16))
            .saturating_mul(2_i128);
        let t = Fraction::new(rise, step)?;
        t.is_positive().then_some(t)
    }

    /// The sky texture row of screen `row`: `Doom`'s 320x200 sky
    /// placement, `(y - horizon) * 200 / height + 100` in pixels,
    /// clamped into a sky `sky_height` rows tall.
    pub(super) fn sky_row(&self, row: i64, sky_height: u32) -> u32 {
        let dy = i128::from(row)
            .saturating_mul(ONE_16)
            .saturating_sub(self.horizon);
        let scaled = quot(
            dy.saturating_mul(200_i128),
            i128::from(self.height).saturating_mul(ONE_16),
        );
        let last = i128::from(sky_height).saturating_sub(1_i128).max(0_i128);
        u32::try_from(scaled.saturating_add(100_i128).clamp(0_i128, last)).unwrap_or(0)
    }
}

/// `width * cot(half)` in raw 16.16; the half angle sits inside
/// `5..=85` degrees (the field-of-view range), so its sine is positive.
pub(super) fn focal_length(width: i64, half: Angle) -> i128 {
    let numerator = width.saturating_mul(i64::from(half.cos().0));
    let denominator = i64::from(half.sin().0).max(1_i64);
    let raw = mul_div_round(numerator, i64::from(FRACUNIT), denominator).unwrap_or(i64::MAX);
    i128::from(raw).max(ONE_16)
}
