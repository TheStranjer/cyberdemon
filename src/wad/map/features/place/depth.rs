//! The outward depth raycast: how far behind a chosen wall the recess
//! may go before it breaks into another sector.
//!
//! [`outward_depth`] casts from a point on the boundary line along its
//! front-side outward normal and answers with the nearest crossed line
//! that bounds a sector other than the one the ray travels through.
//! The ray-versus-segment solves are exact `i128` cross products, the
//! qualification test compares squared fractions against `max_depth`
//! and every non-hit is simply `None`: the cast passes on parallel
//! lines, void, walls faced at the ray and same-sector splits.

use crate::geom::segment::{Delta, cross, delta, dot};
use crate::wad::map::model::{Linedef, Map};
use crate::wad::map::ops::Point;

use super::{endpoints, length_of, scale_shift, wide};

/// One prepared [`outward_depth`] ray.
struct RecessRay {
    /// Where the ray starts, a point on the chosen wall.
    origin: Point,
    /// Index of the wall line the ray starts from (never its own hit).
    source: usize,
    /// The integer outward normal: the boundary line's front-side
    /// perpendicular.
    normal: Delta,
    /// The squared length of [`Self::normal`], for exact comparisons.
    run_squared: i128,
    /// The sector the ray travels through: the boundary's front side.
    near: Option<usize>,
    /// The longest distance to consider.
    max_depth: i32,
}

/// One ray candidate: a crossed line with the sector beyond it.
struct RayHit {
    /// The squared ray fraction `t * t` as `(num, den)`, `den > 0`.
    fraction: (i128, i128),
    /// The whole-map-unit distance from the ray origin.
    distance: i32,
    /// The sector the crossed line bounds away from the ray origin.
    sector: usize,
}

/// The wall the recess hits: cast from `centre` along the integer
/// outward normal of linedef `linedef`'s front-facing side (the room
/// behind it).
///
/// The answer is the first crossed line whose far side bounds a sector
/// other than the one the ray travels through, as
/// `(distance, sector)`. Ray-versus-segment tests are exact `i128` cross products; the hit
/// point rounds to the nearest whole unit and its distance is that
/// rounded point's floored [`hypot`]. Lines parallel to the ray, lines
/// with nothing beyond them (void, or a wall faced at the ray) and
/// lines whose far side is the sector already being crossed do not
/// bound the recess: the cast passes them on. The nearest qualifying
/// hit wins; ties go to the lowest linedef index. `None` when `linedef`
/// or its vertices do not resolve, when `max_depth` is not positive, or
/// when nothing bounds a sector within `max_depth`.
///
/// [`hypot`]: crate::fixed::hypot
#[must_use]
#[inline]
pub fn outward_depth(
    map: &Map,
    linedef: usize,
    centre: Point,
    max_depth: i32,
) -> Option<(i32, usize)> {
    if max_depth <= 0_i32 {
        return None;
    }
    let line = map.linedefs.get(linedef)?;
    let (from, to) = endpoints(map, linedef)?;
    let run = delta(wide(from), wide(to));
    let normal = (run.1, run.0.saturating_neg());
    let ray = RecessRay {
        origin: centre,
        source: linedef,
        normal,
        run_squared: dot(normal, normal),
        near: map.side_sector(line.front),
        max_depth,
    };
    cast_outward(map, &ray).map(|hit| (hit.distance, hit.sector))
}

/// The nearest qualifying hit along `ray`, or `None`.
fn cast_outward(map: &Map, ray: &RecessRay) -> Option<RayHit> {
    let mut best: Option<RayHit> = None;
    for (index, line) in map.linedefs.iter().enumerate() {
        if index == ray.source {
            continue;
        }
        let Some(hit) = ray_hit(map, ray, index, line) else {
            continue;
        };
        if !qualifies(ray, &hit) {
            continue;
        }
        let closer = best.as_ref().is_none_or(|known| {
            hit.fraction
                .0
                .saturating_mul(known.fraction.1)
                .lt(&known.fraction.0.saturating_mul(hit.fraction.1))
        });
        if closer {
            best = Some(hit);
        }
    }
    best
}

/// Where `ray` crosses `line`'s segment, with the sector beyond it:
/// the exact two-line solve of [`crate::geom::segment`], restricted to
/// intersections strictly ahead of the origin and within the segment.
fn ray_hit(map: &Map, ray: &RecessRay, index: usize, line: &Linedef) -> Option<RayHit> {
    let (from, to) = endpoints(map, index)?;
    let run = delta(wide(from), wide(to));
    let determinant = cross(ray.normal, run);
    if determinant == 0_i128 {
        return None;
    }
    let towards = delta(wide(ray.origin), wide(from));
    let (den, along, across) =
        positive_denominator(determinant, cross(towards, run), cross(towards, ray.normal));
    if along <= 0_i128 || across < 0_i128 || across > den {
        return None;
    }
    let origin_side = cross(towards, run);
    let sector = map.side_sector(away_side(origin_side, line))?;
    let point = scale_shift(ray.origin, ray.normal, along, den);
    let distance = length_of(delta(wide(ray.origin), wide(point)));
    if distance == 0_i32 {
        return None;
    }
    Some(RayHit {
        fraction: (along.saturating_mul(along), den.saturating_mul(den)),
        distance,
        sector,
    })
}

/// `(determinant, t, u)` rewritten so the denominator is positive.
const fn positive_denominator(determinant: i128, along: i128, across: i128) -> (i128, i128, i128) {
    if determinant < 0_i128 {
        (
            determinant.saturating_neg(),
            along.saturating_neg(),
            across.saturating_neg(),
        )
    } else {
        (determinant, along, across)
    }
}

/// The side of the crossed line away from the ray origin: `cross(
/// towards, run)` is positive with the origin to the line's left, so
/// the far side is the front, and negative mirrors it. A zero sign
/// grazes along the line and bounds no side.
pub(super) const fn away_side(origin_side: i128, line: &Linedef) -> Option<usize> {
    if origin_side > 0_i128 {
        line.front
    } else if origin_side < 0_i128 {
        line.back
    } else {
        None
    }
}

/// Whether `hit` bounds the recess: within `max_depth` by exact
/// fraction, and behind a sector other than the one the ray travels
/// through.
fn qualifies(ray: &RecessRay, hit: &RayHit) -> bool {
    let cap = i128::from(ray.max_depth).saturating_mul(i128::from(ray.max_depth));
    hit.fraction.0.saturating_mul(ray.run_squared) <= cap.saturating_mul(hit.fraction.1)
        && ray.near.is_none_or(|sector| sector != hit.sector)
}
