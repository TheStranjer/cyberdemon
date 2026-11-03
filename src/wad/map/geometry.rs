//! Pure integer geometry over a decoded [`Map`]: sector boundaries,
//! containment, bounding boxes, line lengths, sector adjacency and the
//! unclosed-boundary-ring analysis.
//!
//! Everything here is a function of the map model alone: no sessions, no
//! MCP. Coordinates are computed in `i64` with checked operations (exact
//! `i128` products for the cross comparisons), so no float ever appears.
//! Dangling references - a linedef pointing past `VERTEXES`, a sidedef
//! pointing past `SECTORS` - are never errors: such parts simply do not
//! take part in the answer. The ring analysis is the one owner of the
//! unclosed-boundary check behind `validate_map`'s `unclosed_sector`
//! rows and the overlay engine's notes.
//!
//! The answers come from the [`SectorIndex`]: one pass over `map.linedefs`
//! that stores per sector its boundary lines, carrying segments, bounding
//! box, neighbours and the bbox-filtered point lookup. Each free function
//! below builds one index and reads its answer from it; the index itself
//! lives in the private `index` submodule. The ring analysis walks the
//! linedefs directly: it needs every side of every line, not one sector's
//! boundary.
//!
//! [`Map`]: crate::wad::map::Map

mod index;

use alloc::collections::BTreeMap;
use core::fmt;

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

/// The out-degree minus in-degree of one sector's boundary vertices.
type Nets = BTreeMap<usize, i32>;

/// One free end of a sector's boundary ring: a vertex of its directed
/// boundary edges with more edges one way than the other.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FreeEnd {
    /// The vertex index in [`Map::vertices`].
    pub vertex: usize,
    /// The vertex' `X` coordinate.
    pub x: i32,
    /// The vertex' `Y` coordinate.
    pub y: i32,
    /// The edges leaving the vertex minus the edges arriving at it.
    pub net: i32,
    /// The sector's boundary linedefs ending at this vertex, in linedef
    /// order.
    pub linedefs: Vec<usize>,
}

impl fmt::Display for FreeEnd {
    #[inline]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let direction = if self.net > 0_i32 {
            "leaving"
        } else {
            "arriving"
        };
        write!(
            f,
            "vertex {} at x {}, y {} with more edges {direction} than the other way ({})",
            self.vertex,
            self.x,
            self.y,
            linedefs_words(&self.linedefs),
        )
    }
}

/// How the free ends are phrased: at most this many are named, the rest
/// counted.
const ENDS_NAMED: usize = 3;

/// The free ends of one ring as one clause: the first few named with
/// coordinates and incident linedefs, the rest counted.
fn ends_clause(ends: &[FreeEnd]) -> String {
    let mut words: Vec<String> = ends
        .iter()
        .take(ENDS_NAMED)
        .map(ToString::to_string)
        .collect();
    let Some(rest) = ends.len().checked_sub(words.len()) else {
        return words.join("; ");
    };
    if rest > 0 {
        words.push(format!("{rest} more"));
    }
    words.join("; ")
}

/// The linedefs ending at one free end, phrased for a message: none,
/// one `linedef`, or several `linedefs a, b and c` naming at most
/// [`ENDS_NAMED`] of them and counting the rest.
fn linedefs_words(lines: &[usize]) -> String {
    let Some(first) = lines.first().copied() else {
        return "no linedef ends free there".to_owned();
    };
    if lines.len() < 2 {
        return format!("free end of linedef {first}");
    }
    let named = lines
        .iter()
        .take(ENDS_NAMED)
        .map(usize::to_string)
        .collect::<Vec<String>>();
    let extra = lines.len().saturating_sub(ENDS_NAMED);
    let tail = if extra > 0 {
        format!(", and {extra} more")
    } else {
        String::new()
    };
    format!("free ends of linedefs {}{tail}", and_list(&named))
}

/// `a`, `a and b`, `a, b and c`: the names comma-joined with an `and`
/// before the last.
fn and_list(names: &[String]) -> String {
    let Some((last, firsts)) = names.split_last() else {
        return String::new();
    };
    if firsts.is_empty() {
        return last.clone();
    }
    format!("{} and {last}", firsts.join(", "))
}

/// A sector whose directed boundary edges leave free ends: the ring does
/// not close.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UnclosedRing {
    /// The sector index in [`Map::sectors`].
    pub sector: usize,
    /// The ring's free ends, in ascending vertex order.
    pub ends: Vec<FreeEnd>,
}

impl UnclosedRing {
    /// The ring's free ends, phrased as one clause: up to three named
    /// with vertex, coordinates, direction and incident linedefs, the
    /// rest counted.
    #[must_use]
    #[inline]
    pub fn ends_words(&self) -> String {
        ends_clause(&self.ends)
    }

    /// Every linedef this ring's free ends name, sorted and deduplicated:
    /// what a caller attaches as the issue's linedef references.
    #[must_use]
    #[inline]
    pub fn linedefs(&self) -> Vec<usize> {
        let mut lines: Vec<usize> = self
            .ends
            .iter()
            .flat_map(|end| end.linedefs.clone())
            .collect();
        lines.sort_unstable();
        lines.dedup();
        lines
    }
}

/// Adds one directed boundary edge `from` → `to` to `sector`'s net table:
/// out-degree minus in-degree, so a closed boundary nets zero at every
/// vertex.
fn add_edge(nets: &mut Nets, from: usize, to: usize) {
    let out = nets.get(&from).copied().unwrap_or(0_i32).saturating_add(1);
    nets.insert(from, out);
    let inn = nets.get(&to).copied().unwrap_or(0_i32).saturating_sub(1);
    nets.insert(to, inn);
}

/// One sector's share of one line, if it has one: `own` must resolve to
/// a sector and must not be the sector on the other side (a line whose
/// two sides share one sector bounds nothing, exactly as the containment
/// tests treat it). `edge` is the directed vertex pair.
fn push_edge(
    ends: &mut BTreeMap<usize, Nets>,
    own: Option<usize>,
    other: Option<usize>,
    edge: (usize, usize),
) {
    let (Some(sector), true) = (own, own != other) else {
        return;
    };
    let (from, to) = edge;
    add_edge(ends.entry(sector).or_default(), from, to);
}

/// The directed boundary edge nets of every sector: a line's front side
/// runs `v1` → `v2`, its back side `v2` → `v1`. Unresolvable parts
/// contribute nothing.
fn directed_nets(map: &Map) -> BTreeMap<usize, Nets> {
    let mut ends: BTreeMap<usize, Nets> = BTreeMap::new();
    for line in &map.linedefs {
        let (Some(_), Some(_)) = (map.vertices.get(line.v1), map.vertices.get(line.v2)) else {
            continue;
        };
        let (front, back) = map.line_sectors(line);
        push_edge(&mut ends, front, back, (line.v1, line.v2));
        push_edge(&mut ends, back, front, (line.v2, line.v1));
    }
    ends
}

/// Whether `line` bounds `sector`: one side resolves to it and the two
/// sides differ (the same rule [`push_edge`] folds into the nets).
fn bounds_sector(map: &Map, line: &Linedef, sector: usize) -> bool {
    let (front, back) = map.line_sectors(line);
    (front == Some(sector) || back == Some(sector)) && back != front
}

/// The `sector` boundary linedefs ending at `vertex`, in linedef order.
fn ending_lines(map: &Map, sector: usize, vertex: usize) -> Vec<usize> {
    map.linedefs
        .iter()
        .enumerate()
        .filter(|&(_, line)| line.v1 == vertex || line.v2 == vertex)
        .filter(|&(_, line)| bounds_sector(map, line, sector))
        .map(|(index, _)| index)
        .collect()
}

/// One free end of `sector`'s ring, `None` when its vertex no longer
/// resolves (it was read from a resolved edge, so that cannot happen).
fn free_end(map: &Map, sector: usize, vertex: usize, net: i32) -> Option<FreeEnd> {
    let point = map.vertices.get(vertex)?;
    Some(FreeEnd {
        vertex,
        x: point.x,
        y: point.y,
        net,
        linedefs: ending_lines(map, sector, vertex),
    })
}

/// The sectors whose directed boundary edges leave free ends: with a
/// front side running `v1` → `v2` and a back side `v2` → `v1`, a closed
/// sector has at every vertex as many edges arriving as leaving.
///
/// Rings are reported in ascending sector order, ends in ascending
/// vertex order; a sector that bounds nothing has no nets and no ring to
/// report. `validate_map`'s `unclosed_sector` check and the overlay
/// engine's notes both phrase rows from this one read.
#[must_use]
#[inline]
pub fn unclosed_rings(map: &Map) -> Vec<UnclosedRing> {
    directed_nets(map)
        .into_iter()
        .filter_map(|(sector, nets)| {
            let ends: Vec<FreeEnd> = nets
                .into_iter()
                .filter(|&(_, net)| net != 0_i32)
                .filter_map(|(vertex, net)| free_end(map, sector, vertex, net))
                .collect();
            (!ends.is_empty()).then_some(UnclosedRing { sector, ends })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{
        linedef_length, neighbors, point_in_sector, sector_bbox, sector_boundary, unclosed_rings,
    };
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

    /// A `Doom` map: one sector, `points` as vertices and `ends` as
    /// one-sided linedef index pairs, each front side on sector 0.
    fn ring_map(points: &[(i32, i32)], ends: &[(usize, usize)]) -> Map {
        let mut map = Map::new_empty(name("MAP01"), MapFormat::Doom);
        map.sectors = vec![room(0, 128, 160, 0)];
        map.vertices = points.iter().map(|&(x, y)| vertex(x, y)).collect();
        map.sidedefs = (0..ends.len())
            .map(|_| side("-", "-", "WALL1", 0))
            .collect();
        map.linedefs = ends
            .iter()
            .enumerate()
            .map(|(index, &(v1, v2))| solid_line(v1, v2, index))
            .collect();
        map
    }

    /// The four corners of a 128-unit square.
    const SQUARE: [(i32, i32); 4] = [(0, 0), (128, 0), (128, 128), (0, 128)];

    #[test]
    fn unclosed_rings_name_every_free_end_with_coordinates_and_linedefs() {
        let open = ring_map(&SQUARE, &[(0, 1), (1, 2), (2, 3)]);
        let rings = unclosed_rings(&open);
        assert_eq!(rings.len(), 1, "the one sector with a hole");
        let ring = rings.first().expect("one ring");
        assert_eq!(ring.sector, 0, "the only sector");
        assert_eq!(ring.ends.len(), 2, "the gap has two free ends");
        let start = ring.ends.first().expect("the first end");
        assert_eq!(start.vertex, 0, "the ring starts free at vertex 0");
        assert_eq!((start.x, start.y), (0_i32, 0_i32), "with its coordinates");
        assert!(start.net > 0_i32, "one edge leaves, none arrives");
        assert_eq!(start.linedefs, vec![0_usize], "the south wall ends there");
        let finish = ring.ends.get(1).expect("the second end");
        assert_eq!(finish.vertex, 3, "and ends free at vertex 3");
        assert_eq!((finish.x, finish.y), (0_i32, 128_i32), "its coordinates");
        assert!(finish.net < 0_i32, "one edge arrives, none leaves");
        assert_eq!(finish.linedefs, vec![2_usize], "the west wall's east end");
        assert_eq!(ring.linedefs(), vec![0_usize, 2], "both open lines");
        let words = ring.ends_words();
        assert!(
            words.contains("vertex 0 at x 0, y 0 with more edges leaving than the other way (free end of linedef 0)"),
            "the first end is fully named: {words}"
        );
        assert!(
            words.contains("vertex 3 at x 0, y 128 with more edges arriving than the other way (free end of linedef 2)"),
            "the second end is fully named: {words}"
        );
    }

    #[test]
    fn a_closed_map_holds_its_rings() {
        assert!(
            unclosed_rings(&ring_map(&SQUARE, &[(0, 1), (1, 2), (2, 3), (3, 0)])).is_empty(),
            "the full square ring closes"
        );
        assert!(
            unclosed_rings(&two_room_map()).is_empty(),
            "the fixture hall and back room close"
        );
        assert!(
            unclosed_rings(&nested_map()).is_empty(),
            "both nested squares close"
        );
    }

    #[test]
    fn a_vertex_wedged_between_two_free_lines_names_both_and_extra_ends_count() {
        let wedge_points = [(0_i32, 0_i32), (10_i32, 10_i32), (20_i32, 0_i32)];
        let wedge = ring_map(&wedge_points, &[(0, 1), (2, 1)]);
        let ring = unclosed_rings(&wedge)
            .into_iter()
            .next()
            .expect("a V leaves three free ends");
        assert_eq!(ring.ends.len(), 3, "two tips and the shared wedge");
        let middle = ring.ends.get(1).expect("the wedge vertex");
        assert_eq!(middle.vertex, 1, "the shared end");
        assert_eq!(middle.net, -2_i32, "both edges arrive there");
        assert_eq!(middle.linedefs, vec![0_usize, 1], "both open lines");
        assert!(
            ring.ends_words().contains("free ends of linedefs 0 and 1"),
            "both linedefs are named: {}",
            ring.ends_words()
        );
        let broken = ring_map(&SQUARE, &[(0, 1), (2, 3)]);
        let words = unclosed_rings(&broken)
            .into_iter()
            .next()
            .expect("two disjoint walls leave four ends")
            .ends_words();
        assert!(
            words.contains("1 more"),
            "the fourth end is counted, not named: {words}"
        );
    }

    #[test]
    fn a_sector_that_bounds_nothing_is_not_an_unclosed_ring() {
        let mut map = ring_map(&SQUARE, &[(0, 1), (1, 2), (2, 3)]);
        map.sectors.push(room(0, 128, 160, 0));
        let rings = unclosed_rings(&map);
        assert_eq!(rings.len(), 1, "only the ring with a hole is open");
        assert_eq!(
            rings.first().expect("one ring").sector,
            0,
            "sector 1 has no boundary to leave open"
        );
    }
}
