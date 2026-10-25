//! Geometric checks over a decoded [`Map`]: sector boundaries that do not
//! close, linedefs crossing each other mid-span, collinear linedefs
//! sharing a stretch, and sector ceilings below their floors.
//!
//! Coordinates are `i64` widened from the `i32` map units and every sign
//! test is an exact `i128` product: no float ever appears. The crossing
//! check sorts the lines by their left end and only compares lines whose
//! bounding boxes overlap in `X`, so a large map does not go quadratic;
//! past [`MAX_SWEEP_LINEDEFS`] linedefs it declines to run at all and
//! [`check_skipped`] reports that. Dangling references are never echoed
//! here: [`super::references`] owns them, and a part that does not
//! resolve takes no part in a measurement.

use alloc::collections::BTreeMap;
use alloc::format;
use alloc::vec;
use alloc::vec::Vec;

use super::{Issue, Severity};
use crate::wad::map::model::{Linedef, Map};

/// Linedef count past which [`crossing_lines`] declines to sweep and
/// [`check_skipped`] says so: the sweep is near-linear per typical map,
/// but a pathological one can still make its candidate pairs quadratic.
const MAX_SWEEP_LINEDEFS: usize = 20_000_usize;

/// A map point with its coordinates widened to `i64`.
#[derive(Clone, Copy, Debug)]
struct P {
    /// `X` in map units.
    x: i64,
    /// `Y` in map units.
    y: i64,
}

/// A linedef with both endpoints resolved, for the crossing and overlap
/// sweeps.
#[derive(Clone, Copy, Debug)]
struct Line {
    /// Index of the linedef in [`Map::linedefs`].
    index: usize,
    /// The start vertex.
    a: P,
    /// The end vertex.
    b: P,
}

/// One collinear group member: the linedef index and the stretch it
/// covers along the group's increasing axis.
#[derive(Clone, Copy, Debug)]
struct Span {
    /// Index of the linedef in [`Map::linedefs`].
    index: usize,
    /// The smaller projected endpoint.
    lo: i64,
    /// The larger projected endpoint.
    hi: i64,
}

/// A line's identity: the gcd-reduced direction `(ndx, ndy)` in
/// canonical sign and the line constant `ndy * x − ndx * y` of its
/// primitive normal `(ndy, -ndx)`.
type LineKey = (i64, i64, i128);

/// The left and right `X` of a line's bounding box.
const fn x_span(line: &Line) -> (i64, i64) {
    (
        if line.a.x < line.b.x {
            line.a.x
        } else {
            line.b.x
        },
        if line.a.x < line.b.x {
            line.b.x
        } else {
            line.a.x
        },
    )
}

/// The bottom and top `Y` of a line's bounding box.
const fn y_span(line: &Line) -> (i64, i64) {
    (
        if line.a.y < line.b.y {
            line.a.y
        } else {
            line.b.y
        },
        if line.a.y < line.b.y {
            line.b.y
        } else {
            line.a.y
        },
    )
}

/// Whether both bounding boxes of two lines touch (borders included:
/// only a proper crossing passes the side tests).
const fn boxes_touch(a: &Line, b: &Line) -> bool {
    let (lo_a, hi_a) = x_span(a);
    let (lo_b, hi_b) = x_span(b);
    let (bottom_a, top_a) = y_span(a);
    let (bottom_b, top_b) = y_span(b);
    lo_a <= hi_b && lo_b <= hi_a && bottom_a <= top_b && bottom_b <= top_a
}

/// The ordered index pair of two linedefs, smaller index first.
const fn ordered_pair(a: usize, b: usize) -> (usize, usize) {
    (if a < b { a } else { b }, if a < b { b } else { a })
}

/// One resolved linedef: both endpoints as [`P`]s, or `None` when either
/// vertex index dangles.
fn resolved_line(map: &Map, index: usize, line: &Linedef) -> Option<Line> {
    let ((ax, ay), (bx, by)) = map.line_endpoints(line)?;
    Some(Line {
        index,
        a: P {
            x: i64::from(ax),
            y: i64::from(ay),
        },
        b: P {
            x: i64::from(bx),
            y: i64::from(by),
        },
    })
}

/// Every linedef whose two vertices resolve, in block order.
fn resolved_lines(map: &Map) -> Vec<Line> {
    map.linedefs
        .iter()
        .enumerate()
        .filter_map(|(index, line)| resolved_line(map, index, line))
        .collect()
}

/// Adds one directed boundary edge `from` → `to` to `sector`'s net table:
/// out-degree minus in-degree, so a closed boundary nets zero at every
/// vertex.
fn add_edge(nets: &mut BTreeMap<usize, i32>, from: usize, to: usize) {
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
    ends: &mut BTreeMap<usize, BTreeMap<usize, i32>>,
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
fn directed_edges(map: &Map) -> BTreeMap<usize, BTreeMap<usize, i32>> {
    let mut ends: BTreeMap<usize, BTreeMap<usize, i32>> = BTreeMap::new();
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

/// The sorted indices of the vertices whose net is not zero.
fn unbalanced(nets: &BTreeMap<usize, i32>) -> Vec<usize> {
    nets.iter()
        .filter(|&(_, net)| *net != 0_i32)
        .map(|(vertex, _)| *vertex)
        .collect()
}

/// One unclosed-sector row: the sector, all its unbalanced vertices and
/// the coordinates of the first.
fn unclosed_issue(map: &Map, sector: usize, nets: &BTreeMap<usize, i32>) -> Option<Issue> {
    let free = unbalanced(nets);
    let first = *free.first()?;
    let net = nets.get(&first).copied().unwrap_or(0_i32);
    let point = map.vertices.get(first)?;
    let direction = if net > 0_i32 { "outgoing" } else { "incoming" };
    Some(
        Issue::new(
            Severity::Error,
            format!(
                "sector {sector} is not closed: {} of its boundary vertices are unbalanced, \
                 the first is vertex {first} at x {}, y {} with more {direction} edges than \
                 arriving; every boundary vertex needs as many edges leaving it as arriving \
                 (build the missing wall or split the long one at the free end)",
                free.len(),
                point.x,
                point.y,
            ),
        )
        .sector(&[sector])
        .vertex(&free)
        .at(point.x, point.y),
    )
}

/// Sectors whose directed boundary edges leave free ends: with a front
/// side running `v1` → `v2` and a back side `v2` → `v1`, a closed sector
/// has at every vertex as many edges arriving as leaving.
pub(super) fn unclosed_sector(map: &Map) -> Vec<Issue> {
    directed_edges(map)
        .iter()
        .filter_map(|(sector, nets)| unclosed_issue(map, *sector, nets))
        .collect()
}

/// Sectors with a ceiling below the floor: nothing renders inside them.
pub(super) fn ceiling_below_floor(map: &Map) -> Vec<Issue> {
    map.sectors
        .iter()
        .enumerate()
        .filter(|&(_, space)| space.ceiling_height < space.floor_height)
        .map(|(index, space)| {
            Issue::new(
                Severity::Error,
                format!(
                    "sector {index} has ceiling {} below floor {}; raise the ceiling or lower \
                     the floor with edit_sector",
                    space.ceiling_height, space.floor_height,
                ),
            )
            .sector(&[index])
        })
        .collect()
}

/// The exact `i128` product of two widened coordinates.
fn wide(a: i64, b: i64) -> i128 {
    i128::from(a)
        .checked_mul(i128::from(b))
        .unwrap_or(i128::MAX)
}

/// The sign of the cross product `(b − a) × (p − a)`: which side of the
/// directed line `a` → `b` the point `p` sits on, `0` for exactly on it.
fn side_of(a: P, b: P, p: P) -> i8 {
    let dx = b.x.checked_sub(a.x).unwrap_or(i64::MAX);
    let dy = b.y.checked_sub(a.y).unwrap_or(i64::MAX);
    let px = p.x.checked_sub(a.x).unwrap_or(i64::MAX);
    let py = p.y.checked_sub(a.y).unwrap_or(i64::MAX);
    match wide(px, dy).cmp(&wide(py, dx)) {
        core::cmp::Ordering::Less => -1_i8,
        core::cmp::Ordering::Equal => 0_i8,
        core::cmp::Ordering::Greater => 1_i8,
    }
}

/// Whether `a` and `b` cross properly: their infinite lines meet inside
/// both open segments (strictly opposite sides at every endpoint, so a
/// shared or touching endpoint, a T-junction and a collinear touch all
/// read false).
fn proper_crossing(a: &Line, b: &Line) -> bool {
    let s1 = side_of(a.a, a.b, b.a);
    let s2 = side_of(a.a, a.b, b.b);
    if s1 == 0 || s2 == 0 || s1 == s2 {
        return false;
    }
    let s3 = side_of(b.a, b.b, a.a);
    let s4 = side_of(b.a, b.b, a.b);
    s3 != 0 && s4 != 0 && s3 != s4
}

/// One sweep step: test the entering line against every active line and
/// keep the pairs that cross properly.
fn sweep_active(line: &Line, active: &[Line], pairs: &mut Vec<(usize, usize)>) {
    for other in active {
        if boxes_touch(line, other) && proper_crossing(line, other) {
            pairs.push(ordered_pair(line.index, other.index));
        }
    }
}

/// The bounding-box `X` sweep: lines enter the active set at their left
/// end and leave it once the current line starts past their right, and
/// only active lines are compared. Each unordered pair is tested once,
/// when its later line is processed.
fn sweep_crossings(lines: &[Line]) -> Vec<(usize, usize)> {
    let mut starts: Vec<Line> = lines.to_vec();
    starts.sort_unstable_by_key(|line| (x_span(line).0, line.index));
    let mut active: Vec<Line> = Vec::new();
    let mut pairs: Vec<(usize, usize)> = Vec::new();
    for line in starts {
        let (lo, _) = x_span(&line);
        active.retain(|other| x_span(other).1 >= lo);
        sweep_active(&line, &active, &mut pairs);
        active.push(line);
    }
    pairs
}

/// One crossing row for a pair of linedefs.
fn crossing_issue(a: usize, b: usize) -> Issue {
    Issue::new(
        Severity::Error,
        format!(
            "linedefs {a} and {b} cross at a point that is not one of their endpoints; split \
             one line (split_linedef) or move an end (move_vertex) so the spans no longer \
             meet, then rebuild the nodes",
        ),
    )
    .linedef(&[a, b])
}

/// Pairs of linedefs that cross mid-span, found by the bounding-box
/// sweep. Above [`MAX_SWEEP_LINEDEFS`] linedefs the map is left to
/// [`check_skipped`].
pub(super) fn crossing_lines(map: &Map) -> Vec<Issue> {
    if map.linedefs.len() > MAX_SWEEP_LINEDEFS {
        return Vec::new();
    }
    sweep_crossings(&resolved_lines(map))
        .into_iter()
        .map(|(a, b)| crossing_issue(a, b))
        .collect()
}

/// The one info row a map too large for the crossing sweep earns instead
/// of that check's verdict.
pub(super) fn check_skipped(map: &Map) -> Vec<Issue> {
    let count = map.linedefs.len();
    if count <= MAX_SWEEP_LINEDEFS {
        return Vec::new();
    }
    vec![Issue::new(
        Severity::Info,
        format!(
            "the crossing-line check was skipped: the map holds {count} linedefs, past the \
             {MAX_SWEEP_LINEDEFS} one run sweeps; check crossings on a map with fewer lines",
        ),
    )]
}

/// The greatest common divisor of two `u64` values, `1` for the `0, 0`
/// pair so the caller's division can never see zero.
const fn gcd_u64(mut left: u64, mut right: u64) -> u64 {
    while right != 0 {
        let next = match left.checked_rem(right) {
            Some(rem) => rem,
            None => 0,
        };
        left = right;
        right = next;
    }
    if left < 1 { 1 } else { left }
}

/// The reduced direction in canonical sign: first component positive,
/// or for a vertical line, the second one.
fn reduced_direction(dx: i64, dy: i64) -> Option<(i64, i64)> {
    let g = i64::try_from(gcd_u64(dx.unsigned_abs(), dy.unsigned_abs())).ok()?;
    let raw_x = dx.checked_div(g)?;
    let raw_y = dy.checked_div(g)?;
    let flip = raw_x < 0 || (raw_x == 0 && raw_y < 0);
    let ndx = if flip { raw_x.checked_neg()? } else { raw_x };
    let ndy = if flip { raw_y.checked_neg()? } else { raw_y };
    Some((ndx, ndy))
}

/// The shared line key of a segment and its projected span (`x` along
/// the direction, `y` on a vertical line); `None` for a zero-length
/// line, which [`super::structure`] already reports.
fn line_key(line: &Line) -> Option<(LineKey, Span)> {
    let dx = line.b.x.checked_sub(line.a.x)?;
    let dy = line.b.y.checked_sub(line.a.y)?;
    if dx == 0 && dy == 0 {
        return None;
    }
    let (ndx, ndy) = reduced_direction(dx, dy)?;
    let c = wide(ndy, line.a.x).checked_add(wide(ndx.checked_neg()?, line.a.y))?;
    let (t1, t2) = if ndx == 0 {
        (line.a.y, line.b.y)
    } else {
        (line.a.x, line.b.x)
    };
    Some((
        (ndx, ndy, c),
        Span {
            index: line.index,
            lo: t1.min(t2),
            hi: t1.max(t2),
        },
    ))
}

/// The pairs `earlier` forms with the later spans of its collinear
/// group: spans are sorted by left end, so a later span shares positive
/// length until its `lo` reaches `earlier.hi`, and none beyond.
fn overlaps_after<'spans, Later>(
    earlier: Span,
    later: Later,
) -> impl Iterator<Item = (usize, usize)> + 'spans
where
    Later: Iterator<Item = &'spans Span> + 'spans,
{
    later
        .take_while(move |span| span.lo < earlier.hi)
        .map(move |span| ordered_pair(earlier.index, span.index))
}

/// The overlapping index pairs of one collinear group.
fn group_overlaps(spans: &mut [Span]) -> Vec<(usize, usize)> {
    spans.sort_unstable_by_key(|span| (span.lo, span.index));
    let mut pairs: Vec<(usize, usize)> = Vec::new();
    for (position, earlier) in spans.iter().enumerate() {
        let Some(skip) = position.checked_add(1) else {
            continue;
        };
        pairs.extend(overlaps_after(*earlier, spans.iter().skip(skip)));
    }
    pairs
}

/// One overlap row for a pair of linedefs.
fn overlap_issue(a: usize, b: usize) -> Issue {
    Issue::new(
        Severity::Error,
        format!(
            "linedefs {a} and {b} lie on one line and share a stretch of positive length; \
             join them (join_linedefs) or move one off the line (move_vertex) so no wall is \
             drawn twice",
        ),
    )
    .linedef(&[a, b])
}

/// Pairs of collinear linedefs sharing a stretch of positive length:
/// lines are grouped by [`LineKey`], so only collinear ones ever meet
/// and their spans are compared in sorted order.
pub(super) fn overlapping_lines(map: &Map) -> Vec<Issue> {
    let mut groups: BTreeMap<LineKey, Vec<Span>> = BTreeMap::new();
    for line in resolved_lines(map) {
        if let Some((key, span)) = line_key(&line) {
            groups.entry(key).or_default().push(span);
        }
    }
    groups
        .values_mut()
        .flat_map(|spans| group_overlaps(spans))
        .map(|(a, b)| overlap_issue(a, b))
        .collect()
}

#[cfg(test)]
mod tests {
    use alloc::vec::Vec;

    use super::{
        MAX_SWEEP_LINEDEFS, P, ceiling_below_floor, check_skipped, crossing_lines, ordered_pair,
        overlapping_lines, proper_crossing, resolved_line, resolved_lines, side_of,
        unclosed_sector,
    };
    use crate::wad::map::common::fixtures::{name, side, solid_line, vertex};
    use crate::wad::map::validate::{Issue, Severity};
    use crate::wad::map::{Map, MapFormat, Sector};

    /// A `Doom` map: `points` as vertices and `ends` as one-sided linedef
    /// index pairs, every front side on sector 0.
    fn map_from(points: &[(i32, i32)], ends: &[(usize, usize)]) -> Map {
        let mut map = Map::new_empty(name("MAP01"), MapFormat::Doom);
        map.sectors = vec![Sector::default()];
        map.sidedefs = vec![side("-", "-", "-", 0)];
        map.vertices = points
            .iter()
            .map(|&(x, y)| vertex(x, y))
            .collect::<Vec<_>>();
        for &(v1, v2) in ends {
            map.linedefs.push(solid_line(v1, v2, 0));
        }
        map
    }

    /// The four corners of a 128-unit square.
    const SQUARE: [(i32, i32); 4] = [pt(0, 0), pt(128, 0), pt(128, 128), pt(0, 128)];

    /// A map-point literal with its coordinate type made explicit.
    const fn pt(x: i32, y: i32) -> (i32, i32) {
        (x, y)
    }

    /// A vertex-index pair literal with its type made explicit.
    const fn vp(a: usize, b: usize) -> (usize, usize) {
        (a, b)
    }

    /// A closed square ring: all four walls.
    fn square() -> Map {
        map_from(&SQUARE, &[vp(0, 1), vp(1, 2), vp(2, 3), vp(3, 0)])
    }

    /// The `(linedef, linedef)` pairs a report names.
    fn linedef_pairs(issues: &[Issue]) -> Vec<(usize, usize)> {
        let mut pairs: Vec<(usize, usize)> = issues
            .iter()
            .map(|issue| {
                let a = *issue.linedefs.first().expect("pair issues carry two");
                let b = *issue.linedefs.get(1).expect("pair issues carry two");
                (a, b)
            })
            .collect();
        pairs.sort_unstable();
        pairs
    }

    #[test]
    fn a_ring_with_one_wall_missing_is_unclosed_and_names_both_free_ends() {
        let open = map_from(&SQUARE, &[vp(0, 1), vp(1, 2), vp(2, 3)]);
        let issues = unclosed_sector(&open);
        let issue = issues.first().expect("three walls cannot close a ring");
        assert_eq!(issue.severity, Severity::Error, "a hole is broken");
        assert_eq!(issue.sectors, vec![0_usize], "the only sector");
        assert_eq!(issue.vertices, vec![0_usize, 3], "both free ends, in order");
        assert_eq!(issue.x, Some(0_i32), "the first free end's x");
        assert_eq!(issue.y, Some(0_i32), "and its y");
        assert!(
            issue.message.contains("sector 0 is not closed"),
            "the message names the sector: {}",
            issue.message
        );
        assert!(
            unclosed_sector(&square()).is_empty(),
            "the full ring closes"
        );
    }

    #[test]
    fn unclosed_sector_skips_same_sector_lines_and_dangling_parts() {
        let mut ring = square();
        ring.sidedefs.push(side("-", "-", "-", 0));
        let mut doubled = solid_line(0, 2, 0);
        doubled.back = Some(1);
        doubled.flags = 4;
        ring.linedefs.push(doubled);
        assert!(
            unclosed_sector(&ring).is_empty(),
            "a line whose two sides share the sector bounds nothing and leaves the ring intact"
        );
        let mut broken = square();
        broken
            .sidedefs
            .get_mut(0)
            .expect("the shared sidedef")
            .sector = 99;
        assert!(
            unclosed_sector(&broken).is_empty(),
            "lines whose sides dangle bound nothing and cannot unbalance"
        );
    }

    #[test]
    fn crossing_lines_finds_the_x_and_forgives_touching_ends() {
        let crossed = map_from(
            &[pt(0, 0), pt(128, 128), pt(0, 128), pt(128, 0)],
            &[vp(0, 1), vp(2, 3)],
        );
        let issues = crossing_lines(&crossed);
        let issue = issues.first().expect("an X crosses");
        assert_eq!(issue.linedefs, vec![0_usize, 1], "both lines named");
        assert!(
            issue.message.contains("cross at a point"),
            "the message explains: {}",
            issue.message
        );
        let parallel = map_from(
            &[pt(0, 0), pt(128, 128), pt(0, 128), pt(128, 256)],
            &[vp(0, 1), vp(2, 3)],
        );
        assert!(
            crossing_lines(&parallel).is_empty(),
            "parallel spans do not meet"
        );
        let vee = map_from(&[pt(0, 0), pt(128, 128), pt(256, 0)], &[vp(0, 1), vp(1, 2)]);
        assert!(
            crossing_lines(&vee).is_empty(),
            "a shared endpoint is not a crossing"
        );
        let tee = map_from(
            &[pt(0, 0), pt(200, 0), pt(100, 100), pt(100, 0)],
            &[vp(0, 1), vp(2, 3)],
        );
        assert!(
            crossing_lines(&tee).is_empty(),
            "a T-junction touches the middle of a line: not a proper crossing"
        );
    }

    #[test]
    fn two_lines_meeting_at_equal_coordinates_but_different_vertices_do_not_cross() {
        let joined = map_from(
            &[pt(0, 0), pt(100, 100), pt(100, 100), pt(0, 200)],
            &[vp(0, 1), vp(2, 3)],
        );
        assert!(
            crossing_lines(&joined).is_empty(),
            "the meeting point is an endpoint of both lines"
        );
    }

    #[test]
    fn check_skipped_says_so_only_past_the_sweep_budget() {
        let small = square();
        assert!(check_skipped(&small).is_empty(), "under the budget");
        let mut huge = Map::new_empty(name("BIG"), MapFormat::Doom);
        huge.sectors = vec![Sector::default()];
        huge.sidedefs = vec![side("-", "-", "-", 0)];
        for i in 0..=MAX_SWEEP_LINEDEFS {
            let x = i32::try_from(i).expect("the budget fits i32");
            huge.vertices.push(vertex(x, 0));
            huge.vertices.push(vertex(x, 1));
        }
        for i in 0..=MAX_SWEEP_LINEDEFS {
            let base = i.checked_mul(2).expect("the budget doubles");
            let next = base.checked_add(1).expect("paired");
            huge.linedefs.push(solid_line(base, next, 0));
        }
        let issue = check_skipped(&huge)
            .first()
            .expect("past the budget there is a note")
            .clone();
        assert_eq!(issue.severity, Severity::Info, "a skip is mild");
        assert!(
            issue.message.contains("20001 linedefs"),
            "the message quotes the count: {}",
            issue.message
        );
        assert!(
            crossing_lines(&huge).is_empty(),
            "the sweep itself declines and reports nothing"
        );
    }

    /// Every later line that properly crosses `line_i`, by plain
    /// quadratic scanning.
    fn brute_crossings<'lines, Later>(line_i: &super::Line, later: Later) -> Vec<(usize, usize)>
    where
        Later: Iterator<Item = &'lines super::Line>,
    {
        later
            .filter(|other| proper_crossing(line_i, other))
            .map(|other| ordered_pair(line_i.index, other.index))
            .collect()
    }

    #[test]
    fn the_sweep_agrees_with_a_brute_force_scan_on_a_scatter_map() {
        let mut scatter = Map::new_empty(name("SCATTER"), MapFormat::Doom);
        scatter.sectors = vec![Sector::default()];
        scatter.sidedefs = vec![side("-", "-", "-", 0)];
        for i in 0..200_i64 {
            let x = i.checked_mul(97).expect("small").rem_euclid(600);
            let y = i.checked_mul(53).expect("small").rem_euclid(440);
            let bx = i
                .checked_mul(131)
                .expect("small")
                .checked_add(17)
                .expect("small")
                .rem_euclid(600);
            let by = i
                .checked_mul(149)
                .expect("small")
                .checked_add(29)
                .expect("small")
                .rem_euclid(440);
            scatter.vertices.push(vertex(
                i32::try_from(x).expect("small coords"),
                i32::try_from(y).expect("small coords"),
            ));
            scatter.vertices.push(vertex(
                i32::try_from(bx).expect("small coords"),
                i32::try_from(by).expect("small coords"),
            ));
        }
        for i in 0..200_usize {
            let start = i.checked_mul(2).expect("paired");
            let end = start.checked_add(1).expect("paired");
            scatter.linedefs.push(solid_line(start, end, 0));
        }
        let swept = linedef_pairs(&crossing_lines(&scatter));
        let lines = resolved_lines(&scatter);
        let mut brute: Vec<(usize, usize)> = Vec::new();
        for (i, line_i) in lines.iter().enumerate() {
            brute.extend(brute_crossings(
                line_i,
                lines.iter().skip(i.saturating_add(1)),
            ));
        }
        brute.sort_unstable();
        assert!(
            swept.len() > 10,
            "a 200-line scatter must actually cross a lot, got {}",
            swept.len()
        );
        assert_eq!(swept, brute, "the sweep and the quadratic scan agree");
    }

    #[test]
    fn overlapping_lines_reports_shared_stretches_only() {
        let overlapped = map_from(
            &[pt(0, 0), pt(200, 0), pt(100, 0), pt(300, 0)],
            &[vp(0, 1), vp(2, 3)],
        );
        let issues = overlapping_lines(&overlapped);
        let issue = issues.first().expect("x 100..200 is drawn twice");
        assert_eq!(issue.linedefs, vec![0_usize, 1], "both lines named");
        assert!(
            issue.message.contains("share a stretch"),
            "the message explains: {}",
            issue.message
        );
        let touching = map_from(&[pt(0, 0), pt(100, 0), pt(200, 0)], &[vp(0, 1), vp(1, 2)]);
        assert!(
            overlapping_lines(&touching).is_empty(),
            "meeting at one endpoint shares zero length"
        );
        let diagonal = map_from(
            &[pt(0, 0), pt(200, 200), pt(50, 50), pt(150, 150)],
            &[vp(0, 1), vp(2, 3)],
        );
        assert_eq!(
            overlapping_lines(&diagonal).len(),
            1,
            "the reduced direction groups collinear diagonals too"
        );
        let stacked = map_from(
            &[pt(0, 0), pt(100, 0), pt(50, 0), pt(60, 0), pt(70, 0)],
            &[vp(0, 1), vp(2, 3), vp(2, 4)],
        );
        assert_eq!(
            overlapping_lines(&stacked).len(),
            3,
            "every mutually overlapping pair reports"
        );
    }

    #[test]
    fn collinear_opposite_directions_still_share_their_line() {
        let opposed = map_from(
            &[pt(0, 0), pt(200, 0), pt(100, 0), pt(-50, 0)],
            &[vp(0, 1), vp(2, 3)],
        );
        assert_eq!(
            overlapping_lines(&opposed).len(),
            1,
            "the direction sign is canonicalised away"
        );
    }

    #[test]
    fn ceiling_below_floor_names_the_inverted_sector() {
        let mut map = square();
        let space = map.sectors.get_mut(0).expect("the only sector");
        space.floor_height = 200_i32;
        space.ceiling_height = 100_i32;
        let issues = ceiling_below_floor(&map);
        let issue = issues.first().expect("the ceiling is under the floor");
        assert_eq!(
            issue.severity,
            Severity::Error,
            "an inverted sector is broken"
        );
        assert_eq!(issue.sectors, vec![0_usize], "the inverted sector");
        assert!(
            issue.message.contains("ceiling 100 below floor 200"),
            "the message quotes both heights: {}",
            issue.message
        );
        let mut equal = square();
        let sealed = equal.sectors.get_mut(0).expect("the only sector");
        sealed.floor_height = 128_i32;
        sealed.ceiling_height = 128_i32;
        assert!(
            ceiling_below_floor(&equal).is_empty(),
            "equal heights are a sealed room, not an inversion"
        );
    }

    #[test]
    fn the_side_test_and_resolution_helpers_hold_their_contract() {
        let crossed = map_from(
            &[pt(0, 0), pt(128, 128), pt(0, 128), pt(128, 0)],
            &[vp(0, 1), vp(2, 3)],
        );
        let lines = resolved_lines(&crossed);
        let first = lines.first().expect("two resolved lines");
        let above = P {
            x: 0_i64,
            y: 128_i64,
        };
        let below = P {
            x: 128_i64,
            y: 0_i64,
        };
        assert_eq!(
            side_of(first.a, first.b, above),
            -1,
            "along the (1, 1) direction, (0, 128) is on the negative side"
        );
        assert_eq!(
            side_of(first.a, first.b, below),
            1,
            "and (128, 0) on the positive"
        );
        assert!(
            proper_crossing(first, lines.get(1).expect("the second line")),
            "the X properly crosses"
        );
        let mut dangling = crossed;
        dangling.linedefs.push(solid_line(0, 900, 0));
        assert!(
            resolved_line(
                &dangling,
                2,
                dangling.linedefs.get(2).expect("the new line")
            )
            .is_none(),
            "a line past the vertex block resolves to nothing"
        );
    }
}
