//! The integer geometry the overlay measures with: every length, box,
//! distance, split and vertex plan of a rebuild, in exact `i128` or
//! checked `i64`/`i32` conversion.
//!
//! [`external_segments`] turns the original linedefs into the arrangement's
//! external side, remembering the lines whose vertices do not resolve;
//! [`plan_vertices`] lays out the rebuilt vertex rows and [`plan_point`]
//! reads one back. [`collect_replaced`] finds the sectors `keep_interior`
//! spares and [`region_of`] reads a piece's two labels. The rest are the
//! arithmetic: [`grid_length`] and [`unit_length`] for lengths,
//! [`midpoint`] and [`distance_point_segment`] for positions, [`widen_box`]
//! and [`box_area`] for the containment boxes, and the `s_`-prefixed
//! helpers for the arrangement's scaled sample coordinates.

use alloc::collections::{BTreeMap, BTreeSet};

use crate::error::CyberError;
use crate::fixed::{hypot, isqrt};
use crate::geom::label::{Label, ScaledPt, contains_point};
use crate::geom::segment::{ExternalSegment, SourceId, cross, delta, dot};
use crate::geom::{Piece, Pt};
use crate::wad::map::model::{Linedef, Map, Sector, Vertex};

use super::labeler::{label_at, side_samples};
use super::rebuilt::Plan;
use super::{Distance, MIDPOINT_DIVISOR, Outside, OverlayRequest, Shape, Source, WallClaim};

/// The integer length of `start` to `finish`, at least one.
pub(super) fn grid_length(start: Pt, finish: Pt) -> i128 {
    let direction = delta(start, finish);
    let squared = dot(direction, direction);
    let magnitude = u64::try_from(squared).unwrap_or(u64::MAX);
    i128::from(isqrt(magnitude)).max(1)
}

/// The original sectors touching the paint polygon's boundary from
/// inside: every boundary piece's inside sample.
pub(super) fn collect_replaced(pieces: &[Piece], shapes: &[Shape], polygon: &[Pt]) -> Vec<u32> {
    let mut replaced: Vec<u32> = pieces
        .iter()
        .filter(|piece| has_layer_origin(piece))
        .flat_map(|piece| side_samples(piece.a, piece.b))
        .filter(|sample| contains_point(polygon, *sample))
        .filter_map(|sample| match label_at(shapes, sample) {
            Label::Region(id) => Some(id),
            Label::Void => None,
        })
        .collect();
    replaced.sort_unstable();
    replaced.dedup();
    replaced
}

/// Whether the piece was cut from the paint polygon's own boundary.
fn has_layer_origin(piece: &Piece) -> bool {
    piece
        .origin
        .iter()
        .any(|origin| matches!(origin.source, SourceId::Layer { .. }))
}

/// The original linedef index the piece continues (the first of its
/// external claims, the smallest when it overlaps several).
pub(super) fn first_external(piece: &Piece) -> Option<usize> {
    piece.origin.iter().find_map(|origin| match origin.source {
        SourceId::External(index) => Some(index),
        SourceId::Layer { .. } => None,
    })
}

/// The sector a label names, `None` for void.
#[must_use]
pub(super) fn region_of(label: Label) -> Option<usize> {
    match label {
        Label::Void => None,
        Label::Region(id) => usize::try_from(id).ok(),
    }
}

/// The vertex plan for `pieces`: reused originals in index order, then
/// the new grid points in ascending coordinate order.
///
/// # Errors
///
/// [`CyberError::LimitExceeded`] when a merged point leaves the `i32`
/// coordinate range.
pub(super) fn plan_vertices(pieces: &[Piece], source: &Source) -> Result<Plan, CyberError> {
    let wanted: BTreeSet<Pt> = pieces.iter().flat_map(|piece| [piece.a, piece.b]).collect();
    let mut index: BTreeMap<Pt, usize> = BTreeMap::new();
    let mut vertices: Vec<Vertex> = Vec::new();
    for vertex in &source.vertices {
        let point = (i64::from(vertex.x), i64::from(vertex.y));
        if wanted.contains(&point) && !index.contains_key(&point) {
            index.insert(point, vertices.len());
            vertices.push(vertex.clone());
        }
    }
    let mut created = Vec::new();
    for point in wanted {
        if index.contains_key(&point) {
            continue;
        }
        index.insert(point, vertices.len());
        vertices.push(grid_vertex(point)?);
        created.push(vertices.len().saturating_sub(1));
    }
    Ok(Plan {
        vertices,
        index,
        created,
    })
}

/// The [`Vertex`] for a grid point that needs one.
///
/// # Errors
///
/// [`CyberError::LimitExceeded`] when a coordinate leaves the `i32`
/// range.
fn grid_vertex(point: Pt) -> Result<Vertex, CyberError> {
    let grid = (i32::try_from(point.0), i32::try_from(point.1));
    match grid {
        (Ok(x), Ok(y)) => Ok(Vertex {
            x,
            y,
            ..Default::default()
        }),
        _ => Err(CyberError::LimitExceeded(format!(
            "the arrangement point {}, {} leaves the map coordinate range",
            point.0, point.1
        ))),
    }
}

/// The index of the planned vertex at `point`.
pub(super) fn plan_point(plan: &Plan, point: Pt) -> Result<usize, CyberError> {
    plan.index
        .get(&point)
        .copied()
        .ok_or_else(|| CyberError::Internal(format!("overlay lost the grid point {point:?}")))
}

/// The [`Linedef`] segment of `line`'s two vertices, `None` when either
/// reference dangles.
pub(super) fn segment_of(source: &Source, line: &Linedef) -> Option<(Pt, Pt)> {
    let start = source.vertices.get(line.v1)?;
    let end = source.vertices.get(line.v2)?;
    Some((
        (i64::from(start.x), i64::from(start.y)),
        (i64::from(end.x), i64::from(end.y)),
    ))
}

/// The sector behind an optional side reference.
pub(super) fn side_sector(source: &Source, side: Option<usize>) -> Option<usize> {
    side.and_then(|index| source.sidedefs.get(index))
        .map(|row| row.sector)
}

/// One external segment per linedef, in linedef order; unresolvable
/// lines become zero-length segments the arrangement drops, keeping the
/// indices aligned.
pub(super) fn external_segments(source: &Source) -> (Vec<ExternalSegment>, Vec<usize>) {
    let mut external = Vec::with_capacity(source.linedefs.len());
    let mut unresolved = Vec::new();
    for (index, line) in source.linedefs.iter().enumerate() {
        if let Some((a, b)) = segment_of(source, line) {
            external.push(ExternalSegment { a, b });
        } else {
            external.push(ExternalSegment {
                a: (0, 0),
                b: (0, 0),
            });
            unresolved.push(index);
        }
    }
    (external, unresolved)
}

/// Refuse an [`Outside::Face`] sector the map does not hold.
///
/// # Errors
///
/// [`CyberError::NotFound`] naming the index and the sector count.
pub(super) fn check_outside(request: &OverlayRequest, map: &Map) -> Result<(), CyberError> {
    match request.outside {
        Outside::Face(index) if index >= map.sectors.len() => {
            let count = map.sectors.len();
            let range = count.checked_sub(1).map_or_else(
                || "the map has no sectors".to_owned(),
                |last| format!("the map has {count} sectors, valid indices are 0..={last}"),
            );
            Err(CyberError::NotFound(format!(
                "sector {index} does not exist ({range}): pass an existing \
                 sector index, \"auto\", or \"none\""
            ))
            .with_hint("describe_map or find_elements returns indices"))
        }
        Outside::Original | Outside::Solid | Outside::Face(_) => Ok(()),
    }
}

/// The `u32` label id of sector `index`.
///
/// # Errors
///
/// [`CyberError::LimitExceeded`] when the index leaves the labeler's
/// range.
pub(super) fn sector_id(index: usize) -> Result<u32, CyberError> {
    u32::try_from(index).map_err(|_error| {
        CyberError::LimitExceeded(format!(
            "sector {index} is past the {} the painter can label",
            u32::MAX
        ))
    })
}

/// The `(x, y)` midpoint of two grid points, truncated toward zero.
pub(super) fn midpoint(from: Pt, to: Pt) -> Pt {
    (half_sum(from.0, to.0), half_sum(from.1, to.1))
}

/// `(from + to) / 2` truncated toward zero, overflow-free.
fn half_sum(from: i64, to: i64) -> i64 {
    let sum = i128::from(from).saturating_add(i128::from(to));
    let half = sum.checked_div(MIDPOINT_DIVISOR).unwrap_or(sum);
    i64::try_from(half).unwrap_or(from)
}

/// The floored `hypot` length of a grid segment, refused when no offset
/// can be shifted by it.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] when the piece is longer than
/// `i32::MAX` map units.
pub(super) fn unit_length(from: Pt, to: Pt) -> Result<i32, CyberError> {
    let length = hypot(to.0.saturating_sub(from.0), to.1.saturating_sub(from.1));
    i32::try_from(length).map_err(|_error| {
        CyberError::InvalidArgument(format!(
            "the split piece is {length} units long, past what an x_offset can shift by"
        ))
    })
}

/// The box covering `bbox` and a segment's endpoints.
pub(super) fn widen_box(bbox: Option<(i64, i64, i64, i64)>, a: Pt, b: Pt) -> (i64, i64, i64, i64) {
    let (min_x, min_y, max_x, max_y) = bbox.unwrap_or((a.0, a.1, a.0, a.1));
    (
        min_x.min(a.0).min(b.0),
        min_y.min(a.1).min(b.1),
        max_x.max(a.0).max(b.0),
        max_y.max(a.1).max(b.1),
    )
}

/// The area of a bounding box, saturating.
pub(super) fn box_area((min_x, min_y, max_x, max_y): (i64, i64, i64, i64)) -> i128 {
    let width = i128::from(max_x)
        .saturating_sub(i128::from(min_x))
        .saturating_add(1);
    let height = i128::from(max_y)
        .saturating_sub(i128::from(min_y))
        .saturating_add(1);
    width.saturating_mul(height)
}

/// The squared distance from a grid point to a grid segment, as an
/// exact fraction `(numerator, denominator)`.
pub(super) fn distance_point_segment(point: Pt, from: Pt, to: Pt) -> (i128, i128) {
    let direction = delta(from, to);
    let offset = delta(from, point);
    let squared = dot(direction, direction);
    let along = dot(direction, offset);
    if squared <= 0 || along <= 0 {
        return (dot(offset, offset), 1_i128);
    }
    if along >= squared {
        let far = delta(to, point);
        return (dot(far, far), 1_i128);
    }
    let cross = cross(direction, offset);
    (cross.saturating_mul(cross), squared)
}

/// Whether fraction `left` is strictly below fraction `right` (saturated
/// products; equal or indistinguishable counts as not closer).
const fn closer(left: Distance, right: Distance) -> bool {
    left.0.saturating_mul(right.1) < right.0.saturating_mul(left.1)
}

/// The nearer of two wall claims, keeping the first on ties.
pub(super) fn nearer(best: WallClaim, claim: WallClaim) -> WallClaim {
    if closer(claim.1, best.1) { claim } else { best }
}

/// The sector's ceiling height, a `faces_differ` probe.
pub(super) const fn ceiling_height(sector: &Sector) -> i32 {
    sector.ceiling_height
}

/// The sector's floor height, a `faces_differ` probe.
pub(super) const fn floor_height(sector: &Sector) -> i32 {
    sector.floor_height
}

/// `to - from` in scaled coordinates.
pub(super) const fn s_delta(from: ScaledPt, to: ScaledPt) -> ScaledPt {
    (to.0.saturating_sub(from.0), to.1.saturating_sub(from.1))
}

/// The `i128` dot product of two scaled vectors.
pub(super) const fn s_dot(left: ScaledPt, right: ScaledPt) -> i128 {
    left.0
        .saturating_mul(right.0)
        .saturating_add(left.1.saturating_mul(right.1))
}

/// The `i128` cross product of two scaled vectors.
pub(super) const fn s_cross(left: ScaledPt, right: ScaledPt) -> i128 {
    left.0
        .saturating_mul(right.1)
        .saturating_sub(left.1.saturating_mul(right.0))
}

/// The squared scaled distance between two sample points.
pub(super) const fn s_dist2(from: ScaledPt, to: ScaledPt) -> i128 {
    let step = s_delta(from, to);
    s_dot(step, step)
}

/// The `Internal` for a source line whose vertices vanished after the
/// external segments were built (defensive: they were checked there).
pub(super) fn dangling_ends() -> CyberError {
    CyberError::Internal("overlay lost a source linedef's vertices".to_owned())
}

/// The `Internal` for a piece whose front side came out void (the
/// orientation rule makes that impossible).
pub(super) fn void_front() -> CyberError {
    CyberError::Internal("overlay found a void front side".to_owned())
}
