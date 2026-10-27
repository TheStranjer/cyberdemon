//! The integer geometry the overlay measures with: every length, box,
//! distance, split and vertex plan of a rebuild, in exact `i128` or
//! checked `i64`/`i32` conversion.
//!
//! [`external_segments`] turns the original linedefs into the arrangement's
//! external side, remembering the lines whose vertices do not resolve;
//! [`plan_vertices`] lays out the rebuilt vertex rows and [`plan_point`]
//! reads one back. [`collect_replaced`] finds the sectors `keep_interior`
//! spares and [`region_of`] reads a piece's two labels. The rest are the
//! arithmetic: [`unit_length`] for lengths and [`midpoint`] for positions
//! (the exact point-to-segment distance the wall claims rank by and the
//! side samples `collect_replaced` reads are [`crate::geom`]'s shared
//! segment geometry), [`widen_box`] and [`box_area`] for the containment
//! boxes.

use alloc::collections::{BTreeMap, BTreeSet};

use crate::error::CyberError;
use crate::fixed;
use crate::geom::label::{Label, contains_point};
use crate::geom::segment::{ExternalSegment, SourceId, grid_hypot, side_samples};
use crate::geom::{Piece, Pt};
use crate::wad::map::model::{Linedef, Map, Sector, Vertex};
use crate::wad::map::query::{ElementKind, check_index};

use super::labeler::label_at;
use super::rebuilt::Plan;
use super::{Distance, Outside, OverlayRequest, Shape, Source, WallClaim};

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
    label.region_id().and_then(|id| usize::try_from(id).ok())
}

/// The vertex plan for `pieces`: reused originals in index order, then
/// the new grid points in ascending coordinate order.
///
/// # Errors
///
/// [`CyberError::LimitExceeded`] when a merged point leaves the `i32`
/// coordinate range.
pub(super) fn plan_vertices(pieces: &[Piece], source: &Source<'_>) -> Result<Plan, CyberError> {
    let wanted: BTreeSet<Pt> = pieces.iter().flat_map(|piece| [piece.a, piece.b]).collect();
    let mut index: BTreeMap<Pt, usize> = BTreeMap::new();
    let mut vertices: Vec<Vertex> = Vec::new();
    for vertex in &source.map.vertices {
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
pub(super) fn segment_of(source: &Source<'_>, line: &Linedef) -> Option<(Pt, Pt)> {
    let ((ax, ay), (bx, by)) = source.map.line_endpoints(line)?;
    Some((
        (i64::from(ax), i64::from(ay)),
        (i64::from(bx), i64::from(by)),
    ))
}

/// One external segment per linedef, in linedef order; unresolvable
/// lines become zero-length segments the arrangement drops, keeping the
/// indices aligned.
pub(super) fn external_segments(source: &Source<'_>) -> (Vec<ExternalSegment>, Vec<usize>) {
    let mut external = Vec::with_capacity(source.map.linedefs.len());
    let mut unresolved = Vec::new();
    for (index, line) in source.map.linedefs.iter().enumerate() {
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

/// Refuse an [`Outside::Face`] sector the map does not hold, through the
/// shared [`check_index`] guard with the overlay's own advice kept as
/// message context.
///
/// # Errors
///
/// [`CyberError::NotFound`] naming the index and the sector count.
///
/// [`check_index`]: crate::wad::map::query::check_index
pub(super) fn check_outside(request: &OverlayRequest, map: &Map) -> Result<(), CyberError> {
    match request.outside {
        Outside::Face(index) => check_index(ElementKind::Sectors, index, map.sectors.len())
            .map_err(|error| {
                error.map_message(|message| {
                    format!("{message}: pass an existing sector index, \"auto\", or \"none\"")
                })
            }),
        Outside::Original | Outside::Solid => Ok(()),
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

/// The `(x, y)` midpoint of two grid points, each coordinate rounded
/// through [`crate::fixed::midpoint`] (halves away from zero).
pub(super) fn midpoint(from: Pt, to: Pt) -> Pt {
    (fixed::midpoint(from.0, to.0), fixed::midpoint(from.1, to.1))
}

/// The floored `hypot` length of a grid segment, refused when no offset
/// can be shifted by it.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] when the piece is longer than
/// `i32::MAX` map units.
pub(super) fn unit_length(from: Pt, to: Pt) -> Result<i32, CyberError> {
    let length = grid_hypot(from, to);
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

/// Whether fraction `left` is strictly below fraction `right` (saturated
/// products; equal or indistinguishable counts as not closer).
const fn closer(left: Distance, right: Distance) -> bool {
    left.0.saturating_mul(right.1) < right.0.saturating_mul(left.1)
}

/// The nearest of two wall claims, keeping the first on ties.
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
