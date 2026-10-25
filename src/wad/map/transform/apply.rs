//! The per-element half of one transform: the planned [`Move`]s of the
//! selected vertices and things, every check that runs before a row is
//! written, and the commit itself.
//!
//! [`crate::wad::map::transform::apply_transform`] drives these steps in
//! order: it picks the origin (the selected bounding box's centre when the
//! caller names none), plans each move through [`Operations::apply_point`],
//! refuses a coordinate the map's format cannot store or a linedef that
//! collapses to zero length, counts the vertices that end up on an unmoved
//! one, and only then writes. Nothing here touches the map until every
//! refusal has been cleared.

use alloc::collections::BTreeMap;

use crate::error::CyberError;
use crate::fixed::midpoint;
use crate::wad::classify::MapFormat;

use super::super::model::Map;
use super::{COORD_MAX, COORD_MIN, DEGREES, Move, Operations, Point, Selection, ThingMove};

/// The planned vertex destinations keyed by index, for the checks that
/// compare positions after the move.
pub(super) fn moved_points(vertices: &[Move]) -> BTreeMap<usize, Point> {
    vertices
        .iter()
        .map(|vertex| (vertex.index, vertex.point()))
        .collect()
}

/// The centre of the selected vertices' and things' bounding box, each
/// axis rounded to nearest. A degenerate span is its own midpoint, and a
/// selection whose references all dangle pivots at the zero point.
pub(super) fn default_origin(map: &Map, selection: &Selection) -> Point {
    let mut span: Option<(i64, i64, i64, i64)> = None;
    for index in &selection.vertices {
        if let Some(vertex) = map.vertices.get(*index) {
            span = Some(widen(span, Point::from_model(vertex.x, vertex.y)));
        }
    }
    for index in &selection.things {
        if let Some(object) = map.things.get(*index) {
            span = Some(widen(span, Point::from_model(object.x, object.y)));
        }
    }
    span.map_or(Point { x: 0, y: 0 }, centre)
}

/// The span covering `span` and one more point; a fresh span when empty.
fn widen(span: Option<(i64, i64, i64, i64)>, at: Point) -> (i64, i64, i64, i64) {
    let (min_x, min_y, max_x, max_y) = span.unwrap_or((at.x, at.y, at.x, at.y));
    (
        min_x.min(at.x),
        min_y.min(at.y),
        max_x.max(at.x),
        max_y.max(at.y),
    )
}

/// The midpoint of a `(min_x, min_y, max_x, max_y)` span.
fn centre((min_x, min_y, max_x, max_y): (i64, i64, i64, i64)) -> Point {
    Point {
        x: midpoint(min_x, max_x),
        y: midpoint(min_y, max_y),
    }
}

/// The planned moves of every selected vertex.
pub(super) fn move_vertices(
    map: &Map,
    selection: &Selection,
    ops: &Operations,
    origin: Point,
) -> Result<Vec<Move>, CyberError> {
    let mut moves = Vec::with_capacity(selection.vertices.len());
    for index in &selection.vertices {
        let vertex = map
            .vertices
            .get(*index)
            .ok_or_else(|| missing("vertex", *index))?;
        let from = Point::from_model(vertex.x, vertex.y);
        let to = ops
            .apply_point(from, origin)
            .ok_or_else(|| overflow("vertex", *index))?;
        let (x, y) = fit(&format!("vertex {index}"), to, map.format)?;
        moves.push(Move {
            index: *index,
            x,
            y,
        });
    }
    Ok(moves)
}

/// The planned moves of every selected thing, with its new facing.
pub(super) fn move_things(
    map: &Map,
    selection: &Selection,
    ops: &Operations,
    origin: Point,
) -> Result<Vec<ThingMove>, CyberError> {
    let mut moves = Vec::with_capacity(selection.things.len());
    for index in &selection.things {
        let object = map
            .things
            .get(*index)
            .ok_or_else(|| missing("thing", *index))?;
        let from = Point::from_model(object.x, object.y);
        let to = ops
            .apply_point(from, origin)
            .ok_or_else(|| overflow("thing", *index))?;
        let (x, y) = fit(&format!("thing {index}"), to, map.format)?;
        let angle = ops
            .rotation
            .map(|rotation| rotated_angle(object.angle, rotation.degrees));
        moves.push(ThingMove {
            position: Move {
                index: *index,
                x,
                y,
            },
            angle,
        });
    }
    Ok(moves)
}

/// The thing facing after a rotation: the current angle plus the
/// rotation's whole degrees, normalised into `0..=359`.
pub(super) fn rotated_angle(current: i32, degrees: i64) -> i32 {
    let turned = i64::from(current)
        .checked_add(degrees)
        .unwrap_or_else(|| i64::from(current))
        .rem_euclid(DEGREES);
    i32::try_from(turned).unwrap_or(current)
}

/// Refuse a transformed point the map's format cannot store: the model's
/// `i32` always, and the binary formats' `i16` columns on top. The `Ok`
/// carries the pair as the model stores it.
fn fit(label: &str, at: Point, format: MapFormat) -> Result<(i32, i32), CyberError> {
    let Point { x, y } = at;
    let (Ok(stored_x), Ok(stored_y)) = (i32::try_from(x), i32::try_from(y)) else {
        return Err(CyberError::InvalidArgument(format!(
            "{label} would land at ({x}, {y}), past the range this model can hold: \
             lower `scale` or `translate`, or move `origin`"
        )));
    };
    let fits =
        (COORD_MIN..=COORD_MAX).contains(&stored_x) && (COORD_MIN..=COORD_MAX).contains(&stored_y);
    if format != MapFormat::Udmf && !fits {
        let name = format_name(format);
        return Err(CyberError::InvalidArgument(format!(
            "{label} would land at ({stored_x}, {stored_y}), outside the i16 range a {name} \
             map stores: lower `scale` or `translate`, move `origin`, or run \
             convert_map_format to UDMF first"
        )));
    }
    Ok((stored_x, stored_y))
}

/// The format's name as map tools spell it.
const fn format_name(format: MapFormat) -> &'static str {
    match format {
        MapFormat::Doom => "Doom",
        MapFormat::Hexen => "Hexen",
        MapFormat::Udmf => "UDMF",
    }
}

/// Refuse any linedef whose moved endpoints collapse onto each other,
/// naming it. A line already zero-length stays zero-length without a
/// refusal; lines with a dangling, unmoved endpoint cannot be checked and
/// stand.
pub(super) fn check_collapse(map: &Map, moved: &BTreeMap<usize, Point>) -> Result<(), CyberError> {
    for (index, line) in map.linedefs.iter().enumerate() {
        if !moved.contains_key(&line.v1) && !moved.contains_key(&line.v2) {
            continue;
        }
        let (Some(from1), Some(from2)) = (map.vertices.get(line.v1), map.vertices.get(line.v2))
        else {
            continue;
        };
        let before = Point::from_model(from1.x, from1.y) != Point::from_model(from2.x, from2.y);
        let to1 = moved
            .get(&line.v1)
            .copied()
            .unwrap_or_else(|| Point::from_model(from1.x, from1.y));
        let to2 = moved
            .get(&line.v2)
            .copied()
            .unwrap_or_else(|| Point::from_model(from2.x, from2.y));
        if before && to1 == to2 {
            let Point { x, y } = to1;
            return Err(CyberError::Conflict(format!(
                "linedef {index} would become zero-length at ({x}, {y}): lower `scale` \
                 or `translate`, or leave one of its vertices out of the selection"
            )));
        }
    }
    Ok(())
}

/// Every `[moved, unmoved]` vertex pair that shares a position after the
/// moves; coincidences are reported, never merged.
pub(super) fn coincident_pairs(map: &Map, moved: &BTreeMap<usize, Point>) -> Vec<[usize; 2]> {
    let mut unmoved: BTreeMap<Point, Vec<usize>> = BTreeMap::new();
    for (index, vertex) in map.vertices.iter().enumerate() {
        if !moved.contains_key(&index) {
            unmoved
                .entry(Point::from_model(vertex.x, vertex.y))
                .or_default()
                .push(index);
        }
    }
    let mut pairs: Vec<[usize; 2]> = moved
        .iter()
        .filter_map(|(index, at)| unmoved.get(at).map(|others| (*index, others)))
        .flat_map(|(index, others)| others.iter().map(move |other| [index, *other]))
        .collect();
    pairs.sort_unstable();
    pairs
}

/// Commit the planned moves to the map's rows (every check has passed).
pub(super) fn write_moves(map: &mut Map, vertices: &[Move], things: &[ThingMove]) {
    for vertex in vertices {
        if let Some(row) = map.vertices.get_mut(vertex.index) {
            row.x = vertex.x;
            row.y = vertex.y;
        }
    }
    for item in things {
        if let Some(row) = map.things.get_mut(item.position.index) {
            row.x = item.position.x;
            row.y = item.position.y;
            row.angle = item.angle.unwrap_or(row.angle);
        }
    }
}

/// The refusal for a selection index past its block (the expansion has
/// already checked; a hand-built selection is the caller's own mismatch).
fn missing(kind: &str, index: usize) -> CyberError {
    CyberError::InvalidArgument(format!(
        "the selection names {kind} {index}, which this map does not hold: \
         re-expand the selection against the current map"
    ))
}

/// The refusal when a transform intermediate or result leaves `i64`.
fn overflow(kind: &str, index: usize) -> CyberError {
    CyberError::InvalidArgument(format!(
        "{kind} {index} would leave the computable range: lower `scale` or \
         `translate`, or move `origin` closer"
    ))
}
