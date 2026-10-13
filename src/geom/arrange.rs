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

use alloc::collections::BTreeMap;

use crate::error::CyberError;
use crate::fixed::isqrt;
use crate::geom::label::{Label, Labeler, ScaledPt};
use crate::geom::segment::{
    self, ExternalSegment, PointMerge, Seg, SegmentOrigin, SourceId, between, cross, delta, dot,
    on_segment_tol, opposite, orient, round_quotient,
};
use crate::geom::{Layer, Piece, Pt};

/// The largest arrangement input accepted.
const MAX_SEGMENTS: usize = 0x0003_0D40; // 200 000
/// The smallest polygon a layer may hold.
const MIN_VERTICES: usize = 3;
/// Half of `SAMPLE_SCALE`: turns an endpoint sum into the scaled midpoint.
const SAMPLE_HALF: i128 = 0x8_0000;
/// Side samples sit an eighth of a unit off the piece, along the unit
/// normal, in sample-scale units.
const SAMPLE_OFFSET: i128 = 0x2_0000;
/// A line class for collinear merging: primitive direction `(ux, uy)`
/// with `ux > 0 || (ux == 0 && uy > 0)` and the cross constant
/// `ux * y - uy * x`, equal for every integer point on the line.
type LineKey = (i128, i128, i128);

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
fn normalize_polygon(points: &[Pt]) -> Vec<Pt> {
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
fn ring_edges(vertices: &[Pt]) -> Vec<(Pt, Pt)> {
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
fn polygon_area2(vertices: &[Pt]) -> i128 {
    ring_edges(vertices).iter().fold(0_i128, |total, edge| {
        let (start, end) = *edge;
        let from = (i128::from(start.0), i128::from(start.1));
        let to = (i128::from(end.0), i128::from(end.1));
        total.saturating_add(cross(from, to))
    })
}

/// Whether two segments cross at a point strictly inside both.
fn segments_cross(first: &(Pt, Pt), second: &(Pt, Pt)) -> bool {
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
fn self_crossing(vertices: &[Pt]) -> Option<(usize, usize)> {
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
fn validate_polygon(index: usize, label: Label, vertices: &[Pt]) -> Result<(), CyberError> {
    let name = label.describe();
    if vertices.len() < MIN_VERTICES {
        return Err(CyberError::InvalidArgument(format!(
            "layer {index} ({name}): needs at least {MIN_VERTICES} distinct vertices, found {}",
            vertices.len()
        )));
    }
    if polygon_area2(vertices) == 0 {
        return Err(CyberError::InvalidArgument(format!(
            "layer {index} ({name}): polygon has zero area"
        )));
    }
    if let Some((first, second)) = self_crossing(vertices) {
        return Err(CyberError::InvalidArgument(format!(
            "layer {index} ({name}): polygon is self-intersecting, edges {first} and {second} cross"
        )));
    }
    Ok(())
}

/// Validate every layer and collect polygon edges plus external segments.
fn collect_segments(
    layers: &[Layer],
    external: &[ExternalSegment],
) -> Result<Vec<Seg>, CyberError> {
    let mut segs: Vec<Seg> = Vec::new();
    for (index, layer) in layers.iter().enumerate() {
        let vertices = normalize_polygon(&layer.polygon);
        validate_polygon(index, layer.label, &vertices)?;
        // After normalization every ring edge (and thus every piece
        // below it) has positive length.
        for (edge, (start, end)) in ring_edges(&vertices).into_iter().enumerate() {
            segs.push(Seg {
                a: start,
                b: end,
                source: SourceId::Layer { layer: index, edge },
            });
        }
    }
    for (index, item) in external.iter().enumerate() {
        if item.a != item.b {
            segs.push(Seg {
                a: item.a,
                b: item.b,
                source: SourceId::External(index),
            });
        }
    }
    if segs.len() > MAX_SEGMENTS {
        return Err(CyberError::LimitExceeded(format!(
            "arrangement input holds {} segments, the limit is {MAX_SEGMENTS}",
            segs.len()
        )));
    }
    Ok(segs)
}

/// The order key of a cut point along `start`→`end`: projection first,
/// perpendicular offset second.
#[inline]
fn chain_key(start: Pt, end: Pt, point: Pt) -> (i128, i128) {
    let direction = delta(start, end);
    (
        dot(direction, delta(start, point)),
        cross(direction, delta(start, point)),
    )
}

/// Push every merged point lying on `start`→`end` within half a unit
/// perpendicular of it.
fn add_nearby_nodes(cuts: &mut Vec<Pt>, start: Pt, end: Pt, points: &[Pt]) {
    let low = start.0.min(end.0).saturating_sub(1);
    let high = start.0.max(end.0).saturating_add(1);
    let begin = points.partition_point(|point| point.0 < low);
    let finish = points.partition_point(|point| point.0 <= high);
    let Some(window) = points.get(begin..finish) else {
        return;
    };
    for point in window {
        if on_segment_tol(start, end, *point) {
            cuts.push(*point);
        }
    }
}

/// The ordered endpoints of one segment after merging: start, every cut
/// strictly between, end. Own crossing points always split (the rounded
/// point may sit a little off the line); other merged points must pass
/// the point-on-segment tolerance test.
fn split_chain(seg: &Seg, raw: &[Pt], merged: &PointMerge, points: &[Pt]) -> Vec<Pt> {
    let (Some(start), Some(end)) = (merged.canonical(seg.a), merged.canonical(seg.b)) else {
        return Vec::new();
    };
    let mut cuts: Vec<Pt> = raw
        .iter()
        .filter_map(|point| merged.canonical(*point))
        .filter(|point| between(*point, start, end))
        .collect();
    add_nearby_nodes(&mut cuts, start, end, points);
    cuts.sort_unstable_by_key(|point| chain_key(start, end, *point));
    cuts.dedup();
    let mut chain: Vec<Pt> = Vec::with_capacity(cuts.len().saturating_add(2));
    chain.push(start);
    chain.extend(cuts);
    chain.push(end);
    chain
}

/// `gcd` of two non-negative `i128` values, at least one.
#[must_use]
#[inline]
const fn gcd(mut left: i128, mut right: i128) -> i128 {
    while right != 0 {
        let next = match left.checked_rem(right) {
            Some(rem) => rem,
            None => 0,
        };
        left = right;
        right = next;
    }
    if left < 1 { 1 } else { left }
}

/// Exact division by a `gcd` result (never zero, never overflowing for
/// in-range inputs; falls back to the dividend in the impossible case).
#[must_use]
#[inline]
const fn divided(value: i128, divisor: i128) -> i128 {
    match value.checked_div(divisor) {
        Some(exact) => exact,
        None => value,
    }
}

/// Reduce a fraction and force the denominator positive.
#[must_use]
const fn normalize_fraction(fraction: (i128, i128)) -> (i128, i128) {
    let (raw_num, raw_den) = fraction;
    let (num, den) = if raw_den < 0 {
        (raw_num.saturating_neg(), raw_den.saturating_neg())
    } else {
        (raw_num, raw_den)
    };
    if den == 0 {
        return (0, 1);
    }
    let divisor = gcd(num.saturating_abs(), den);
    (divided(num, divisor), divided(den, divisor))
}

/// The fraction along the source segment where a piece starting at `at`
/// begins, measured on the dominant axis.
fn origin_fraction(seg: &Seg, at: Pt) -> (i128, i128) {
    let (num, den) = if seg.a.0 == seg.b.0 {
        (
            i128::from(at.1).saturating_sub(i128::from(seg.a.1)),
            i128::from(seg.b.1).saturating_sub(i128::from(seg.a.1)),
        )
    } else {
        (
            i128::from(at.0).saturating_sub(i128::from(seg.a.0)),
            i128::from(seg.b.0).saturating_sub(i128::from(seg.a.0)),
        )
    };
    normalize_fraction((num, den))
}

/// The dedupe key of a piece: its endpoints ordered.
#[must_use]
#[inline]
fn ordered_pair(first: Pt, second: Pt) -> (Pt, Pt) {
    if first <= second {
        (first, second)
    } else {
        (second, first)
    }
}

/// Record every piece of one split chain into the dedupe table.
fn add_chain_pieces(table: &mut BTreeMap<(Pt, Pt), Vec<SegmentOrigin>>, seg: &Seg, chain: &[Pt]) {
    for pair in chain.windows(2) {
        let [start, end] = *pair else {
            continue;
        };
        let origin = SegmentOrigin {
            source: seg.source,
            from_t: origin_fraction(seg, start),
        };
        table
            .entry(ordered_pair(start, end))
            .or_default()
            .push(origin);
    }
}

/// Split every segment and collect the resulting pieces by endpoint pair,
/// merging duplicates' origins.
fn dedupe_segments(
    segs: &[Seg],
    cuts: &[Vec<Pt>],
    merged: &PointMerge,
) -> BTreeMap<(Pt, Pt), Vec<SegmentOrigin>> {
    let points = merged.points();
    let mut table: BTreeMap<(Pt, Pt), Vec<SegmentOrigin>> = BTreeMap::new();
    for (seg, raw) in segs.iter().zip(cuts) {
        let chain = split_chain(seg, raw, merged, &points);
        add_chain_pieces(&mut table, seg, &chain);
    }
    table
}

/// The integer length of `start`→`end`, at least one.
fn segment_length(start: Pt, end: Pt) -> i128 {
    let direction = delta(start, end);
    let length_squared = dot(direction, direction);
    let magnitude = u64::try_from(length_squared).unwrap_or(u64::MAX);
    i128::from(isqrt(magnitude)).max(1)
}

/// The unit normal of `start`→`end` scaled to an eighth of a unit, left
/// positive.
fn side_offset(start: Pt, end: Pt) -> ScaledPt {
    let direction = delta(start, end);
    let length = segment_length(start, end);
    (
        round_quotient(
            direction.1.saturating_neg().saturating_mul(SAMPLE_OFFSET),
            length,
        )
        .unwrap_or(0),
        round_quotient(direction.0.saturating_mul(SAMPLE_OFFSET), length).unwrap_or(0),
    )
}

/// Sample just left and just right of the midpoint (in scaled
/// coordinates) and ask `labeler` for both sides.
fn side_labels(start: Pt, end: Pt, labeler: &dyn Labeler) -> (Label, Label) {
    let centre = (
        i128::from(start.0)
            .saturating_add(i128::from(end.0))
            .saturating_mul(SAMPLE_HALF),
        i128::from(start.1)
            .saturating_add(i128::from(end.1))
            .saturating_mul(SAMPLE_HALF),
    );
    let offset = side_offset(start, end);
    let left = labeler.label((
        centre.0.saturating_add(offset.0),
        centre.1.saturating_add(offset.1),
    ));
    let right = labeler.label((
        centre.0.saturating_sub(offset.0),
        centre.1.saturating_sub(offset.1),
    ));
    (left, right)
}

/// Label every deduped piece and drop the ones whose sides agree.
fn label_pieces(
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
    let from = (i128::from(piece.a.0), i128::from(piece.a.1));
    (unit.0, unit.1, cross(unit, from))
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
fn merge_collinear(pieces: &mut Vec<Piece>) {
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

#[cfg(test)]
mod tests {
    use super::{
        arrange, gcd, normalize_fraction, normalize_polygon, polygon_area2, ring_edges,
        segments_cross, self_crossing,
    };
    use crate::geom::label::{Label, Labeler, ScaledPt};
    use crate::geom::segment::{ExternalSegment, SourceId};
    use crate::geom::{Layer, LayerLabeler, Piece, Pt};
    use core::cell::RefCell;

    /// A counter labeler: every side sample gets its own region, so no
    /// piece is ever interior and every call is observable.
    #[derive(Default)]
    struct CountingLabeler {
        /// Side samples asked about so far.
        calls: RefCell<u32>,
    }

    impl CountingLabeler {
        /// How many times the pipeline asked for a side label.
        fn calls(&self) -> u32 {
            *self.calls.borrow()
        }
    }

    impl Labeler for CountingLabeler {
        fn label(&self, _point: ScaledPt) -> Label {
            let mut guard = self.calls.borrow_mut();
            *guard = guard.saturating_add(1);
            Label::Region(*guard)
        }
    }

    /// An axis-aligned rectangle ring, counter-clockwise.
    fn rect(low_x: i64, low_y: i64, high_x: i64, high_y: i64) -> Vec<Pt> {
        vec![
            (low_x, low_y),
            (high_x, low_y),
            (high_x, high_y),
            (low_x, high_y),
        ]
    }

    /// A region layer over a rectangle.
    fn room(id: u32, ring: Vec<Pt>) -> Layer {
        Layer {
            label: Label::Region(id),
            polygon: ring,
        }
    }

    /// Find the kept piece between two endpoints, either direction.
    fn find_piece(pieces: &[Piece], a: Pt, b: Pt) -> Option<&Piece> {
        pieces
            .iter()
            .find(|piece| (piece.a == a && piece.b == b) || (piece.a == b && piece.b == a))
    }

    /// One origin as a (source, fraction) pair.
    type Claim = (SourceId, (i128, i128));

    /// The source claims of a piece.
    fn piece_claims(piece: &Piece) -> Vec<Claim> {
        piece
            .origin
            .iter()
            .map(|origin| (origin.source, origin.from_t))
            .collect()
    }

    /// Whether two collinear segments share more than one point.
    fn collinear_overlap(one: (Pt, Pt), two: (Pt, Pt)) -> bool {
        let (start_a, end_a) = one;
        let (start_b, end_b) = two;
        if super::orient(start_a, end_a, start_b) != 0 || super::orient(start_a, end_a, end_b) != 0
        {
            return false;
        }
        let (low_a, high_a, low_b, high_b) = if start_a.0 == end_a.0 {
            (start_a.1, end_a.1, start_b.1, end_b.1)
        } else {
            (start_a.0, end_a.0, start_b.0, end_b.0)
        };
        let (first_low, first_high) = ordered(low_a, high_a);
        let (second_low, second_high) = ordered(low_b, high_b);
        second_low.max(first_low) < first_high.min(second_high)
    }

    /// `(low, high)` sorted.
    fn ordered(low: i64, high: i64) -> (i64, i64) {
        if low <= high {
            (low, high)
        } else {
            (high, low)
        }
    }

    /// Whether one pair of output pieces is clean: they neither cross
    /// properly nor overlap as collinear segments.
    fn pair_is_clean(first: &Piece, second: &Piece) -> bool {
        let one = (first.a, first.b);
        let two = (second.a, second.b);
        !segments_cross(&one, &two) && !collinear_overlap(one, two)
    }

    /// Check `first` against every piece after `index`.
    fn check_following(pieces: &[Piece], index: usize, first: &Piece) {
        for second in pieces.iter().skip(index.saturating_add(1)) {
            assert!(
                pair_is_clean(first, second),
                "pieces touch badly: {:?} x {:?}",
                (first.a, first.b),
                (second.a, second.b)
            );
        }
    }

    /// Brute-force invariant: no output piece properly crosses another,
    /// and no two collinear pieces share an interior point.
    fn assert_clean(pieces: &[Piece]) {
        for (index, first) in pieces.iter().enumerate() {
            check_following(pieces, index, first);
        }
    }

    #[test]
    fn polygon_helpers_normalize_ring_and_area() {
        let closed = vec![(0, 0), (10, 0), (10, 10), (0, 10), (0, 0)];
        assert_eq!(normalize_polygon(&closed).len(), 4, "closing vertex drops");
        let open = vec![(0, 0), (5, 5), (5, 5), (10, 0), (10, 0)];
        assert_eq!(
            normalize_polygon(&open).len(),
            3,
            "consecutive repeats drop"
        );
        assert_eq!(
            ring_edges(&[(1, 2)]).len(),
            1,
            "a lone point loops to itself"
        );
        assert_eq!(ring_edges(&[]).len(), 0, "empty ring");
        assert_eq!(polygon_area2(&rect(0, 0, 10, 10)), 200, "twice the area");
        assert_eq!(
            polygon_area2(&rect(0, 0, 10, 10).into_iter().rev().collect::<Vec<_>>()),
            -200,
            "winding flips sign"
        );
        assert_eq!(polygon_area2(&[(0, 0), (5, 0), (10, 0)]), 0, "flat");
        assert_eq!(self_crossing(&rect(0, 0, 10, 10)), None, "simple");
        assert_eq!(
            self_crossing(&[(0, 0), (10, 10), (10, 0), (0, 5)]),
            Some((0, 2)),
            "crossed quadrilateral: edges 0 and 2"
        );
    }

    #[test]
    fn fraction_helpers_reduce_and_sign() {
        assert_eq!(
            normalize_fraction((4, -8)),
            (-1, 2),
            "sign moves to numerator"
        );
        assert_eq!(normalize_fraction((4, 8)), (1, 2), "plain reduction");
        assert_eq!(normalize_fraction((0, 7)), (0, 1), "zero stays zero");
        assert_eq!(normalize_fraction((5, 0)), (0, 1), "degenerate is zero");
        assert_eq!(gcd(12, 18), 6, "plain gcd");
        assert_eq!(gcd(0, 5), 5, "zero entry");
    }

    #[test]
    fn two_squares_sharing_an_edge_yield_one_shared_piece() {
        let layers = vec![room(0, rect(0, 0, 10, 10)), room(1, rect(10, 0, 20, 10))];
        let pieces = arrange(&layers, &[], &LayerLabeler::new(&layers)).unwrap();
        assert_eq!(pieces.len(), 7, "seven boundary pieces, shared edge once");
        let shared = find_piece(&pieces, (10, 0), (10, 10)).unwrap();
        assert_eq!(shared.origin.len(), 2, "both squares claim the edge");
        assert_eq!(shared.a, (10, 0), "sorted start");
        assert_eq!(shared.b, (10, 10), "sorted end");
        assert_eq!(shared.left, Label::Region(0), "left is square A");
        assert_eq!(shared.right, Label::Region(1), "right is square B");
        let keys: Vec<(Pt, Pt)> = pieces.iter().map(|piece| (piece.a, piece.b)).collect();
        let mut sorted = keys.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), keys.len(), "no duplicate pieces");
    }

    #[test]
    fn t_junction_splits_the_touched_edge_into_three() {
        let layers = vec![
            room(0, rect(0, 0, 10, 10)),
            Layer {
                label: Label::Region(1),
                polygon: vec![(10, 2), (16, 2), (16, 8), (10, 8)],
            },
        ];
        let pieces = arrange(&layers, &[], &LayerLabeler::new(&layers)).unwrap();
        assert!(
            find_piece(&pieces, (10, 0), (10, 10)).is_none(),
            "A's edge was split"
        );
        let low = find_piece(&pieces, (10, 0), (10, 2)).unwrap();
        let middle = find_piece(&pieces, (10, 2), (10, 8)).unwrap();
        let high = find_piece(&pieces, (10, 8), (10, 10)).unwrap();
        assert_eq!(low.origin.len(), 1, "only square A reaches here");
        assert_eq!(high.origin.len(), 1, "only square A reaches here");
        assert_eq!(middle.origin.len(), 2, "A's edge and B's edge overlap");
        assert_eq!(middle.left, Label::Region(0), "left stays A");
        assert_eq!(middle.right, Label::Region(1), "right is B");
        assert_clean(&pieces);
    }

    #[test]
    fn the_top_square_relabels_below_and_drops_interior_edges() {
        let layers = vec![room(0, rect(0, 0, 10, 10)), room(1, rect(5, 5, 15, 15))];
        let pieces = arrange(&layers, &[], &LayerLabeler::new(&layers)).unwrap();
        assert!(
            find_piece(&pieces, (10, 5), (10, 10)).is_none(),
            "A's edge inside B is dropped"
        );
        assert!(
            find_piece(&pieces, (5, 10), (10, 10)).is_none(),
            "A's top inside B is dropped"
        );
        let edge = find_piece(&pieces, (10, 0), (10, 5)).unwrap();
        assert_eq!(edge.left, Label::Region(0), "outside B, inside A");
        assert_eq!(edge.right, Label::Void, "east of A is nothing");
        let b_floor = find_piece(&pieces, (5, 5), (10, 5)).unwrap();
        assert_eq!(b_floor.left, Label::Region(1), "inside both: top wins");
        assert_eq!(b_floor.right, Label::Region(0), "below is only A");
        assert_clean(&pieces);
    }

    #[test]
    fn a_void_strip_on_top_separates_two_rooms() {
        let layers = vec![
            room(1, rect(0, 0, 28, 10)),
            room(2, rect(32, 0, 60, 10)),
            Layer {
                label: Label::Void,
                polygon: rect(28, 0, 32, 10),
            },
        ];
        let pieces = arrange(&layers, &[], &LayerLabeler::new(&layers)).unwrap();
        let left_wall = find_piece(&pieces, (28, 0), (28, 10)).unwrap();
        assert_eq!(left_wall.left, Label::Region(1), "west is room 1");
        assert_eq!(left_wall.right, Label::Void, "east is the void strip");
        assert_eq!(left_wall.origin.len(), 2, "room and strip share the wall");
        let right_wall = find_piece(&pieces, (32, 0), (32, 10)).unwrap();
        assert_eq!(right_wall.left, Label::Void, "west is the void strip");
        assert_eq!(right_wall.right, Label::Region(2), "east is room 2");
        assert!(
            find_piece(&pieces, (28, 0), (32, 0)).is_none(),
            "the strip floor borders void both sides and is dropped"
        );
        assert!(
            find_piece(&pieces, (28, 10), (32, 10)).is_none(),
            "and the ceiling"
        );
        assert_clean(&pieces);
    }

    #[test]
    fn crossing_diagonals_split_at_the_rounded_point() {
        let external = vec![
            ExternalSegment {
                a: (0, 0),
                b: (10, 10),
            },
            ExternalSegment {
                a: (0, 9),
                b: (9, 0),
            },
        ];
        let labeler = CountingLabeler::default();
        let pieces = arrange(&[], &external, &labeler).unwrap();
        assert!(labeler.calls() > 0, "the labeler was used");
        assert_eq!(pieces.len(), 4, "both diagonals split at (5, 5)");
        assert!(find_piece(&pieces, (0, 0), (5, 5)).is_some(), "first leg 1");
        assert!(
            find_piece(&pieces, (5, 5), (10, 10)).is_some(),
            "first leg 2"
        );
        assert!(
            find_piece(&pieces, (0, 9), (5, 5)).is_some(),
            "second leg 1"
        );
        assert!(
            find_piece(&pieces, (5, 5), (9, 0)).is_some(),
            "second leg 2"
        );
        assert_clean(&pieces);
    }

    #[test]
    fn collinear_overlap_merges_into_one_piece_with_both_origins() {
        let external = vec![
            ExternalSegment {
                a: (0, 0),
                b: (10, 0),
            },
            ExternalSegment {
                a: (5, 0),
                b: (15, 0),
            },
        ];
        let pieces = arrange(&[], &external, &CountingLabeler::default()).unwrap();
        assert_eq!(pieces.len(), 3, "0-5, 5-10, 10-15");
        let middle = find_piece(&pieces, (5, 0), (10, 0)).unwrap();
        assert_eq!(middle.origin.len(), 2, "both external segments claim it");
        assert_eq!(
            middle.origin[0].source,
            SourceId::External(0),
            "first source"
        );
        assert_eq!(middle.origin[0].from_t, (1, 2), "half along the first");
        assert_eq!(
            middle.origin[1].source,
            SourceId::External(1),
            "second source"
        );
        assert_eq!(
            middle.origin[1].from_t,
            (0, 1),
            "at the start of the second"
        );
    }

    #[test]
    fn external_segments_split_and_keep_origin_fractions() {
        let layers = vec![room(0, rect(0, 0, 10, 10))];
        let external = vec![ExternalSegment {
            a: (-5, 5),
            b: (15, 5),
        }];
        let labeler = CountingLabeler::default();
        let pieces = arrange(&layers, &external, &labeler).unwrap();
        assert!(labeler.calls() > 0, "the labeler callback was used");
        let before = find_piece(&pieces, (-5, 5), (0, 5)).unwrap();
        let inside = find_piece(&pieces, (0, 5), (10, 5)).unwrap();
        let after = find_piece(&pieces, (10, 5), (15, 5)).unwrap();
        let claim = |t: (i128, i128)| (SourceId::External(0), t);
        assert!(
            piece_claims(before).contains(&claim((0, 1))),
            "starts at zero: {:?}",
            piece_claims(before)
        );
        assert!(
            piece_claims(inside).contains(&claim((1, 4))),
            "one quarter along: {:?}",
            piece_claims(inside)
        );
        assert!(
            piece_claims(after).contains(&claim((3, 4))),
            "three quarters along: {:?}",
            piece_claims(after)
        );
        assert_eq!(
            inside.origin.len(),
            1,
            "a perpendicular crossing shares no piece"
        );
        assert_eq!(
            inside.origin[0].source,
            SourceId::External(0),
            "single external claim"
        );
        assert!(
            find_piece(&pieces, (0, 0), (0, 10)).is_none(),
            "the crossed wall is split at (0, 5)"
        );
        let wall_low = find_piece(&pieces, (0, 0), (0, 5)).unwrap();
        assert!(
            matches!(
                wall_low.origin[0].source,
                SourceId::Layer { layer: 0, edge: 3 }
            ),
            "wall halves keep the layer edge as their source"
        );
    }

    #[test]
    fn degenerate_layers_are_invalid_arguments() {
        let bowtie = Layer {
            label: Label::Region(9),
            polygon: vec![(0, 0), (10, 10), (10, 0), (0, 5)],
        };
        let crossing_error = arrange(&[bowtie], &[], &LayerLabeler::new(&[])).unwrap_err();
        assert_eq!(crossing_error.code(), "invalid_argument", "self-crossing");
        let message = crossing_error.to_string();
        assert!(
            message.contains("layer 0")
                && message.contains("region 9")
                && message.contains("self-intersecting"),
            "names the layer and the fault: {message}"
        );

        let flat = Layer {
            label: Label::Void,
            polygon: vec![(0, 0), (5, 0), (10, 0)],
        };
        let flat_error = arrange(&[flat], &[], &LayerLabeler::new(&[])).unwrap_err();
        assert_eq!(flat_error.code(), "invalid_argument", "zero area");
        assert!(flat_error.to_string().contains("void"), "names the label");

        let few = Layer {
            label: Label::Region(1),
            polygon: vec![(0, 0), (5, 5), (5, 5)],
        };
        let few_error = arrange(&[few], &[], &LayerLabeler::new(&[])).unwrap_err();
        assert_eq!(few_error.code(), "invalid_argument", "too few vertices");
    }

    #[test]
    fn more_than_the_limit_of_segments_is_rejected() {
        let external: Vec<ExternalSegment> = (0..200_001_u32)
            .map(|index| {
                let x = i64::from(index);
                ExternalSegment {
                    a: (x, 0),
                    b: (x.saturating_add(1), 0),
                }
            })
            .collect();
        let error = arrange(&[], &external, &LayerLabeler::new(&[])).unwrap_err();
        assert_eq!(error.code(), "limit_exceeded", "segment cap");
    }

    #[test]
    fn the_same_input_produces_the_same_output() {
        let layers = vec![
            room(0, rect(0, 0, 10, 10)),
            room(1, rect(5, 5, 15, 15)),
            Layer {
                label: Label::Void,
                polygon: rect(12, 0, 13, 20),
            },
        ];
        let external = vec![ExternalSegment {
            a: (-4, 7),
            b: (18, 2),
        }];
        let first = arrange(&layers, &external, &LayerLabeler::new(&layers)).unwrap();
        let second = arrange(&layers, &external, &LayerLabeler::new(&layers)).unwrap();
        assert_eq!(first, second, "deterministic run to run");
        assert!(!first.is_empty(), "the fixture arranges something");
        assert_clean(&first);
    }

    /// A tiny xorshift so the property cases are deterministic.
    struct Rng(u64);

    impl Rng {
        /// A seeded generator; xorshift needs a nonzero state.
        fn new(seed: u64) -> Self {
            let mixed = seed
                .wrapping_add(0x9E37_79B9_7F4A_7C15)
                .wrapping_mul(0x0000_0100_0000_0001);
            Self(mixed.max(1))
        }

        /// The next pseudo-random word.
        fn next_u64(&mut self) -> u64 {
            let mut value = self.0;
            value ^= value.wrapping_shl(13);
            value ^= value.wrapping_shr(7);
            value ^= value.wrapping_shl(17);
            self.0 = value;
            value
        }

        /// The next pseudo-random value in `0..bound`.
        fn below(&mut self, bound: u64) -> u64 {
            self.next_u64().rem_euclid(bound)
        }
    }

    /// Void one cases in eight, else region `id`.
    fn pick_label(rng: &mut Rng, id: u32) -> Label {
        if rng.below(8) == 0 {
            Label::Void
        } else {
            Label::Region(id)
        }
    }

    /// Half the time, flip the ring's winding.
    fn maybe_reverse(rng: &mut Rng, polygon: &mut [Pt]) {
        if rng.below(2) == 0 {
            polygon.reverse();
        }
    }

    /// Three to six random rectangle layers on a 4-unit grid, some void.
    fn random_layers(rng: &mut Rng) -> Vec<Layer> {
        let count = rng.below(4).saturating_add(3);
        let mut layers = Vec::new();
        for index in 0..count {
            let low_x = i64::try_from(rng.below(48).saturating_mul(4)).unwrap();
            let low_y = i64::try_from(rng.below(48).saturating_mul(4)).unwrap();
            let wide = i64::try_from(rng.below(7).saturating_add(2))
                .unwrap()
                .saturating_mul(4);
            let tall = i64::try_from(rng.below(7).saturating_add(2))
                .unwrap()
                .saturating_mul(4);
            let id = u32::try_from(index).unwrap().saturating_add(1);
            let mut polygon = rect(
                low_x,
                low_y,
                low_x.saturating_add(wide),
                low_y.saturating_add(tall),
            );
            maybe_reverse(rng, &mut polygon);
            layers.push(Layer {
                label: pick_label(rng, id),
                polygon,
            });
        }
        layers
    }

    /// Every kept piece is oriented and carries at least one origin.
    fn check_kept(pieces: &[Piece], case: u64) {
        for piece in pieces {
            assert!(piece.a < piece.b, "case {case}: piece is oriented");
            assert!(!piece.origin.is_empty(), "case {case}: piece has an origin");
        }
    }

    #[test]
    fn random_axis_aligned_rectangles_never_cross_or_overlap() {
        for case in 0..200_u64 {
            let mut rng = Rng::new(case);
            let layers = random_layers(&mut rng);
            let pieces = arrange(&layers, &[], &LayerLabeler::new(&layers))
                .unwrap_or_else(|error| panic!("case {case} failed: {error}"));
            check_kept(&pieces, case);
            assert_clean(&pieces);
        }
    }

    #[test]
    fn zero_length_inputs_do_not_produce_pieces() {
        let external = vec![ExternalSegment {
            a: (3, 3),
            b: (3, 3),
        }];
        let pieces = arrange(&[], &external, &CountingLabeler::default()).unwrap();
        assert!(pieces.is_empty(), "a point is not a segment");
    }
}
