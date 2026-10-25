//! The shape reads behind a described sector: the exact integer geometry
//! of [`super::SectorFacts`] — the even-odd shoelace area of a directed
//! boundary loop, the label point the renderer's centre scan answers with,
//! and the widening, halving and saturating helpers both walk through.
//!
//! Everything here is plain integer maths over [`Edge`]s: no map, no
//! sectors and no game definitions, so the boundary in `super::roles` and
//! the sector rows in `super::reader` fold into these shapes.

use alloc::vec::Vec;

use crate::fixed::{midpoint, narrow_i64};

use super::{Edge, Point};

/// Half of the shoelace sum: the doubled area divides by it.
const AREA_HALF_OF: u128 = 2_u128;

/// The `u64` floor area of the directed `edges` loop set: the shoelace
/// sum of every edge's vertex cross product in exact `i128`, halved on its
/// absolute value. One consistent direction makes a hole's loop subtract
/// itself, so a sector with an island reports the even-odd sum.
pub(super) fn shoelace_area(edges: &[Edge]) -> u64 {
    let mut sum = 0_i128;
    for edge in edges {
        let left = i128::from(edge.0.0).saturating_mul(i128::from(edge.1.1));
        let right = i128::from(edge.1.0).saturating_mul(i128::from(edge.0.1));
        sum = sum.saturating_add(left.saturating_sub(right));
    }
    let half = sum.unsigned_abs().checked_div(AREA_HALF_OF);
    u64::try_from(half.unwrap_or(u128::MAX)).unwrap_or(u64::MAX)
}

/// The world-space port of the renderer's sector label point: the middle
/// row of the boundary's box is scanned with the even-odd rule, the
/// longest inside interval wins and its midpoint is the centre; a polygon
/// the row misses falls back to its first boundary vertex. No edges, no
/// centre.
pub(super) fn label_point(edges: &[Edge]) -> Option<[i64; 2]> {
    let (min_y, max_y) = edge_rows(edges)?;
    let row = midpoint(min_y, max_y);
    let mut crossings: Vec<i64> = edges
        .iter()
        .filter(|edge| crosses_row(edge, row))
        .map(|edge| intersect_x(edge.0, edge.1, row))
        .collect();
    crossings.sort_unstable();
    longest_interval(&crossings)
        .map(|(lo, hi)| [midpoint(lo, hi), row])
        .or_else(|| edges.first().map(|edge| <[i64; 2]>::from(edge.0)))
}

/// Whether `edge` straddles row `row` under the half-open even-odd rule.
const fn crosses_row(edge: &Edge, row: i64) -> bool {
    (edge.0.1 > row) != (edge.1.1 > row)
}

/// The `(lowest, highest)` endpoint rows over `edges`, `None` when empty.
fn edge_rows(edges: &[Edge]) -> Option<(i64, i64)> {
    let rows = edges.iter().flat_map(|edge| [edge.0.1, edge.1.1]);
    let min = rows.clone().min()?;
    Some((min, rows.max()?))
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

/// The column where the `start` to `end` line crosses row `y`,
/// floor-rounded in exact `i128`; a horizontal edge answers `start`'s
/// column (the even-odd filter never asks about one).
fn intersect_x(start: Point, end: Point, y: i64) -> i64 {
    let rise = i128::from(y).saturating_sub(i128::from(start.1));
    let span = i128::from(end.1).saturating_sub(i128::from(start.1));
    scale_down(start.0, end.0.saturating_sub(start.0), rise, span)
}

/// `base + delta * num / den` in exact `i128`, floor-rounded and
/// saturating back to `i64`; a zero `den` answers `base`.
fn scale_down(base: i64, delta: i64, num: i128, den: i128) -> i64 {
    if den == 0_i128 {
        return base;
    }
    let (n, d) = if den < 0_i128 {
        (num.saturating_neg(), den.saturating_neg())
    } else {
        (num, den)
    };
    let drawn = i128::from(base)
        .saturating_mul(d)
        .saturating_add(i128::from(delta).saturating_mul(n))
        .div_euclid(d);
    narrow_i64(drawn)
}
