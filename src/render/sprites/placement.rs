//! Sprite projection: the pixels-per-map-unit scale at a sprite's depth,
//! the two projected axes that walk a patch over the raster and the
//! [`Placement`] that folds both into the screen columns and rows to paint.
//!
//! [`Placement::of`] is the transparent pass's whole reading of a
//! [`SpriteItem`]: it takes [`Scale`] from the caster's [`Projection`] and
//! the item's depth, lays the patch out along a horizontal and a vertical
//! [`Axis`] and answers with the pixels each of them touches together with
//! the texel every one of them reads. [`view_centre`],
//! [`strict_last_below`] and [`clamped_texel`] are the integer maths the
//! two types share.

use alloc::vec::Vec;

use super::{SpriteItem, item_depth};
use crate::render::view::{ONE_16, Projection};

/// The projected size of one map unit at a sprite's depth: pixels per map
/// unit `k = (focal2 / 2) / tz`, kept as an exact `i128` pair and never
/// rounded to whole pixels.
///
/// `k` is `num / den`, so a raw-16.16 map-unit distance times `num` over
/// `den` is the same distance in raw-16.16 pixels. To keep every step
/// exact, projected positions are carried in *scale units* — raw-16.16
/// pixels multiplied by `den` — so [`Scale::units`] and [`Scale::pixels`]
/// are plain saturating multiplications and only the final whole-number
/// screen index divides.
#[derive(Clone, Copy, Debug)]
struct Scale {
    /// Numerator: twice the focal length, raw 16.16 pixels.
    num: i128,
    /// Denominator: twice the item's depth, raw 16.16 map units, always
    /// positive.
    den: i128,
    /// Scale units in one screen pixel: `2^16 * den`.
    pixel: i128,
}

impl Scale {
    /// The scale of an item at perpendicular depth `tz16` (raw 16.16 map
    /// units) under `proj`: `focal2 / (2 * tz16)` pixels per map unit.
    /// `None` for an item at or behind the camera, and for the depths
    /// whose double does not fit: an item the caller has not filtered out
    /// simply draws nothing.
    fn of(proj: &Projection, tz16: i128) -> Option<Self> {
        let den = tz16.checked_mul(2_i128)?;
        (den > 0_i128).then_some(Self {
            num: proj.focal2(),
            den,
            pixel: ONE_16.saturating_mul(den),
        })
    }

    /// A raw-16.16 map-unit distance in scale units: `value * num`, which
    /// is `value * k * den` raw-16.16 pixels.
    const fn units(self, value16: i128) -> i128 {
        value16.saturating_mul(self.num)
    }

    /// A raw-16.16 pixel distance in scale units.
    const fn pixels(self, value16: i128) -> i128 {
        value16.saturating_mul(self.den)
    }

    /// Scale units in one map unit, which is one texel of a patch: `k`
    /// raw-16.16 pixels, carried in scale units.
    const fn texel(self) -> i128 {
        ONE_16.saturating_mul(self.num)
    }
}

/// One projected axis of a sprite patch: where its texel edges land on
/// the screen and which texel a screen pixel reads.
///
/// The patch's span runs from [`Axis::start`] to [`Axis::end`] in scale
/// units, along the axis' own screen direction — columns left to right,
/// rows top to bottom — and texel `j` occupies the span's `j`-th `step`,
/// so a pixel touching the span reads the last texel it overlaps.
#[derive(Clone, Copy, Debug)]
struct Axis {
    /// Scale units from the frame's edge to texel 0's near edge.
    start: i128,
    /// Scale units per texel: one map unit at the item's depth.
    step: i128,
    /// Scale units per screen pixel: `k`'s denominator scaled by `2^16`.
    pixel: i128,
    /// Texels along this axis: the patch's width or height, always at
    /// least 1.
    count: i128,
}

impl Axis {
    /// The screen pixels the span touches inside `0..limit`, each with
    /// the texel it reads, in ascending screen order. Empty when the patch
    /// misses the frame.
    fn pixels(self, limit: i64) -> Vec<(i64, u32)> {
        let low = self
            .start
            .div_euclid(self.pixel)
            .clamp(0_i128, i128::from(i64::MAX));
        let high = strict_last_below(self.end(), self.pixel)
            .min(i128::from(limit.saturating_sub(1_i64)))
            .clamp(i128::from(i64::MIN), i128::from(i64::MAX));
        let first = i64::try_from(low).unwrap_or(i64::MAX);
        let last = i64::try_from(high).unwrap_or(-1_i64);
        let mut touched = Vec::new();
        for screen in first..=last {
            touched.push((screen, self.texel_at(screen)));
        }
        touched
    }

    /// Scale units just past the span's far edge.
    const fn end(self) -> i128 {
        self.start
            .saturating_add(self.count.saturating_mul(self.step))
    }

    /// The texel screen pixel `screen` reads: the last one whose span
    /// touches it, clamped into the patch.
    fn texel_at(self, screen: i64) -> u32 {
        let far_edge = i128::from(screen)
            .saturating_add(1_i128)
            .saturating_mul(self.pixel)
            .saturating_sub(self.start);
        clamped_texel(strict_last_below(far_edge, self.step), self.count)
    }
}

/// A sprite item projected onto the raster, ready to paint.
pub(super) struct Placement {
    /// The screen columns the patch touches, with the texel column each
    /// reads, ascending.
    pub(super) columns: Vec<(i64, u32)>,
    /// The screen rows the patch touches, with the texel row each reads,
    /// ascending.
    pub(super) rows: Vec<(i64, u32)>,
    /// The item's perpendicular depth in whole map units: the number the
    /// depth test compares against the caster's buffer and the lighting
    /// fade reads.
    pub(super) depth: i64,
}

impl Placement {
    /// Projects `item` through `proj`. The centre column is
    /// `W / 2 + tx·k`, the patch's left edge `left_offset·k` to its left
    /// and its top row `horizon - (base + top_offset - eye)·k`: the
    /// caster's own row formula at the item's depth, since
    /// `1 / (2t) = focal2 / (2 * tz)` is exactly `k`. `None` when the
    /// item's depth is not in front of the camera.
    pub(super) fn of(proj: &Projection, item: &SpriteItem) -> Option<Self> {
        let scale = Scale::of(proj, item.tz16)?;
        let centre = scale
            .pixels(view_centre(proj))
            .saturating_add(scale.units(item.tx16));
        let columns = Axis {
            start: centre.saturating_sub(scale.units(item.left_offset16)),
            step: scale.texel(),
            pixel: scale.pixel,
            count: i128::from(item.image.width),
        }
        .pixels(proj.width());
        let top = i128::from(item.base)
            .saturating_mul(ONE_16)
            .saturating_add(item.top_offset16);
        let rows = Axis {
            start: scale
                .pixels(proj.horizon16())
                .saturating_sub(scale.units(top.saturating_sub(proj.eye16()))),
            step: scale.texel(),
            pixel: scale.pixel,
            count: i128::from(item.image.height),
        }
        .pixels(proj.height());
        Some(Self {
            columns,
            rows,
            depth: item_depth(item.tz16),
        })
    }
}

/// The raw-16.16 pixel column at the middle of the raster: `W / 2`, the
/// edge the caster's centre ray (`u = 0`) passes through.
fn view_centre(proj: &Projection) -> i128 {
    i128::from(proj.width().div_euclid(2)).saturating_mul(ONE_16)
}

/// The largest integer whose `step`-sized multiple still falls strictly
/// below `value` (`step` positive): the number of whole spans a pixel's
/// far edge has passed.
pub(super) const fn strict_last_below(value: i128, step: i128) -> i128 {
    value.saturating_sub(1_i128).div_euclid(step)
}

/// A texel index clamped into a `count`-texel axis, as a `u32`.
fn clamped_texel(index: i128, count: i128) -> u32 {
    let last = count.saturating_sub(1_i128);
    u32::try_from(index.clamp(0_i128, last)).unwrap_or(0)
}
