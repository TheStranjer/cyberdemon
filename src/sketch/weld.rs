//! Welding neighbouring shapes so their shared walls coincide exactly.
//!
//! [`weld`] looks at every shape of one vectorised sketch together. The
//! `x` of every vertical edge and the `y` of every horizontal edge are
//! collected across all shapes, sorted, and greedily clustered: values
//! whose neighbours differ by at most `tolerance` join one cluster, and
//! every member is rewritten to the cluster's rounded mean. Rooms that
//! drew the same wall at `x = 100` and `x = 102` end up on one `x`. On
//! top of that every vertex is snapped onto the nearest earlier shape's
//! vertex within `tolerance` (the lower shape index wins), so diagonal
//! walls meet too; finally duplicate and collinear vertices are dropped.

use alloc::collections::BTreeMap;

use super::simplify::{clean_ring, dist2, rounded_mean};

/// A polygon vertex in pixel-corner coordinates.
type Point = (i64, i64);

/// Make the walls shared by neighbouring `shapes` coincide, with a
/// clustering tolerance in pixels (the caller disables welding itself by
/// not calling). `shapes` are rewritten in place.
#[inline]
pub fn weld(shapes: &mut [Vec<Point>], tolerance: u32) {
    let mut verticals: Vec<i64> = Vec::new();
    let mut horizontals: Vec<i64> = Vec::new();
    for shape in shapes.iter() {
        collect_axes(shape, &mut verticals, &mut horizontals);
    }
    let xmap = cluster_map(&verticals, tolerance);
    let ymap = cluster_map(&horizontals, tolerance);
    let maps = AxisMaps { x: &xmap, y: &ymap };
    for shape in shapes.iter_mut() {
        rewrite_axes(shape, &maps);
    }
    snap_vertices(shapes, tolerance);
    for shape in shapes.iter_mut() {
        let clean = clean_ring(shape);
        *shape = clean;
    }
}

/// The `x` of every vertical and the `y` of every horizontal edge of one
/// closed ring, endpoints equal on that axis included once per edge.
fn collect_axes(shape: &[Point], verticals: &mut Vec<i64>, horizontals: &mut Vec<i64>) {
    let Some(first) = shape.first().copied() else {
        return;
    };
    let rotated = shape.iter().skip(1).chain(core::iter::once(&first));
    for (one, two) in shape.iter().zip(rotated) {
        if one.0 == two.0 {
            verticals.push(one.0);
            continue;
        }
        if one.1 == two.1 {
            horizontals.push(one.1);
        }
    }
}

/// Each distinct value mapped to the rounded mean of its greedy cluster;
/// consecutive sorted values join when they differ by at most
/// `tolerance`.
fn cluster_map(values: &[i64], tolerance: u32) -> BTreeMap<i64, i64> {
    let mut sorted = values.to_vec();
    sorted.sort_unstable();
    let gap = i64::from(tolerance);
    let mut out: BTreeMap<i64, i64> = BTreeMap::new();
    let mut cluster: Vec<i64> = Vec::new();
    for value in sorted {
        let joins = cluster
            .last()
            .is_some_and(|&last| value.saturating_sub(last) <= gap);
        if !joins {
            flush_cluster(&mut cluster, &mut out);
        }
        cluster.push(value);
    }
    flush_cluster(&mut cluster, &mut out);
    out
}

/// Files every value of the finished cluster under its rounded mean.
fn flush_cluster(cluster: &mut Vec<i64>, out: &mut BTreeMap<i64, i64>) {
    if cluster.is_empty() {
        return;
    }
    let mean = rounded_mean(cluster);
    for &value in cluster.iter() {
        out.insert(value, mean);
    }
    cluster.clear();
}

/// The two cluster maps, kept together so edge rewriting stays in the
/// argument budget.
struct AxisMaps<'weld> {
    /// Vertical-edge `x` values mapped to their cluster mean.
    x: &'weld BTreeMap<i64, i64>,
    /// Horizontal-edge `y` values mapped to their cluster mean.
    y: &'weld BTreeMap<i64, i64>,
}

/// One edge of a ring, as vertex indices.
#[derive(Clone, Copy, Debug)]
struct Edge {
    /// Index of the edge's first vertex.
    from: usize,
    /// Index of the edge's second vertex.
    to: usize,
}

impl AxisMaps<'_> {
    /// Move one edge onto its cluster mean; diagonal edges keep both
    /// endpoints.
    fn rewrite_edge(&self, shape: &mut [Point], edge: Edge) {
        let Some((one, two)) = pair(shape, edge) else {
            return;
        };
        if one.0 == two.0 {
            return Self::shift(shape, edge, self.x.get(&one.0), true);
        }
        if one.1 == two.1 {
            Self::shift(shape, edge, self.y.get(&one.1), false);
        }
    }

    /// Move both endpoints onto a looked-up cluster mean; `vertical`
    /// picks the axis to rewrite.
    fn shift(shape: &mut [Point], edge: Edge, value: Option<&i64>, vertical: bool) {
        let Some(&moved) = value else {
            return;
        };
        if vertical {
            set_x(shape, edge, moved);
            return;
        }
        set_y(shape, edge, moved);
    }
}

/// Rewrite the owned coordinate of every axis-aligned edge through its
/// cluster map; diagonal edges keep both endpoints.
fn rewrite_axes(shape: &mut [Point], maps: &AxisMaps<'_>) {
    let count = shape.len();
    if count < 2 {
        return;
    }
    for index in 0..count {
        let next = index
            .checked_add(1)
            .and_then(|followed| followed.checked_rem(count))
            .unwrap_or(0);
        maps.rewrite_edge(
            shape,
            Edge {
                from: index,
                to: next,
            },
        );
    }
}

/// The two vertices of `edge`, if the ring has them.
fn pair(shape: &[Point], edge: Edge) -> Option<(Point, Point)> {
    Some((*shape.get(edge.from)?, *shape.get(edge.to)?))
}

/// Move both endpoints of one vertical edge to `value`.
fn set_x(shape: &mut [Point], edge: Edge, value: i64) {
    for slot in [edge.from, edge.to] {
        if let Some(point) = shape.get_mut(slot) {
            point.0 = value;
        }
    }
}

/// Move both endpoints of one horizontal edge to `value`.
fn set_y(shape: &mut [Point], edge: Edge, value: i64) {
    for slot in [edge.from, edge.to] {
        if let Some(point) = shape.get_mut(slot) {
            point.1 = value;
        }
    }
}

/// Snap every vertex of a later shape onto the first earlier vertex
/// within `tolerance` (Euclidean, squared comparison), so wall corners
/// that neither axis clustering joined still meet.
fn snap_vertices(shapes: &mut [Vec<Point>], tolerance: u32) {
    let wide = i128::from(tolerance);
    let limit2 = wide.checked_mul(wide).unwrap_or(i128::MAX);
    for target in 1..shapes.len() {
        let anchors: Vec<Point> = shapes
            .iter()
            .take(target)
            .flat_map(|shape| shape.iter().copied())
            .collect();
        if let Some(shape) = shapes.get_mut(target) {
            snap_shape(shape, &anchors, limit2);
        }
    }
}

/// Snaps one shape's vertices onto the given earlier anchors.
fn snap_shape(shape: &mut [Point], anchors: &[Point], limit2: i128) {
    for vertex in shape.iter_mut() {
        let found = anchors
            .iter()
            .copied()
            .find(|&anchor| within(anchor, *vertex, limit2));
        if let Some(anchor) = found {
            *vertex = anchor;
        }
    }
}

/// Whether the vertex lies within the squared tolerance of the anchor.
fn within(anchor: Point, vertex: Point, limit2: i128) -> bool {
    dist2(anchor, vertex) <= limit2
}

#[cfg(test)]
mod tests {
    use super::weld;

    #[test]
    fn nearby_shared_walls_become_one_wall() {
        let mut shapes = vec![
            vec![(0, 0), (100, 0), (100, 60), (0, 60)],
            vec![(102, 0), (200, 0), (200, 60), (102, 60)],
        ];
        weld(&mut shapes, 2);
        let inner_one = shapes[0].iter().map(|p| p.0).max().unwrap_or(0);
        let inner_two = shapes[1].iter().map(|p| p.0).min().unwrap_or(0);
        assert_eq!(inner_one, inner_two, "the shared wall got one x");
        assert_eq!(inner_one, 101, "the cluster mean is 101");
        assert_eq!(shapes[0][0], (0, 0), "the outer walls did not move");
        assert_eq!(shapes[1][2].0, 200, "and neither did the far wall");
    }

    #[test]
    fn walls_farther_apart_than_the_tolerance_stay_separate() {
        let mut shapes = vec![
            vec![(0, 0), (100, 0), (100, 60), (0, 60)],
            vec![(105, 0), (200, 0), (200, 60), (105, 60)],
        ];
        weld(&mut shapes, 2);
        let x_one = shapes[0].iter().map(|p| p.0).max().unwrap_or(0);
        let x_two = shapes[1].iter().map(|p| p.0).min().unwrap_or(0);
        assert_eq!(
            (x_one, x_two),
            (100, 105),
            "greedy clusters kept them apart"
        );
    }

    #[test]
    fn a_diagonal_vertex_snaps_onto_the_earlier_shape() {
        let mut shapes = vec![
            vec![(0, 0), (50, 50), (0, 100)],
            vec![(52, 49), (80, 0), (80, 100)],
        ];
        weld(&mut shapes, 3);
        assert_eq!(shapes[1][0], (50, 50), "lower shape index wins");
        assert_eq!(shapes[0][1], (50, 50), "and the earlier shape is not moved");
    }

    #[test]
    fn welding_drops_the_mid_edge_vertex_it_creates() {
        let mut shapes = vec![
            vec![(0, 0), (50, 0), (100, 0), (100, 40), (0, 40)],
            vec![(102, 0), (150, 0), (150, 40), (102, 40)],
        ];
        weld(&mut shapes, 2);
        assert_eq!(shapes[0].len(), 4, "the stray (50, 0) vertex went");
        let x_one = shapes[0].iter().map(|p| p.0).max().unwrap_or(0);
        let x_two = shapes[1].iter().map(|p| p.0).min().unwrap_or(0);
        assert_eq!(x_one, x_two, "the shared wall got one x");
    }

    #[test]
    fn empty_and_pointless_shapes_survive_the_weld() {
        let mut shapes: Vec<Vec<(i64, i64)>> = vec![Vec::new(), vec![(3, 3)]];
        weld(&mut shapes, 4);
        assert_eq!(shapes.len(), 2, "nothing was dropped");
        assert!(shapes[0].is_empty(), "the empty ring stayed empty");
    }
}
