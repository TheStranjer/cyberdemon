//! Which screen columns an edge can appear in: the [`Edge`] endpoints
//! projected onto the column fan, clipped at the camera plane, and the
//! per-column [`Bucketed`] lists the caster tests instead of every edge
//! in every column.
//!
//! A column ray starts at the camera and moves forward, so every point
//! it can hit lies strictly ahead of the camera plane (the line through
//! the camera perpendicular to its facing): an edge with both endpoints
//! on or behind that plane is dropped outright. An edge fully in front
//! subtends the contiguous column span between its endpoints' projected
//! columns, widened by one at each end to absorb the flooring. An edge
//! crossing the plane is clipped at the near-plane crossing: its visible
//! piece subtends the span from its front endpoint's column to the
//! screen edge on the crossing's side. Only the degenerate crossing
//! through the camera point itself keeps every column, in [`Bucketed`]'s
//! `always` list.

use alloc::vec;
use alloc::vec::Vec;
use core::cmp::Ordering;

use crate::render::view::ONE_16;
use crate::render::view::projection::Projection;
use crate::render::view::walk::Edge;

/// The edge indices bucketed by the columns each can appear in.
pub(super) struct Bucketed {
    /// One ascending list of castable-edge indices per screen column.
    pub(super) columns: Vec<Vec<usize>>,
    /// Edge indices every column must test: the whole-width spans.
    pub(super) always: Vec<usize>,
}

/// Bucket `edges` by [`edge_span`]: an edge whose span covers the whole
/// width joins `always` and costs one entry, the rest are pushed onto
/// every column of their span in edge order.
pub(super) fn bucket_edges(edges: &[Edge], proj: &Projection, cam: (i64, i64)) -> Bucketed {
    let width = usize::try_from(proj.width()).unwrap_or(0_usize);
    let last = proj.width().saturating_sub(1_i64);
    let mut bucketed = Bucketed {
        columns: vec![Vec::new(); width],
        always: Vec::new(),
    };
    for (index, edge) in edges.iter().enumerate() {
        let Some((lo, hi)) = edge_span(edge, proj, cam) else {
            continue;
        };
        if lo == 0_i64 && hi == last {
            bucketed.always.push(index);
            continue;
        }
        let from = usize::try_from(lo).unwrap_or(0_usize);
        let to = usize::try_from(hi.saturating_add(1)).unwrap_or(width);
        for bucket in bucketed
            .columns
            .iter_mut()
            .skip(from)
            .take(to.saturating_sub(from))
        {
            bucket.push(index);
        }
    }
    bucketed
}

/// The inclusive span of screen columns whose rays can reach `edge`,
/// `None` when no column can: an edge wholly on or behind the camera
/// plane, or wholly outside the field of view, spans nothing.
pub(super) fn edge_span(edge: &Edge, proj: &Projection, cam: (i64, i64)) -> Option<(i64, i64)> {
    let one = camera_axes(edge.x0, edge.y0, proj, cam);
    let two = camera_axes(
        edge.x0.saturating_add(edge.ex),
        edge.y0.saturating_add(edge.ey),
        proj,
        cam,
    );
    if one.0 > 0_i128 && two.0 > 0_i128 {
        return front_span(one, two, proj);
    }
    if one.0 <= 0_i128 && two.0 <= 0_i128 {
        return None;
    }
    let (front, back) = if one.0 > 0_i128 {
        (one, two)
    } else {
        (two, one)
    };
    clip_span(front, back, proj)
}

/// The camera-plane axes of the point `(x, y)` relative to `cam`: its
/// forward `depth` and screen `side`, both raw 16.16 map units against
/// the camera facing's cos/sin (the `Projection`'s right vector is
/// `(sin, -cos)`, the column directions are built from).
fn camera_axes(x: i64, y: i64, proj: &Projection, cam: (i64, i64)) -> (i128, i128) {
    let dx = i128::from(x).saturating_sub(i128::from(cam.0));
    let dy = i128::from(y).saturating_sub(i128::from(cam.1));
    let cos = i128::from(proj.cos());
    let sin = i128::from(proj.sin());
    (
        dx.saturating_mul(cos)
            .saturating_add(dy.saturating_mul(sin)),
        dx.saturating_mul(sin)
            .saturating_sub(dy.saturating_mul(cos)),
    )
}

/// The span of an edge with both endpoints in front of the camera
/// plane: the two projected columns, widened by one at each end for
/// the flooring, clamped to the frame.
fn front_span(one: (i128, i128), two: (i128, i128), proj: &Projection) -> Option<(i64, i64)> {
    let first = projected_column(one.1, one.0, proj);
    let second = projected_column(two.1, two.0, proj);
    clamp_span(
        first.min(second).saturating_sub(1_i128),
        first.max(second).saturating_add(1_i128),
        proj,
    )
}

/// The span of an edge crossing the camera plane, clipped at the
/// crossing: from the `front` endpoint's projected column (widened by
/// one) to the screen edge on the side the near-plane crossing falls
/// on. `back` lies on or behind the plane; a crossing exactly through
/// the camera point keeps every column.
fn clip_span(front: (i128, i128), back: (i128, i128), proj: &Projection) -> Option<(i64, i64)> {
    let column = projected_column(front.1, front.0, proj);
    let crossing = front
        .0
        .saturating_mul(back.1)
        .saturating_sub(back.0.saturating_mul(front.1));
    match crossing.cmp(&0_i128) {
        Ordering::Greater => clamp_span(column.saturating_sub(1_i128), i128::MAX, proj),
        Ordering::Less => clamp_span(0_i128, column.saturating_add(1_i128), proj),
        Ordering::Equal => clamp_span(0_i128, i128::MAX, proj),
    }
}

/// The floor of the screen column whose ray passes through the
/// camera-plane point `(side, depth)`: `u = side * focal2 /
/// (depth * ONE_16)` pixels and `c = (u + width - 1) / 2`, with
/// `depth` positive.
fn projected_column(side: i128, depth: i128, proj: &Projection) -> i128 {
    let u = side
        .saturating_mul(proj.focal2())
        .div_euclid(depth.saturating_mul(ONE_16));
    u.saturating_add(i128::from(proj.width()))
        .saturating_sub(1_i128)
        .div_euclid(2_i128)
}

/// Clamp the inclusive `lo..=hi` column span to the frame, `None` when
/// it lies wholly outside it.
fn clamp_span(lo: i128, hi: i128, proj: &Projection) -> Option<(i64, i64)> {
    let last = proj.width().saturating_sub(1_i64);
    if hi < 0_i128 || lo > i128::from(last) {
        return None;
    }
    let from = i64::try_from(lo.max(0_i128)).unwrap_or(0_i64);
    let to = i64::try_from(hi.min(i128::from(last))).unwrap_or(0_i64);
    (from <= to).then_some((from, to))
}

#[cfg(test)]
mod tests;
