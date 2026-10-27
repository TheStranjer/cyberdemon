//! Shared ring and polygon primitives, generic over the [`Point`] trait.
//!
//! The closing edge list, ring normalisation, the exact `i128` shoelace,
//! one collinear-drop pass, the non-adjacent edge-crossing walk, and the
//! even-odd interior label point the describer and the renderer share.
//!
//! Every function is a pure fold over an ordered vertex ring or an
//! explicit directed edge list (a boundary may be several loops, so some
//! reads take edges directly). Coordinates widen to `i64`, products are
//! taken in exact `i128`: no float ever appears.

use alloc::vec::Vec;

use crate::fixed::{midpoint, narrow_i64};
use crate::geom::segment::{cross, crosses_properly, orient, segments_meet, widen};
use crate::geom::{Point, Pt};

/// The closing edge ring `(v0, v1), ..., (vn-1, v0)` of a vertex list:
/// one edge per vertex. A ring of fewer than two vertices has no edge
/// (a lone point closes to nothing).
#[must_use]
pub(crate) fn ring_edges<P: Point>(vertices: &[P]) -> Vec<(P, P)> {
    if vertices.len() < 2 {
        return Vec::new();
    }
    let Some(first) = vertices.first().copied() else {
        return Vec::new();
    };
    let mut edges: Vec<(P, P)> = Vec::with_capacity(vertices.len());
    let mut previous = first;
    for vertex in vertices.iter().skip(1) {
        edges.push((previous, *vertex));
        previous = *vertex;
    }
    edges.push((previous, first));
    edges
}

/// Drop repeated neighbours and an explicit closing vertex so every edge
/// of the returned ring has positive length.
#[must_use]
pub(crate) fn normalize_ring<P: Point + PartialEq>(points: &[P]) -> Vec<P> {
    let mut out: Vec<P> = Vec::with_capacity(points.len());
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

/// twice the signed area of a directed edge set, in exact `i128`
/// (shoelace): positive is counterclockwise with `y` up. A set of
/// consistently directed loops subtracts its holes, so one fold answers
/// even-odd area for polygons with islands.
#[must_use]
pub(crate) fn edge_area2<P: Point>(edges: &[(P, P)]) -> i128 {
    edges.iter().fold(0_i128, |total, &(start, end)| {
        total.saturating_add(cross(widen(start), widen(end)))
    })
}

/// twice the signed area of the closed ring `vertices` (shoelace).
#[must_use]
pub(crate) fn area2<P: Point>(vertices: &[P]) -> i128 {
    edge_area2(&ring_edges(vertices))
}

/// The vertex at cyclic index `index`, `None` for an empty ring.
fn ring_get<P: Point>(ring: &[P], index: usize) -> Option<P> {
    let wrapped = index.checked_rem(ring.len())?;
    ring.get(wrapped).copied()
}

/// One pass dropping ring vertices that lie exactly on the line through
/// their two (distinct) neighbours; reports whether anything went. Rings
/// of fewer than three vertices are returned unchanged.
#[must_use]
pub(crate) fn drop_collinear<P: Point + PartialEq>(ring: &[P]) -> (Vec<P>, bool) {
    let count = ring.len();
    if count < 3 {
        return (ring.to_vec(), false);
    }
    let mut out: Vec<P> = Vec::with_capacity(count);
    let mut dropped = false;
    for (index, point) in ring.iter().enumerate() {
        let back = index
            .checked_sub(1)
            .unwrap_or_else(|| count.saturating_sub(1));
        let (Some(prev), Some(next)) = (
            ring_get(ring, back),
            ring_get(ring, index.saturating_add(1)),
        ) else {
            out.push(*point);
            continue;
        };
        if prev != next && orient(prev, next, *point) == 0 {
            dropped = true;
            continue;
        }
        out.push(*point);
    }
    (out, dropped)
}

/// The first pair of non-adjacent ring edges that `meets` reports:
/// edges are tested in order, pairs sharing a vertex (including the
/// closing pair `(0, n-1)`) are never asked about.
fn first_meeting_pair<P: Point, F>(edges: &[(P, P)], meets: F) -> Option<(usize, usize)>
where
    F: Fn(&(P, P), &(P, P)) -> bool,
{
    let count = edges.len();
    for first in 0..count {
        let Some(second) = find_meeting(edges, first, count, &meets) else {
            continue;
        };
        return Some((first, second));
    }
    None
}

/// The first edge after `first`'s neighbours that `meets` accepts, if
/// any: scan starts two ahead (edge `first + 1` shares a vertex), and
/// the last edge is skipped for `first == 0` (it closes onto edge `0`).
fn find_meeting<P: Point, F>(
    edges: &[(P, P)],
    first: usize,
    count: usize,
    meets: &F,
) -> Option<usize>
where
    F: Fn(&(P, P), &(P, P)) -> bool,
{
    let last = count.saturating_sub(1);
    for second in first.saturating_add(2)..count {
        if first == 0 && second == last {
            continue;
        }
        let (Some(one), Some(two)) = (edges.get(first), edges.get(second)) else {
            return None;
        };
        if meets(one, two) {
            return Some(second);
        }
    }
    None
}

/// The first pair of non-adjacent edges of the ring `vertices` that
/// properly cross (adjacent edges may fold flat; a touch that is not a
/// proper crossing is not reported — see [`is_simple`] for that).
#[must_use]
pub(crate) fn self_crossing<P: Point>(vertices: &[P]) -> Option<(usize, usize)> {
    first_meeting_pair(&ring_edges(vertices), crosses_properly)
}

/// Whether the closed ring `vertices` is simple: no two non-adjacent
/// edges meet at all (properly crossing or touching). Rings of fewer
/// than three edges are simple by definition.
#[must_use]
pub(crate) fn is_simple<P: Point>(vertices: &[P]) -> bool {
    first_meeting_pair(&ring_edges(vertices), segments_meet).is_none()
}

/// The middle row of the edges' bounding box, `(lowest, highest)`
/// endpoint rows over `edges`, `None` when empty.
fn edge_rows<P: Point>(edges: &[(P, P)]) -> Option<(i64, i64)> {
    let rows = edges.iter().flat_map(|&(start, end)| [start.y(), end.y()]);
    let min = rows.clone().min()?;
    Some((min, rows.max()?))
}

/// Whether `edge` straddles row `row` under the half-open even-odd rule.
#[inline]
fn crosses_row<P: Point>(edge: &(P, P), row: i64) -> bool {
    (edge.0.y() > row) != (edge.1.y() > row)
}

/// The widest `(lo, hi)` interval between consecutive pairs of the sorted
/// `crossings`, `None` when fewer than two of them exist.
fn longest_interval(crossings: &[i64]) -> Option<(i64, i64)> {
    let mut best: Option<(i64, i64)> = None;
    for pair in crossings.chunks_exact(2) {
        let Ok(&[lo, hi]) = <&[i64; 2]>::try_from(pair) else {
            continue;
        };
        if wider_than((lo, hi), best) {
            best = Some((lo, hi));
        }
    }
    best
}

/// Whether the interval `one` is wider than `best` (or `best` is unset).
const fn wider_than(one: (i64, i64), best: Option<(i64, i64)>) -> bool {
    match best {
        None => true,
        Some((b_lo, b_hi)) => one.1.saturating_sub(one.0) > b_hi.saturating_sub(b_lo),
    }
}

/// A guaranteed interior anchor of a boundary edge set: the middle row of
/// the set's box is scanned with the even-odd rule, the longest inside
/// interval wins and its midpoint is the label point; a set the row
/// misses falls back to its first edge's start. No edges, no anchor.
///
/// Both [`crate::wad::map::describe`]'s sector centres and
/// [`crate::render`]'s top-down index labels are read through this one
/// scan, so the two agree on the very same pixel by construction.
#[must_use]
pub(crate) fn label_point<P: Point>(edges: &[(P, P)]) -> Option<Pt> {
    let (min_y, max_y) = edge_rows(edges)?;
    let row = midpoint(min_y, max_y);
    let mut crossings: Vec<i64> = edges
        .iter()
        .filter(|edge| crosses_row(edge, row))
        .map(|edge| intersect_x(edge.0, edge.1, row))
        .collect();
    crossings.sort_unstable();
    longest_interval(&crossings)
        .map(|(lo, hi)| (midpoint(lo, hi), row))
        .or_else(|| edges.first().map(|edge| (edge.0.x(), edge.0.y())))
}

/// `base + delta * num / den` in exact `i128`, floor-rounded and
/// saturating back to `i64`; a zero `den` (a degenerate segment) answers
/// `base`.
fn scale_down(base: i64, delta: i64, num: i128, den: i128) -> i64 {
    if den == 0 {
        return base;
    }
    let (n, d) = if den < 0 {
        (
            i128::from(delta).saturating_neg().saturating_mul(num),
            den.saturating_neg(),
        )
    } else {
        (i128::from(delta).saturating_mul(num), den)
    };
    let drawn = i128::from(base)
        .saturating_mul(d)
        .saturating_add(n)
        .div_euclid(d);
    narrow_i64(drawn)
}

/// The column where the `start` to `end` line crosses row `y`,
/// floor-rounded in exact `i128`; a horizontal edge answers `start`'s
/// column (the even-odd filter never asks about one).
#[must_use]
pub(crate) fn intersect_x<P: Point>(start: P, end: P, y: i64) -> i64 {
    let rise = i128::from(y).saturating_sub(i128::from(start.y()));
    let span = i128::from(end.y()).saturating_sub(i128::from(start.y()));
    scale_down(start.x(), end.x().saturating_sub(start.x()), rise, span)
}

/// The row where the `start` to `end` line crosses column `x`,
/// floor-rounded in exact `i128`; a vertical edge answers `start`'s row.
#[must_use]
pub(crate) fn intersect_y<P: Point>(start: P, end: P, x: i64) -> i64 {
    let run = i128::from(x).saturating_sub(i128::from(start.x()));
    let span = i128::from(end.x()).saturating_sub(i128::from(start.x()));
    scale_down(start.y(), end.y().saturating_sub(start.y()), run, span)
}

#[cfg(test)]
mod tests {
    use super::{
        area2, drop_collinear, edge_area2, intersect_x, intersect_y, is_simple, label_point,
        normalize_ring, ring_edges, self_crossing,
    };
    use crate::bsp::Point as FixedPoint;

    /// A `128x64` rectangle ring, counterclockwise.
    fn rect() -> Vec<(i64, i64)> {
        vec![(0, 0), (128, 0), (128, 64), (0, 64)]
    }

    #[test]
    fn ring_edges_closes_every_ring_and_skips_the_degenerate_ones() {
        assert!(ring_edges::<(i64, i64)>(&[]).is_empty(), "empty");
        assert!(ring_edges(&[(7, -7)]).is_empty(), "a lone point");
        assert_eq!(
            ring_edges(&[(0, 0), (4, 0)]),
            vec![((0, 0), (4, 0)), ((4, 0), (0, 0))],
            "a two-point ring goes there and back"
        );
        let edges = ring_edges(&rect());
        assert_eq!(edges.len(), 4, "one edge per vertex");
        assert_eq!(edges[3], ((0, 64), (0, 0)), "the closing edge");
    }

    #[test]
    fn normalize_ring_drops_repeats_and_the_closing_vertex() {
        let closed = vec![(0, 0), (10, 0), (10, 10), (0, 0)];
        assert_eq!(normalize_ring(&closed), vec![(0, 0), (10, 0), (10, 10)]);
        let doubled = vec![(0, 0), (0, 0), (10, 0), (10, 10)];
        assert_eq!(normalize_ring(&doubled), vec![(0, 0), (10, 0), (10, 10)]);
        assert!(normalize_ring::<(i64, i64)>(&[]).is_empty(), "empty");
        assert_eq!(
            normalize_ring(&[(3, 3), (3, 3)]),
            vec![(3, 3)],
            "a doubled point collapses to one"
        );
    }

    #[test]
    fn area2_is_the_signed_shoelace() {
        assert_eq!(area2(&rect()), 0x4000, "twice the 128x64 area");
        let clockwise: Vec<(i64, i64)> = rect().into_iter().rev().collect();
        assert_eq!(area2(&clockwise), -0x4000, "winding flips the sign");
        assert_eq!(area2(&[(0, 0), (5, 0), (10, 0)]), 0, "a flat ring");
        assert_eq!(area2(&[(0, 0), (0, 0)]), 0, "a collapsed ring");
        assert_eq!(area2::<(i64, i64)>(&[]), 0, "nothing");
        assert_eq!(
            area2(&[(-4, -4), (4, -4), (4, 4), (-4, 4)]),
            128_i128,
            "negative coordinates carry the same 8x8 area"
        );
    }

    #[test]
    fn area2_saturates_at_the_i64_corners_rather_than_wrapping() {
        let ccw = [
            (i64::MIN, i64::MIN),
            (i64::MAX, i64::MIN),
            (i64::MAX, i64::MAX),
            (i64::MIN, i64::MAX),
        ];
        assert_eq!(area2(&ccw), i128::MAX, "the sum saturates upward");
        let cw: Vec<(i64, i64)> = ccw.into_iter().rev().collect();
        assert_eq!(area2(&cw), i128::MIN, "and the reversed winding downward");
    }

    #[test]
    fn edge_area2_sums_directed_loops_even_odd() {
        let outer = ring_edges(&rect());
        let mut hole_ring = [(2, 2), (6, 2), (6, 6), (2, 6)].to_vec();
        hole_ring.reverse();
        let inner = ring_edges(&hole_ring);
        let mut both = outer.clone();
        both.extend(inner);
        assert_eq!(edge_area2(&outer), 0x4000, "the outer loop alone");
        assert_eq!(
            edge_area2(&both),
            0x4000 - 32,
            "the reversed hole subtracts"
        );
        assert_eq!(edge_area2::<(i64, i64)>(&[]), 0, "no edges, no area");
    }

    #[test]
    fn drop_collinear_peels_mid_edge_points_only() {
        let ring = [(0, 0), (4, 0), (4, 3), (2, 3), (0, 3)];
        let (peeled, dropped_mid) = drop_collinear(&ring);
        assert!(dropped_mid, "the mid point on the top run goes");
        assert_eq!(peeled, vec![(0, 0), (4, 0), (4, 3), (0, 3)]);
        let (corners, dropped_corner) = drop_collinear(&rect());
        assert!(!dropped_corner, "a rectangle's corners all turn");
        assert_eq!(corners, rect());
        let tiny = [(1, 1), (2, 2)];
        assert_eq!(drop_collinear(&tiny), (tiny.to_vec(), false), "under three");
        let spiked = [(0, 0), (5, 5), (0, 0), (5, 0)];
        let (kept_spike, dropped_spike) = drop_collinear(&spiked);
        assert!(
            !dropped_spike,
            "a vertex between equal neighbours is protected: {kept_spike:?}"
        );
        assert_eq!(kept_spike, spiked.to_vec());
        let negatives = [(-8, -8), (-4, -8), (0, -8), (0, -2)];
        let (kept_flat, dropped_flat) = drop_collinear(&negatives);
        assert!(
            dropped_flat,
            "a collinear run of negatives still drops its middle"
        );
        assert_eq!(kept_flat, vec![(-8, -8), (0, -8), (0, -2)]);
    }

    #[test]
    fn self_crossing_reports_the_first_proper_pair() {
        assert_eq!(self_crossing(&rect()), None, "a rectangle is simple");
        assert_eq!(
            self_crossing(&[(0, 0), (10, 10), (10, 0), (0, 5)]),
            Some((0, 2)),
            "crossed quadrilateral: edges 0 and 2"
        );
        assert_eq!(
            self_crossing(&[(-1, -1), (1, 1), (1, -1), (-1, 1)]),
            Some((0, 2)),
            "negative coordinates cross the same"
        );
        assert_eq!(self_crossing(&[(0, 0), (1, 1)]), None, "two-point ring");
        assert_eq!(self_crossing::<(i64, i64)>(&[]), None, "empty ring");
    }

    #[test]
    fn self_crossing_forgives_touches_that_are_not_proper_crossings() {
        // A vertex sitting exactly on a non-adjacent edge is not a proper
        // crossing, so the arrangement's own validation passes on it (the
        // stricter `is_simple` refuses it; see the simplicity test).
        assert_eq!(
            self_crossing(&[(0, 0), (4, 0), (4, 4), (2, 0), (0, 4)]),
            None,
            "a T-touch on the base is not a proper crossing"
        );
    }

    #[test]
    fn is_simple_refuses_every_non_adjacent_meeting() {
        assert!(is_simple(&rect()), "a rectangle is simple");
        assert!(
            !is_simple(&[(0, 0), (3, 3), (3, 0), (0, 3)]),
            "a bow-tie crosses itself"
        );
        assert!(
            !is_simple(&[(0, 0), (4, 0), (4, 4), (2, 0), (0, 4)]),
            "a vertex on a non-adjacent edge is not simple"
        );
        assert!(
            is_simple(&[(0, 0), (1, 1)]),
            "under three vertices cannot cross"
        );
        assert!(is_simple::<(i64, i64)>(&[]), "an empty ring is simple");
        assert!(
            is_simple(&[(-2, -2), (2, -2), (2, 2), (-2, 2)]),
            "a negative square is simple"
        );
    }

    #[test]
    fn the_walks_read_the_fixed_point_space_through_the_same_code() {
        let point = |x: i64, y: i64| FixedPoint { x, y };
        let square = vec![
            point(0, 0),
            point(10_000, 0),
            point(10_000, 10_000),
            point(0, 10_000),
        ];
        assert_eq!(area2(&square), 2 * 10_000_i128 * 10_000, "16.16 area");
        assert_eq!(self_crossing(&square), None, "convex");
        assert!(is_simple(&square), "simple");
        assert_eq!(drop_collinear(&square), (square.clone(), false), "corners");
    }

    #[test]
    fn label_point_finds_the_widest_middle_interval() {
        let edges = ring_edges(&rect());
        assert_eq!(
            label_point(&edges),
            Some((64, 32)),
            "the centre of a 128x64 box"
        );
        let negative = ring_edges(&[(-10, -10), (0, -10), (0, 0), (-10, 0)]);
        assert_eq!(
            label_point(&negative),
            Some((-5, -5)),
            "negative box centre"
        );
        assert_eq!(label_point::<(i64, i64)>(&[]), None, "no edges, no point");
    }

    #[test]
    fn label_point_prefers_the_wider_of_two_intervals_on_the_row() {
        // A square boundary with an off-centre hole: the middle row cuts
        // (0..30) and (50..100); the wider one wins.
        let mut edges = ring_edges(&[(0, 0), (100, 0), (100, 100), (0, 100)]);
        edges.extend(ring_edges(&[(30, 20), (50, 20), (50, 80), (30, 80)]));
        assert_eq!(label_point(&edges), Some((75, 50)), "the (50, 100) span");
    }

    #[test]
    fn label_point_falls_back_to_the_first_vertex_when_the_row_misses() {
        let edges = vec![((0, 0), (10, 0)), ((10, 0), (20, 0))];
        assert_eq!(
            label_point(&edges),
            Some((0, 0)),
            "a flat boundary's middle row crosses nothing"
        );
    }

    #[test]
    fn intersect_x_floors_towards_negative_infinity() {
        assert_eq!(intersect_x((0, 0), (4, 2), 1), 2, "exact half");
        assert_eq!(intersect_x((0, 0), (3, 2), 1), 1, "1.5 floors to 1");
        assert_eq!(intersect_x((0, 0), (-3, 2), 1), -2, "-1.5 floors to -2");
        assert_eq!(
            intersect_x((5, 7), (9, 7), 3),
            5,
            "a horizontal edge answers its start"
        );
        assert_eq!(
            intersect_x((0, 4), (4, 0), 2),
            2,
            "a descending line crosses its middle"
        );
    }

    #[test]
    fn intersect_y_mirrors_intersect_x() {
        assert_eq!(intersect_y((0, 0), (2, 4), 1), 2, "exact half");
        assert_eq!(intersect_y((0, 0), (2, 3), 1), 1, "1.5 floors to 1");
        assert_eq!(
            intersect_y((7, 5), (7, 9), 3),
            5,
            "a vertical edge answers its start"
        );
    }

    #[test]
    fn label_point_extremes_stay_exact_through_the_i128_products() {
        let ring = [
            (i64::MIN, i64::MIN),
            (i64::MAX, i64::MIN),
            (i64::MAX, i64::MAX),
            (i64::MIN, i64::MAX),
        ];
        let edges = ring_edges(&ring);
        let at = label_point(&edges).expect("the huge box labels");
        assert_eq!(at, (-1, -1), "the exact middle of the whole i64 range");
    }
}
