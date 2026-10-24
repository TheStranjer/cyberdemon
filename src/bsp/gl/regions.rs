//! One subsector's closed region polygon and the clockwise seg loop
//! stretched over it: the real segs of the leaf ordered along the region's
//! edges, with a miniseg filling every stretch no real seg covers.
//!
//! [`region_polygons`] walks the tree once per leaf, clipping the map box
//! through the ancestor partitions ([`collect_regions`]) and cutting the
//! result by the leaf's own seg lines, then [`collapse_collinear`] drops
//! the vertices that do not turn. [`close_region`] buckets that region's
//! segs onto the edge that holds each ([`place_segs`], [`edge_of`],
//! [`holds`], [`on_line`], [`sort_along`]) and walks the polygon
//! corner to corner, closing the loop exactly ([`push_gap`], [`gap_to`],
//! [`snap_into`]) so consecutive segs share one exact endpoint, each
//! pointing at its own [`GlSeg`] pair of `16.16` points. The
//! refusal messages these steps report live beside
//! [`to_gl`](super::to_gl).

use super::{COLLINEAR_SLACK, MARGIN_UNITS, SNAP, broken, collapsed, lost_seg, no_vertices};
use crate::bsp::polygon::{Polygon, bounds_polygon, clip, region_polygon};
use crate::bsp::side::FixedLine;
use crate::bsp::{BspTree, BuildSeg, Child, Point};
use crate::error::CyberError;
use crate::fixed::hypot;
use crate::wad::map::Map;

/// Every subsector's closed region polygon, in subsector order: the map
/// box walked through the tree's partition halves, then cut by the leaf's
/// own seg lines and ordered clockwise.
pub(super) fn region_polygons(map: &Map, tree: &BspTree) -> Result<Vec<Polygon>, CyberError> {
    let base = bounds_polygon(map, MARGIN_UNITS).ok_or_else(no_vertices)?;
    let mut regions = alloc::vec![Vec::new(); tree.subsectors.len()];
    collect_regions(tree, &base, tree.root, &mut regions)?;
    for (index, region) in regions.iter_mut().enumerate() {
        let segs = tree.subsector_segs(index).ok_or_else(|| broken(index))?;
        *region = region_polygon(region, segs);
        collapse_collinear(region);
    }
    Ok(regions)
}

/// Walk from `child` down, clipping `polygon` into each half at every
/// node, and store the finished polygon of every leaf.
fn collect_regions(
    tree: &BspTree,
    polygon: &[Point],
    child: Child,
    out: &mut [Vec<Point>],
) -> Result<(), CyberError> {
    match child {
        Child::Subsector(index) => {
            let slot = out.get_mut(index).ok_or_else(|| broken(index))?;
            *slot = polygon.to_vec();
            Ok(())
        }
        Child::Node(index) => {
            let node = tree.nodes.get(index).ok_or_else(|| broken(index))?;
            let line = FixedLine::from_partition(node.partition);
            collect_regions(tree, &clip(polygon, &line, true), node.right, out)?;
            collect_regions(tree, &clip(polygon, &line, false), node.left, out)
        }
    }
}

/// Drop vertices that sit exactly on the line between their neighbours,
/// so every polygon edge marks a genuine direction change. Polygonal
/// shapes of three points or fewer are left alone.
fn collapse_collinear(polygon: &mut Polygon) {
    let mut previous = polygon.len().saturating_add(1);
    while polygon.len() > 3 && polygon.len() < previous {
        previous = polygon.len();
        *polygon = drop_collinear(polygon);
    }
}

/// `polygon` without its strictly-collinear middle vertices.
fn drop_collinear(polygon: &[Point]) -> Polygon {
    let count = polygon.len();
    let before_edge = polygon
        .iter()
        .cycle()
        .skip(count.saturating_sub(1))
        .take(count);
    let after_edge = polygon.iter().cycle().skip(1).take(count);
    polygon
        .iter()
        .zip(before_edge)
        .zip(after_edge)
        .filter_map(|((here, before), after)| {
            (!strictly_collinear(*before, *here, *after)).then_some(*here)
        })
        .collect()
}

/// Whether `here` lies exactly on the line from `before` through `next`.
fn strictly_collinear(before: Point, here: Point, next: Point) -> bool {
    let ax = i128::from(here.x).saturating_sub(i128::from(before.x));
    let ay = i128::from(here.y).saturating_sub(i128::from(before.y));
    let bx = i128::from(next.x).saturating_sub(i128::from(here.x));
    let by = i128::from(next.y).saturating_sub(i128::from(here.y));
    cross(ax, ay, bx, by) == 0_i128
}

/// The `i128` cross product of two direction vectors.
const fn cross(ax: i128, ay: i128, bx: i128, by: i128) -> i128 {
    ax.saturating_mul(by).saturating_sub(ay.saturating_mul(bx))
}

/// The `i128` dot product of two direction vectors.
const fn dot(ax: i128, ay: i128, bx: i128, by: i128) -> i128 {
    ax.saturating_mul(bx).saturating_add(ay.saturating_mul(by))
}

/// `value * 2` through `i128`, saturating at the extremes.
const fn twice(value: i128) -> i128 {
    value.saturating_add(value)
}

/// One seg of a subsector's closed loop, before its endpoints become
/// vertex words.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct GlSeg {
    /// Loop-direction start point, `16.16`.
    pub(super) start: Point,
    /// Loop-direction end point, `16.16`.
    pub(super) end: Point,
    /// Source linedef index; `None` marks a miniseg.
    pub(super) linedef: Option<usize>,
    /// Source side number; `0` on minisegs.
    pub(super) side: u8,
}

impl GlSeg {
    /// Whether this loop seg is a miniseg.
    const fn is_mini(&self) -> bool {
        self.linedef.is_none()
    }
}

/// Order the loop around the clockwise `region`, stretching minisegs
/// over the uncovered stretch of every edge and closing it exactly.
pub(super) fn close_region(region: &[Point], segs: &[BuildSeg]) -> Result<Vec<GlSeg>, CyberError> {
    if region.len() < 3 {
        return Err(collapsed());
    }
    let placed = place_segs(region, segs)?;
    let mut loop_segs: Vec<GlSeg> = Vec::new();
    let mut cursor = region.first().copied().unwrap_or_default();
    for (edge, bucket) in placed.iter().enumerate() {
        for seg in bucket {
            push_gap(&mut loop_segs, &mut cursor, seg.start);
            loop_segs.push(real_seg(seg));
            cursor = seg.end;
        }
        let corner = next_vertex(region, edge).copied().unwrap_or(cursor);
        push_gap(&mut loop_segs, &mut cursor, corner);
    }
    let first = loop_segs.first().map_or(cursor, |head| head.start);
    push_gap(&mut loop_segs, &mut cursor, first);
    Ok(loop_segs)
}

/// The seg buckets of a region's edges: one list per polygon edge.
type EdgeBuckets<'segs> = Vec<Vec<&'segs BuildSeg>>;

/// The segs of `segs` bucketed onto the edge of `region` that holds each,
/// sorted along it. A seg no edge holds is a builder inconsistency,
/// reported as `Internal`.
fn place_segs<'segs>(
    region: &[Point],
    segs: &'segs [BuildSeg],
) -> Result<EdgeBuckets<'segs>, CyberError> {
    let mut buckets: EdgeBuckets<'segs> = (0..region.len()).map(|_| Vec::new()).collect();
    for (index, seg) in segs.iter().enumerate() {
        let Some(edge) = edge_of(region, seg) else {
            return Err(lost_seg(index, seg.linedef, seg.side));
        };
        let Some(bucket) = buckets.get_mut(edge) else {
            return Err(lost_seg(index, seg.linedef, seg.side));
        };
        bucket.push(seg);
    }
    for (edge, bucket) in buckets.iter_mut().enumerate() {
        sort_along(region, edge, bucket);
    }
    Ok(buckets)
}

/// The edge of `region` whose line holds `seg` and whose span covers its
/// middle, `None` when no edge does.
fn edge_of(region: &[Point], seg: &BuildSeg) -> Option<usize> {
    (0..region.len()).find(|&edge| holds(region, edge, seg))
}

/// Whether `seg` lies on the infinite line of `region`'s edge and its
/// middle projects inside the edge's span.
fn holds(region: &[Point], edge: usize, seg: &BuildSeg) -> bool {
    let (Some(from), Some(to)) = (region.get(edge), next_vertex(region, edge)) else {
        return false;
    };
    let dx = to.x.saturating_sub(from.x);
    let dy = to.y.saturating_sub(from.y);
    let slack = i128::from(hypot(dx, dy)).saturating_mul(COLLINEAR_SLACK);
    let both = on_line(*from, *to, seg.start, slack) && on_line(*from, *to, seg.end, slack);
    if !both {
        return false;
    }
    let mid_x = i128::from(seg.start.x)
        .saturating_add(i128::from(seg.end.x))
        .saturating_sub(twice(i128::from(from.x)));
    let mid_y = i128::from(seg.start.y)
        .saturating_add(i128::from(seg.end.y))
        .saturating_sub(twice(i128::from(from.y)));
    let along = dot(mid_x, mid_y, i128::from(dx), i128::from(dy));
    along >= 0_i128
        && along
            <= twice(dot(
                i128::from(dx),
                i128::from(dy),
                i128::from(dx),
                i128::from(dy),
            ))
}

/// Whether `point` is within `1/16` unit of the line through `from`
/// along `(dx, dy)`: the ticket's cross-product test, perpendicular
/// distance `|cross| / len <= 4096` scaled to `|cross| <= len * 4096`.
fn on_line(from: Point, to: Point, point: Point, slack: i128) -> bool {
    let dx = i128::from(to.x.saturating_sub(from.x));
    let dy = i128::from(to.y.saturating_sub(from.y));
    let px = i128::from(point.x).saturating_sub(i128::from(from.x));
    let py = i128::from(point.y).saturating_sub(i128::from(from.y));
    cross(dx, dy, px, py).saturating_abs() <= slack
}

/// The vertex after `edge`, wrapping around the polygon.
fn next_vertex(region: &[Point], edge: usize) -> Option<&Point> {
    let wrapped = edge.checked_add(1)?.checked_rem(region.len())?;
    region.get(wrapped)
}

/// Sort one edge's segs by their start's projection along the edge.
fn sort_along(region: &[Point], edge: usize, bucket: &mut Vec<&BuildSeg>) {
    let (Some(from), Some(to)) = (region.get(edge), next_vertex(region, edge)) else {
        return;
    };
    let dx = i128::from(to.x.saturating_sub(from.x));
    let dy = i128::from(to.y.saturating_sub(from.y));
    bucket.sort_by_key(|seg| {
        dot(
            i128::from(seg.start.x).saturating_sub(i128::from(from.x)),
            i128::from(seg.start.y).saturating_sub(i128::from(from.y)),
            dx,
            dy,
        )
    });
}

/// A real seg as emitted: its provenance, loop direction as stored.
const fn real_seg(seg: &BuildSeg) -> GlSeg {
    GlSeg {
        start: seg.start,
        end: seg.end,
        linedef: Some(seg.linedef),
        side: seg.side,
    }
}

/// A miniseg from `start` to `end`.
const fn miniseg(start: Point, end: Point) -> GlSeg {
    GlSeg {
        start,
        end,
        linedef: None,
        side: 0,
    }
}

/// Close the stretch from `cursor` to `target`, possibly with a miniseg.
/// A stretch of at most `1/16` unit is *snapped* instead: swallowed into
/// the last emitted miniseg (or, at the head of the loop, skipped whole),
/// so consecutive segs always share one exact endpoint.
fn gap_to(loop_segs: &mut [GlSeg], cursor: &mut Point, target: Point) -> Option<GlSeg> {
    if *cursor == target {
        return None;
    }
    let from = *cursor;
    let distance = hypot(
        target.x.saturating_sub(cursor.x),
        target.y.saturating_sub(cursor.y),
    );
    let needs_filler = distance > SNAP || !snap_into(loop_segs, target);
    *cursor = target;
    needs_filler.then(|| miniseg(from, target))
}

/// Run [`gap_to`], appending the filler miniseg it asks for.
pub(super) fn push_gap(loop_segs: &mut Vec<GlSeg>, cursor: &mut Point, target: Point) {
    if let Some(filler) = gap_to(loop_segs, cursor, target) {
        loop_segs.push(filler);
    }
}

/// Absorb a hairline gap into the loop's tail: at the head of the loop it
/// is dropped whole, otherwise the last miniseg simply reaches to
/// `target`. `false` when neither applies and a fresh miniseg is needed.
const fn snap_into(loop_segs: &mut [GlSeg], target: Point) -> bool {
    let Some(tail) = loop_segs.last_mut() else {
        return true;
    };
    if !tail.is_mini() {
        return false;
    }
    tail.end = target;
    true
}
