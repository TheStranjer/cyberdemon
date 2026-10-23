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

use alloc::collections::VecDeque;

use crate::error::CyberError;
use crate::fixed::{hypot, mul_div_round};
use crate::geom::Pt;
use crate::geom::segment::{Delta, cross, delta, dot, round_quotient};
use crate::wad::map::model::{Linedef, Map, Vertex};
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
    let (Some(front), Some(back)) = (side_sector(map, line.front), side_sector(map, line.back))
    else {
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
fn endpoints(map: &Map, index: usize) -> Option<(Point, Point)> {
    let line = map.linedefs.get(index)?;
    let start = map.vertices.get(line.v1)?;
    let finish = map.vertices.get(line.v2)?;
    Some((point_of(start), point_of(finish)))
}

/// A [`Point`] at a [`Vertex`]'s coordinates.
const fn point_of(vertex: &Vertex) -> Point {
    Point {
        x: vertex.x,
        y: vertex.y,
    }
}

/// A [`Point`] widened to the exact-arithmetic grid.
fn wide(point: Point) -> Pt {
    (i64::from(point.x), i64::from(point.y))
}

/// An exact [`Delta`] from [`Point`] axes.
fn vector(point: Point) -> Delta {
    (i128::from(point.x), i128::from(point.y))
}

/// The sector the sidedef `side` opens into: `None` when the side is
/// absent, its sidedef index dangles, or that sidedef's sector dangles.
fn side_sector(map: &Map, side: Option<usize>) -> Option<usize> {
    let sidedef = side.and_then(|index| map.sidedefs.get(index))?;
    map.sectors.get(sidedef.sector).map(|_| sidedef.sector)
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
    let length = hypot(saturate_i64(run.0), saturate_i64(run.1));
    i32::try_from(length).unwrap_or(i32::MAX)
}

/// An `i128` saturated into `i64` (deltas of `i32` coordinates never
/// leave `i64`; the saturation is defensive).
fn saturate_i64(value: i128) -> i64 {
    match i64::try_from(value) {
        Ok(narrow) => narrow,
        Err(_) if value < 0_i128 => i64::MIN,
        Err(_) => i64::MAX,
    }
}

/// An `i128` saturated into `i32`.
fn narrow_axis(value: i128) -> i32 {
    match i32::try_from(value) {
        Ok(narrow) => narrow,
        Err(_) if value < 0_i128 => i32::MIN,
        Err(_) => i32::MAX,
    }
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
    narrow_axis(round_quotient(scaled, den).unwrap_or(scaled))
}

/// One coordinate of the midpoint of `first` and `second`, halves
/// rounded away from zero.
fn half_axis(first: i32, second: i32) -> i32 {
    let sum = i128::from(first).saturating_add(i128::from(second));
    narrow_axis(round_quotient(sum, 2_i128).unwrap_or(sum))
}

/// The componentwise mean of two offset vectors, halves rounded away
/// from zero: a chain corner's bevelled strip offset.
fn half_vector(low: Delta, high: Delta) -> Delta {
    let mean = |sum: i128| round_quotient(sum, 2_i128).unwrap_or(sum);
    (
        mean(low.0.saturating_add(high.0)),
        mean(low.1.saturating_add(high.1)),
    )
}

/// A [`Point`] from exact axes, saturated into `i32`.
fn from_axes(x: i128, y: i128) -> Point {
    Point {
        x: narrow_axis(x),
        y: narrow_axis(y),
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

/// The linedef `index` endpoints for a `role` that must exist: refuses
/// out-of-range lines and dangling vertices with instructions.
fn span(map: &Map, index: usize, role: &str) -> Result<(Point, Point), CyberError> {
    let count = map.linedefs.len();
    if index >= count {
        return Err(invalid(&format!(
            "{role} linedef {index} is out of range: the map holds {count} of them; \
             re-read the map and pass current indices"
        )));
    }
    endpoints(map, index).ok_or_else(|| {
        invalid(&format!(
            "{role} linedef {index} references a vertex the map does not hold: resolve \
             its endpoints first"
        ))
    })
}

/// The `at` point projected onto linedef `linedef` (clamped to its
/// endpoints), or the segment's midpoint when `at` is `None`.
///
/// Projection is the exact `i128` dot-product fraction rounded to the
/// nearest whole map unit. A line the map cannot resolve (no such
/// linedef, or dangling vertices) has nothing to project onto: the
/// origin is reported for it, so [`door_segment`] and [`door_wall`]
/// refuse on their own index checks instead.
#[must_use]
#[inline]
pub fn project_onto_line(map: &Map, linedef: usize, at: Option<Point>) -> Point {
    let Some((from, to)) = endpoints(map, linedef) else {
        return ORIGIN;
    };
    let Some(point) = at else {
        return Point {
            x: half_axis(from.x, to.x),
            y: half_axis(from.y, to.y),
        };
    };
    let run = delta(wide(from), wide(to));
    let squared = dot(run, run);
    if squared == 0_i128 {
        return from;
    }
    let along = dot(delta(wide(from), wide(point)), run).clamp(0_i128, squared);
    scale_shift(from, run, along, squared)
}

/// A checked wall line for one [`door_segment`] call: the numbers the
/// on-line and fit tests share.
struct CutLine {
    /// Index in [`Map::linedefs`], for the messages.
    linedef: usize,
    /// The line's floored integer length in map units.
    available: i32,
    /// The `v1` to `v2` vector.
    run: Delta,
    /// The squared run length.
    squared: i128,
}

/// The `width`-wide chunk of linedef `linedef` centred on `centre`, as
/// its two endpoints and the wall line's available length.
///
/// The chunk runs along the line from `centre`, half the requested
/// width each way (each endpoint rounded to the nearest whole map unit
/// over twice the line's floored integer length). The centre must sit
/// on the line (within one map unit, the [`crate::wad::map::ops`]
/// tolerance) and the chunk must span no further than either endpoint.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] when the linedef index or its
/// vertices do not resolve, when `width` is not positive, when the line
/// is zero-length, when `centre` is off the line, or when the chunk
/// does not fit: every refusal message carries the line's available
/// length so the tool can reword it.
#[inline]
pub fn door_segment(
    map: &Map,
    linedef: usize,
    centre: Point,
    width: i32,
) -> Result<(VertexPair, i32), CyberError> {
    let (from, to) = span(map, linedef, "door")?;
    if width <= 0_i32 {
        return Err(invalid(
            "the door width must be a positive number of map units; pass the width the \
             door texture covers",
        ));
    }
    let run = delta(wide(from), wide(to));
    let squared = dot(run, run);
    if squared == 0_i128 {
        return Err(invalid(&format!(
            "linedef {linedef} is zero-length: it cannot carry a door; delete it or \
             move a vertex first"
        )));
    }
    let cut = CutLine {
        linedef,
        available: length_of(run),
        run,
        squared,
    };
    let offset = delta(wide(from), wide(centre));
    require_on_line(&cut, centre, offset)?;
    require_fits(&cut, offset, width)?;
    let span_vector = i128::from(width);
    let twice = i128::from(cut.available).saturating_mul(2_i128);
    let start = scale_shift(centre, run, span_vector.saturating_neg(), twice);
    let finish = scale_shift(centre, run, span_vector, twice);
    Ok(((start, finish), cut.available))
}

/// Refuses a `centre` more than one map unit off the cut line: the
/// squared-cross-product test [`crate::wad::map::ops::split_linedef`]
/// uses.
fn require_on_line(cut: &CutLine, centre: Point, offset: Delta) -> Result<(), CyberError> {
    let perpendicular = cross(cut.run, offset);
    if perpendicular.saturating_mul(perpendicular) > cut.squared {
        return Err(invalid(&format!(
            "the door centre {}, {} is more than one map unit off linedef {} (it is {} \
             map units long): project the point with project_onto_line first",
            centre.x, centre.y, cut.linedef, cut.available
        )));
    }
    Ok(())
}

/// Refuses a `width` chunk centred at the run fraction `offset` that
/// would reach past either endpoint of the line.
fn require_fits(cut: &CutLine, offset: Delta, width: i32) -> Result<(), CyberError> {
    let along = dot(offset, cut.run);
    let need = i128::from(width)
        .saturating_mul(i128::from(width))
        .saturating_mul(cut.squared);
    let low = along.saturating_mul(along).saturating_mul(4_i128);
    let rest = cut.squared.saturating_sub(along);
    let high = rest.saturating_mul(rest).saturating_mul(4_i128);
    if along < 0_i128 || along > cut.squared || low < need || high < need {
        return Err(invalid(&format!(
            "a {width}-unit door does not fit linedef {} centred here: the wall is only \
             {available} map units long; shrink the width or pick another wall \
             (nearest_boundary reports the candidates)",
            cut.linedef,
            available = cut.available
        )));
    }
    Ok(())
}

/// The chosen wall with its cut: [`project_onto_line`] and
/// [`door_segment`] composed.
///
/// `at` projects onto the line (its midpoint serves when `None`) and
/// the width fit is checked around that centre;
/// [`DoorWall::half_dir`] points from the centre to the cut's far
/// endpoint, so the cut ends are `point - half_dir` and
/// `point + half_dir`.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] through [`door_segment`].
#[inline]
pub fn door_wall(
    map: &Map,
    linedef: usize,
    at: Option<Point>,
    width: i32,
) -> Result<DoorWall, CyberError> {
    let centre = project_onto_line(map, linedef, at);
    let ((_, finish), available) = door_segment(map, linedef, centre, width)?;
    Ok(DoorWall {
        linedef,
        point: centre,
        half_dir: Point {
            x: finish.x.saturating_sub(centre.x),
            y: finish.y.saturating_sub(centre.y),
        },
        available_width: available,
    })
}

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
        near: side_sector(map, line.front),
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
    let sector = side_sector(map, away_side(origin_side, line))?;
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
const fn away_side(origin_side: i128, line: &Linedef) -> Option<usize> {
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

/// The recess polygon: the door cut's `width * depth` rectangle, four
/// corners, ordered counter-clockwise so the winding runs toward the
/// door's far side whatever way the wall line faces.
///
/// The near edge is the cut itself, `wall.point` plus or minus
/// `wall.half_dir`; the far edge sits `depth` whole map units along
/// `normal`: any outward perpendicular, rounded to its own unit length
/// before the depth is applied (the [`outward_depth`] ray direction
/// serves, as does the line's raw `(dy, -dx)`). Set `from_room_side`
/// when the room sits in front of the normal and the rectangle grows
/// the other way. A zero-length `normal` collapses the rectangle onto
/// the cut.
#[must_use]
#[inline]
pub fn door_rect(wall: &DoorWall, normal: Point, depth: i32, from_room_side: bool) -> Vec<Point> {
    let perpendicular = vector(normal);
    let length = i128::from(length_of(perpendicular)).max(1_i128);
    let reach = if from_room_side {
        i128::from(depth).saturating_neg()
    } else {
        i128::from(depth)
    };
    let offset = vector(scale_shift(ORIGIN, perpendicular, reach, length));
    let centre = vector(wall.point);
    let half = vector(wall.half_dir);
    let near_left = from_axes(
        centre.0.saturating_sub(half.0),
        centre.1.saturating_sub(half.1),
    );
    let near_right = from_axes(
        centre.0.saturating_add(half.0),
        centre.1.saturating_add(half.1),
    );
    let far_right = moved(near_right, offset);
    let far_left = moved(near_left, offset);
    if cross(half, offset) >= 0_i128 {
        vec![near_left, near_right, far_right, far_left]
    } else {
        vec![near_left, far_left, far_right, near_right]
    }
}

/// The shared boundary `boundaries` thickened into one carve strip:
/// every line offset half of `thickness` toward each side, joined into
/// one polygon for [`crate::wad::map::overlay::apply_overlay`].
///
/// The lines are ordered into a connected run (each re-oriented to
/// follow its neighbours); corners join by averaging the two segments'
/// offsets, so an L-shaped boundary carves cleanly and a straight run
/// of several lines merges into one full-length band. The polygon
/// spans the whole thickness.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] when `boundaries` is empty, when
/// `thickness` is not positive, when a linedef index or one of its
/// vertices does not resolve, when a line is zero-length, or when the
/// lines do not form one connected run: carve each disconnected
/// boundary run with its own call then.
#[inline]
pub fn void_strip(
    map: &Map,
    boundaries: &[usize],
    thickness: i32,
) -> Result<BoundaryStrip, CyberError> {
    let half = checked_half_thickness(thickness)?;
    let ordered = boundary_chain(map, boundaries)?;
    Ok(BoundaryStrip {
        polygon: strip_polygon(&ordered, half),
    })
}

/// Half of `thickness`, rounded to nearest (halves away from zero), so
/// the strip spans the full requested thickness across.
fn checked_half_thickness(thickness: i32) -> Result<i32, CyberError> {
    if thickness <= 0_i32 {
        return Err(invalid(
            "the void strip thickness must be a positive number of map units; the door \
             tools carve with 16",
        ));
    }
    let half =
        mul_div_round(i64::from(thickness), 1_i64, 2_i64).unwrap_or_else(|| i64::from(thickness));
    Ok(i32::try_from(half).unwrap_or(i32::MAX))
}

/// The boundary lines ordered into one connected vertex chain, each
/// re-oriented to follow its neighbour.
fn boundary_chain(map: &Map, boundaries: &[usize]) -> Result<VecDeque<Point>, CyberError> {
    if boundaries.is_empty() {
        return Err(invalid(
            "no boundary linedefs were given: pass the lines longest_boundary reports",
        ));
    }
    let mut open: Vec<(usize, Point, Point)> = Vec::with_capacity(boundaries.len());
    for position in boundaries {
        let (from, to) = span(map, *position, "boundary")?;
        if from == to {
            return Err(invalid(&format!(
                "boundary linedef {position} is zero-length: it offsets to nothing; \
                 leave it out of the boundary"
            )));
        }
        open.push((*position, from, to));
    }
    let (first, head, tail) = open.remove(0);
    let mut ordered: VecDeque<Point> = VecDeque::new();
    ordered.push_back(head);
    ordered.push_back(tail);
    let mut ends = (head, tail);
    while chain_step(&mut open, &mut ordered, &mut ends) {}
    if let Some(&(index, _, _)) = open.first() {
        return Err(invalid(&format!(
            "boundary linedef {index} does not touch the run started by linedef {first}: \
             the shared boundary is not one connected run; carve each run with its own \
             void_strip call"
        )));
    }
    Ok(ordered)
}

/// Extend the chain by any open line touching an end, preferring the
/// tail (a line may attach to either end in either orientation); false
/// when nothing touches the run.
fn chain_step(
    open: &mut Vec<(usize, Point, Point)>,
    ordered: &mut VecDeque<Point>,
    ends: &mut (Point, Point),
) -> bool {
    let (head, tail) = *ends;
    let position = open
        .iter()
        .position(|&(_, from, to)| from == tail || to == tail || from == head || to == head);
    let Some(found) = position else {
        return false;
    };
    let (_, from, to) = open.swap_remove(found);
    if from == tail {
        ordered.push_back(to);
        ends.1 = to;
    } else if to == tail {
        ordered.push_back(from);
        ends.1 = from;
    } else if from == head {
        ordered.push_front(to);
        ends.0 = to;
    } else {
        ordered.push_front(from);
        ends.0 = from;
    }
    true
}

/// The strip polygon of the ordered chain: every vertex offset by the
/// bevelled mean of its segments' half-thickness normals, one side
/// walked forward and the other back.
fn strip_polygon(ordered: &VecDeque<Point>, half: i32) -> Vec<Point> {
    let vertices: Vec<Point> = ordered.iter().copied().collect();
    let offsets: Vec<Delta> = vertices
        .iter()
        .zip(vertices.iter().skip(1))
        .map(|(from, to)| edge_offset(*from, *to, half))
        .collect();
    let bevels = bevelled(&offsets);
    let mut polygon: Vec<Point> = Vec::with_capacity(vertices.len().saturating_mul(2));
    for (vertex, bevel) in vertices.iter().zip(bevels.iter()) {
        polygon.push(moved(*vertex, *bevel));
    }
    for (vertex, bevel) in vertices.iter().zip(bevels.iter()).rev() {
        polygon.push(moved(
            *vertex,
            (bevel.0.saturating_neg(), bevel.1.saturating_neg()),
        ));
    }
    polygon
}

/// The chain vertex offsets: each interior vertex gets the mean of its
/// two segments' offsets, the ends keep their own.
fn bevelled(offsets: &[Delta]) -> Vec<Delta> {
    let Some(first) = offsets.first().copied() else {
        return Vec::new();
    };
    let mut bevels: Vec<Delta> = Vec::with_capacity(offsets.len().saturating_add(1));
    bevels.push(first);
    let mut previous = first;
    for current in offsets.iter().skip(1).copied() {
        bevels.push(half_vector(previous, current));
        previous = current;
    }
    bevels.push(offsets.last().copied().unwrap_or(first));
    bevels
}

/// The half-thickness offset of one chain segment: its front-side
/// perpendicular scaled to `half` map units, rounded.
fn edge_offset(from: Point, to: Point, half: i32) -> Delta {
    let run = delta(wide(from), wide(to));
    let perpendicular = (run.1, run.0.saturating_neg());
    let length = i128::from(length_of(run)).max(1_i128);
    let point = scale_shift(ORIGIN, perpendicular, i128::from(half), length);
    (i128::from(point.x), i128::from(point.y))
}

#[cfg(test)]
mod tests {
    use super::{
        BoundaryStrip, DoorWall, ORIGIN, away_side, door_rect, door_segment, door_wall, endpoints,
        longest_boundary, nearest_boundary, outward_depth, project_onto_line, void_strip,
    };
    use crate::error::CyberError;
    use crate::wad::classify::MapFormat;
    use crate::wad::map::common::fixtures::{side, two_room_map, vertex};
    use crate::wad::map::model::{Linedef, Map, Sector};
    use crate::wad::map::ops::Point;

    /// Whether `error` is the `InvalidArgument` mentioning `needle`.
    fn refuses(error: CyberError, needle: &str) -> bool {
        let CyberError::InvalidArgument(message) = error else {
            panic!("not an InvalidArgument: {error:?}");
        };
        message.contains(needle)
    }

    /// A point at `x`/`y`.
    const fn point(x: i32, y: i32) -> Point {
        Point { x, y }
    }

    /// A line from vertex `v1` to `v2` with the given sides.
    fn line(v1: usize, v2: usize, front: Option<usize>, back: Option<usize>) -> Linedef {
        Linedef {
            v1,
            v2,
            flags: 4_u32,
            front,
            back,
            ..Default::default()
        }
    }

    /// `count` plain sectors.
    fn sectors(count: usize) -> Vec<Sector> {
        vec![Sector::default(); count]
    }

    /// Two rooms sharing the vertical line `(128, 0)` to `(128, 256)`:
    /// sector 0 on its front (left) side, sector 1 behind.
    fn shared_line_map() -> Map {
        Map {
            format: MapFormat::Doom,
            vertices: vec![vertex(128, 0), vertex(128, 256)],
            linedefs: vec![line(0, 1, Some(0), Some(1))],
            sidedefs: vec![side("-", "-", "-", 0), side("-", "-", "-", 1)],
            sectors: sectors(2),
            ..Map::default()
        }
    }

    /// The same rooms sharing an L: `(0, 0)`-`(128, 0)` (linedef 0) and
    /// `(128, 0)`-`(128, 128)` (linedef 1).
    fn shared_l_map() -> Map {
        Map {
            format: MapFormat::Doom,
            vertices: vec![vertex(0, 0), vertex(128, 0), vertex(128, 128)],
            linedefs: vec![line(0, 1, Some(0), Some(1)), line(1, 2, Some(0), Some(1))],
            sidedefs: vec![side("-", "-", "-", 0), side("-", "-", "-", 1)],
            sectors: sectors(2),
            ..Map::default()
        }
    }

    /// A horizontal wall `(0, 0)` to `(128, 0)` between sectors 0 and 1.
    fn wide_wall_map() -> Map {
        Map {
            format: MapFormat::Doom,
            vertices: vec![vertex(0, 0), vertex(128, 0)],
            linedefs: vec![line(0, 1, Some(0), Some(1))],
            sidedefs: vec![side("-", "-", "-", 0), side("-", "-", "-", 1)],
            sectors: sectors(2),
            ..Map::default()
        }
    }

    /// The recess fixture: room sector 0 north of the east-running
    /// boundary `(0, 128)`-`(256, 128)` (its back side), hall sector 1
    /// south of it (its front side), and a free far row at `y = 96`
    /// (vertices 2 and 3) for tests to wall across.
    fn recess_map() -> Map {
        Map {
            format: MapFormat::Doom,
            vertices: vec![
                vertex(0, 128),
                vertex(256, 128),
                vertex(0, 96),
                vertex(256, 96),
            ],
            linedefs: vec![line(0, 1, Some(0), Some(1))],
            sidedefs: vec![side("-", "-", "-", 1), side("-", "-", "-", 0)],
            sectors: sectors(4),
            ..Map::default()
        }
    }

    #[test]
    fn longest_boundary_lists_the_shared_wall_in_order() {
        let map = two_room_map();
        assert_eq!(
            longest_boundary(&map, 0, 1),
            Some(vec![2_usize]),
            "the two-room fixture shares only the step line",
        );
        assert_eq!(
            longest_boundary(&map, 1, 0),
            Some(vec![2_usize]),
            "sector order does not matter",
        );
        assert_eq!(longest_boundary(&map, 0, 5), None, "no sector 5 exists");
        assert_eq!(
            longest_boundary(&map, 0, 0),
            None,
            "a sector shares no line with itself",
        );
        let l = shared_l_map();
        assert_eq!(
            longest_boundary(&l, 0, 1),
            Some(vec![0_usize, 1]),
            "both legs of the L are the shared boundary",
        );
    }

    #[test]
    fn nearest_boundary_picks_the_closest_leg_and_lowest_index_on_ties() {
        let map = shared_l_map();
        assert_eq!(
            nearest_boundary(&map, 0, 1, point(64, -16)),
            Some(0_usize),
            "the horizontal leg is below",
        );
        assert_eq!(
            nearest_boundary(&map, 0, 1, point(144, 64)),
            Some(1_usize),
            "the vertical leg is right",
        );
        assert_eq!(
            nearest_boundary(&map, 0, 1, point(0, 128)),
            Some(0_usize),
            "128 units to both legs: the lower index wins",
        );
        assert_eq!(
            nearest_boundary(&map, 0, 5, point(0, 0)),
            None,
            "no shared boundary to be near",
        );
        let single = shared_line_map();
        assert_eq!(
            nearest_boundary(&single, 0, 1, point(500, 500)),
            Some(0_usize),
            "the only line is the nearest one",
        );
    }

    #[test]
    fn project_onto_line_places_on_off_and_clamped_points() {
        let map = shared_line_map();
        assert_eq!(
            project_onto_line(&map, 0, Some(point(128, 100))),
            point(128, 100),
            "a point on the line stands",
        );
        assert_eq!(
            project_onto_line(&map, 0, Some(point(64, 100))),
            point(128, 100),
            "an off-line point lands perpendicular",
        );
        assert_eq!(
            project_onto_line(&map, 0, Some(point(10_000, -50))),
            point(128, 0),
            "below v1: clamped to it",
        );
        assert_eq!(
            project_onto_line(&map, 0, Some(point(0, 500))),
            point(128, 256),
            "past v2: clamped to it",
        );
    }

    #[test]
    fn project_onto_line_defaults_to_the_midpoint_and_survives_nothing() {
        let map = shared_line_map();
        assert_eq!(
            project_onto_line(&map, 0, None),
            point(128, 128),
            "the segment midpoint",
        );
        assert_eq!(
            project_onto_line(&map, 9, None),
            ORIGIN,
            "a line that does not resolve has nothing to project onto",
        );
        let flat = Map {
            vertices: vec![vertex(7, 9), vertex(7, 9)],
            ..shared_line_map()
        };
        assert_eq!(
            project_onto_line(&flat, 0, Some(point(100, 100))),
            point(7, 9),
            "a zero-length line projects onto its own vertex",
        );
    }

    #[test]
    fn door_segment_fits_the_exact_line_width() {
        let map = wide_wall_map();
        let ((start, finish), available) =
            door_segment(&map, 0, point(64, 0), 128_i32).expect("the whole wall");
        assert_eq!(start, point(0, 0), "to v1");
        assert_eq!(finish, point(128, 0), "to v2");
        assert_eq!(available, 128_i32, "the integer length");
        let small = door_segment(&map, 0, point(100, 0), 48_i32).expect("an off-centre door");
        assert_eq!(
            small.0,
            (point(76, 0), point(124, 0)),
            "48 units around 100"
        );
    }

    #[test]
    fn door_segment_refuses_widths_past_the_wall_with_its_length() {
        let map = wide_wall_map();
        let error = door_segment(&map, 0, point(64, 0), 129_i32).expect_err("one unit too wide");
        assert_eq!(error.code(), "invalid_argument", "fit refusal");
        assert!(refuses(error, "128"), "the message carries the length");
        let off_centre =
            door_segment(&map, 0, point(16, 0), 64_i32).expect_err("the chunk runs past v1");
        assert!(refuses(off_centre, "128"), "and says so with the length");
    }

    #[test]
    fn door_segment_refuses_bad_walls_centres_and_widths() {
        let map = wide_wall_map();
        let missing = door_segment(&map, 9, point(64, 0), 32_i32).expect_err("no linedef 9");
        assert!(refuses(missing, "linedef 9"), "the index is named");
        let mut dangling = map.clone();
        dangling.linedefs.get_mut(0).expect("the line").v2 = 9;
        let broken = door_segment(&dangling, 0, point(64, 0), 32_i32).expect_err("vertex 9 gone");
        assert!(refuses(broken, "vertex"), "the dangling vertex is named");
        let flat = Map {
            vertices: vec![vertex(0, 0), vertex(0, 0)],
            ..wide_wall_map()
        };
        let zero = door_segment(&flat, 0, point(0, 0), 32_i32).expect_err("nothing to span");
        assert!(refuses(zero, "zero-length"), "the degenerate line");
        let off = door_segment(&map, 0, point(64, 20), 32_i32).expect_err("centre off the wall");
        assert!(refuses(off, "project_onto_line"), "the fix is named");
        let none = door_segment(&map, 0, point(64, 0), 0_i32).expect_err("no such door");
        assert!(refuses(none, "positive"), "the width rule");
    }

    #[test]
    fn door_wall_composes_projection_and_fit() {
        let map = wide_wall_map();
        let error = door_wall(&map, 0, Some(point(200, 10)), 64_i32)
            .expect_err("clamped onto v2 there is no room");
        assert!(refuses(error, "128"), "the fit refusal names the length");
        let centred = door_wall(&map, 0, None, 64_i32).expect("the midpoint fits");
        assert_eq!(centred.linedef, 0_usize, "the chosen line");
        assert_eq!(centred.point, point(64, 0), "the line midpoint");
        assert_eq!(
            centred.half_dir,
            point(32, 0),
            "half the door, along the wall"
        );
        assert_eq!(centred.available_width, 128_i32, "the whole wall");
        let off = door_wall(&map, 0, Some(point(100, 2)), 48_i32).expect("a projected door");
        assert_eq!(off.point, point(100, 0), "projected back onto the line");
        assert_eq!(off.half_dir, point(24, 0), "half of 48");
    }

    #[test]
    fn outward_depth_reaches_the_room_behind_the_far_wall() {
        let mut map = recess_map();
        map.sidedefs.push(side("-", "-", "-", 2));
        map.linedefs.push(line(2, 3, Some(2), None));
        let hit = outward_depth(&map, 0, point(128, 128), 64_i32).expect("the far wall");
        assert_eq!(hit.0, 32_i32, "the hall is 32 units deep");
        assert_eq!(hit.1, 2_usize, "the sector beyond the wall");
    }

    #[test]
    fn outward_depth_finds_nothing_over_open_void_or_behind_a_bare_wall() {
        let void = recess_map();
        assert_eq!(
            outward_depth(&void, 0, point(128, 128), 256_i32),
            None,
            "nothing bounds a sector south of the boundary",
        );
        let mut faced_at_ray = recess_map();
        faced_at_ray.sidedefs.push(side("-", "-", "-", 2));
        faced_at_ray.linedefs.push(line(3, 2, Some(2), None));
        assert_eq!(
            outward_depth(&faced_at_ray, 0, point(128, 128), 64_i32),
            None,
            "a wall faced at the ray bounds nothing beyond it",
        );
        let mut back_only = recess_map();
        back_only.sidedefs.push(side("-", "-", "-", 2));
        back_only.linedefs.push(line(2, 3, None, Some(2)));
        assert_eq!(
            outward_depth(&back_only, 0, point(128, 128), 64_i32),
            None,
            "the far side is void there",
        );
    }

    #[test]
    fn outward_depth_skips_lines_that_bound_the_crossed_sector() {
        let mut map = recess_map();
        map.sidedefs.push(side("-", "-", "-", 1));
        map.sidedefs.push(side("-", "-", "-", 1));
        map.linedefs.push(line(2, 3, Some(2), Some(3))); // a split inside the hall
        map.sidedefs.push(side("-", "-", "-", 2));
        map.linedefs.push(line(2, 3, Some(4), None)); // the far wall
        let hit = outward_depth(&map, 0, point(128, 128), 64_i32).expect("the far wall");
        assert_eq!(
            hit,
            (32_i32, 2_usize),
            "the same-sector split line does not bound the recess",
        );
    }

    #[test]
    fn outward_depth_reports_the_nearest_qualifying_wall() {
        let mut map = recess_map();
        map.vertices.push(vertex(0, 112));
        map.vertices.push(vertex(256, 112));
        map.sidedefs.push(side("-", "-", "-", 3));
        map.linedefs.push(line(4, 5, Some(2), None)); // near, sector 3
        map.sidedefs.push(side("-", "-", "-", 2));
        map.linedefs.push(line(2, 3, Some(3), None)); // far, sector 2
        let hit = outward_depth(&map, 0, point(128, 128), 64_i32).expect("a wall");
        assert_eq!(hit.0, 16_i32, "the closer wall wins");
        assert_eq!(hit.1, 3_usize, "and reports its sector");
    }

    #[test]
    fn outward_depth_stops_at_the_reach_and_on_unresolved_lines() {
        let mut map = recess_map();
        map.sidedefs.push(side("-", "-", "-", 2));
        map.linedefs.push(line(2, 3, Some(2), None));
        assert_eq!(
            outward_depth(&map, 0, point(128, 128), 16_i32),
            None,
            "32 units is past the 16-unit reach",
        );
        assert_eq!(
            outward_depth(&map, 0, point(128, 128), 0_i32),
            None,
            "no reach, no answer",
        );
        assert_eq!(
            outward_depth(&map, 9, point(128, 128), 64_i32),
            None,
            "no such boundary line",
        );
        assert_eq!(
            outward_depth(&Map::default(), 0, point(128, 128), 64_i32),
            None,
            "an empty map casts nowhere",
        );
    }

    #[test]
    fn away_side_names_the_front_for_a_left_of_line_origin() {
        let probe = line(0, 1, Some(7), Some(9));
        assert_eq!(away_side(1_i128, &probe), Some(7_usize), "left: front");
        assert_eq!(away_side(-1_i128, &probe), Some(9_usize), "right: back");
        assert_eq!(away_side(0_i128, &probe), None, "grazing: no side");
    }

    /// The chosen 64-wide cut on the horizontal wall, for the polygon
    /// tests.
    fn sample_wall() -> DoorWall {
        DoorWall {
            linedef: 0_usize,
            point: point(64, 0),
            half_dir: point(32, 0),
            available_width: 128_i32,
        }
    }

    /// Twice the signed area of `polygon` in `i128` (shoelace).
    fn area2(polygon: &[Point]) -> i128 {
        polygon
            .iter()
            .zip(polygon.iter().skip(1).chain(polygon.iter()))
            .fold(0_i128, |total, (first, second)| {
                let x1 = i128::from(first.x);
                let y1 = i128::from(first.y);
                let x2 = i128::from(second.x);
                let y2 = i128::from(second.y);
                total
                    .saturating_add(x1.saturating_mul(y2))
                    .saturating_sub(x2.saturating_mul(y1))
            })
    }

    #[test]
    fn door_rect_grows_away_from_the_room_and_winds_consistently() {
        let wall = sample_wall();
        let south = door_rect(&wall, point(0, -128), 16_i32, false);
        assert_eq!(
            south,
            vec![point(32, 0), point(32, -16), point(96, -16), point(96, 0)],
            "near-left first, counter-clockwise toward the far side",
        );
        assert!(area2(&south) > 0_i128, "counter-clockwise");
    }

    #[test]
    fn door_rect_flips_across_the_line_when_the_room_is_in_front() {
        let wall = sample_wall();
        let north = door_rect(&wall, point(0, -128), 16_i32, true);
        assert_eq!(
            north,
            vec![point(32, 0), point(96, 0), point(96, 16), point(32, 16)],
            "the same walk grown along the -normal",
        );
        assert!(area2(&north) > 0_i128, "and wound the same way");
    }

    #[test]
    fn door_rect_collapses_onto_the_cut_without_a_normal() {
        let wall = sample_wall();
        let flat = door_rect(&wall, ORIGIN, 16_i32, false);
        assert_eq!(flat.first(), flat.last(), "the rectangle degenerated");
        assert_eq!(area2(&flat), 0_i128, "no area to paint");
    }

    #[test]
    fn void_strip_offsets_a_straight_boundary_to_the_full_thickness() {
        let map = wide_wall_map();
        let strip = void_strip(&map, &[0_usize], 16_i32).expect("one clean line");
        assert_eq!(
            strip,
            BoundaryStrip {
                polygon: vec![point(0, -8), point(128, -8), point(128, 8), point(0, 8)],
            },
            "a 128 by 16 band centred on the wall",
        );
    }

    #[test]
    fn void_strip_bevels_an_l_shared_boundary_into_one_polygon() {
        let map = shared_l_map();
        let strip = void_strip(&map, &[0_usize, 1], 16_i32).expect("a connected L");
        assert_eq!(
            strip.polygon,
            vec![
                point(0, -8),
                point(132, -4),
                point(136, 128),
                point(120, 128),
                point(124, 4),
                point(0, 8),
            ],
            "one bevelled hexagon, not two crossing rectangles",
        );
        assert!(area2(&strip.polygon) > 0_i128, "a real area");
        let flipped = void_strip(&map, &[1_usize, 0], 16_i32).expect("reversed input");
        assert_eq!(
            flipped.polygon, strip.polygon,
            "the chain joins either way round",
        );
    }

    #[test]
    fn void_strip_refuses_empty_thin_and_broken_boundaries() {
        let map = wide_wall_map();
        let empty = void_strip(&map, &[], 16_i32).expect_err("nothing to thicken");
        assert!(refuses(empty, "longest_boundary"), "the fix is named");
        let thin = void_strip(&map, &[0_usize], 0_i32).expect_err("no such thickness");
        assert!(refuses(thin, "positive"), "the rule is named");
        let missing = void_strip(&map, &[9_usize], 16_i32).expect_err("no linedef 9");
        assert!(refuses(missing, "linedef 9"), "the index is named");
        let flat = Map {
            vertices: vec![vertex(0, 0), vertex(0, 0)],
            ..wide_wall_map()
        };
        let degenerate = void_strip(&flat, &[0_usize], 16_i32).expect_err("a point, not a wall");
        assert!(refuses(degenerate, "zero-length"), "the degenerate line");
        let broken = Map {
            vertices: vec![
                vertex(0, 0),
                vertex(128, 0),
                vertex(500, 500),
                vertex(700, 700),
            ],
            linedefs: vec![line(0, 1, Some(0), Some(1)), line(2, 3, Some(0), Some(1))],
            ..wide_wall_map()
        };
        let apart = void_strip(&broken, &[0_usize, 1], 16_i32).expect_err("two runs");
        assert!(refuses(apart, "void_strip call"), "carve each run apart");
    }

    #[test]
    fn endpoints_resolve_only_complete_lines() {
        let map = wide_wall_map();
        assert_eq!(
            endpoints(&map, 0),
            Some((point(0, 0), point(128, 0))),
            "a resolvable line",
        );
        assert_eq!(endpoints(&map, 1), None, "no linedef 1");
    }
}
