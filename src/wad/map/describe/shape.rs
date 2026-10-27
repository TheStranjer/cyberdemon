//! The shape reads behind a described sector: the exact integer geometry
//! of [`super::SectorFacts`] — the even-odd shoelace area of a directed
//! boundary loop and the label point of the shared
//! [`label_point`](crate::geom::ring::label_point) scan — plus the
//! halving and saturating wrap the report needs.
//!
//! Everything here is a fold of [`Edge`]s through `crate::geom`'s one
//! polygon implementation: no map, no sectors and no game definitions,
//! and — identical to the renderer's top-down labels by construction —
//! no second copy of any of the row-scan arithmetic.

use crate::geom::{Pt, ring};

use super::Edge;

/// Half of the shoelace sum: the doubled area divides by it.
const AREA_HALF_OF: u128 = 2_u128;

/// The `u64` floor area of the directed `edges` loop set: the shoelace
/// sum of every edge's vertex cross product in exact `i128`, halved on its
/// absolute value. One consistent direction makes a hole's loop subtract
/// itself, so a sector with an island reports the even-odd sum.
pub(super) fn shoelace_area(edges: &[Edge]) -> u64 {
    let sum = ring::edge_area2(edges);
    let half = sum.unsigned_abs().checked_div(AREA_HALF_OF);
    u64::try_from(half.unwrap_or(u128::MAX)).unwrap_or(u64::MAX)
}

/// The world-space label point of the boundary: [`crate::geom`]'s one
/// even-odd interior anchor scan, so the described centre and the
/// renderer's sector label land on the very same map unit. No edges, no
/// centre.
pub(super) fn label_point(edges: &[Edge]) -> Option<Pt> {
    ring::label_point(edges)
}
