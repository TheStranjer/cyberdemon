//! Pure integer geometry over a decoded [`Map`]: sector boundaries,
//! containment, bounding boxes, line lengths and sector adjacency.
//!
//! Everything here is a function of the map model alone: no sessions, no
//! MCP. Coordinates are computed in `i64` with checked operations (exact
//! `i128` products for the cross comparisons), so no float ever appears.
//! Dangling references - a linedef pointing past `VERTEXES`, a sidedef
//! pointing past `SECTORS` - are never errors: such parts simply do not
//! take part in the answer.
//!
//! The answers come from the [`SectorIndex`]: one pass over `map.linedefs`
//! that stores per sector its boundary lines, carrying segments, bounding
//! box, neighbours and the bbox-filtered point lookup. Each free function
//! below builds one index and reads its answer from it; the index itself
//! lives in the private `index` submodule.
//!
//! [`Map`]: crate::wad::map::Map

mod index;

use super::model::{Endpoints, Linedef, Map};
use crate::fixed::hypot;
pub use index::SectorIndex;

/// A linedef with both vertices resolved, coordinates widened to `i64`.
#[derive(Clone, Copy, Debug)]
struct Segment {
    /// Start vertex `X`.
    x1: i64,
    /// Start vertex `Y`.
    y1: i64,
    /// End vertex `X`.
    x2: i64,
    /// End vertex `Y`.
    y2: i64,
}

impl Segment {
    /// The segment across resolved `endpoints`.
    fn from_endpoints(((x1, y1), (x2, y2)): Endpoints) -> Self {
        Self {
            x1: i64::from(x1),
            y1: i64::from(y1),
            x2: i64::from(x2),
            y2: i64::from(y2),
        }
    }
}

/// The endpoints of `line` as a [`Segment`], when both vertices resolve.
fn segment_of(map: &Map, line: &Linedef) -> Option<Segment> {
    map.line_endpoints(line).map(Segment::from_endpoints)
}

/// Whether `line`'s two sides point at the same sector; such a line bounds
/// nothing and the containment tests ignore it.
fn same_sector_sides(map: &Map, line: &Linedef) -> bool {
    let (front, back) = map.line_sectors(line);
    front.is_some() && front == back
}

/// The linedefs having a side in `sector`, as `(linedef index, is_front)`
/// pairs in linedef order. A line whose two sides both sit in `sector`
/// appears once, flagged front.
///
/// References that do not resolve never contribute.
#[must_use]
#[inline]
pub fn sector_boundary(map: &Map, sector: usize) -> Vec<(usize, bool)> {
    SectorIndex::new(map).boundary(sector).to_vec()
}

/// The bounding box `(min_x, min_y, max_x, max_y)` of the vertices on
/// `sector`'s boundary, or `None` when the sector bounds nothing with a
/// resolvable vertex.
#[must_use]
#[inline]
pub fn sector_bbox(map: &Map, sector: usize) -> Option<(i32, i32, i32, i32)> {
    SectorIndex::new(map).bbox(sector)
}

/// The box covering `bbox` and a segment's endpoints; a fresh box when
/// there is none yet.
fn widen(
    bbox: Option<(i32, i32, i32, i32)>,
    ((ax, ay), (bx, by)): Endpoints,
) -> (i32, i32, i32, i32) {
    let (min_x, min_y, max_x, max_y) = bbox.unwrap_or((ax, ay, ax, ay));
    (
        min_x.min(ax).min(bx),
        min_y.min(ay).min(by),
        max_x.max(ax).max(bx),
        max_y.max(ay).max(by),
    )
}

/// The length of linedef `linedef` in map units, floor-rounded
/// (`hypot` of the vertex deltas), or `None` when the linedef or one of
/// its vertices does not resolve.
#[must_use]
#[inline]
pub fn linedef_length(map: &Map, linedef: usize) -> Option<u64> {
    let line = map.linedefs.get(linedef)?;
    let segment = segment_of(map, line)?;
    let dx = segment.x2.checked_sub(segment.x1)?;
    let dy = segment.y2.checked_sub(segment.y1)?;
    Some(hypot(dx, dy))
}

/// The sectors across `sector`'s two-sided boundary lines, sorted and
/// deduplicated. Lines whose two sides share a sector contribute nothing.
#[must_use]
#[inline]
pub fn neighbors(map: &Map, sector: usize) -> Vec<usize> {
    SectorIndex::new(map).neighbours(sector).to_vec()
}

/// The sector containing the point `x`/`y`, or `None` when no sector
/// holds it.
///
/// Only sectors whose bounding box covers the point - an exact
/// pre-filter - get the even-odd ray-crossing test; a point exactly on a
/// line counts as inside the sector on that line's front side. When nested
/// sectors all hold the point the one with the smallest bounding-box area
/// wins (ties by the lower sector index).
#[must_use]
#[inline]
pub fn point_in_sector(map: &Map, x: i64, y: i64) -> Option<usize> {
    SectorIndex::new(map).point_in_sector(x, y)
}

#[cfg(test)]
mod tests {
    use super::{linedef_length, neighbors, point_in_sector, sector_bbox, sector_boundary};
    use crate::wad::map::common::fixtures::{name, room, side, solid_line, two_room_map, vertex};
    use crate::wad::map::{Map, MapFormat};

    /// A sector nested in a bigger one: an outer square `0..1000` (sector
    /// 0) and an inner square `400..600` (sector 1), all lines one-sided.
    fn nested_map() -> Map {
        let mut map = Map::new_empty(name("MAP01"), MapFormat::Doom);
        map.vertices = vec![
            vertex(0, 0),
            vertex(1000, 0),
            vertex(1000, 1000),
            vertex(0, 1000),
            vertex(400, 400),
            vertex(600, 400),
            vertex(600, 600),
            vertex(400, 600),
        ];
        map.linedefs = vec![
            solid_line(0, 1, 0),
            solid_line(1, 2, 1),
            solid_line(2, 3, 2),
            solid_line(3, 0, 3),
            solid_line(4, 5, 4),
            solid_line(5, 6, 5),
            solid_line(6, 7, 6),
            solid_line(7, 4, 7),
        ];
        map.sidedefs = (0_usize..8)
            .map(|index| side("-", "-", "WALL1", usize::from(index >= 4)))
            .collect();
        map.sectors = vec![room(0, 128, 160, 0), room(0, 128, 160, 1)];
        map
    }

    #[test]
    fn sector_boundary_lists_the_lines_touching_a_sector() {
        let map = two_room_map();
        assert_eq!(
            sector_boundary(&map, 0),
            vec![(0_usize, true), (1, true), (2, true), (3, true)],
            "the hall owns the left box, front side on every line",
        );
        assert_eq!(
            sector_boundary(&map, 1),
            vec![(2, false), (4, true), (5, true), (6, true)],
            "the back room is the step line's back and three fronts",
        );
        assert!(sector_boundary(&map, 2).is_empty(), "no sector 2 exists");
    }

    #[test]
    fn sector_bbox_spans_the_boundary_vertices() {
        let map = two_room_map();
        assert_eq!(
            sector_bbox(&map, 0),
            Some((0_i32, 0_i32, 256_i32, 256_i32)),
            "the hall box"
        );
        assert_eq!(
            sector_bbox(&map, 1),
            Some((256_i32, 0_i32, 512_i32, 256_i32)),
            "the back room box"
        );
        assert_eq!(
            sector_bbox(&map, 9),
            None,
            "a missing sector bounds nothing"
        );
    }

    #[test]
    fn linedef_length_is_the_floored_hypot() {
        let mut map = two_room_map();
        assert_eq!(linedef_length(&map, 0), Some(256_u64), "the west wall");
        assert_eq!(linedef_length(&map, 6), Some(256_u64), "a floor line");
        map.linedefs.push(solid_line(0, 3, 0));
        let diagonal = map.linedefs.len().checked_sub(1).expect("just pushed");
        assert_eq!(
            linedef_length(&map, diagonal),
            Some(572_u64),
            "hypot(512, 256) = 572.43 floors to 572"
        );
        assert_eq!(linedef_length(&map, 99), None, "no such linedef");
    }

    #[test]
    fn neighbors_lists_across_sectors_sorted_and_deduplicated() {
        let map = two_room_map();
        assert_eq!(neighbors(&map, 0), vec![1_usize], "across the step line");
        assert_eq!(neighbors(&map, 1), vec![0_usize], "and back");
        assert!(
            neighbors(&map, 9).is_empty(),
            "a missing sector neighbours nothing"
        );
    }

    #[test]
    fn point_in_sector_finds_rooms_walls_and_the_void() {
        let map = two_room_map();
        assert_eq!(point_in_sector(&map, 64, 128), Some(0), "inside the hall");
        assert_eq!(
            point_in_sector(&map, 384, 128),
            Some(1),
            "inside the back room"
        );
        assert_eq!(
            point_in_sector(&map, 256, 128),
            Some(0),
            "on the step line: the sector on its front side",
        );
        assert_eq!(point_in_sector(&map, 600, 128), None, "right of both rooms");
        assert_eq!(point_in_sector(&map, 128, 300), None, "above the hall");
        assert_eq!(
            point_in_sector(&map, -1, -1),
            None,
            "below-left of everything"
        );
    }

    #[test]
    fn nested_sectors_pick_the_smallest_box() {
        let map = nested_map();
        assert_eq!(
            point_in_sector(&map, 500, 500),
            Some(1),
            "inside the inner square: the smaller box wins",
        );
        assert_eq!(
            point_in_sector(&map, 200, 200),
            Some(0),
            "between the squares: only the outer holds the point",
        );
        assert_eq!(
            point_in_sector(&map, 500, 400),
            Some(1),
            "on the inner square's bottom line, and the inner square's front is sector 1"
        );
        assert_eq!(point_in_sector(&map, 1100, 500), None, "outside both");
    }

    #[test]
    fn dangling_references_are_tolerated() {
        let mut map = two_room_map();
        map.sidedefs.get_mut(4).expect("sidedef 4").sector = 99;
        map.linedefs.get_mut(3).expect("linedef 3").v1 = 999;
        assert!(
            !sector_boundary(&map, 0).contains(&(3_usize, true)),
            "the line whose sidedef sector dangles drops out",
        );
        assert_eq!(
            linedef_length(&map, 3),
            None,
            "the line whose vertex dangles has no length",
        );
        assert_eq!(
            point_in_sector(&map, 64, 128),
            Some(0),
            "the rest of the map still resolves",
        );
        assert_eq!(
            neighbors(&map, 0),
            vec![1_usize],
            "the step line is untouched"
        );
    }
}
