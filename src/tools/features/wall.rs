//! The wall choice and void-strip machinery `add_door` and `add_window`
//! share: resolving a [`crate::tools::features::door::Target`] against
//! the map into a [`crate::tools::features::door::Chosen`] wall
//! ([`choose_wall`], [`choose_between`], [`choose_linedef`]), carving the
//! 16-unit void strip that pulls touching rooms apart ([`carve_strip`]
//! through [`void_request`], re-finding the room's face with
//! [`strip_wall`]), fitting the rectangle to paint ([`place_rect`] over
//! [`crate::wad::map::features::door_wall`] and
//! [`crate::wad::map::features::door_rect`]) and the integer point maths
//! ([`centroid`], [`half_axis`], [`outward_normal`]) all of it runs
//! through.

use crate::error::CyberError;
use crate::geom::Pt;
use crate::geom::segment::{Delta, round_quotient};
use crate::wad::map::features::{
    DoorWall, door_rect, door_wall, longest_boundary, nearest_boundary, outward_depth,
    project_onto_line, void_strip,
};
use crate::wad::map::geometry::{linedef_length, point_in_sector};
use crate::wad::map::ops::Point;
use crate::wad::map::overlay::{self, Outside, OverlayRequest, Paint, WallTextures};
use crate::wad::map::{Linedef, Map};

use super::door::{Chosen, DoorSpec, MAX_DEPTH, Placed, Target};
use super::{invalid, no_room, out_of_range, wall_lost};

/// The void strip `add_door` and `add_window` carve between touching
/// rooms, in map units.
pub(super) const STRIP_THICKNESS: i32 = 16_i32;

/// Divisor turning a four-corner sum into the polygon centroid.
const CENTROID_DIVISOR: i128 = 4_i128;

/// Divisor of the two-endpoint sum that gives a midpoint coordinate.
const MIDPOINT_DIVISOR: i128 = 2_i128;

/// The sector the sidedef `slot` opens into, when sidedef and sector
/// both resolve.
pub(super) fn side_sector(map: &Map, slot: Option<usize>) -> Option<usize> {
    let sector = slot.and_then(|index| map.sidedefs.get(index))?.sector;
    map.sectors.get(sector).map(|_| sector)
}

/// The two endpoint coordinates of linedef `index`, when both vertices
/// resolve.
fn line_ends(map: &Map, index: usize) -> Option<(Point, Point)> {
    let line = map.linedefs.get(index)?;
    let start = map.vertices.get(line.v1)?;
    let finish = map.vertices.get(line.v2)?;
    Some((
        Point {
            x: start.x,
            y: start.y,
        },
        Point {
            x: finish.x,
            y: finish.y,
        },
    ))
}

/// The `v1` to `v2` vector of linedef `index` as an exact `i128` delta.
fn wall_run(map: &Map, index: usize) -> Option<Delta> {
    let (from, to) = line_ends(map, index)?;
    Some((
        i128::from(to.x).saturating_sub(i128::from(from.x)),
        i128::from(to.y).saturating_sub(i128::from(from.y)),
    ))
}

/// The front-side outward perpendicular `(dy, -dx)` of linedef `index`:
/// the [`outward_depth`] ray direction, pointing at the line's front.
pub(super) fn outward_normal(map: &Map, index: usize) -> Option<Point> {
    let run = wall_run(map, index)?;
    Some(Point {
        x: narrow(run.1),
        y: narrow(run.0.saturating_neg()),
    })
}

/// An exact `i128` axis saturated into `i32`.
fn narrow(value: i128) -> i32 {
    i32::try_from(value).unwrap_or(i32::MAX)
}

/// The mean of two coordinates, halves rounded away from zero and
/// saturated back into `i32`.
pub(super) fn half_axis(first: i32, second: i32) -> i32 {
    let sum = i128::from(first).saturating_add(i128::from(second));
    narrow(round_quotient(sum, MIDPOINT_DIVISOR).unwrap_or(sum))
}

/// The centroid of a polygon: the mean of its corner coordinates.
pub(super) fn centroid(points: &[Point]) -> Point {
    let mut sum_x = 0_i128;
    let mut sum_y = 0_i128;
    for corner in points {
        sum_x = sum_x.saturating_add(i128::from(corner.x));
        sum_y = sum_y.saturating_add(i128::from(corner.y));
    }
    Point {
        x: narrow(round_quotient(sum_x, CENTROID_DIVISOR).unwrap_or(sum_x)),
        y: narrow(round_quotient(sum_y, CENTROID_DIVISOR).unwrap_or(sum_y)),
    }
}

/// The squared distance from `from` to the midpoint of linedef `index`,
/// `None` when its vertices do not resolve.
fn mid_distance_sq(map: &Map, index: usize, from: Point) -> Option<i128> {
    let (a, b) = line_ends(map, index)?;
    let dx = i128::from(half_axis(a.x, b.x)).saturating_sub(i128::from(from.x));
    let dy = i128::from(half_axis(a.y, b.y)).saturating_sub(i128::from(from.y));
    Some(dx.saturating_mul(dx).saturating_add(dy.saturating_mul(dy)))
}

/// Whether the line's front faces `room` and nothing sits behind it: a
/// one-sided wall of that room.
fn room_face(map: &Map, line: &Linedef, room: usize) -> bool {
    side_sector(map, line.front) == Some(room) && side_sector(map, line.back).is_none()
}

/// The door wall for `between` sectors: their shared boundary's nearest
/// line to `at` (else its longest), with both sector indices validated.
pub(super) fn choose_between(
    map: &Map,
    a: usize,
    b: usize,
    at: Option<Point>,
) -> Result<Chosen, CyberError> {
    if map.sectors.get(a).is_none() {
        return Err(out_of_range("sector", a, map.sectors.len()));
    }
    if map.sectors.get(b).is_none() {
        return Err(out_of_range("sector", b, map.sectors.len()));
    }
    let boundary = longest_boundary(map, a, b).ok_or_else(|| {
        invalid(&format!(
            "sectors {a} and {b} share no two-sided boundary: draw the rooms touching (or \
             give the door wall's linedef index); get_map_summary lists each sector's \
             neighbours"
        ))
    })?;
    let line = at
        .map_or_else(
            || longest_line(map, &boundary),
            |point| nearest_boundary(map, a, b, point),
        )
        .ok_or_else(|| {
            invalid(&format!(
                "no line of the shared boundary of sectors {a} and {b} has resolvable \
                 vertices: repair the dangling vertex references first"
            ))
        })?;
    Ok(Chosen {
        room_a: a,
        room_b: b,
        wall: line,
        centre: project_onto_line(map, line, at),
        touching: true,
    })
}

/// The longest line of `boundary` by floored integer length, falling
/// back to the lowest index when none resolves.
fn longest_line(map: &Map, boundary: &[usize]) -> Option<usize> {
    let mut best: Option<(u64, usize)> = None;
    for index in boundary {
        let Some(length) = linedef_length(map, *index) else {
            continue;
        };
        if best.is_none_or(|(known, _)| length > known) {
            best = Some((length, *index));
        }
    }
    best.map(|(_, index)| index)
        .or_else(|| boundary.iter().copied().next())
}

/// The door wall for an explicit `linedef`: two-sided lines behave like
/// [`choose_between`] on their own sectors (honouring the given wall),
/// one-sided ones open towards the space their front faces.
pub(super) fn choose_linedef(
    map: &Map,
    line: usize,
    at: Option<Point>,
) -> Result<Chosen, CyberError> {
    let Some(def) = map.linedefs.get(line) else {
        return Err(out_of_range("linedef", line, map.linedefs.len()));
    };
    match (side_sector(map, def.front), side_sector(map, def.back)) {
        (Some(a), Some(b)) if a != b => Ok(Chosen {
            room_a: a,
            room_b: b,
            wall: line,
            centre: project_onto_line(map, line, at),
            touching: true,
        }),
        (Some(a), Some(_)) => Err(invalid(&format!(
            "both sides of linedef {line} face sector {a}: a door wall separates two \
             sectors or bounds the space behind it, not one sector twice"
        ))),
        (Some(a), None) => Ok(Chosen {
            room_a: a,
            room_b: a,
            wall: line,
            centre: project_onto_line(map, line, at),
            touching: false,
        }),
        (None, Some(b)) => Err(invalid(&format!(
            "linedef {line} has only a back side (sector {b}): flip it or give it a \
             front sidedef before cutting a door into it (see add_sidedef)"
        ))),
        (None, None) => Err(invalid(&format!(
            "linedef {line} has no usable sidedef: a door wall needs a front side \
             facing its room (see add_sidedef)"
        ))),
    }
}

/// The wall the parameters pick, validated against `map`.
pub(super) fn choose_wall(
    map: &Map,
    target: Target,
    centre: Option<Point>,
) -> Result<Chosen, CyberError> {
    match target {
        Target::Between { a, b } => choose_between(map, a, b, centre),
        Target::Linedef(line) => choose_linedef(map, line, centre),
    }
}

/// The [`OverlayRequest`] carving `polygon` out as solid rock, dressing
/// bare sectors' new walls with `wall`.
fn void_request(polygon: &[Point], wall: &str) -> OverlayRequest {
    OverlayRequest {
        polygon: to_pts(polygon),
        paint: Paint::Void,
        keep_interior: false,
        textures: WallTextures {
            wall: wall.to_owned(),
            upper: wall.to_owned(),
            lower: wall.to_owned(),
        },
        outside: Outside::Original,
    }
}

/// A [`Point`] list widened to the arrangement's `i64` grid.
pub(super) fn to_pts(points: &[Point]) -> Vec<Pt> {
    points
        .iter()
        .map(|corner| (i64::from(corner.x), i64::from(corner.y)))
        .collect()
}

/// Carve the void strip along the whole shared boundary of the chosen
/// wall's sectors, and report the notes the carve left.
///
/// Touching rooms would share their wall with the feature drawn across
/// it: the strip (thickness [`STRIP_THICKNESS`]) turns them apart so the
/// recess or window has a real space to span. `wall` dresses the bare
/// edges and `spanner` names the thing that spans the strip in the note.
pub(super) fn carve_strip(
    map: &mut Map,
    chosen: &Chosen,
    wall: &str,
    spanner: &str,
) -> Result<Vec<String>, CyberError> {
    let (a, b) = (chosen.room_a, chosen.room_b);
    let boundary = longest_boundary(map, a, b).ok_or_else(|| {
        invalid(&format!(
            "sectors {a} and {b} share no two-sided boundary to carve; re-read the map \
             and pass current indices"
        ))
    })?;
    let strip = void_strip(map, &boundary, STRIP_THICKNESS)?;
    let drawn = overlay::apply_overlay(map, &void_request(&strip.polygon, wall))?;
    let mut notes = vec![format!(
        "carved a {STRIP_THICKNESS}-unit void strip along the shared boundary of sectors \
         {a} and {b}: {spanner} spans it"
    )];
    notes.extend(drawn.notes);
    Ok(notes)
}

/// The room's wall along a freshly carved strip: the one-sided wall of
/// `room` whose midpoint sits nearest `near`; `None` when the carve left
/// the room without a face to it.
pub(super) fn strip_wall(map: &Map, room: usize, near: Point) -> Option<usize> {
    let mut best: Option<(i128, usize)> = None;
    for (index, line) in map.linedefs.iter().enumerate() {
        if !room_face(map, line, room) {
            continue;
        }
        let Some(square) = mid_distance_sq(map, index, near) else {
            continue;
        };
        if best.is_none_or(|(known, _)| square < known) {
            best = Some((square, index));
        }
    }
    best.map(|(_, index)| index)
}

/// The refusal when the strip carve left room `room` unfaceable.
pub(super) fn strip_face_lost(room: usize) -> CyberError {
    invalid(&format!(
        "sector {room} has no wall facing the carved strip: the void carve removed its \
         boundary; check the two sectors' shared boundary and re-read the map"
    ))
}

/// The recess depth behind a one-sided wall: the explicit `depth`, or
/// the distance the outward ray measures to the next sector's wall.
pub(super) fn recess_depth(map: &Map, wall: &DoorWall, spec: &DoorSpec) -> Result<i32, CyberError> {
    match spec.depth {
        Some(depth) if depth > 0_i32 => Ok(depth),
        Some(_) => Err(invalid(
            "the door depth must be a positive number of map units (the recess the \
             door slides into)",
        )),
        None => outward_depth(map, wall.linedef, wall.point, MAX_DEPTH)
            .map_or_else(|| Err(no_room()), |hit| Ok(hit.0)),
    }
}

/// Refuse a one-sided recess whose far middle falls in the void: only a
/// recess that lands on (or inside) a sector can ever act as a door.
pub(super) fn require_landing(map: &Map, rect: &[Point]) -> Result<(), CyberError> {
    let middle = centroid(rect);
    if point_in_sector(map, i64::from(middle.x), i64::from(middle.y)).is_some() {
        return Ok(());
    }
    Err(no_room())
}

/// The rectangle to paint: the cut fitted to the wall (a door too wide
/// for it refuses with the wall's available length, before any drawing),
/// grown to the strip's thickness or the measured recess depth.
pub(super) fn place_rect(
    map: &Map,
    chosen: &Chosen,
    spec: &DoorSpec,
) -> Result<Placed, CyberError> {
    let wall = door_wall(map, chosen.wall, Some(chosen.centre), spec.width)?;
    let normal = outward_normal(map, chosen.wall).ok_or_else(|| wall_lost(chosen.wall))?;
    let (depth, grown_back) = if chosen.touching {
        (STRIP_THICKNESS, true)
    } else {
        (recess_depth(map, &wall, spec)?, false)
    };
    let rect = door_rect(&wall, normal, depth, grown_back);
    if !chosen.touching {
        require_landing(map, &rect)?;
    }
    Ok(Placed {
        wall: chosen.wall,
        room_a: chosen.room_a,
        rect,
        tag: None,
    })
}
