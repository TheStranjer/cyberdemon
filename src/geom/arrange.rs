//! The arrangement pipeline: validate, split, merge, dedupe, label.
//!
//! `arrange` folds the steps of the ticket into one deterministic pass:
//! polygon edges and external segments become source segments, the cut
//! finder locates where they meet, the point merge collapses the
//! candidate points, every segment is split at the points on it, equal
//! pieces are deduped with their origins merged, each side is sampled
//! and labelled, pieces whose sides agree are dropped, and collinear
//! neighbours are stitched back together where nothing else touches
//! between them.
//!
//! The layer checks, the shared constants and the geometry hint stay
//! here beside [`arrange`]; cutting every segment at the points that lie
//! on it and collecting the resulting pieces by endpoint pair is the
//! private `split` module, sampling both sides of a piece, dropping the
//! interior ones and stitching collinear runs together the private
//! `merge` module. Both are implementation details of the one entry
//! point below.

mod merge;
mod split;

use crate::error::CyberError;
use crate::geom::label::{Label, Labeler};
use crate::geom::segment::{self, ExternalSegment, PointMerge, cross, opposite, orient};
use crate::geom::{Layer, Piece, Pt};
use merge::{label_pieces, merge_collinear};
use split::{collect_segments, dedupe_segments};

/// The largest arrangement input accepted.
pub(super) const MAX_SEGMENTS: usize = 0x0003_0D40; // 200 000
/// The smallest polygon a layer may hold.
const MIN_VERTICES: usize = 3;
/// Half of `SAMPLE_SCALE`: turns an endpoint sum into the scaled midpoint.
pub(super) const SAMPLE_HALF: i128 = 0x8_0000;
/// Side samples sit an eighth of a unit off the piece, along the unit
/// normal, in sample-scale units.
pub(super) const SAMPLE_OFFSET: i128 = 0x2_0000;
/// A line class for collinear merging: primitive direction `(ux, uy)`
/// with `ux > 0 || (ux == 0 && uy > 0)` and the cross constant
/// `ux * y - uy * x`, equal for every integer point on the line.
pub(super) type LineKey = (i128, i128, i128);

/// Arrange layered polygons and external segments into non-overlapping,
/// side-labelled pieces.
///
/// `layers` are in painter's order (bottom first); each polygon needs at
/// least three distinct vertices, non-zero area and no self-crossing
/// (adjacent edges may fold flat, non-adjacent edges may not cross
/// properly). `external` segments are cut like edges but carry no fill;
/// pieces keep their sources in [`Piece::origin`] so callers can
/// map results back to the inputs they came from. `labeler` decides the
/// label on each side of every piece (see [`LayerLabeler`](crate::geom::LayerLabeler)
/// for the painter's-algorithm default). Pieces whose two sides carry the
/// same label are interior and dropped. The output is sorted by `(a, b)`
/// and is a pure function of the inputs.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] when a layer polygon is degenerate or
/// self-crossing (naming the layer); [`CyberError::LimitExceeded`] when
/// the inputs carry more than 200 000 segments.
#[inline]
pub fn arrange(
    layers: &[Layer],
    external: &[ExternalSegment],
    labeler: &dyn Labeler,
) -> Result<Vec<Piece>, CyberError> {
    let segs = collect_segments(layers, external)?;
    let cuts = segment::find_cuts(&segs);
    let merged = PointMerge::from_segments(&segs, &cuts);
    let table = dedupe_segments(&segs, &cuts, &merged);
    let mut pieces = label_pieces(table, labeler);
    merge_collinear(&mut pieces);
    pieces.sort_by_key(|piece| (piece.a, piece.b));
    Ok(pieces)
}

/// Drop repeated neighbours and an explicit closing vertex so every edge
/// of the returned ring has positive length.
pub(super) fn normalize_polygon(points: &[Pt]) -> Vec<Pt> {
    let mut out: Vec<Pt> = Vec::with_capacity(points.len());
    for point in points {
        if out.last() != Some(point) {
            out.push(*point);
        }
    }
    while out.len() > 1 && out.first() == out.last() {
        out.pop();
    }
    out
}

/// The closing edge ring `(v0, v1), ..., (vn-1, v0)` of a vertex list.
pub(super) fn ring_edges(vertices: &[Pt]) -> Vec<(Pt, Pt)> {
    let Some(first) = vertices.first().copied() else {
        return Vec::new();
    };
    let mut edges: Vec<(Pt, Pt)> = Vec::with_capacity(vertices.len());
    let mut previous = first;
    for vertex in vertices.iter().skip(1).chain(core::iter::once(&first)) {
        edges.push((previous, *vertex));
        previous = *vertex;
    }
    edges
}

/// twice the signed area of the ring, in `i128` (shoelace).
pub(super) fn polygon_area2(vertices: &[Pt]) -> i128 {
    ring_edges(vertices).iter().fold(0_i128, |total, edge| {
        let (start, end) = *edge;
        let from = (i128::from(start.0), i128::from(start.1));
        let to = (i128::from(end.0), i128::from(end.1));
        total.saturating_add(cross(from, to))
    })
}

/// Whether two segments cross at a point strictly inside both.
pub(super) fn segments_cross(first: &(Pt, Pt), second: &(Pt, Pt)) -> bool {
    let (start_a, end_a) = *first;
    let (start_b, end_b) = *second;
    opposite(
        orient(start_a, end_a, start_b),
        orient(start_a, end_a, end_b),
    ) && opposite(
        orient(start_b, end_b, start_a),
        orient(start_b, end_b, end_a),
    )
}

/// The first pair of non-adjacent edges that properly cross.
pub(super) fn self_crossing(vertices: &[Pt]) -> Option<(usize, usize)> {
    let edges = ring_edges(vertices);
    let count = edges.len();
    let all = &edges;
    all.iter()
        .enumerate()
        .flat_map(|(first, edge)| {
            all.iter()
                .enumerate()
                .skip(first.saturating_add(2))
                .map(move |(second, other)| (first, second, edge, other))
        })
        .find(|&(first, second, edge, other)| {
            let wraps_around = first == 0 && second.saturating_add(1) == count;
            !wraps_around && segments_cross(edge, other)
        })
        .map(|(first, second, _, _)| (first, second))
}

/// Reject a layer polygon the arrangement cannot represent.
///
/// Every refusal carries the geometry hint: the calling map tools are
/// undoable, so the client's way out is always to look closer or step back.
pub(super) fn validate_polygon(
    index: usize,
    label: Label,
    vertices: &[Pt],
) -> Result<(), CyberError> {
    let name = label.describe();
    if vertices.len() < MIN_VERTICES {
        return Err(CyberError::InvalidArgument(format!(
            "layer {index} ({name}): needs at least {MIN_VERTICES} distinct vertices, found {}",
            vertices.len()
        ))
        .with_hint(GEOMETRY_HINT));
    }
    if polygon_area2(vertices) == 0 {
        return Err(CyberError::InvalidArgument(format!(
            "layer {index} ({name}): polygon has zero area"
        ))
        .with_hint(GEOMETRY_HINT));
    }
    if let Some((first, second)) = self_crossing(vertices) {
        return Err(CyberError::InvalidArgument(format!(
            "layer {index} ({name}): polygon is self-intersecting, edges {first} and {second} cross"
        ))
        .with_hint(GEOMETRY_HINT));
    }
    Ok(())
}

/// The hint attached to every invalid-geometry refusal from the
/// arrangement engine.
const GEOMETRY_HINT: &str = "validate_map shows details; undo reverts the last change";

#[cfg(test)]
mod tests;
