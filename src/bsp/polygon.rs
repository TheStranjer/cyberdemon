//! Convex polygon clipping: the regions a `GL` subsector must fill.
//!
//! A region polygon starts as the map's bounding box enlarged by a margin
//! and is then cut by half-planes: one per ancestor partition line on the
//! walk down the [`BspTree`](super::BspTree), and one per seg line of the
//! leaf's own segs (every seg bounds its subsector's region on its right).
//! [`clip`] is `Sutherland`–`Hodgman`: points on the line stay in both
//! halves, so the two children's polygons share their boundary exactly and
//! the miniseg loops the [`gl`](super::gl) writer grows from them close.
//! Crossings are computed in `i128` and rounded to the nearest `16.16`
//! point, the same convention [`split`](super::split) uses for seg cuts.
//! Nothing here knows about archives, sessions or `MCP`.

use crate::bsp::side::{FixedLine, side_value};
use crate::bsp::split::scale_round;
use crate::bsp::{BuildSeg, Point};
use crate::fixed::FRACBITS;
use crate::wad::map::Map;

/// A convex region boundary in `16.16` [`Point`]s, consecutive vertices
/// joined by edges, the last closing back to the first.
pub type Polygon = Vec<Point>;

/// Whether the side value `c` is kept when clipping to the front (right)
/// half-plane: strictly right (`c > 0`) or exactly on the line.
const fn keep_right_side(c: i128) -> bool {
    c >= 0_i128
}

/// Whether the side value `c` is kept when clipping to the back (left)
/// half-plane: strictly left or exactly on the line.
const fn keep_left_side(c: i128) -> bool {
    c <= 0_i128
}

/// The intersection of the edge `from` to `to` with the clip line, given
/// the signed side values at its ends. `None` when the arithmetic leaves
/// the range; the caller then emits no cut point for this edge.
fn crossing(from: Point, to: Point, c_from: i128, c_to: i128) -> Option<Point> {
    let den = c_from.checked_sub(c_to)?;
    let dx = i128::from(to.x).saturating_sub(i128::from(from.x));
    let dy = i128::from(to.y).saturating_sub(i128::from(from.y));
    let x = scale_round(dx, c_from, den)?.checked_add(from.x)?;
    let y = scale_round(dy, c_from, den)?.checked_add(from.y)?;
    Some(Point { x, y })
}

/// Appends `point` to `out` unless it duplicates the point just added.
fn push_distinct(out: &mut Polygon, point: Point) {
    if out.last().is_some_and(|last| *last == point) {
        return;
    }
    out.push(point);
}

/// A half-plane to clip to: its boundary line and which side to keep.
/// Points on the line belong to both sides.
#[derive(Clone, Copy)]
struct HalfPlane<'line> {
    /// The boundary line.
    line: &'line FixedLine,
    /// Whether the front (right) half is kept, not the back (left).
    keep_right: bool,
}

impl HalfPlane<'_> {
    /// Whether the side value `c` falls inside this half-plane.
    const fn keeps(self, c: i128) -> bool {
        if self.keep_right {
            keep_right_side(c)
        } else {
            keep_left_side(c)
        }
    }
}

/// One `Sutherland`–`Hodgman` edge step: emit what the edge `from` to
/// `to` contributes to the clipped polygon.
fn clip_edge(plane: HalfPlane<'_>, from: Point, to: Point, out: &mut Polygon) {
    let c_from = side_value(plane.line, from);
    let c_to = side_value(plane.line, to);
    if plane.keeps(c_from) != plane.keeps(c_to) {
        if let Some(cut) = crossing(from, to, c_from, c_to) {
            push_distinct(out, cut);
        }
    }
    if plane.keeps(c_to) {
        push_distinct(out, to);
    }
}

/// Clips the convex `polygon` to one half-plane of `line`: the front
/// (right, `c >= 0`) when `keep_right`, the back (left, `c <= 0`)
/// otherwise. Points on the line belong to both halves.
#[must_use]
#[inline]
pub fn clip(polygon: &[Point], line: &FixedLine, keep_right: bool) -> Polygon {
    let mut out = Polygon::with_capacity(polygon.len().saturating_add(1));
    if polygon.is_empty() {
        return out;
    }
    let plane = HalfPlane { line, keep_right };
    let edges = polygon
        .iter()
        .zip(polygon.iter().cycle().skip(1).take(polygon.len()));
    for (from, to) in edges {
        clip_edge(plane, *from, *to, &mut out);
    }
    if out.len() > 1 && out.first().zip(out.last()).is_some_and(|(a, b)| a == b) {
        out.pop();
    }
    out
}

/// The signed `i128` shoelace sum of `polygon`: positive is
/// counterclockwise with `y` up.
#[must_use]
fn shoelace(polygon: &[Point]) -> i128 {
    polygon
        .iter()
        .zip(polygon.iter().cycle().skip(1).take(polygon.len()))
        .fold(0_i128, |sum, (here, next)| {
            sum.saturating_add(
                i128::from(here.x)
                    .saturating_mul(i128::from(next.y))
                    .saturating_sub(i128::from(next.x).saturating_mul(i128::from(here.y))),
            )
        })
}

/// Reorders `polygon` in place to run clockwise as seen with `y` up, so
/// its interior lies right of every edge — the direction `Doom` segs face
/// their subsector.
#[inline]
pub fn ensure_clockwise(polygon: &mut Polygon) {
    if polygon.len() > 2 && shoelace(polygon) > 0_i128 {
        polygon.reverse();
    }
}

/// The `16.16` form of whole units plus or minus a whole-unit margin,
/// refusing what leaves the range: unlike [`crate::bsp::records::scaled`],
/// which shifts an `i32` and cannot fail, this one must not silently clip
/// a huge map box.
const fn to_fixed(value: i64) -> Option<i64> {
    value.checked_shl(FRACBITS)
}

/// The map's vertex bounding box enlarged by `margin` whole units, as a
/// closed `4`-point counterclockwise polygon. `None` when the map has no
/// vertices or the box leaves the `16.16` range.
#[must_use]
#[inline]
pub fn bounds_polygon(map: &Map, margin: i64) -> Option<Polygon> {
    let mut low_x = i64::MAX;
    let mut low_y = i64::MAX;
    let mut high_x = i64::MIN;
    let mut high_y = i64::MIN;
    for vertex in &map.vertices {
        low_x = low_x.min(i64::from(vertex.x));
        low_y = low_y.min(i64::from(vertex.y));
        high_x = high_x.max(i64::from(vertex.x));
        high_y = high_y.max(i64::from(vertex.y));
    }
    if map.vertices.is_empty() {
        return None;
    }
    let back = margin.checked_neg()?;
    let left = to_fixed(low_x.checked_add(back)?)?;
    let right = to_fixed(high_x.checked_add(margin)?)?;
    let bottom = to_fixed(low_y.checked_add(back)?)?;
    let top = to_fixed(high_y.checked_add(margin)?)?;
    Some(vec![
        Point { x: left, y: bottom },
        Point {
            x: right,
            y: bottom,
        },
        Point { x: right, y: top },
        Point { x: left, y: top },
    ])
}

/// The polygon of a subsector region: the ancestor-clipped `polygon` cut
/// further by the right half-plane of every one of the leaf's `segs`,
/// then ordered clockwise.
#[must_use]
#[inline]
pub fn region_polygon(polygon: &[Point], segs: &[BuildSeg]) -> Polygon {
    let mut region = polygon.to_vec();
    for seg in segs {
        if region.len() < 3 {
            break;
        }
        region = clip(&region, &FixedLine::through_seg(seg), true);
    }
    ensure_clockwise(&mut region);
    region
}

#[cfg(test)]
mod tests {
    use super::{Polygon, bounds_polygon, clip, ensure_clockwise, region_polygon, shoelace};
    use crate::bsp::side::FixedLine;
    use crate::bsp::{BuildSeg, Point};
    use crate::wad::map::{Map, Vertex};

    /// The `16.16` scale factor.
    const UNIT: i64 = 0x1_0000_i64;

    /// A fixed point at whole map units.
    fn point(x: i64, y: i64) -> Point {
        Point {
            x: x.checked_mul(UNIT).unwrap(),
            y: y.checked_mul(UNIT).unwrap(),
        }
    }

    /// A polygon through whole-unit points.
    fn ring(points: &[(i64, i64)]) -> Polygon {
        points.iter().map(|&(x, y)| point(x, y)).collect()
    }

    /// A polygon as a sorted point set, for rotation-insensitive
    /// comparisons.
    fn sorted(polygon: &[Point]) -> Vec<(i64, i64)> {
        let mut keys: Vec<(i64, i64)> = polygon.iter().map(|p| (p.x, p.y)).collect();
        keys.sort_unstable();
        keys
    }

    /// Requires both polygons to hold the same points, in any order.
    fn assert_same_set(got: &[Point], want: &[(i64, i64)]) {
        assert_eq!(sorted(got), sorted(&ring(want)), "the same corner set");
    }

    /// A north-facing line through `(x, 0)`.
    fn north_line(x: i64) -> FixedLine {
        FixedLine {
            x: x.checked_mul(UNIT).unwrap(),
            y: 0,
            dx: 0,
            dy: 256_i64.checked_mul(UNIT).unwrap(),
        }
    }

    /// The `(0,0)-(256,256)` square counterclockwise.
    fn square() -> Polygon {
        ring(&[(0, 0), (256, 0), (256, 256), (0, 256)])
    }

    /// A seg between whole-unit endpoints.
    fn seg(x1: i64, y1: i64, x2: i64, y2: i64) -> BuildSeg {
        BuildSeg {
            start: point(x1, y1),
            end: point(x2, y2),
            linedef: 0,
            side: 0,
            offset: 0,
            sector: 0,
        }
    }

    #[test]
    fn clipping_a_square_by_its_diagonal_keeps_one_triangle() {
        // The anti-diagonal through (0,256) and (256,0): its right
        // half-plane is the (0,0) corner triangle, its left the
        // (256,256) one, and the shared edge stays in both.
        let diagonal = FixedLine {
            x: 0,
            y: 256_i64.checked_mul(UNIT).unwrap(),
            dx: 256_i64.checked_mul(UNIT).unwrap(),
            dy: 256_i64.checked_mul(UNIT).unwrap().checked_neg().unwrap(),
        };
        let right = clip(&square(), &diagonal, true);
        assert_same_set(&right, &[(0, 0), (256, 0), (0, 256)]);
        let left = clip(&square(), &diagonal, false);
        assert_same_set(&left, &[(256, 0), (256, 256), (0, 256)]);
    }

    #[test]
    fn bounds_polygon_grows_the_vertex_box_by_the_margin() {
        let mut map = Map::default();
        for (x, y) in [(0_i32, 0_i32), (256_i32, 128_i32)] {
            map.vertices.push(Vertex {
                x,
                y,
                ..Default::default()
            });
        }
        let box_polygon = bounds_polygon(&map, 64).expect("a box");
        assert_eq!(
            box_polygon,
            ring(&[(-64, -64), (320, -64), (320, 192), (-64, 192)]),
            "64 units outside the box, counterclockwise"
        );
        assert!(bounds_polygon(&Map::default(), 64).is_none(), "no vertices");
    }

    #[test]
    fn ensure_clockwise_reverses_counterclockwise_rings_only() {
        let mut counter = square();
        assert!(shoelace(&counter) > 0_i128, "the test ring is ccw");
        ensure_clockwise(&mut counter);
        assert!(shoelace(&counter) < 0_i128, "now it runs clockwise");
        let mut once = counter.clone();
        ensure_clockwise(&mut once);
        assert_eq!(once, counter, "idempotent on a clockwise ring");
        ensure_clockwise(&mut once);
        assert_eq!(once, counter, "and again");
    }

    #[test]
    fn region_polygon_of_a_room_square_is_the_room() {
        // The four walls of the (0,0)-(256,256) room, interior right.
        let walls = [
            seg(256, 0, 0, 0),
            seg(256, 256, 256, 0),
            seg(0, 256, 256, 256),
            seg(0, 0, 0, 256),
        ];
        let outside = ring(&[(-64, -64), (320, -64), (320, 320), (-64, 320)]);
        let mut region = region_polygon(&outside, &walls);
        assert_same_set(&region, &[(0, 0), (256, 0), (256, 256), (0, 256)]);
        assert!(shoelace(&region) < 0_i128, "the room comes out clockwise");
        region.reverse();
        assert_same_set(&region, &[(0, 0), (256, 0), (256, 256), (0, 256)]);
    }

    #[test]
    fn a_half_plane_clip_keeps_points_on_the_line() {
        // The two halves of the square cut at x = 128 share that edge.
        let box_polygon = square();
        let line = north_line(128);
        let east = clip(&box_polygon, &line, true);
        let west = clip(&box_polygon, &line, false);
        assert_same_set(&east, &[(128, 0), (256, 0), (256, 256), (128, 256)]);
        assert_same_set(&west, &[(0, 0), (128, 0), (128, 256), (0, 256)]);
        assert!(
            east.iter()
                .any(|p| p.x == 128_i64.checked_mul(UNIT).unwrap())
                && west
                    .iter()
                    .any(|p| p.x == 128_i64.checked_mul(UNIT).unwrap()),
            "the cut line's vertices are in both halves"
        );
    }

    #[test]
    fn a_kink_on_the_cut_line_folds_into_the_clip() {
        let kinked = ring(&[(0, 0), (128, 0), (256, 0), (256, 256), (128, 256), (0, 256)]);
        let east = clip(&kinked, &north_line(128), true);
        assert_same_set(&east, &[(128, 0), (256, 0), (256, 256), (128, 256)]);
        assert_eq!(east.len(), 4, "the x = 128 kinks collapse into one edge");
    }

    #[test]
    fn a_diagonal_clip_rounds_to_nearest_16_16() {
        // Cutting along x = y gives the south-east triangle with the
        // diagonal as one edge; a line shifted one unit right still
        // crosses the square's bottom at the exact whole point (1, 0).
        let diagonal = FixedLine {
            x: 0,
            y: 0,
            dx: 256_i64.checked_mul(UNIT).unwrap(),
            dy: 256_i64.checked_mul(UNIT).unwrap(),
        };
        let south_east = clip(&square(), &diagonal, true);
        assert_same_set(&south_east, &[(0, 0), (256, 0), (256, 256)]);
        let off = FixedLine {
            x: UNIT,
            y: 0,
            dx: diagonal.dx,
            dy: diagonal.dy,
        };
        let cut = clip(&square(), &off, true);
        assert_same_set(&cut, &[(1, 0), (256, 0), (256, 255)]);
    }

    #[test]
    fn clipping_a_fully_outside_polygon_empties_it() {
        let line = north_line(1024);
        assert!(clip(&square(), &line, true).is_empty(), "nothing is right");
        let kept = clip(&square(), &line, false);
        assert_same_set(&kept, &[(0, 0), (256, 0), (256, 256), (0, 256)]);
        assert_eq!(kept.len(), 4, "everything is left, unrotated size");
        assert!(clip(&[], &line, true).is_empty(), "empty in, empty out");
        assert!(
            clip(&[point(1, 1)], &line, false).len() < 2,
            "a lone point is nothing"
        );
    }
}
