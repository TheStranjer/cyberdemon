//! Sampling both sides of a deduped piece and stitching collinear runs
//! back together.
//!
//! [`label_pieces`] asks the caller's [`Labeler`] just left and just right
//! of every piece's midpoint and drops the pieces whose two sides agree:
//! those are interior to the same region and carry no wall. What survives
//! goes through [`merge_collinear`], which groups pieces by their line
//! class ([`line_key`]), walks each group in order along the line
//! ([`line_position`]) and extends a run only across an endpoint no other
//! piece uses ([`endpoint_degrees`]) when labels and origins still match.

use alloc::collections::BTreeMap;

use super::LineKey;
use super::split::divided;
use crate::fixed::gcd;
use crate::geom::Piece;
use crate::geom::Pt;
use crate::geom::label::{Label, Labeler, ScaledPt};
use crate::geom::segment::{SegmentOrigin, cross, delta, side_samples, widen};

/// Ask `labeler` for both sides of the piece `start`→`end` (the shared
/// side samples an eighth of a unit off its midpoint, left and right).
fn side_labels(start: Pt, end: Pt, labeler: &dyn Labeler) -> (Label, Label) {
    let [left, right]: [ScaledPt; 2] = side_samples(start, end);
    (labeler.label(left), labeler.label(right))
}

/// Label every deduped piece and drop the ones whose sides agree.
pub(super) fn label_pieces(
    table: BTreeMap<(Pt, Pt), Vec<SegmentOrigin>>,
    labeler: &dyn Labeler,
) -> Vec<Piece> {
    let mut pieces: Vec<Piece> = Vec::new();
    for ((start, end), mut origin) in table {
        origin.sort_unstable();
        let (left, right) = side_labels(start, end, labeler);
        if left != right {
            pieces.push(Piece {
                a: start,
                b: end,
                left,
                right,
                origin,
            });
        }
    }
    pieces
}

/// Count how many pieces use each endpoint.
fn endpoint_degrees(pieces: &[Piece]) -> BTreeMap<Pt, u32> {
    let mut degrees: BTreeMap<Pt, u32> = BTreeMap::new();
    for piece in pieces {
        for endpoint in [piece.a, piece.b] {
            let slot = degrees.entry(endpoint).or_default();
            *slot = slot.saturating_add(1);
        }
    }
    degrees
}

/// The line class of a piece: primitive direction and cross constant.
fn line_key(piece: &Piece) -> LineKey {
    let direction = delta(piece.a, piece.b);
    let divisor = gcd(direction.0.saturating_abs(), direction.1.saturating_abs());
    let unit = (divided(direction.0, divisor), divided(direction.1, divisor));
    (unit.0, unit.1, cross(unit, widen(piece.a)))
}

/// Position of a piece's start along its unit direction.
fn line_position(piece: &Piece) -> i128 {
    let (unit_x, unit_y, _) = line_key(piece);
    i128::from(piece.a.0)
        .saturating_mul(unit_x)
        .saturating_add(i128::from(piece.a.1).saturating_mul(unit_y))
}

/// Whether `next` continues `current` on the same line, untouched
/// between: same labels, same origins, and a shared endpoint of degree 2.
fn can_merge(current: &Piece, next: &Piece, degrees: &BTreeMap<Pt, u32>) -> bool {
    let shared_degree = degrees.get(&current.b).copied().unwrap_or(0);
    current.b == next.a
        && shared_degree == 2
        && current.left == next.left
        && current.right == next.right
        && current.origin == next.origin
}

/// Greedily stitch one line's pieces into maximal tidy runs.
fn merge_group(group: &mut Vec<Piece>, degrees: &BTreeMap<Pt, u32>, out: &mut Vec<Piece>) {
    let mut iter = group.drain(..);
    let mut run: Option<Piece> = iter.next();
    for piece in iter {
        extend_or_start(&mut run, piece, degrees, out);
    }
    if let Some(done) = run {
        out.push(done);
    }
}

/// Add `piece` to the open run when it continues it, else close the run
/// and start a new one.
fn extend_or_start(
    run: &mut Option<Piece>,
    piece: Piece,
    degrees: &BTreeMap<Pt, u32>,
    out: &mut Vec<Piece>,
) {
    let extendable = run
        .as_ref()
        .is_some_and(|current| can_merge(current, &piece, degrees));
    if extendable {
        if let Some(current) = run.as_mut() {
            current.b = piece.b;
        }
    } else {
        if let Some(done) = run.take() {
            out.push(done);
        }
        *run = Some(piece);
    }
}

/// Merge consecutive collinear pieces whose shared endpoint is used by
/// no other piece and whose labels and origins are identical.
pub(super) fn merge_collinear(pieces: &mut Vec<Piece>) {
    let degrees = endpoint_degrees(pieces);
    let mut lines: BTreeMap<LineKey, Vec<Piece>> = BTreeMap::new();
    for piece in pieces.drain(..) {
        lines.entry(line_key(&piece)).or_default().push(piece);
    }
    let mut merged: Vec<Piece> = Vec::new();
    for group in lines.values_mut() {
        group.sort_by_key(line_position);
        merge_group(group, &degrees, &mut merged);
    }
    *pieces = merged;
}
