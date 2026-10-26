//! Integer segment geometry for the arrangement.
//!
//! Orientation and rounding primitives, the per-pair discovery of where
//! segments meet (proper crossings rounded to the nearest integer point,
//! endpoint T-junctions and collinear overlaps), the bounding-box
//! sort-and-prune sweep along x, and the point merge union-find that
//! collapses candidate points registered closer than one unit apart.

use alloc::collections::BTreeMap;

use crate::fixed::div_round;
use crate::geom::Pt;

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
pub(crate) fn delta(from: Pt, to: Pt) -> Delta {
    (
        i128::from(to.0).saturating_sub(i128::from(from.0)),
        i128::from(to.1).saturating_sub(i128::from(from.1)),
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

/// The sign of `(b - a) x (c - a)`: positive with `c` left of `a`→`b`,
/// zero on the line, negative right.
#[inline]
pub(crate) fn orient(a: Pt, b: Pt, c: Pt) -> i128 {
    cross(delta(a, b), delta(a, c))
}

/// Whether two orientation values have strictly opposite signs.
#[inline]
pub(crate) fn opposite(left: i128, right: i128) -> bool {
    left.signum()
        .checked_mul(right.signum())
        .is_some_and(|product| product < 0)
}

/// `(low, high)` sorted.
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
pub(crate) const fn between(point: Pt, from: Pt, to: Pt) -> bool {
    if from.0 == to.0 {
        let (low, high) = ordered(from.1, to.1);
        point.1 > low && point.1 < high
    } else {
        let (low, high) = ordered(from.0, to.0);
        point.0 > low && point.0 < high
    }
}

/// Whether `point` lies on segment `from`→`to`, strictly between the
/// ends, within half a unit perpendicular: squared distance <= 1/4, i.e.
/// `4 * cross^2 <= len^2`.
pub(crate) fn on_segment_tol(from: Pt, to: Pt, point: Pt) -> bool {
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
/// representative.
pub(crate) struct PointMerge {
    /// One slot per registration.
    nodes: Vec<Pt>,
    /// Union-find parent links, indexed like `nodes`.
    parent: Vec<usize>,
    /// First registration id of each distinct point.
    seen: BTreeMap<Pt, usize>,
}

impl PointMerge {
    /// A merge containing every segment end and every cut point.
    pub(crate) fn from_segments(segs: &[Seg], cuts: &[Vec<Pt>]) -> Self {
        let mut merge = Self {
            nodes: Vec::new(),
            parent: Vec::new(),
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
        let id = self.nodes.len();
        self.nodes.push(point);
        self.parent.push(id);
        let Some(first) = self.seen.get(&point).copied() else {
            self.seen.insert(point, id);
            return;
        };
        self.link(first, id);
    }

    /// Union two classes; the class of the smaller point wins.
    fn link(&mut self, first: usize, second: usize) {
        let (left, right) = (self.root(first), self.root(second));
        if left == right {
            return;
        }
        let keep_left = self
            .nodes
            .get(left)
            .zip(self.nodes.get(right))
            .is_some_and(|(low, high)| low <= high);
        let (winner, loser) = if keep_left {
            (left, right)
        } else {
            (right, left)
        };
        if let Some(slot) = self.parent.get_mut(loser) {
            *slot = winner;
        }
    }

    /// The class root of one registration. Classes stay shallow: a link
    /// always points a non-root straight at the winner.
    fn root(&self, id: usize) -> usize {
        let mut current = id;
        while self.parent.get(current).copied().unwrap_or(current) != current {
            current = self.parent.get(current).copied().unwrap_or(current);
        }
        current
    }

    /// The canonical representative of a registered point.
    pub(crate) fn canonical(&self, point: Pt) -> Option<Pt> {
        let first = self.seen.get(&point).copied()?;
        self.nodes.get(self.root(first)).copied()
    }

    /// Every distinct merged point, sorted by `(x, y)`.
    pub(crate) fn points(&self) -> Vec<Pt> {
        let mut list: Vec<Pt> = (0..self.nodes.len())
            .filter(|id| self.root(*id) == *id)
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
        PointMerge, Seg, between, cross, delta, dot, on_segment_tol, opposite, orient, sweep_cuts,
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
    fn opposite_requires_strict_signs() {
        assert!(opposite(1, -1), "opposite");
        assert!(!opposite(1, 0), "zero is not opposite");
        assert!(!opposite(-1, -2), "same side");
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
