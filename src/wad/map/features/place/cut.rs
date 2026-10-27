//! Cut placement: resolving a wall line, projecting a point onto it,
//! fitting a door width around the centre and composing the chosen
//! [`DoorWall`] with its recess rectangle.
//!
//! [`span`] resolves a linedef to its endpoints or refuses with an
//! instruction, [`project_onto_line`] lands a point on a wall (clamped,
//! the midpoint on request), [`door_segment`] cuts a `width`-wide chunk
//! around a centre, [`door_wall`] composes the projection and the fit
//! into one [`DoorWall`], and [`door_rect`] stands the `width * depth`
//! recess polygon on that cut for [`crate::wad::map::overlay`] to draw.

use crate::error::CyberError;
use crate::geom::segment::{Delta, cross, delta, dot, widen};
use crate::wad::map::model::Map;
use crate::wad::map::ops::Point;
use crate::wad::map::query::{ElementKind, check_index};

use super::{
    DoorWall, ORIGIN, VertexPair, endpoints, from_axes, half_axis, length_of, moved, scale_shift,
    wide,
};

/// The linedef `index` endpoints for a `role` that must exist: refuses
/// out-of-range lines with the shared [`check_index`] guard, the role
/// kept as message context, and dangling vertices with instructions.
///
/// [`check_index`]: crate::wad::map::query::check_index
pub(super) fn span(map: &Map, index: usize, role: &str) -> Result<(Point, Point), CyberError> {
    check_index(ElementKind::Linedefs, index, map.linedefs.len())
        .map_err(|error| error.map_message(|message| format!("{role} {message}")))?;
    endpoints(map, index).ok_or_else(|| {
        CyberError::invalid(&format!(
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
        return Err(CyberError::invalid(
            "the door width must be a positive number of map units; pass the width the \
             door texture covers",
        ));
    }
    let run = delta(wide(from), wide(to));
    let squared = dot(run, run);
    if squared == 0_i128 {
        return Err(CyberError::invalid(&format!(
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
        return Err(CyberError::invalid(&format!(
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
        return Err(CyberError::invalid(&format!(
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
///
/// [`outward_depth`]: super::outward_depth
#[must_use]
#[inline]
pub fn door_rect(wall: &DoorWall, normal: Point, depth: i32, from_room_side: bool) -> Vec<Point> {
    let perpendicular = widen(normal);
    let length = i128::from(length_of(perpendicular)).max(1_i128);
    let reach = if from_room_side {
        i128::from(depth).saturating_neg()
    } else {
        i128::from(depth)
    };
    let offset = widen(scale_shift(ORIGIN, perpendicular, reach, length));
    let centre = widen(wall.point);
    let half = widen(wall.half_dir);
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
