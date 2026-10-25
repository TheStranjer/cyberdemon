//! The integer plane-to-pixel mapping every top-down or isometric frame
//! fits: the [`Plane`] of world coordinates a call frames, the [`View`]
//! that maps it onto canvas pixels (and back, for the underlay and the
//! grid), the plane builders and the rounding helpers the painters share.

use crate::render::{Pt, scale_down};
use crate::wad::map::Map;

use super::params::Bounds;
use super::topdown::TransformReport;

/// Pixels kept clear around the fitted view.
const MARGIN_PX: i64 = 16;

/// The axis range a fitted view must place inside the canvas margins.
pub(super) struct Plane {
    /// Smallest horizontal plane coordinate.
    pub(super) min_u: i64,
    /// Largest horizontal plane coordinate.
    pub(super) max_u: i64,
    /// Smallest vertical plane coordinate.
    pub(super) min_v: i64,
    /// Largest vertical plane coordinate.
    pub(super) max_v: i64,
}

/// The integer plane-to-pixel mapping one render fits:
/// `px = offset_x + u * num / den`, `py = offset_y - v * num / den`
/// (floor-rounded), `y` flipped so north is up.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct View {
    /// Scale numerator: the fitted pixel span.
    pub(super) num: i64,
    /// Scale denominator: the matching plane span (`>= 1`).
    pub(super) den: i64,
    /// Pixel column of plane coordinate `0`.
    pub(super) offset_x: i64,
    /// Pixel row of plane coordinate `0`.
    pub(super) offset_y: i64,
}

impl View {
    /// The plane distance `value` measures in pixels (floor-rounded).
    pub(super) fn scale(self, value: i64) -> i64 {
        scale_down(value, self.num, self.den)
    }

    /// The pixel of plane point `(u, v)`.
    pub(super) fn point(self, u: i64, v: i64) -> Pt {
        (
            self.offset_x.saturating_add(self.scale(u)),
            self.offset_y.saturating_sub(self.scale(v)),
        )
    }

    /// The pixel of world point `x`/`y`/`z` under the isometric projection.
    pub(super) fn iso(self, x: i64, y: i64, z: i64) -> Pt {
        self.point(
            x.saturating_sub(y),
            half(x.saturating_add(y)).saturating_sub(z),
        )
    }

    /// The world point whose pixel lands at canvas `(col, row)`:
    /// [`point`](Self::point)'s mapping inverted in integer arithmetic,
    /// floor-rounded. `self.num` is at least `1` for every fitted view,
    /// so the division always has a non-zero denominator.
    pub(super) fn unproject(self, col: i64, row: i64) -> (i64, i64) {
        (
            scale_down(col.saturating_sub(self.offset_x), self.den, self.num),
            scale_down(self.offset_y.saturating_sub(row), self.den, self.num),
        )
    }

    /// The `transform` object this view reports.
    pub(super) fn report(self) -> TransformReport {
        TransformReport {
            scale_num: u64::try_from(self.num).unwrap_or(u64::MAX),
            scale_den: u64::try_from(self.den).unwrap_or(u64::MAX),
            offset_x: self.offset_x,
            offset_y: self.offset_y,
        }
    }
}

/// The largest integer ratio `num / den` that fits `plane` into the canvas
/// minus [`MARGIN_PX`] on every side, centred in what is left.
pub(super) fn fit(plane: &Plane, width: u32, height: u32) -> View {
    let avail_w = i64::from(width)
        .saturating_sub(MARGIN_PX.saturating_mul(2))
        .max(1);
    let avail_h = i64::from(height)
        .saturating_sub(MARGIN_PX.saturating_mul(2))
        .max(1);
    let span_u = plane.max_u.saturating_sub(plane.min_u).max(1);
    let span_v = plane.max_v.saturating_sub(plane.min_v).max(1);
    let wide_fit = i128::from(avail_w).saturating_mul(i128::from(span_v));
    let tall_fit = i128::from(avail_h).saturating_mul(i128::from(span_u));
    let (num, den) = if wide_fit <= tall_fit {
        (avail_w, span_u)
    } else {
        (avail_h, span_v)
    };
    let (u0, u1) = (
        scale_down(plane.min_u, num, den),
        scale_down(plane.max_u, num, den),
    );
    let (v0, v1) = (
        scale_down(plane.min_v, num, den),
        scale_down(plane.max_v, num, den),
    );
    let slack_x = avail_w
        .saturating_sub(u1.saturating_sub(u0))
        .checked_div(2)
        .unwrap_or(0);
    let slack_y = avail_h
        .saturating_sub(v1.saturating_sub(v0))
        .checked_div(2)
        .unwrap_or(0);
    View {
        num,
        den,
        offset_x: MARGIN_PX.saturating_add(slack_x).saturating_sub(u0),
        offset_y: MARGIN_PX.saturating_add(slack_y).saturating_add(v1),
    }
}

/// The plane a top-down render frames: the world bounds themselves.
pub(super) const fn bounds_plane(bounds: &Bounds) -> Plane {
    Plane {
        min_u: bounds.min_x,
        max_u: bounds.max_x,
        min_v: bounds.min_y,
        max_v: bounds.max_y,
    }
}

/// The plane an isometric render frames: the projected bounds spanning
/// `heights` (the map's floor-to-ceiling range).
pub(super) fn iso_plane(bounds: &Bounds, heights: (i64, i64)) -> Plane {
    let (min_z, max_z) = heights;
    Plane {
        min_u: bounds.min_x.saturating_sub(bounds.max_y),
        max_u: bounds.max_x.saturating_sub(bounds.min_y),
        min_v: half(bounds.min_x.saturating_add(bounds.min_y)).saturating_sub(max_z),
        max_v: half(bounds.max_x.saturating_add(bounds.max_y)).saturating_sub(min_z),
    }
}

/// The map's `(lowest floor, highest ceiling)` as integers.
pub(super) fn height_span(map: &Map) -> (i64, i64) {
    let mut span = (i64::MAX, i64::MIN);
    for sector in &map.sectors {
        span = (
            span.0.min(i64::from(sector.floor_height)),
            span.1.max(i64::from(sector.ceiling_height)),
        );
    }
    if span.0 <= span.1 { span } else { (0, 0) }
}

/// Floor-halving `value` (an arithmetic shift, so negatives round down).
fn half(value: i64) -> i64 {
    value.checked_shr(1).unwrap_or(0)
}

/// Moves a point by `offset` on both axes (saturating).
pub(super) const fn offset_pt(point: Pt, offset: i64) -> Pt {
    (
        point.0.saturating_add(offset),
        point.1.saturating_add(offset),
    )
}
