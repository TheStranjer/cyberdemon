//! Integer segment geometry for the arrangement.
//!
//! Orientation and rounding primitives, generic over the [`Point`] trait
//! so every coordinate space in the crate shares one implementation, the
//! shared exact point-to-segment distance, the exact segment-meeting
//! tests (proper crossings, endpoint touches), the per-pair discovery of
//! where segments meet (crossings rounded to the nearest integer point,
//! endpoint T-junctions and collinear overlaps), the bounding-box
//! sort-and-prune sweep along x, the point merge union-find that
//! collapses candidate points registered closer than one unit apart, and
//! the side-sample geometry the arrangement and the overlay labelers ask
//! a piece's two sides about.

use alloc::collections::BTreeMap;

use crate::fixed::{div_round, hypot, isqrt};
use crate::geom::label::{SAMPLE_HALF, SAMPLE_OFFSET, ScaledPt};
use crate::geom::{Point, Pt, union::UnionFind};

/// At or above this many segments, `find_cuts` sweeps instead of
/// comparing every pair.
const SWEEP_MIN: usize = 512;

/// An `i128` vector between two integer points (saturating arithmetic).
pub(crate) type Delta = (i128, i128);

/// A caller-supplied segment, e.g. an existing linedef the arrangement
/// must cut its new pieces against.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ExternalSegment {
    /// First endpoint.
    pub a: Pt,
    /// Second endpoint.
    pub b: Pt,
}

/// Which input a piece came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SourceId {
    /// One edge of a layer polygon.
    Layer {
        /// Zero-based layer index.
        layer: usize,
        /// Zero-based edge index in the polygon's vertex order.
        edge: usize,
    },
    /// Zero-based index into the caller's external segments.
    External(usize),
}

/// One source segment's claim on an output piece.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct SegmentOrigin {
    /// Which input segment produced the piece.
    pub source: SourceId,
    /// Fraction along the source segment where this piece starts, as
    /// `(numerator, denominator)`: the denominator is positive, the
    /// fraction is reduced, and it lies in `[0, 1]`.
    pub from_t: (i128, i128),
}

/// A source segment under arrangement: one polygon edge or external
/// segment, before splitting.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Seg {
    /// Start endpoint, in source order.
    pub(crate) a: Pt,
    /// End endpoint, in source order.
    pub(crate) b: Pt,
    /// The input this segment came from.
    pub(crate) source: SourceId,
}

impl Seg {
    /// The smaller `x` coordinate.
    pub(crate) fn min_x(self) -> i64 {
        self.a.0.min(self.b.0)
    }

    /// The larger `x` coordinate.
    pub(crate) fn max_x(self) -> i64 {
        self.a.0.max(self.b.0)
    }

    /// The smaller `y` coordinate.
    fn min_y(self) -> i64 {
        self.a.1.min(self.b.1)
    }

    /// The larger `y` coordinate.
    fn max_y(self) -> i64 {
        self.a.1.max(self.b.1)
    }

    /// Whether the two segments' bounding boxes touch or overlap.
    fn boxes_overlap(self, other: Self) -> bool {
        self.min_x() <= other.max_x()
            && other.min_x() <= self.max_x()
            && self.min_y() <= other.max_y()
            && other.min_y() <= self.max_y()
    }
}

/// `to - from` as an `i128` vector (saturating at the extremes).
#[inline]
pub(crate) fn delta<P: Point>(from: P, to: P) -> Delta {
    (
        i128::from(to.x()).saturating_sub(i128::from(from.x())),
        i128::from(to.y()).saturating_sub(i128::from(from.y())),
    )
}

/// The `z` component of `u x v` (saturating).
#[must_use]
#[inline]
pub(crate) const fn cross(u: Delta, v: Delta) -> i128 {
    u.0.saturating_mul(v.1)
        .saturating_sub(u.1.saturating_mul(v.0))
}

/// The dot product `u . v` (saturating).
#[must_use]
#[inline]
pub(crate) const fn dot(u: Delta, v: Delta) -> i128 {
    u.0.saturating_mul(v.0)
        .saturating_add(u.1.saturating_mul(v.1))
}

/// The squared length of an exact vector.
#[must_use]
#[inline]
pub(crate) const fn squared_length(u: Delta) -> i128 {
    dot(u, u)
}

/// The squared distance between two points, exact in `i128`.
#[must_use]
#[inline]
pub(crate) fn distance_squared<P: Point>(a: P, b: P) -> i128 {
    squared_length(delta(a, b))
}

/// The sign of `(b - a) x (c - a)`: positive with `c` left of `a`→`b`,
/// zero on the line, negative right.
#[inline]
pub(crate) fn orient<P: Point>(a: P, b: P, c: P) -> i128 {
    cross(delta(a, b), delta(a, c))
}

/// Whether two orientation values have strictly opposite signs.
#[inline]
pub(crate) fn opposite(left: i128, right: i128) -> bool {
    left.signum()
        .checked_mul(right.signum())
        .is_some_and(|product| product < 0)
}

/// Whether the two closed segments cross at a point strictly inside
/// both: every endpoint of one is on the opposite strict side of the
/// other's line (a shared or touching endpoint, a T-junction and a
/// collinear touch all answer `false`).
#[must_use]
pub(crate) fn crosses_properly<P: Point>(first: &(P, P), second: &(P, P)) -> bool {
    let (head, tail) = *first;
    let (other_head, other_tail) = *second;
    opposite(
        orient(head, tail, other_head),
        orient(head, tail, other_tail),
    ) && opposite(
        orient(other_head, other_tail, head),
        orient(other_head, other_tail, tail),
    )
}

/// Whether `point` lies on the closed segment `from`→`to`: collinear and
/// inside the segment's bounding box (endpoints included).
#[must_use]
fn endpoint_on<P: Point>(from: P, to: P, point: P) -> bool {
    if orient(from, to, point) != 0 {
        return false;
    }
    point.x() >= from.x().min(to.x())
        && point.x() <= from.x().max(to.x())
        && point.y() >= from.y().min(to.y())
        && point.y() <= from.y().max(to.y())
}

/// Whether the two closed segments meet at any point: a proper crossing,
/// or an endpoint of either lying on the other. The strict simplicity
/// test walks with this predicate; the arrangement's own validation uses
/// the weaker [`crosses_properly`].
#[must_use]
pub(crate) fn segments_meet<P: Point>(first: &(P, P), second: &(P, P)) -> bool {
    let (head, tail) = *first;
    let (other_head, other_tail) = *second;
    crosses_properly(first, second)
        || endpoint_on(head, tail, other_head)
        || endpoint_on(head, tail, other_tail)
        || endpoint_on(other_head, other_tail, head)
        || endpoint_on(other_head, other_tail, tail)
}

/// `low` and `high` sorted.
#[must_use]
#[inline]
const fn ordered(low: i64, high: i64) -> (i64, i64) {
    if low <= high {
        (low, high)
    } else {
        (high, low)
    }
}

/// Whether `point`'s dominant-axis coordinate lies strictly between the
/// segment ends (call only after a collinearity test).
#[must_use]
#[inline]
pub(crate) fn between<P: Point>(point: P, from: P, to: P) -> bool {
    if from.x() == to.x() {
        let (low, high) = ordered(from.y(), to.y());
        point.y() > low && point.y() < high
    } else {
        let (low, high) = ordered(from.x(), to.x());
        point.x() > low && point.x() < high
    }
}

/// Whether `point` lies on segment `from`→`to`, strictly between the
/// ends, within half a unit perpendicular: squared distance <= 1/4, i.e.
/// `4 * cross^2 <= len^2`.
pub(crate) fn on_segment_tol<P: Point>(from: P, to: P, point: P) -> bool {
    let direction = delta(from, to);
    let length_squared = dot(direction, direction);
    if length_squared <= 0 {
        return false;
    }
    let perpendicular = cross(direction, delta(from, point));
    let close = perpendicular
        .checked_abs()
        .and_then(|magnitude| magnitude.checked_mul(magnitude))
        .and_then(|squared| squared.checked_mul(4))
        .is_some_and(|four_cross_squared| four_cross_squared <= length_squared);
    close && between(point, from, to)
}

/// `point` widened from the `i64` grid into the exact `i128` vector space.
#[must_use]
#[inline]
pub(crate) fn widen<P: Point>(point: P) -> Delta {
    (i128::from(point.x()), i128::from(point.y()))
}

/// The floored `hypot` length of the grid segment `from`→`to`.
#[must_use]
#[inline]
pub(crate) fn grid_hypot<P: Point>(from: P, to: P) -> u64 {
    hypot(
        to.x().saturating_sub(from.x()),
        to.y().saturating_sub(from.y()),
    )
}

/// The integer length of `start`→`end`: the `isqrt` of the exact squared
/// delta, at least one.
#[must_use]
pub(crate) fn integer_length<P: Point>(start: P, end: P) -> i128 {
    let direction = delta(start, end);
    let length_squared = dot(direction, direction);
    let magnitude = u64::try_from(length_squared).unwrap_or(u64::MAX);
    i128::from(isqrt(magnitude)).max(1)
}

/// The squared distance from `point` to the segment `from`→`to` as an
/// exact fraction `(numerator, denominator)` with `denominator` positive,
/// exact in `i128`: clamped endpoint distances use `1`, the perpendicular
/// interior distance the squared run length. Two of them compare by
/// cross-multiplication, and the distance is at least a squared `min`
/// exactly when the numerator is at least `min * min * denominator`.
#[must_use]
#[inline]
pub(crate) const fn distance_point_segment(point: Delta, from: Delta, to: Delta) -> (i128, i128) {
    let run = (to.0.saturating_sub(from.0), to.1.saturating_sub(from.1));
    let offset = (
        point.0.saturating_sub(from.0),
        point.1.saturating_sub(from.1),
    );
    let squared = dot(run, run);
    let along = dot(run, offset);
    if squared <= 0_i128 || along <= 0_i128 {
        return (dot(offset, offset), 1_i128);
    }
    if along >= squared {
        let back = (point.0.saturating_sub(to.0), point.1.saturating_sub(to.1));
        return (dot(back, back), 1_i128);
    }
    let perpendicular = cross(run, offset);
    (perpendicular.saturating_mul(perpendicular), squared)
}

/// The unit normal of `start`→`end` scaled to [`SAMPLE_OFFSET`] sample
/// units, left positive.
fn side_offset(start: Pt, end: Pt) -> ScaledPt {
    let direction = delta(start, end);
    let length = integer_length(start, end);
    (
        div_round(
            direction.1.saturating_neg().saturating_mul(SAMPLE_OFFSET),
            length,
        )
        .unwrap_or(0_i128),
        div_round(direction.0.saturating_mul(SAMPLE_OFFSET), length).unwrap_or(0_i128),
    )
}

/// The arrangement's two side samples of the piece `start`→`end`: the
/// points an eighth of a unit just left and just right of its midpoint,
/// in [`SAMPLE_SCALE`](crate::geom::label::SAMPLE_SCALE) coordinates.
#[must_use]
pub(crate) fn side_samples(start: Pt, end: Pt) -> [ScaledPt; 2] {
    let centre = (
        i128::from(start.0)
            .saturating_add(i128::from(end.0))
            .saturating_mul(SAMPLE_HALF),
        i128::from(start.1)
            .saturating_add(i128::from(end.1))
            .saturating_mul(SAMPLE_HALF),
    );
    let offset = side_offset(start, end);
    [
        (
            centre.0.saturating_add(offset.0),
            centre.1.saturating_add(offset.1),
        ),
        (
            centre.0.saturating_sub(offset.0),
            centre.1.saturating_sub(offset.1),
        ),
    ]
}

/// The point where the two segment lines cross, rounded to the nearest
/// integer point; `None` when the point does not fit the `i64` grid.
fn crossing_point(first: &Seg, second: &Seg) -> Option<Pt> {
    let run = delta(first.a, first.b);
    let rise = delta(second.a, second.b);
    let offset = delta(first.a, second.a);
    let determinant = cross(run, rise);
    let numerator = cross(offset, rise);
    let scaled_x = i128::from(first.a.0)
        .saturating_mul(determinant)
        .saturating_add(run.0.saturating_mul(numerator));
    let scaled_y = i128::from(first.a.1)
        .saturating_mul(determinant)
        .saturating_add(run.1.saturating_mul(numerator));
    Some((
        div_round(scaled_x, determinant).and_then(|axis| i64::try_from(axis).ok())?,
        div_round(scaled_y, determinant).and_then(|axis| i64::try_from(axis).ok())?,
    ))
}

/// Append a cut point to one segment's list, ignoring duplicates.
fn push_cut(cuts: &mut [Vec<Pt>], index: usize, point: Pt) {
    if let Some(list) = cuts.get_mut(index) {
        if !list.contains(&point) {
            list.push(point);
        }
    }
}

/// Endpoints of each collinear segment that lie strictly inside the
/// other: the ends of the overlap.
fn overlap_cuts(segs: &[Seg], first: usize, second: usize, cuts: &mut [Vec<Pt>]) {
    let (Some(sa), Some(sb)) = (segs.get(first), segs.get(second)) else {
        return;
    };
    for endpoint in [sb.a, sb.b] {
        if between(endpoint, sa.a, sa.b) {
            push_cut(cuts, first, endpoint);
        }
    }
    for endpoint in [sa.a, sa.b] {
        if between(endpoint, sb.a, sb.b) {
            push_cut(cuts, second, endpoint);
        }
    }
}

/// Endpoints sitting exactly on the other segment (T-junctions and
/// shared-line touches of non-parallel segments).
fn junction_cuts(segs: &[Seg], first: usize, second: usize, cuts: &mut [Vec<Pt>]) {
    let (Some(sa), Some(sb)) = (segs.get(first), segs.get(second)) else {
        return;
    };
    if orient(sa.a, sa.b, sb.a) == 0 && between(sb.a, sa.a, sa.b) {
        push_cut(cuts, first, sb.a);
    }
    if orient(sa.a, sa.b, sb.b) == 0 && between(sb.b, sa.a, sa.b) {
        push_cut(cuts, first, sb.b);
    }
    if orient(sb.a, sb.b, sa.a) == 0 && between(sa.a, sb.a, sb.b) {
        push_cut(cuts, second, sa.a);
    }
    if orient(sb.a, sb.b, sa.b) == 0 && between(sa.b, sb.a, sb.b) {
        push_cut(cuts, second, sa.b);
    }
}

/// Where two non-parallel segments meet: the rounded proper crossing, or
/// otherwise any endpoint-on-segment junctions.
fn crossing_cuts(segs: &[Seg], first: usize, second: usize, cuts: &mut [Vec<Pt>]) {
    let (Some(sa), Some(sb)) = (segs.get(first), segs.get(second)) else {
        return;
    };
    if opposite(orient(sa.a, sa.b, sb.a), orient(sa.a, sa.b, sb.b))
        && opposite(orient(sb.a, sb.b, sa.a), orient(sb.a, sb.b, sa.b))
    {
        if let Some(point) = crossing_point(sa, sb) {
            push_cut(cuts, first, point);
            push_cut(cuts, second, point);
        }
        return;
    }
    junction_cuts(segs, first, second, cuts);
}

/// Record every integer point where the two indexed segments meet into
/// both cut lists.
fn pair_cuts(segs: &[Seg], first: usize, second: usize, cuts: &mut [Vec<Pt>]) {
    let (Some(sa), Some(sb)) = (segs.get(first), segs.get(second)) else {
        return;
    };
    if !sa.boxes_overlap(*sb) {
        return;
    }
    if cross(delta(sa.a, sa.b), delta(sb.a, sb.b)) == 0 {
        if orient(sa.a, sa.b, sb.a) == 0 {
            overlap_cuts(segs, first, second, cuts);
        }
    } else {
        crossing_cuts(segs, first, second, cuts);
    }
}

/// Compare every pair directly; the fallback below the sweep threshold.
fn brute_cuts(segs: &[Seg], cuts: &mut [Vec<Pt>]) {
    for first in 0..segs.len() {
        for second in first.saturating_add(1)..segs.len() {
            pair_cuts(segs, first, second, cuts);
        }
    }
}

/// Sort-and-prune sweep along x: compare only segments whose bounding
/// boxes can still overlap.
fn sweep_cuts(segs: &[Seg], cuts: &mut [Vec<Pt>]) {
    let mut order: Vec<usize> = (0..segs.len()).collect();
    order.sort_by_key(|index| segs.get(*index).map_or(i64::MIN, |seg| seg.min_x()));
    let mut active: Vec<usize> = Vec::new();
    for current in order {
        let low = segs.get(current).map_or(i64::MIN, |seg| seg.min_x());
        active.retain(|other| segs.get(*other).is_some_and(|seg| seg.max_x() >= low));
        for other in &active {
            pair_cuts(segs, current, *other, cuts);
        }
        active.push(current);
    }
}

/// The cut points of every segment: all crossings, junctions and overlap
/// ends, as integers (crossings rounded to the nearest point).
pub(crate) fn find_cuts(segs: &[Seg]) -> Vec<Vec<Pt>> {
    let mut cuts: Vec<Vec<Pt>> = (0..segs.len()).map(|_| Vec::new()).collect();
    if segs.len() < SWEEP_MIN {
        brute_cuts(segs, &mut cuts);
    } else {
        sweep_cuts(segs, &mut cuts);
    }
    cuts
}

/// Union-find over the arrangement's candidate points.
///
/// Every candidate is already an integer point (crossings are rounded
/// when found), and distinct integer points are at least one unit apart,
/// so "closer than one unit" merges exactly the duplicates: each class
/// keeps its lexicographically smallest point as the canonical
/// representative, nominated through the shared [`UnionFind`].
pub(crate) struct PointMerge {
    /// One slot per registration.
    nodes: Vec<Pt>,
    /// Class links over the registration ids.
    classes: UnionFind,
    /// First registration id of each distinct point.
    seen: BTreeMap<Pt, usize>,
}

impl PointMerge {
    /// A merge containing every segment end and every cut point.
    pub(crate) fn from_segments(segs: &[Seg], cuts: &[Vec<Pt>]) -> Self {
        let mut merge = Self {
            nodes: Vec::new(),
            classes: UnionFind::with_len(0),
            seen: BTreeMap::new(),
        };
        for seg in segs {
            merge.register(seg.a);
            merge.register(seg.b);
        }
        for point in cuts.iter().flatten() {
            merge.register(*point);
        }
        merge
    }

    /// Register one candidate, uniting it with its earlier copies.
    fn register(&mut self, point: Pt) {
        let id = self.classes.push();
        self.nodes.push(point);
        let Some(first) = self.seen.get(&point).copied() else {
            self.seen.insert(point, id);
            return;
        };
        self.link(first, id);
    }

    /// Union two classes; the class of the smaller point wins.
    fn link(&mut self, first: usize, second: usize) {
        let (left, right) = (self.classes.find(first), self.classes.find(second));
        if left == right {
            return;
        }
        let keep_left = self
            .nodes
            .get(left)
            .zip(self.nodes.get(right))
            .is_some_and(|(low, high)| low <= high);
        let winner = if keep_left { left } else { right };
        self.classes.unite_keeping(first, second, winner);
    }

    /// The canonical representative of a registered point.
    pub(crate) fn canonical(&self, point: Pt) -> Option<Pt> {
        let first = self.seen.get(&point).copied()?;
        self.nodes.get(self.classes.find(first)).copied()
    }

    /// Every distinct merged point, sorted by `(x, y)`.
    pub(crate) fn points(&self) -> Vec<Pt> {
        let mut list: Vec<Pt> = self
            .classes
            .roots()
            .into_iter()
            .filter_map(|id| self.nodes.get(id).copied())
            .collect();
        list.sort_unstable();
        list.dedup();
        list
    }
}

#[cfg(test)]
mod tests {
    use super::{
        PointMerge, Seg, between, cross, crosses_properly, delta, distance_point_segment,
        distance_squared, dot, grid_hypot, integer_length, on_segment_tol, opposite, orient,
        segments_meet, side_samples, squared_length, sweep_cuts, widen,
    };
    use crate::geom::Pt;
    use crate::geom::segment::SourceId;

    /// A throwaway segment for the tests.
    fn seg(a: Pt, b: Pt) -> Seg {
        Seg {
            a,
            b,
            source: SourceId::External(0),
        }
    }

    /// A cut list for `count` segments.
    fn empty_cuts(count: usize) -> Vec<Vec<Pt>> {
        let mut cuts = Vec::new();
        for _ in 0..count {
            cuts.push(Vec::new());
        }
        cuts
    }

    #[test]
    fn primitives_compute_orient_and_products() {
        assert_eq!(orient((0, 0), (10, 0), (5, 5)), 50, "left is positive");
        assert_eq!(orient((0, 0), (10, 0), (5, -5)), -50, "right is negative");
        assert_eq!(orient((0, 0), (10, 0), (20, 0)), 0, "collinear is zero");
        assert_eq!(
            dot(delta((0, 0), (3, 4)), delta((0, 0), (3, 4))),
            25,
            "len squared"
        );
        assert_eq!(cross((0, 5), (0, -5)), 0, "parallel cross is zero");
    }

    #[test]
    fn the_primitives_read_non_tuple_points_through_the_trait() {
        let from = crate::bsp::Point { x: 0, y: 0 };
        let to = crate::bsp::Point { x: 10, y: 0 };
        let left = crate::bsp::Point { x: 5, y: 5 };
        assert_eq!(
            orient(from, to, left),
            50,
            "the fixed point space orients alike"
        );
        assert_eq!(distance_squared(from, to), 100, "and measures alike");
        assert_eq!(integer_length(from, to), 10, "and rounds its root alike");
        assert_eq!(
            widen(to),
            (i128::from(10_i64), 0_i128),
            "and widens axis by axis"
        );
    }

    #[test]
    fn distance_squared_and_lengths_measure_the_grid() {
        assert_eq!(distance_squared((0, 0), (3, 4)), 25, "the 3-4-5 hypotenuse");
        assert_eq!(distance_squared((0, 0), (0, 0)), 0, "same point");
        assert_eq!(
            distance_squared((-3, -4), (0, 0)),
            25,
            "negatives square positive"
        );
        assert_eq!(grid_hypot((0, 0), (3, 4)), 5, "floored hypot");
        assert_eq!(grid_hypot((2, 2), (2, 2)), 0, "a point's own length");
        assert_eq!(integer_length((0, 0), (3, 4)), 5, "exact root");
        assert_eq!(integer_length((0, 0), (0, 0)), 1, "clamped to one");
        assert_eq!(integer_length((0, 0), (1, 1)), 1, "sqrt(2) floors to one");
        assert_eq!(
            squared_length(delta((0, 0), (i64::MAX, i64::MAX))),
            {
                let max = i128::from(i64::MAX);
                max.saturating_mul(max)
                    .saturating_add(max.saturating_mul(max))
            },
            "the whole-range diagonal squares exactly inside i128"
        );
    }

    #[test]
    fn extreme_deltas_saturate_and_never_wrap() {
        assert_eq!(
            orient((i64::MIN, i64::MIN), (i64::MAX, i64::MIN), (0, i64::MAX)),
            i128::MAX,
            "the huge left turn saturates up"
        );
        assert_eq!(
            orient((i64::MIN, i64::MAX), (i64::MAX, i64::MAX), (0, i64::MIN)),
            i128::MIN,
            "and the huge right turn down"
        );
        assert_eq!(
            orient((i64::MIN, 0), (i64::MAX, 0), (i64::MIN, 0)),
            0,
            "collinear"
        );
        assert_eq!(
            distance_squared((i64::MIN, i64::MIN), (i64::MAX, i64::MAX)),
            i128::MAX,
            "the extreme distance saturates, never wraps"
        );
    }

    #[test]
    fn crosses_properly_demands_strict_interior_meeting() {
        assert!(
            crosses_properly(&((0, 0), (10, 10)), &((0, 9), (9, 0))),
            "an X crosses"
        );
        assert!(
            !crosses_properly(&((0, 0), (10, 0)), &((10, 0), (10, 10))),
            "a shared endpoint is not a crossing"
        );
        assert!(
            !crosses_properly(&((0, 0), (10, 0)), &((4, 0), (4, 6))),
            "a T-junction touches, not crosses"
        );
        assert!(
            !crosses_properly(&((0, 0), (10, 0)), &((5, 0), (15, 0))),
            "collinear overlap never crosses"
        );
        assert!(
            !crosses_properly(&((0, 0), (10, 0)), &((20, 0), (30, 0))),
            "parallel runs never cross"
        );
        assert!(
            crosses_properly(&((-5, -5), (5, 5)), &((-5, 4), (4, -5))),
            "negatives cross the same"
        );
    }

    #[test]
    fn segments_meet_adds_the_endpoint_touches() {
        assert!(
            segments_meet(&((0, 0), (10, 10)), &((0, 9), (9, 0))),
            "crossing"
        );
        assert!(
            segments_meet(&((0, 0), (10, 0)), &((10, 0), (10, 10))),
            "shared end"
        );
        assert!(
            segments_meet(&((0, 0), (10, 0)), &((4, 0), (4, 6))),
            "T-junction"
        );
        assert!(
            segments_meet(&((0, 0), (10, 0)), &((5, 0), (15, 0))),
            "collinear overlap"
        );
        assert!(
            !segments_meet(&((0, 0), (1, 0)), &((5, 5), (6, 6))),
            "disjoint"
        );
        assert!(
            !segments_meet(&((0, 0), (10, 0)), &((5, 1), (5, 6))),
            "a point a whole unit off the line touches nothing"
        );
    }

    #[test]
    fn side_samples_sit_an_eighth_left_and_right_of_the_midpoint() {
        let [east_left, east_right] = side_samples((0, 0), (10, 0));
        assert_eq!(
            east_left,
            (0x50_0000, 0x2_0000),
            "a left-facing eighth above"
        );
        assert_eq!(east_right, (0x50_0000, -0x2_0000), "and its mirror below");
        let [west_left, west_right] = side_samples((10, 0), (0, 0));
        assert_eq!(
            west_left,
            (0x50_0000, -0x2_0000),
            "west walks put left south"
        );
        assert_eq!(west_right, (0x50_0000, 0x2_0000), "and right north");
        let [degenerate_left, degenerate_right] = side_samples((5, 5), (5, 5));
        assert_eq!(
            degenerate_left, degenerate_right,
            "a degenerate piece samples its own point"
        );
        assert_eq!(
            degenerate_left,
            (0x50_0000, 0x50_0000),
            "the scaled midpoint stands"
        );
    }

    #[test]
    fn opposite_requires_strict_signs() {
        assert!(opposite(1, -1), "opposite");
        assert!(!opposite(1, 0), "zero is not opposite");
        assert!(!opposite(-1, -2), "same side");
    }

    #[test]
    fn widen_lifts_a_grid_point_into_the_exact_space() {
        assert_eq!(widen((3_i64, -4_i64)), (3_i128, -4_i128), "axis by axis");
        assert_eq!(
            widen((i64::from(i32::MIN), i64::from(i32::MAX))),
            (
                i128::from(i64::from(i32::MIN)),
                i128::from(i64::from(i32::MAX))
            ),
            "every map coordinate survives the lift"
        );
    }

    #[test]
    fn distance_point_segment_clamps_to_the_ends_off_span() {
        let from = widen((0_i64, 0_i64));
        let to = widen((10_i64, 0_i64));
        assert_eq!(
            distance_point_segment(widen((-3_i64, 4_i64)), from, to),
            (25_i128, 1_i128),
            "before the start: the squared distance to `from`"
        );
        assert_eq!(
            distance_point_segment(widen((13_i64, 4_i64)), from, to),
            (25_i128, 1_i128),
            "past the end: the squared distance to `to`"
        );
        assert_eq!(
            distance_point_segment(widen((0_i64, 0_i64)), from, to),
            (0_i128, 1_i128),
            "exactly on an end"
        );
        assert_eq!(
            distance_point_segment(from, from, from),
            (0_i128, 1_i128),
            "a zero-length segment measures to its one point"
        );
    }

    #[test]
    fn distance_point_segment_divides_by_the_squared_run_in_span() {
        let from = widen((0_i64, 0_i64));
        let to = widen((10_i64, 0_i64));
        assert_eq!(
            distance_point_segment(widen((5_i64, 3_i64)), from, to),
            (900_i128, 100_i128),
            "cross^2 over len^2: 30^2/10^2 = 9 squared units, unreduced"
        );
        assert_eq!(
            distance_point_segment(widen((5_i64, 0_i64)), from, to),
            (0_i128, 100_i128),
            "on the segment itself"
        );
        let steep = widen((10_i64, 15_i64));
        assert_eq!(
            distance_point_segment(steep, from, to),
            (225_i128, 1_i128),
            "level with the end and 15 above: the endpoint distance to (10, 0)"
        );
    }

    #[test]
    fn distance_point_segment_agrees_with_the_at_or_below_test() {
        // The clearance tests compare a squared `min` against the
        // fraction by `num >= min * min * den`; check that contract on a
        // diagonal, where the perpendicular case runs, against the
        // hand-computed distance of (4, 0) from the line `y = x`.
        let from = widen((0_i64, 0_i64));
        let to = widen((4_i64, 4_i64));
        let point = widen((4_i64, 0_i64));
        let (num, den) = distance_point_segment(point, from, to);
        assert_eq!(
            (num, den),
            (256_i128, 32_i128),
            "256/32 = 8 = (2*sqrt(2))^2"
        );
        for (min, at_least) in [(1_i128, true), (2_i128, true), (3_i128, false)] {
            assert_eq!(
                num >= min.saturating_mul(min).saturating_mul(den),
                at_least,
                "is the squared distance at least {min}^2?"
            );
        }
    }

    #[test]
    fn between_uses_the_dominant_axis() {
        assert!(between((5, 0), (0, 0), (10, 0)), "interior of horizontal");
        assert!(!between((0, 0), (0, 0), (10, 0)), "end is not interior");
        assert!(between((3, 4), (3, 0), (3, 10)), "vertical uses y");
        assert!(!between((3, 11), (3, 0), (3, 10)), "past the end");
    }

    #[test]
    fn on_segment_tol_applies_the_half_unit_rule() {
        assert!(on_segment_tol((0, 0), (10, 0), (5, 0)), "exact");
        assert!(on_segment_tol((0, 0), (4, 3), (2, 2)), "0.4 off passes");
        assert!(!on_segment_tol((0, 0), (10, 0), (5, 1)), "1 off fails");
        assert!(
            !on_segment_tol((0, 0), (10, 0), (11, 0)),
            "past the end fails"
        );
        assert!(
            !on_segment_tol((0, 0), (0, 0), (0, 0)),
            "degenerate segment"
        );
    }

    #[test]
    fn pair_cuts_records_crossings_junctions_and_overlaps() {
        let crossing = [seg((0, 0), (10, 10)), seg((0, 9), (9, 0))];
        let mut cross_cuts = empty_cuts(2);
        super::pair_cuts(&crossing, 0, 1, &mut cross_cuts);
        assert_eq!(cross_cuts[0], vec![(5, 5)], "rounded crossing on both");
        assert_eq!(cross_cuts[1], vec![(5, 5)], "same point recorded twice");

        let junction = [seg((0, 0), (10, 0)), seg((4, 0), (4, 6))];
        let mut junction_cuts = empty_cuts(2);
        super::pair_cuts(&junction, 0, 1, &mut junction_cuts);
        assert_eq!(junction_cuts[0], vec![(4, 0)], "T on the first");
        assert!(
            junction_cuts[1].is_empty(),
            "the junction is the second's end"
        );

        let overlap = [seg((0, 0), (10, 0)), seg((5, 0), (15, 0))];
        let mut overlap_cuts = empty_cuts(2);
        super::pair_cuts(&overlap, 0, 1, &mut overlap_cuts);
        assert_eq!(
            overlap_cuts[0],
            vec![(5, 0)],
            "second's start is inside the first"
        );
        assert_eq!(
            overlap_cuts[1],
            vec![(10, 0)],
            "first's end is inside the second"
        );

        let disjoint = [seg((0, 0), (1, 0)), seg((5, 5), (6, 6))];
        let mut disjoint_cuts = empty_cuts(2);
        super::pair_cuts(&disjoint, 0, 1, &mut disjoint_cuts);
        assert!(disjoint_cuts[0].is_empty(), "boxes reject");
        assert!(disjoint_cuts[1].is_empty(), "boxes reject both ways");
    }

    #[test]
    fn sweep_finds_the_same_cuts_as_brute_force() {
        let mut segs: Vec<Seg> = Vec::new();
        for index in 0..150_usize {
            let base = i64::try_from(index).unwrap().saturating_mul(3);
            segs.push(seg((base, 0), (base, 30)));
            let row = base.rem_euclid(30);
            segs.push(seg((0, row), (500, row.saturating_add(10))));
        }
        let mut swept = empty_cuts(segs.len());
        sweep_cuts(&segs, &mut swept);
        let mut brute = empty_cuts(segs.len());
        super::brute_cuts(&segs, &mut brute);
        assert!(brute.len() == swept.len(), "same shape");
        for pair in swept.iter().zip(brute.iter()) {
            let (left, right) = pair;
            let mut one = left.clone();
            let mut two = right.clone();
            one.sort_unstable();
            two.sort_unstable();
            assert_eq!(one, two, "cut sets agree per segment");
        }
        assert!(
            brute.iter().any(|list| !list.is_empty()),
            "the fixture must actually produce cuts"
        );
    }

    #[test]
    fn point_merge_canonicalises_duplicates_and_lists_roots() {
        let segs = vec![seg((0, 0), (10, 0)), seg((10, 0), (10, 10))];
        let cuts = vec![vec![(5, 0), (5, 0)], vec![(10, 0)]];
        let merged = PointMerge::from_segments(&segs, &cuts);
        assert_eq!(
            merged.canonical((5, 0)),
            Some((5, 0)),
            "duplicate keeps value"
        );
        assert_eq!(merged.canonical((0, 0)), Some((0, 0)), "endpoint");
        assert_eq!(merged.canonical((99, 99)), None, "unknown point");
        assert_eq!(
            merged.points(),
            vec![(0, 0), (5, 0), (10, 0), (10, 10)],
            "distinct sorted points"
        );
    }
}
