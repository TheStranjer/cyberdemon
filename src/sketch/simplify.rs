//! Turning crack-traced pixel rings into clean polygons: Douglas-Peucker
//! simplification, ring cleanup and right-angle snapping.
//!
//! [`simplify`] drops vertices that sit close enough to the chord across
//! them. The keep test is the ticket's division-free one: keep `p` between
//! chord `a..b` while `cross(b - a, p - a)^2 > tolerance^2 * |b - a|^2`,
//! evaluated in `i128`, so no distance and no float is ever computed. A
//! closed ring is first split at the vertex farthest from its first
//! vertex and both chains are simplified before rejoining; if cleanup
//! leaves fewer than three vertices the ring collapses to the bounding
//! box rectangle of its input.
//!
//! [`snap_right_angles`] straightens hand-traced walls: every maximal run
//! of near-horizontal edges is flattened onto the run's rounded mean `y`,
//! every near-vertical run onto its mean `x`; edges that are neither (more
//! than `max_skew_permille` off axis) keep their slope.

use crate::geom::Pt;
use crate::geom::ring::{drop_collinear, normalize_ring};
use crate::geom::segment::{distance_squared, orient};
use crate::gfx::ops::rounded_div;

/// A polygon vertex in pixel-corner coordinates.
type Point = Pt;

/// Which coordinate an edge run flattens onto.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Axis {
    /// Vertical runs share one `x`.
    X,
    /// Horizontal runs share one `y`.
    Y,
}

impl Axis {
    /// Reads the coordinate this axis owns.
    const fn get(self, point: Point) -> i64 {
        match self {
            Self::X => point.0,
            Self::Y => point.1,
        }
    }

    /// Writes the coordinate this axis owns.
    const fn set(self, point: &mut Point, value: i64) {
        match self {
            Self::X => point.0 = value,
            Self::Y => point.1 = value,
        }
    }
}

/// Douglas-Peucker simplification of a closed ring.
///
/// Vertices are dropped for `tolerance` pixels; collinear and duplicate
/// vertices are removed afterwards; a ring reduced below three vertices
/// comes back as the bounding box rectangle of the input.
#[must_use]
#[inline]
pub fn simplify(points: &[Point], tolerance: u32) -> Vec<Point> {
    let reduced = dp_closed(points, tolerance);
    let clean = clean_ring(&reduced);
    if clean.len() < 3 {
        return bbox_ring(points);
    }
    clean
}

/// Straightens `points` onto right angles.
///
/// Each maximal run of near-axis-aligned edges is flattened onto its
/// rounded mean coordinate, horizontal runs first and vertical runs
/// second, then collinear and duplicate vertices are removed.
/// `max_skew_permille` is the tolerated skew in thousandths of the edge
/// length along the other axis; `170` accepts about `9.6` degrees. Edges
/// that are neither stay diagonal.
#[must_use]
#[inline]
pub fn snap_right_angles(points: &[Point], max_skew_permille: u32) -> Vec<Point> {
    let mut ring = clean_ring(points);
    if ring.len() < 3 {
        return ring;
    }
    let horizontal = edge_flags(&ring, Axis::Y, max_skew_permille);
    let vertical = edge_flags(&ring, Axis::X, max_skew_permille);
    for run in runs(&horizontal) {
        flatten_run(&mut ring, &run, Axis::Y);
    }
    for run in runs(&vertical) {
        flatten_run(&mut ring, &run, Axis::X);
    }
    clean_ring(&ring)
}

/// Whether every edge of the ring is axis-aligned (a diagonal edge, or
/// fewer than two vertices, answers `false`).
pub(crate) fn edges_axis_aligned(points: &[Point]) -> bool {
    if points.len() < 2 {
        return false;
    }
    let rotated = points.iter().skip(1).chain(points.first());
    points
        .iter()
        .zip(rotated)
        .all(|(one, two)| one.0 == two.0 || one.1 == two.1)
}

/// Drops duplicate and collinear vertices of the closed ring `points`,
/// keeping the result cyclically reduced.
pub(crate) fn clean_ring(points: &[Point]) -> Vec<Point> {
    let mut out = normalize_ring(points);
    loop {
        let (shrunk, dropped) = drop_collinear(&out);
        out = shrunk;
        if !dropped || out.len() < 3 {
            return out;
        }
    }
}

/// The vertex at cyclic index `index`, `(0, 0)` for an empty ring.
fn ring_get(points: &[Point], index: usize) -> Point {
    let wrapped = index.checked_rem(points.len()).unwrap_or(usize::MAX);
    points.get(wrapped).copied().unwrap_or((0, 0))
}

/// The clockwise bounding box rectangle of `points`; empty input yields
/// an empty ring.
fn bbox_ring(points: &[Point]) -> Vec<Point> {
    let Some(&(first_x, first_y)) = points.first() else {
        return Vec::new();
    };
    let mut x0 = first_x;
    let mut x1 = first_x;
    let mut y0 = first_y;
    let mut y1 = first_y;
    for &(x, y) in points.iter().skip(1) {
        x0 = x0.min(x);
        x1 = x1.max(x);
        y0 = y0.min(y);
        y1 = y1.max(y);
    }
    vec![(x0, y0), (x1, y0), (x1, y1), (x0, y1)]
}

/// The rounded mean of `values`, halves away from zero; `0` when empty.
pub(crate) fn rounded_mean(values: &[i64]) -> i64 {
    let sum = values
        .iter()
        .fold(0_i64, |acc, &value| acc.saturating_add(value));
    let Ok(len) = u64::try_from(values.len()) else {
        return sum;
    };
    if len == 0 {
        return 0;
    }
    let mean = rounded_div(sum.unsigned_abs(), len);
    let Ok(magnitude) = i64::try_from(mean) else {
        return if sum < 0 { i64::MIN } else { i64::MAX };
    };
    if sum < 0 {
        magnitude.saturating_neg()
    } else {
        magnitude
    }
}

/// The Douglas-Peucker split: farthest vertex from the first, then both
/// chains kept above the tolerance, rejoined in ring order.
fn dp_closed(points: &[Point], tolerance: u32) -> Vec<Point> {
    if points.len() < 3 {
        return points.to_vec();
    }
    let Some(first) = points.first().copied() else {
        return Vec::new();
    };
    let split = farthest_index(points, first);
    let head = dp_line(&chain(points, 0, split), tolerance);
    let tail = dp_line(&chain(points, split, 0), tolerance);
    let mut out = head;
    out.extend(tail.into_iter().skip(1));
    out
}

/// The index of the vertex farthest from `from`; the first maximum wins.
fn farthest_index(points: &[Point], from: Point) -> usize {
    let mut best = (0_i128, 0_usize);
    for (index, &point) in points.iter().enumerate() {
        let distance = distance_squared(from, point);
        if distance > best.0 {
            best = (distance, index);
        }
    }
    best.1
}

/// The ring vertices walked forward from index `from` to index `to`,
/// both inclusive.
fn chain(points: &[Point], from: usize, to: usize) -> Vec<Point> {
    let count = points.len();
    let mut out = Vec::new();
    if count == 0 {
        return out;
    }
    let limit = count.checked_add(1).unwrap_or(count);
    let mut index = from.min(count.saturating_sub(1));
    for _ in 0..limit {
        if let Some(&point) = points.get(index) {
            out.push(point);
        }
        if index == to {
            break;
        }
        index = index
            .checked_add(1)
            .and_then(|next| next.checked_rem(count))
            .unwrap_or(0);
    }
    out
}

/// Open-polyline Douglas-Peucker keeping the endpoints, iterative.
fn dp_line(points: &[Point], tolerance: u32) -> Vec<Point> {
    let count = points.len();
    if count < 3 {
        return points.to_vec();
    }
    let wide = i128::from(tolerance);
    let tol2 = wide.checked_mul(wide).unwrap_or(i128::MAX);
    let mut keep = vec![false; count];
    if let Some(slot) = keep.first_mut() {
        *slot = true;
    }
    if let Some(slot) = keep.last_mut() {
        *slot = true;
    }
    let mut stack = vec![(0_usize, count.saturating_sub(1))];
    while let Some((head, tail)) = stack.pop() {
        let Some(worst) = worst_between(points, head, tail, tol2) else {
            continue;
        };
        if let Some(slot) = keep.get_mut(worst) {
            *slot = true;
        }
        stack.push((head, worst));
        stack.push((worst, tail));
    }
    points
        .iter()
        .enumerate()
        .filter(|&(index, _)| keep.get(index).copied().unwrap_or(false))
        .map(|(_, &point)| point)
        .collect()
}

/// The interior vertex of the chord `head..tail` with the largest
/// perpendicular excess, if any fails the keep test for `tol2`.
fn worst_between(points: &[Point], head: usize, tail: usize, tol2: i128) -> Option<usize> {
    let (a, b) = (points.get(head).copied()?, points.get(tail).copied()?);
    let span = distance_squared(a, b);
    let mut worst: Option<(i128, usize)> = None;
    for index in head.saturating_add(1)..tail {
        let point = *points.get(index)?;
        let offset = orient(a, b, point);
        let excess = offset.checked_mul(offset).unwrap_or(i128::MAX);
        if excess > tol2.saturating_mul(span) && worst.is_none_or(|(best, _)| excess > best) {
            worst = Some((excess, index));
        }
    }
    worst.map(|(_, index)| index)
}

/// Per-edge flags: whether edge `i` (vertex `i` to `i + 1`) belongs to a
/// run of `axis` (horizontal edges are the `Axis::Y` runs).
fn edge_flags(ring: &[Point], axis: Axis, max_skew_permille: u32) -> Vec<bool> {
    ring.iter()
        .enumerate()
        .map(|(index, &one)| {
            let two = ring_get(ring, index.saturating_add(1));
            near_axis(one, two, axis, max_skew_permille)
        })
        .collect()
}

/// Whether the edge leans no more than `max_skew_permille` thousandths
/// away from the axis: a horizontal edge (`Axis::Y` run) needs a small
/// change in `y` against its change in `x`, and vice versa.
fn near_axis(one: Point, two: Point, axis: Axis, max_skew_permille: u32) -> bool {
    let dx = two.0.saturating_sub(one.0).abs();
    let dy = two.1.saturating_sub(one.1).abs();
    let skew = i64::from(max_skew_permille);
    match axis {
        Axis::Y => dy.saturating_mul(1_000) <= dx.saturating_mul(skew),
        Axis::X => dx.saturating_mul(1_000) <= dy.saturating_mul(skew),
    }
}

/// The maximal cyclic runs of `true` in `flags`, each a list of edge
/// indices; an all-`true` ring is the single full run.
fn runs(flags: &[bool]) -> Vec<Vec<usize>> {
    let count = flags.len();
    let Some(start) = flags.iter().position(|&flag| !flag) else {
        return vec![(0..count).collect()];
    };
    let mut out: Vec<Vec<usize>> = Vec::new();
    let mut current: Vec<usize> = Vec::new();
    for step in 1..=count {
        let index = start
            .checked_add(step)
            .and_then(|sum| sum.checked_rem(count))
            .unwrap_or(0);
        if flags.get(index).copied().unwrap_or(false) {
            current.push(index);
            continue;
        }
        if !current.is_empty() {
            out.push(core::mem::take(&mut current));
        }
    }
    if !current.is_empty() {
        out.push(current);
    }
    out
}

/// Flattens the ring vertices touched by the edge run onto their rounded
/// mean coordinate along `axis`.
fn flatten_run(ring: &mut [Point], edges: &[usize], axis: Axis) {
    let vertices = run_vertices(ring.len(), edges);
    let values: Vec<i64> = vertices
        .iter()
        .map(|&index| axis.get(ring_get(ring, index)))
        .collect();
    let mean = rounded_mean(&values);
    for &index in &vertices {
        if let Some(point) = ring.get_mut(index) {
            axis.set(point, mean);
        }
    }
}

/// The distinct cyclic vertex indices a consecutive edge run touches:
/// every edge's tail and head.
fn run_vertices(ring_len: usize, edges: &[usize]) -> Vec<usize> {
    let mut out: Vec<usize> = Vec::with_capacity(edges.len().saturating_add(1));
    for &edge in edges {
        let tail = edge.checked_rem(ring_len).unwrap_or(0);
        let head = tail
            .checked_add(1)
            .and_then(|next| next.checked_rem(ring_len))
            .unwrap_or(0);
        push_unique(&mut out, tail);
        push_unique(&mut out, head);
    }
    out
}

/// Appends `index` when the run does not have it yet.
fn push_unique(out: &mut Vec<usize>, index: usize) {
    if !out.contains(&index) {
        out.push(index);
    }
}

#[cfg(test)]
mod tests {
    use super::{
        bbox_ring, clean_ring, edges_axis_aligned, rounded_mean, simplify, snap_right_angles,
    };
    use crate::sketch::outline::is_simple;

    /// A rectangular ring `(0, 0)..(width, height)` whose four border
    /// runs wobble one pixel off in a fixed pattern.
    fn jittered_box(width: i64, height: i64) -> Vec<(i64, i64)> {
        let mut points: Vec<(i64, i64)> = Vec::new();
        for x in 0..=width {
            points.push((x, jitter(x)));
        }
        for y in 1..=height {
            points.push((width.saturating_add(jitter(y)), y));
        }
        for x in (0..width).rev() {
            points.push((x, height.saturating_add(jitter(x.saturating_add(3)))));
        }
        for y in (1..height).rev() {
            points.push((jitter(y.saturating_add(7)), y));
        }
        points
    }

    /// The deterministic `-1`, `0` or `1` wobble of a coordinate.
    fn jitter(value: i64) -> i64 {
        match value.checked_rem(3).unwrap_or(0) {
            1 => 1,
            2 => -1,
            _ => 0,
        }
    }

    #[test]
    fn a_jittery_rectangle_simplifies_to_four_corners() {
        let noisy = jittered_box(40, 30);
        assert!(is_simple(&noisy), "the fixture itself does not cross");
        let clean = simplify(&noisy, 2);
        assert_eq!(clean.len(), 4, "tolerance 2 keeps only the corners");
        assert!(is_simple(&clean), "the result does not cross itself");
        let ideal = [(0, 0), (40, 0), (40, 30), (0, 30)];
        for (corner, want) in clean.iter().zip(ideal) {
            let (wx, wy) = want;
            assert!(
                (corner.0.saturating_sub(wx)).abs() <= 1
                    && (corner.1.saturating_sub(wy)).abs() <= 1,
                "corner {corner:?} is far from the box corner ({wx}, {wy})"
            );
        }
    }

    #[test]
    fn tolerance_zero_keeps_every_non_collinear_vertex() {
        let stepped = [(0, 0), (0, 2), (2, 2), (2, 4), (4, 4), (4, 0)];
        let kept = simplify(&stepped, 0);
        assert_eq!(kept, stepped, "a staircase corner is not noise at 0");
        assert!(simplify(&[], 3).is_empty(), "an empty ring stays empty");
    }

    #[test]
    fn a_degenerate_ring_collapses_to_its_bounding_box() {
        let line = [(3, 5), (9, 5)];
        assert_eq!(
            simplify(&line, 0),
            vec![(3, 5), (9, 5), (9, 5), (3, 5)],
            "below three vertices the bbox rectangle answers"
        );
        assert_eq!(
            simplify(&[(4, 4), (4, 4), (4, 4)], 9),
            vec![(4, 4), (4, 4), (4, 4), (4, 4)],
            "a point has a point box"
        );
        assert_eq!(
            bbox_ring(&[(2, 1), (6, 5)]),
            vec![(2, 1), (6, 1), (6, 5), (2, 5)],
            "bbox corners"
        );
    }

    #[test]
    fn clean_ring_drops_duplicates_and_collinears() {
        let ring = [(0, 0), (4, 0), (4, 0), (4, 3), (2, 3), (0, 3), (0, 0)];
        let clean = clean_ring(&ring);
        assert_eq!(
            clean,
            vec![(0, 0), (4, 0), (4, 3), (0, 3)],
            "mid-edge points go"
        );
        assert!(edges_axis_aligned(&clean), "a rectangle is axis-aligned");
        assert!(
            !edges_axis_aligned(&[(0, 0), (4, 2), (4, 4), (0, 4)]),
            "a slanted edge is not"
        );
    }

    #[test]
    fn a_skewed_rectangle_snaps_to_the_axes() {
        let skewed = [(0, 0), (100, 2), (99, 80), (2, 79)];
        let straight = snap_right_angles(&skewed, 170);
        assert_eq!(
            straight,
            vec![(1, 1), (100, 1), (100, 80), (1, 80)],
            "runs flatten onto their rounded means"
        );
        assert!(
            edges_axis_aligned(&straight),
            "every edge became horizontal or vertical, got {straight:?}"
        );
        assert!(is_simple(&straight), "the straightened ring is simple");
    }

    #[test]
    fn a_diagonal_edge_stays_diagonal() {
        let diamond = [(0, 0), (50, 0), (100, 50), (50, 100), (0, 50)];
        let snapped = snap_right_angles(&diamond, 170);
        assert_eq!(snapped, diamond, "no edge is near enough to an axis");
        assert!(is_simple(&snapped), "and the ring is still simple");
    }

    #[test]
    fn rounded_means_split_halves_away_from_zero() {
        assert_eq!(rounded_mean(&[100, 99]), 100, "99.5 goes up");
        assert_eq!(rounded_mean(&[0, 0, 1]), 0, "0.33 stays down");
        assert_eq!(rounded_mean(&[-3, -4]), -4, "-3.5 goes away from zero");
        assert_eq!(rounded_mean(&[]), 0, "an empty run means zero");
    }

    #[test]
    fn an_axis_aligned_box_is_untouched() {
        let box_ = [(0, 0), (10, 0), (10, 6), (0, 6)];
        let straight = snap_right_angles(&box_, 170);
        assert_eq!(straight, box_, "nothing to straighten");
    }
}
