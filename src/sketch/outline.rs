//! Crack-following outlines of labelled regions, in pixel-corner space.
//!
//! [`trace_outline`] walks the outer boundary of one region label: the
//! vertices are pixel **corners**, so a single pixel at `(x, y)` has the
//! outline `(x, y) - (x + 1, y) - (x + 1, y + 1) - (x, y + 1)`. The walk
//! starts at the top-left-most pixel's top-left corner heading east, keeps
//! the region on its right hand and stops when it returns to that start
//! state; a vertex is emitted only where the walk turns. The ring is
//! therefore clockwise in image coordinates (`y` down). A region split
//! into several 4-connected pieces has only its largest traced, with the
//! piece count reported for the caller's warnings.
//!
//! [`is_simple`] is the validation predicate for the polygons the rest of
//! the vectoriser produces: it asks `geom::ring`'s simplicity walk, so the
//! exact non-adjacent-edge meeting tests (orientation, bounding box, the
//! shared crossing walk) are the arrangement's, read with no integer
//! copies of its own.

use crate::sketch::components::{Component, Connectivity, Labels, label_components};
use crate::sketch::mask::Mask;

/// A polygon vertex in pixel-corner coordinates.
type Point = crate::geom::Pt;

/// The traced outer boundary of one region label.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Outline {
    /// Ring vertices, clockwise (`y` down), first at the start corner.
    pub points: Vec<Point>,
    /// Non-region 4-connected islands inside the traced piece's bounding
    /// box that do not touch the box edge (an approximation, reported only).
    pub holes: u32,
    /// 4-connected pieces the labelled region has.
    pub pieces: u32,
}

/// A unit step along pixel edges.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Dir {
    /// Corner `x` grows.
    East,
    /// Corner `y` grows.
    South,
    /// Corner `x` shrinks.
    West,
    /// Corner `y` shrinks.
    North,
}

impl Dir {
    /// The corner offset of one step.
    const fn delta(self) -> Point {
        match self {
            Self::East => (1, 0),
            Self::South => (0, 1),
            Self::West => (-1, 0),
            Self::North => (0, -1),
        }
    }

    /// This direction turned 90 degrees clockwise.
    const fn right(self) -> Self {
        match self {
            Self::East => Self::South,
            Self::South => Self::West,
            Self::West => Self::North,
            Self::North => Self::East,
        }
    }

    /// This direction turned 90 degrees counter-clockwise.
    const fn left(self) -> Self {
        match self {
            Self::East => Self::North,
            Self::North => Self::West,
            Self::West => Self::South,
            Self::South => Self::East,
        }
    }

    /// The pixel on the right of the edge leaving `corner` this way.
    const fn right_pixel(self, corner: Point) -> Point {
        let (x, y) = corner;
        match self {
            Self::East => (x, y),
            Self::South => (x.saturating_sub(1), y),
            Self::West => (x.saturating_sub(1), y.saturating_sub(1)),
            Self::North => (x, y.saturating_sub(1)),
        }
    }

    /// The pixel on the left of the edge leaving `corner` this way.
    const fn left_pixel(self, corner: Point) -> Point {
        let (x, y) = corner;
        match self {
            Self::East => (x, y.saturating_sub(1)),
            Self::South => (x, y),
            Self::West => (x.saturating_sub(1), y),
            Self::North => (x.saturating_sub(1), y.saturating_sub(1)),
        }
    }
}

/// Traces the outer boundary of region `id` in `labels`: `None` when the
/// label has no pixels. See the module docs for the walk's conventions.
#[must_use]
#[inline]
pub fn trace_outline(labels: &Labels, id: u32) -> Option<Outline> {
    let (pieces, components) = region_pieces(labels, id);
    let largest = components
        .iter()
        .max_by_key(|component| (component.area, core::cmp::Reverse(component.id)))?;
    let start = first_piece_pixel(&pieces, largest.id)?;
    let points = follow_cracks(labels, id, start);
    let holes = count_holes(labels, id, largest.bbox);
    let Ok(pieces_count) = u32::try_from(components.len()) else {
        return None;
    };
    Some(Outline {
        points,
        holes,
        pieces: pieces_count,
    })
}

/// Whether the closed ring `points` is simple: no two non-adjacent edges
/// share a point. Quadratic in the vertex count, integer only.
#[must_use]
#[inline]
pub fn is_simple(points: &[Point]) -> bool {
    crate::geom::ring::is_simple(points)
}

/// The 4-connected pieces of label `id`: the piece-labelled raster (pixels
/// outside the region are `0`) and the component measurements.
fn region_pieces(labels: &Labels, id: u32) -> (Labels, Vec<Component>) {
    let mask = region_mask(labels, id);
    let (pieces, components) = label_components(&mask, true, Connectivity::Four);
    (pieces, components)
}

/// A mask with the pixels labelled `id` set.
fn region_mask(labels: &Labels, id: u32) -> Mask {
    let bits = labels.ids.iter().map(|&pixel| pixel == id).collect();
    Mask {
        width: labels.width,
        height: labels.height,
        bits,
    }
}

/// The scan-order first pixel belonging to piece `piece`, as `(x, y)`.
fn first_piece_pixel(pieces: &Labels, piece: u32) -> Option<(u32, u32)> {
    let width = usize::try_from(pieces.width).ok().filter(|&w| w > 0)?;
    let index = pieces.ids.iter().position(|&pixel| pixel == piece)?;
    let x = u32::try_from(index.checked_rem(width)?).ok()?;
    let y = u32::try_from(index.checked_div(width)?).ok()?;
    Some((x, y))
}

/// Walks the region boundary keeping the region on the right, starting at
/// the top-left corner of `(x, y)` heading east; vertices are emitted only
/// where the walk turns.
fn follow_cracks(labels: &Labels, id: u32, start: (u32, u32)) -> Vec<Point> {
    let (sx, sy) = (i64::from(start.0), i64::from(start.1));
    let origin = (sx, sy);
    let mut points = vec![origin];
    let mut corner = origin;
    let mut dir = Dir::East;
    for _ in 0..crack_limit(labels) {
        let (dx, dy) = dir.delta();
        let next = (
            corner.0.checked_add(dx).unwrap_or(corner.0),
            corner.1.checked_add(dy).unwrap_or(corner.1),
        );
        let arrived = dir;
        dir = choose_dir(labels, id, next, arrived);
        corner = next;
        if corner == origin && dir == Dir::East {
            break;
        }
        if dir != arrived {
            points.push(corner);
        }
    }
    points
}

/// A generous bound on the crack walk: every pixel edge of the raster at
/// most once. The walk closes on itself long before this.
fn crack_limit(labels: &Labels) -> usize {
    labels.ids.len().saturating_mul(4).saturating_add(4)
}

/// The outgoing crack direction at `corner`: the first of right-turn,
/// straight, left-turn that has region on the right and paper on the left.
fn choose_dir(labels: &Labels, id: u32, corner: Point, incoming: Dir) -> Dir {
    let candidates = [incoming.right(), incoming, incoming.left()];
    for candidate in candidates {
        if is_boundary_edge(labels, id, corner, candidate) {
            return candidate;
        }
    }
    incoming
}

/// Whether the edge leaving `corner` this way is a crack of the region.
fn is_boundary_edge(labels: &Labels, id: u32, corner: Point, dir: Dir) -> bool {
    in_region(labels, id, dir.right_pixel(corner)) && !in_region(labels, id, dir.left_pixel(corner))
}

/// Whether pixel `(x, y)` in corner coordinates carries label `id`.
fn in_region(labels: &Labels, id: u32, point: Point) -> bool {
    let (Ok(x), Ok(y)) = (u32::try_from(point.0), u32::try_from(point.1)) else {
        return false;
    };
    labels.get(x, y) == Some(id)
}

/// The number of 4-connected non-region islands inside the inclusive
/// bounding box that do not touch the box edge.
fn count_holes(labels: &Labels, id: u32, bbox: (u32, u32, u32, u32)) -> u32 {
    let (x0, y0, x1, y1) = bbox;
    let width = x1.saturating_add(1).saturating_sub(x0);
    let height = y1.saturating_add(1).saturating_sub(y0);
    let mut mask = Mask::blank(width, height);
    let points = (y0..=y1).flat_map(|y| (x0..=x1).map(move |x| (x, y)));
    for (x, y) in points {
        let free = labels.get(x, y).is_none_or(|pixel| pixel != id);
        mask.set(x.saturating_sub(x0), y.saturating_sub(y0), free);
    }
    let (_, components) = label_components(&mask, true, Connectivity::Four);
    let islands = components.iter().filter(|c| !c.touches_border).count();
    u32::try_from(islands).unwrap_or(u32::MAX)
}

#[cfg(test)]
mod tests {
    use super::{is_simple, trace_outline};
    use crate::sketch::components::Labels;

    /// A 6x6 raster carrying the given `(x, y, id)` pixels.
    fn labelled(points: &[(u32, u32, u32)]) -> Labels {
        let mut labels = Labels::blank(6, 6);
        for &(x, y, id) in points {
            assert!(labels.set(x, y, id), "fixture in bounds");
        }
        labels
    }

    /// The rectangle `box_` inclusive as label `id`.
    fn block((x0, y0, x1, y1): (u32, u32, u32, u32), id: u32) -> Labels {
        let points: Vec<(u32, u32, u32)> = (y0..=y1)
            .flat_map(|y| (x0..=x1).map(move |x| (x, y, id)))
            .collect();
        labelled(&points)
    }

    /// A 3x3 ring of label 1 with the centre free.
    fn ring() -> Labels {
        let mut points: Vec<(u32, u32, u32)> = (1..=3_u32)
            .flat_map(|y| (1..=3_u32).map(move |x| (x, y, 1)))
            .collect();
        points.retain(|&(x, y, _)| x != 2 || y != 2);
        labelled(&points)
    }

    #[test]
    fn a_single_pixel_traces_its_four_corners() {
        let outline = trace_outline(&block((2, 3, 2, 3), 1), 1).expect("label exists");
        assert_eq!(
            outline.points,
            vec![(2, 3), (3, 3), (3, 4), (2, 4)],
            "clockwise from the top-left corner"
        );
        assert_eq!(outline.pieces, 1, "one piece");
        assert_eq!(outline.holes, 0, "a pixel has no room for a hole");
    }

    #[test]
    fn a_three_by_two_block_traces_a_rectangle() {
        let outline = trace_outline(&block((1, 2, 3, 3), 1), 1).expect("label exists");
        assert_eq!(
            outline.points,
            vec![(1, 2), (4, 2), (4, 4), (1, 4)],
            "corners of the 3x2 block"
        );
        assert!(is_simple(&outline.points), "a rectangle is simple");
    }

    #[test]
    fn an_l_shape_traces_six_clockwise_vertices() {
        let labels = labelled(&[(1, 1, 1), (1, 2, 1), (2, 2, 1)]);
        let outline = trace_outline(&labels, 1).expect("label exists");
        assert_eq!(
            outline.points,
            vec![(1, 1), (2, 1), (2, 2), (3, 2), (3, 3), (1, 3)],
            "the inner corner turns the walk"
        );
        assert_eq!(outline.points.len(), 6, "six vertices");
        assert!(is_simple(&outline.points), "an L outline is simple");
        let twice_area = crate::geom::ring::area2(&outline.points);
        assert!(twice_area > 0, "positive shoelace is clockwise with y down");
    }

    #[test]
    fn a_ring_reports_one_hole_and_keeps_its_outer_edge() {
        let outline = trace_outline(&ring(), 1).expect("label exists");
        assert_eq!(
            outline.points,
            vec![(1, 1), (4, 1), (4, 4), (1, 4)],
            "only the outer boundary is traced"
        );
        assert_eq!(outline.holes, 1, "the free centre is one island");
        assert_eq!(outline.pieces, 1, "the ring is connected");
    }

    #[test]
    fn a_split_region_traces_its_largest_piece() {
        let mut labels = block((0, 0, 1, 1), 1);
        assert!(labels.set(5, 5, 1), "the stray single pixel");
        let outline = trace_outline(&labels, 1).expect("label exists");
        assert_eq!(outline.pieces, 2, "two disconnected pieces");
        assert_eq!(
            outline.points,
            vec![(0, 0), (2, 0), (2, 2), (0, 2)],
            "the 2x2 piece is the larger one"
        );
    }

    #[test]
    fn a_missing_label_traces_nothing() {
        assert!(
            trace_outline(&block((0, 0, 2, 2), 1), 2).is_none(),
            "no pixels with label 2"
        );
    }

    #[test]
    fn bowties_and_touching_rings_are_not_simple() {
        assert!(
            is_simple(&[(0, 0), (3, 0), (3, 3), (0, 3)]),
            "a square is simple"
        );
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
    }
}
