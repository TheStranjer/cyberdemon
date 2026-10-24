//! Splitting every source segment at the points on it and collecting the
//! resulting pieces by endpoint pair.
//!
//! [`collect_segments`] validates every layer and turns the inputs into
//! the source segments the cut finder reads; [`split_chain`] in
//! [`dedupe_segments`] orders the cut points along one segment, and
//! [`add_chain_pieces`] records each piece of that chain under its
//! ordered endpoints, with [`origin_fraction`] carrying where along its
//! source the piece starts. Equal pieces therefore meet in one table
//! entry and their origins merge.

use alloc::collections::BTreeMap;

use super::{MAX_SEGMENTS, normalize_polygon, ring_edges, validate_polygon};
use crate::error::CyberError;
use crate::geom::segment::{
    ExternalSegment, PointMerge, Seg, SegmentOrigin, SourceId, between, cross, delta, dot,
    on_segment_tol,
};
use crate::geom::{Layer, Pt};

/// Validate every layer and collect polygon edges plus external segments.
pub(super) fn collect_segments(
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
pub(super) const fn gcd(mut left: i128, mut right: i128) -> i128 {
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
pub(super) const fn divided(value: i128, divisor: i128) -> i128 {
    match value.checked_div(divisor) {
        Some(exact) => exact,
        None => value,
    }
}

/// Reduce a fraction and force the denominator positive.
#[must_use]
pub(super) const fn normalize_fraction(fraction: (i128, i128)) -> (i128, i128) {
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
pub(super) fn dedupe_segments(
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
