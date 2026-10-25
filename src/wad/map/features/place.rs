//! Pure door placement geometry: wall selection, width fit, depth raycast.
//!
//! The door tools dress a doorway the [`doors`] engine can close, but
//! first they must find one: which wall of a room faces the sector the
//! door should open into, where along that wall the cut sits, how wide
//! it may be, and how deep the recess can go before it breaks into
//! another sector. This module answers those questions as plain integer
//! geometry over a decoded [`Map`]: [`longest_boundary`] and
//! [`nearest_boundary`] select the shared wall, [`project_onto_line`]
//! and [`door_segment`] place the cut, [`outward_depth`] casts a ray
//! through the wall to the first sector beyond it, [`door_rect`] turns
//! the chosen cut into the polygon [`crate::wad::map::overlay`] draws,
//! and [`void_strip`] thickens a whole shared boundary into the strip a
//! void carve uses. [`door_wall`] composes the selection into one
//! [`DoorWall`] description.
//!
//! Everything is a function of the [`Map`] model and plain integers:
//! distances compare as exact `i128` fractions, hit points round to
//! whole map units, and every refusal is a
//! [`CyberError::InvalidArgument`] saying what to do next. Like the
//! sibling engines in [`crate::wad::map::ops`] and
//! [`crate::wad::map::overlay`], nothing here mutates the map and no
//! `MCP` type appears.
//!
//! [`doors`]: super::doors

mod cut;
mod depth;
mod strip;

#[cfg(test)]
mod tests;

pub use cut::{door_rect, door_segment, door_wall, project_onto_line};
pub use depth::outward_depth;
pub use strip::void_strip;

use crate::error::CyberError;
use crate::fixed::{div_round, hypot, midpoint, narrow_i32, narrow_i64};
use crate::geom::Pt;
use crate::geom::segment::{Delta, cross, delta, dot};
use crate::wad::map::model::{Linedef, Map};
use crate::wad::map::ops::Point;

/// A pair of endpoint coordinates, `(start, end)`.
pub type VertexPair = (Point, Point);

/// The origin of the coordinate plane, the fallback of
/// [`project_onto_line`] and the zero vector a [`Point`] becomes.
const ORIGIN: Point = Point { x: 0_i32, y: 0_i32 };

/// The chosen door cut: where on which wall the doorway sits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DoorWall {
    /// Index in [`Map::linedefs`] of the wall line the door is cut into.
    pub linedef: usize,
    /// The centre of the cut, a point on the wall line.
    pub point: Point,
    /// The vector from [`Self::point`] toward the cut's far endpoint:
    /// half the door width, along the wall.
    pub half_dir: Point,
    /// The wall line's integer length in map units: the widest door the
    /// whole line could carry.
    pub available_width: i32,
}

/// A whole shared boundary thickened into one carve polygon.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BoundaryStrip {
    /// The strip's corners, in order, implicitly closed: ready for
    /// [`crate::wad::map::overlay::apply_overlay`].
    pub polygon: Vec<Point>,
}

/// Every two-sided linedef between sectors `a` and `b`, in linedef
/// order: the pair's whole shared boundary.
///
/// A line counts when its front and back sides resolve to sectors and
/// those two are exactly `a` and `b` in either order. Dangling side or
/// sector references never contribute, and `a == b` finds nothing.
/// `None` when no line separates the two sectors.
#[must_use]
#[inline]
pub fn longest_boundary(map: &Map, a: usize, b: usize) -> Option<Vec<usize>> {
    let shared: Vec<usize> = map
        .linedefs
        .iter()
        .enumerate()
        .filter(|&(_, line)| joins_sectors(map, line, a, b))
        .map(|(index, _)| index)
        .collect();
    (!shared.is_empty()).then_some(shared)
}

/// Whether `line` is two-sided between sectors `first` and `second`,
/// either way around.
fn joins_sectors(map: &Map, line: &Linedef, first: usize, second: usize) -> bool {
    let (Some(front), Some(back)) = map.line_sectors(line) else {
        return false;
    };
    (front == first && back == second) || (front == second && back == first)
}

/// A squared distance as an exact fraction `num / den` plus the linedef
/// index it was measured to.
type Scored = ((i128, i128), usize);

/// The closest of [`longest_boundary`]'s lines to `at`, by exact
/// integer point-to-segment distance; ties go to the lowest linedef
/// index.
///
/// Only lines whose two vertices both resolve can win. `None` when the
/// sectors share no boundary or none of it resolves.
#[must_use]
#[inline]
pub fn nearest_boundary(map: &Map, a: usize, b: usize, at: Point) -> Option<usize> {
    let mut best: Option<Scored> = None;
    for index in longest_boundary(map, a, b)? {
        let Some((from, to)) = endpoints(map, index) else {
            continue;
        };
        let distance = distance_fraction(at, from, to);
        let nearer = best.is_none_or(|(known, _)| {
            distance
                .0
                .saturating_mul(known.1)
                .lt(&known.0.saturating_mul(distance.1))
        });
        if nearer {
            best = Some((distance, index));
        }
    }
    best.map(|(_, index)| index)
}

/// The two endpoint coordinates of linedef `index`, when both vertices
/// resolve.
#[must_use]
#[inline]
pub fn endpoints(map: &Map, index: usize) -> Option<VertexPair> {
    let ((ax, ay), (bx, by)) = map.line_endpoints(map.linedefs.get(index)?)?;
    Some((point_at(ax, ay), point_at(bx, by)))
}

/// A [`Point`] at map-unit coordinates.
const fn point_at(x: i32, y: i32) -> Point {
    Point { x, y }
}

/// A [`Point`] widened to the exact-arithmetic grid.
fn wide(point: Point) -> Pt {
    (i64::from(point.x), i64::from(point.y))
}

/// An exact [`Delta`] from [`Point`] axes.
fn vector(point: Point) -> Delta {
    (i128::from(point.x), i128::from(point.y))
}

/// The squared distance from `at` to segment `from`-`to` as an exact
/// fraction `num / den` with `den` positive.
///
/// Clamped endpoint distances use `1`, the perpendicular interior
/// distance the squared run length. Two of them compare by
/// cross-multiplication, which is how [`nearest_boundary`] and the
/// [`crate::plan`] feature engine's wall choice rank their candidates
/// without a float.
#[must_use]
#[inline]
pub fn distance_fraction(at: Point, from: Point, to: Point) -> (i128, i128) {
    let run = delta(wide(from), wide(to));
    let offset = delta(wide(from), wide(at));
    let squared = dot(run, run);
    let along = dot(offset, run);
    if squared == 0_i128 || along <= 0_i128 {
        return (dot(offset, offset), 1_i128);
    }
    if along >= squared {
        let back = delta(wide(to), wide(at));
        return (dot(back, back), 1_i128);
    }
    let perpendicular = cross(run, offset);
    (perpendicular.saturating_mul(perpendicular), squared)
}

/// The floored integer length of an `i128` delta vector, saturated into
/// `i32` (only `i32`-coordinate deltas are ever measured).
fn length_of(run: Delta) -> i32 {
    let length = hypot(narrow_i64(run.0), narrow_i64(run.1));
    i32::try_from(length).unwrap_or(i32::MAX)
}

/// `from + vector * num / den` per coordinate, each rounded to the
/// nearest whole map unit (halves away from zero) and saturated into
/// `i32`. `den` must be positive.
fn scale_shift(from: Point, vector: Delta, num: i128, den: i128) -> Point {
    Point {
        x: shift_axis(from.x, vector.0, num, den),
        y: shift_axis(from.y, vector.1, num, den),
    }
}

/// `from + axis * num / den` rounded to nearest for one coordinate.
fn shift_axis(from: i32, axis: i128, num: i128, den: i128) -> i32 {
    let scaled = i128::from(from)
        .saturating_mul(den)
        .saturating_add(axis.saturating_mul(num));
    narrow_i32(div_round(scaled, den).unwrap_or(scaled))
}

/// One coordinate of the midpoint of `first` and `second`, halves
/// rounded away from zero and saturated back into `i32`.
fn half_axis(first: i32, second: i32) -> i32 {
    narrow_i32(i128::from(midpoint(i64::from(first), i64::from(second))))
}

/// The componentwise mean of two offset vectors, halves rounded away
/// from zero: a chain corner's bevelled strip offset.
fn half_vector(low: Delta, high: Delta) -> Delta {
    let mean = |sum: i128| div_round(sum, 2_i128).unwrap_or(sum);
    (
        mean(low.0.saturating_add(high.0)),
        mean(low.1.saturating_add(high.1)),
    )
}

/// A [`Point`] from exact axes, saturated into `i32`.
fn from_axes(x: i128, y: i128) -> Point {
    Point {
        x: narrow_i32(x),
        y: narrow_i32(y),
    }
}

/// `vertex + offset` with an exact offset vector, saturated into `i32`.
fn moved(vertex: Point, offset: Delta) -> Point {
    from_axes(
        i128::from(vertex.x).saturating_add(offset.0),
        i128::from(vertex.y).saturating_add(offset.1),
    )
}

/// The `InvalidArgument` with an actionable `message`.
fn invalid(message: &str) -> CyberError {
    CyberError::InvalidArgument(message.to_owned())
}
