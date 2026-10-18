//! `glBSP` v2 serialization of a built [`BspTree`]: the `GL_*` lump run
//! whose every subsector is a closed convex polygon.
//!
//! `OpenGL` ports cannot draw a subsector whose seg loop leaves gaps
//! along the partition lines, so this builder recreates every region
//! polygon (the map box, the ancestor partition halves and the leaf's own
//! seg lines — see [`polygon`](super::polygon)) and stretches a
//! **miniseg** (a seg with linedef `0xFFFF`) over every edge stretch no
//! real seg covers, snapping endpoints within `1/16` unit so each
//! subsector's loop closes exactly, clockwise, its interior right.
//! Segs that face each other across a partition (one's `(v1, v2)` is the
//! other's `(v2, v1)`) become **partners**; unmatched segs get partner
//! `0xFFFF`. The result is the record lumps: `GL_VERT` (`gNd2` magic plus
//! the fraction points beyond `VERTEXES`), `GL_SEGS` (10 bytes each),
//! `GL_SSECT` and `GL_NODES` (the vanilla layout, children with `bit 15`
//! marking a subsector), plus an empty `GL_PVS`. Nothing here knows about
//! archives, sessions or `MCP`: the caller owns the lump names and the
//! classic run written alongside.

use alloc::collections::{BTreeMap, BTreeSet};

use crate::bsp::polygon::{Polygon, bounds_polygon, clip, region_polygon};
use crate::bsp::side::FixedLine;
use crate::bsp::{BspTree, BuildSeg, BuiltNode, Child, Point};
use crate::error::CyberError;
use crate::fixed::{FRACBITS, hypot};
use crate::wad::bytes::Writer;
use crate::wad::map::Map;

/// Whole map units the bounding box is enlarged by.
const MARGIN_UNITS: i64 = 64;
/// Snap threshold and miniseg cutoff: `1/16` of a unit, raw `16.16`.
const SNAP: u64 = 4096_u64;
/// Collinearity slack as a multiple of the edge length: `1/16` unit.
const COLLINEAR_SLACK: i128 = 4096_i128;
/// `GL_VERT` magic of the `glBSP` v2 run.
const MAGIC_GND2: &[u8; 4] = b"gNd2";
/// Flag marking a seg endpoint as an index into `GL_VERT`.
const GL_VERTEX_FLAG: u16 = 0x8000_u16;
/// Flag marking a `GL_NODES` child as a subsector leaf.
const SUBSECTOR_FLAG: u16 = 0x8000_u16;
/// `linedef` word of a miniseg.
const MINISEG_LINEDEF: u16 = 0xFFFF_u16;
/// Partner word of a seg with no partner.
const NO_PARTNER: u16 = 0xFFFF_u16;
/// Highest vertex, node or subsector index below the `bit 15` flag.
const MAX_INDEX: usize = 0x7FFF_usize;
/// Highest `GL_SEGS` index below the `0xFFFF` sentinel.
const MAX_SEG_INDEX: usize = 0xFFFE_usize;
/// Highest linedef index below the miniseg sentinel.
const MAX_LINEDEF_INDEX: usize = 0xFFFE_usize;
/// Bytes in one `GL_SEGS` record.
const SEG_RECORD_LEN: usize = 10;
/// Bytes in one `GL_SSECT` record.
const SSECT_RECORD_LEN: usize = 4;
/// Bytes in one `GL_NODES` record.
const NODE_RECORD_LEN: usize = 28;
/// Bytes in one `GL_VERT` row.
const VERTEX_ROW_LEN: usize = 8;

/// The whole `glBSP` v2 run of one tree: the record lumps and the counts
/// a report needs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GlNodes {
    /// The `GL_VERT` lump: `gNd2` magic, then new vertices as raw
    /// `16.16` `i32` pairs.
    pub vertices: Vec<u8>,
    /// The `GL_SEGS` lump: 10 bytes per seg, in subsector order.
    pub segs: Vec<u8>,
    /// The `GL_SSECT` lump: 4 bytes per subsector (count, first).
    pub ssectors: Vec<u8>,
    /// The `GL_NODES` lump: 28 bytes per node, root last.
    pub nodes: Vec<u8>,
    /// The `GL_PVS` lump: always empty (no visibility set is computed).
    pub pvs: Vec<u8>,
    /// Total segs written, real and mini alike.
    pub seg_count: usize,
    /// Minisegs among them.
    pub minisegs: usize,
    /// Vertices written to `GL_VERT`, beyond the map's own.
    pub new_vertices: usize,
}

/// Serialize `tree` as the `glBSP` v2 `GL_*` lumps over `map`.
///
/// # Errors
///
/// [`CyberError::LimitExceeded`] when a vertex, seg, node, subsector or
/// linedef index leaves the words the records hold, or a `GL_VERT`
/// coordinate leaves `i32`; [`CyberError::Internal`] when a region
/// collapses or a seg cannot be placed on its subsector's polygon.
#[inline]
pub fn to_gl(map: &Map, tree: &BspTree) -> Result<GlNodes, CyberError> {
    let regions = region_polygons(map, tree)?;
    let mut vertices = VertexTable::new(map)?;
    let mut run = SegPool::default();
    for (index, region) in regions.iter().enumerate() {
        let segs = tree.subsector_segs(index).ok_or_else(|| broken(index))?;
        run.emit(close_region(region, segs)?, &mut vertices)?;
    }
    let new_vertices = vertices.added_count();
    let records = run.write()?;
    Ok(GlNodes {
        vertices: write_vertices(&vertices),
        segs: records.segs,
        ssectors: records.ssectors,
        nodes: write_nodes(tree)?,
        pvs: Vec::new(),
        seg_count: records.seg_count,
        minisegs: records.minisegs,
        new_vertices,
    })
}

/// Every subsector's closed region polygon, in subsector order: the map
/// box walked through the tree's partition halves, then cut by the leaf's
/// own seg lines and ordered clockwise.
fn region_polygons(map: &Map, tree: &BspTree) -> Result<Vec<Polygon>, CyberError> {
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
struct GlSeg {
    /// Loop-direction start point, `16.16`.
    start: Point,
    /// Loop-direction end point, `16.16`.
    end: Point,
    /// Source linedef index; `None` marks a miniseg.
    linedef: Option<usize>,
    /// Source side number; `0` on minisegs.
    side: u8,
}

/// Order the loop around the clockwise `region`, stretching minisegs
/// over the uncovered stretch of every edge and closing it exactly.
fn close_region(region: &[Point], segs: &[BuildSeg]) -> Result<Vec<GlSeg>, CyberError> {
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
fn push_gap(loop_segs: &mut Vec<GlSeg>, cursor: &mut Point, target: Point) {
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

impl GlSeg {
    /// Whether this loop seg is a miniseg.
    const fn is_mini(&self) -> bool {
        self.linedef.is_none()
    }
}

/// A `16.16` point to its `GL_SEGS` vertex word: an exact map vertex
/// keeps its plain index, anything else becomes a new fraction-precise
/// `GL_VERT` row indexed with the `bit 15` flag.
struct VertexTable {
    /// Exact `16.16` point to its vertex word.
    points: BTreeMap<(i64, i64), u16>,
    /// The new vertices, raw `16.16` pairs in lump order.
    added: Vec<(i32, i32)>,
}

impl VertexTable {
    /// Seed the table with `map`'s own vertices scaled to `16.16`.
    ///
    /// # Errors
    ///
    /// [`CyberError::LimitExceeded`] when the map holds more vertices
    /// than the plain half of a vertex word can index.
    fn new(map: &Map) -> Result<Self, CyberError> {
        if map.vertices.len() > MAX_INDEX.saturating_add(1) {
            return Err(oversized("map vertices", shown(map.vertices.len())));
        }
        let mut points = BTreeMap::new();
        for (index, vertex) in map.vertices.iter().enumerate() {
            let word = u16::try_from(index).unwrap_or(u16::MAX);
            points
                .entry((scaled(vertex.x), scaled(vertex.y)))
                .or_insert(word);
        }
        Ok(Self {
            points,
            added: Vec::new(),
        })
    }

    /// The word for `point`, appending a `GL_VERT` row on a miss.
    ///
    /// # Errors
    ///
    /// [`CyberError::LimitExceeded`] when the `GL_VERT` table is full or
    /// the point leaves the `i32` range of the vertex lump.
    fn word(&mut self, point: Point) -> Result<u16, CyberError> {
        if let Some(word) = self.points.get(&(point.x, point.y)).copied() {
            return Ok(word);
        }
        if self.added.len() > MAX_INDEX {
            return Err(oversized("GL_VERT entries", shown(self.added.len())));
        }
        let raw_x = i32::try_from(point.x).map_err(|_error| coordinate(point.x))?;
        let raw_y = i32::try_from(point.y).map_err(|_error| coordinate(point.y))?;
        let gl_index = u16::try_from(self.added.len())
            .map_err(|_error| oversized("GL_VERT entries", shown(self.added.len())))?;
        let word = gl_index | GL_VERTEX_FLAG;
        self.points.insert((point.x, point.y), word);
        self.added.push((raw_x, raw_y));
        Ok(word)
    }

    /// Rows written to `GL_VERT`.
    fn added_count(&self) -> usize {
        self.added.len()
    }
}

/// The `16.16` form of a whole-unit coordinate; the widening shift of an
/// `i32` into `i64` cannot fail.
fn scaled(value: i32) -> i64 {
    let widened = i64::from(value);
    widened.checked_shl(FRACBITS).unwrap_or(widened)
}

/// Every loop seg of every subsector, in emission order, waiting on the
/// partner pass with their endpoint pairs.
#[derive(Default)]
struct SegPool {
    /// One row per emitted seg; the partner is filled by [`Self::write`].
    rows: Vec<SegRow>,
    /// Seg indices by directed endpoint pair.
    pairs: BTreeMap<(u16, u16), Vec<usize>>,
    /// Minisegs counted for the report.
    minisegs: usize,
    /// Subsector seg counts, in order.
    ssectors: Vec<usize>,
}

/// The `GL_SEGS` and `GL_SSECT` lump bytes with their counts.
struct WrittenRecords {
    /// The `GL_SEGS` lump.
    segs: Vec<u8>,
    /// The `GL_SSECT` lump.
    ssectors: Vec<u8>,
    /// Total segs.
    seg_count: usize,
    /// Minisegs among them.
    minisegs: usize,
}

/// One finished `GL_SEGS` record.
struct SegRow {
    /// First endpoint word, flag included.
    v1: u16,
    /// Second endpoint word, flag included.
    v2: u16,
    /// Source linedef, or the miniseg sentinel.
    linedef: u16,
    /// Source side.
    side: u16,
    /// Partner seg index or the no-partner sentinel.
    partner: u16,
}

impl SegPool {
    /// Append one subsector's closed loop, resolving its endpoints.
    ///
    /// # Errors
    ///
    /// [`CyberError::LimitExceeded`] when an index leaves its field.
    fn emit(
        &mut self,
        loop_segs: Vec<GlSeg>,
        vertices: &mut VertexTable,
    ) -> Result<(), CyberError> {
        let count = emit_loop(self, loop_segs, vertices)?;
        self.ssectors.push(count);
        Ok(())
    }

    /// Pair every seg with the reversed-endpoint seg facing it, then
    /// serialize the `GL_SEGS` and `GL_SSECT` lumps.
    ///
    /// # Errors
    ///
    /// [`CyberError::LimitExceeded`] when a subsector count or first-seg
    /// index leaves its word.
    fn write(mut self) -> Result<WrittenRecords, CyberError> {
        pair_faces(&mut self.rows, &self.pairs);
        let seg_count = self.rows.len();
        let minisegs = self.minisegs;
        let mut segs = Writer::with_capacity(seg_count.saturating_mul(SEG_RECORD_LEN));
        for row in &self.rows {
            segs.put_u16(row.v1);
            segs.put_u16(row.v2);
            segs.put_u16(row.linedef);
            segs.put_u16(row.side);
            segs.put_u16(row.partner);
        }
        let mut ssectors =
            Writer::with_capacity(self.ssectors.len().saturating_mul(SSECT_RECORD_LEN));
        let mut first = 0_usize;
        for count in self.ssectors {
            ssectors.put_u16(word_of(count, "subsector seg count")?);
            ssectors.put_u16(word_of(first, "subsector first seg")?);
            first = first
                .checked_add(count)
                .ok_or_else(|| oversized("GL_SEGS entries", shown(first)))?;
        }
        Ok(WrittenRecords {
            segs: segs.into_inner(),
            ssectors: ssectors.into_inner(),
            seg_count,
            minisegs,
        })
    }
}

/// Append one closed loop to the pool, resolving endpoints and words,
/// and report how many segs it held.
///
/// # Errors
///
/// [`CyberError::LimitExceeded`] when an index leaves its field.
fn emit_loop(
    pool: &mut SegPool,
    loop_segs: Vec<GlSeg>,
    vertices: &mut VertexTable,
) -> Result<usize, CyberError> {
    let mut count = 0_usize;
    for seg in loop_segs {
        count = count.saturating_add(1);
        if pool.rows.len() > MAX_SEG_INDEX {
            return Err(oversized("GL_SEGS entries", shown(pool.rows.len())));
        }
        let v1 = vertices.word(seg.start)?;
        let v2 = vertices.word(seg.end)?;
        let linedef = line_word(pool, seg.linedef)?;
        note_pair(pool, v1, v2);
        pool.rows.push(SegRow {
            v1,
            v2,
            linedef,
            side: u16::from(seg.side),
            partner: NO_PARTNER,
        });
    }
    Ok(count)
}

/// The `linedef` word: the source index, or the miniseg sentinel (and
/// the miniseg count grows).
///
/// # Errors
///
/// [`CyberError::LimitExceeded`] for an index at or past the sentinel.
fn line_word(pool: &mut SegPool, source: Option<usize>) -> Result<u16, CyberError> {
    match source {
        None => {
            pool.minisegs = pool.minisegs.saturating_add(1);
            Ok(MINISEG_LINEDEF)
        }
        Some(index) if index <= MAX_LINEDEF_INDEX => {
            u16::try_from(index).map_err(|_error| oversized("linedef index", shown(index)))
        }
        Some(index) => Err(oversized("linedef index", shown(index))),
    }
}

/// Record the directed endpoint pair of the row about to be pushed.
fn note_pair(pool: &mut SegPool, v1: u16, v2: u16) {
    let index = pool.rows.len();
    if let Some(list) = pool.pairs.get_mut(&(v1, v2)) {
        list.push(index);
        return;
    }
    pool.pairs.insert((v1, v2), vec![index]);
}

/// Pair every seg with the rank-wise reversed-endpoint seg facing it;
/// each seg partners at most one other.
fn pair_faces(rows: &mut [SegRow], pairs: &BTreeMap<(u16, u16), Vec<usize>>) {
    let mut used: BTreeSet<usize> = BTreeSet::new();
    for key in pairs.keys().copied().collect::<Vec<(u16, u16)>>() {
        face_one(rows, pairs, &mut used, key);
    }
}

/// Pair the one directed edge `key` with its reverse, rank by rank.
fn face_one(
    rows: &mut [SegRow],
    pairs: &BTreeMap<(u16, u16), Vec<usize>>,
    used: &mut BTreeSet<usize>,
    key: (u16, u16),
) {
    let forward = pairs.get(&key).map_or_else(Vec::new, Vec::clone);
    let backward = pairs.get(&(key.1, key.0)).map_or_else(Vec::new, Vec::clone);
    for (rank, mine) in forward.iter().copied().enumerate() {
        if let Some(&theirs) = backward.get(rank) {
            link_pair(rows, used, mine, theirs);
        }
    }
}

/// Record `mine` and `theirs` as partners unless either already has one.
fn link_pair(rows: &mut [SegRow], used: &mut BTreeSet<usize>, mine: usize, theirs: usize) {
    if mine == theirs || used.contains(&mine) || used.contains(&theirs) {
        return;
    }
    used.insert(mine);
    used.insert(theirs);
    if let Some(row) = rows.get_mut(mine) {
        row.partner = u16::try_from(theirs).unwrap_or(NO_PARTNER);
    }
    if let Some(row) = rows.get_mut(theirs) {
        row.partner = u16::try_from(mine).unwrap_or(NO_PARTNER);
    }
}

/// The `u16` of a count or index, naming the field on refusal.
fn word_of(value: usize, what: &str) -> Result<u16, CyberError> {
    u16::try_from(value).map_err(|_error| oversized(what, shown(value)))
}

/// The `GL_VERT` lump: magic plus the new fraction-precise vertices.
fn write_vertices(table: &VertexTable) -> Vec<u8> {
    let mut out = Writer::with_capacity(
        MAGIC_GND2
            .len()
            .saturating_add(table.added.len().saturating_mul(VERTEX_ROW_LEN)),
    );
    out.put_bytes(MAGIC_GND2);
    for &(x, y) in &table.added {
        out.put_i32(x);
        out.put_i32(y);
    }
    out.into_inner()
}

/// The `GL_NODES` lump: one 28-byte record per interior node, root last.
fn write_nodes(tree: &BspTree) -> Result<Vec<u8>, CyberError> {
    let mut out = Writer::with_capacity(tree.nodes.len().saturating_mul(NODE_RECORD_LEN));
    for node in &tree.nodes {
        write_node(&mut out, node)?;
    }
    Ok(out.into_inner())
}

/// One node record: the partition line, both child boxes and the two
/// child words.
fn write_node(out: &mut Writer, node: &BuiltNode) -> Result<(), CyberError> {
    out.put_i16(i16_or(node.partition.x, "partition x")?);
    out.put_i16(i16_or(node.partition.y, "partition y")?);
    out.put_i16(i16_or(node.partition.dx, "partition dx")?);
    out.put_i16(i16_or(node.partition.dy, "partition dy")?);
    write_boxes(out, node)?;
    out.put_u16(child_word(node.right)?);
    out.put_u16(child_word(node.left)?);
    Ok(())
}

/// Both bounding boxes of one node, right first.
fn write_boxes(out: &mut Writer, node: &BuiltNode) -> Result<(), CyberError> {
    for bbox in [node.right_bbox, node.left_bbox] {
        out.put_i16(i16_or(bbox.top, "bbox top")?);
        out.put_i16(i16_or(bbox.bottom, "bbox bottom")?);
        out.put_i16(i16_or(bbox.left, "bbox left")?);
        out.put_i16(i16_or(bbox.right, "bbox right")?);
    }
    Ok(())
}

/// The `i16` of a whole-unit value, naming the field on refusal.
fn i16_or(value: i32, what: &str) -> Result<i16, CyberError> {
    i16::try_from(value).map_err(|_error| oversized(what, i64::from(value)))
}

/// The `GL` child word: `bit 15` marks a subsector leaf.
fn child_word(child: Child) -> Result<u16, CyberError> {
    let (index, flag) = match child {
        Child::Node(index) => (index, 0_u16),
        Child::Subsector(index) => (index, SUBSECTOR_FLAG),
    };
    if index > MAX_INDEX {
        return Err(oversized("node child", shown(index)));
    }
    Ok(u16::try_from(index).unwrap_or(u16::MAX) | flag)
}

/// The refusal for a value past a `GL` record field.
fn oversized(what: &str, value: i64) -> CyberError {
    CyberError::LimitExceeded(format!(
        "the GL node records cannot store {what} {value}: the glBSP v2 words \
         are 16 bits; shrink the map or build variant \"extended\""
    ))
}

/// The `i64` display form of a counted index.
fn shown(value: usize) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

/// The `LimitExceeded` for a `16.16` coordinate past the `i32` vertex field.
fn coordinate(value: i64) -> CyberError {
    CyberError::LimitExceeded(format!(
        "the GL vertex records cannot store the 16.16 coordinate {value}: it \
         leaves the i32 field; keep the map inside the 32767-unit box every \
         engine enforces"
    ))
}

/// The `Internal` refusal for a map whose vertices are gone.
fn no_vertices() -> CyberError {
    CyberError::Internal(
        "the GL build found a map with no vertices to bound: give the map \
         geometry before building nodes"
            .to_owned(),
    )
}

/// The `Internal` refusal for a tree reference that is not there.
fn broken(index: usize) -> CyberError {
    CyberError::Internal(format!(
        "the node tree references subsector or node {index}, which it does \
         not hold: rebuild the nodes"
    ))
}

/// The `Internal` refusal for a region the clips reduced below a polygon.
fn collapsed() -> CyberError {
    CyberError::Internal(
        "a GL subsector region collapsed below a triangle: its partition and \
         seg lines leave no area; build variant \"classic\" and adjust \
         split_cost"
            .to_owned(),
    )
}

/// The `Internal` refusal for a real seg no polygon edge could hold.
fn lost_seg(seg: usize, linedef: usize, side: u8) -> CyberError {
    CyberError::Internal(format!(
        "seg {seg} (linedef {linedef} side {side}) lies outside every edge of \
         its subsector's region: build variant \"classic\", or adjust \
         split_cost and build again"
    ))
}

#[cfg(test)]
mod tests {
    use super::push_gap;
    use super::{
        GL_VERTEX_FLAG, MAGIC_GND2, NO_PARTNER, SEG_RECORD_LEN, VertexTable, close_region, to_gl,
    };
    use crate::bsp::BspTree;
    use crate::bsp::{
        BBox, BuildOptions, BuildSeg, BuiltNode, Child, GlNodes, Partition, Point, build_tree,
    };
    use crate::wad::map::{Linedef, Map, Sector, Sidedef, Vertex};
    use alloc::collections::BTreeMap;

    /// The `16.16` scale factor.
    const UNIT: i64 = 0x1_0000_i64;
    /// The no-linedef flag of a parsed miniseg row.
    const MINI: u16 = super::MINISEG_LINEDEF;

    /// Append a vertex, returning its index.
    fn vertex(map: &mut Map, x: i64, y: i64) -> usize {
        let vx = i32::try_from(x).unwrap();
        let vy = i32::try_from(y).unwrap();
        map.vertices.push(Vertex {
            x: vx,
            y: vy,
            ..Default::default()
        });
        map.vertices.len().checked_sub(1).unwrap()
    }

    /// Append a sidedef onto `sector`, returning its index.
    fn sidedef(map: &mut Map, sector: usize) -> usize {
        map.sidedefs.push(Sidedef {
            sector,
            ..Sidedef::default()
        });
        map.sidedefs.len().checked_sub(1).unwrap()
    }

    /// Append a linedef from `ends.0` to `ends.1` with the given sides.
    fn linedef(map: &mut Map, ends: (usize, usize), sides: (Option<usize>, Option<usize>)) {
        let (v1, v2) = ends;
        let (front, back) = sides;
        map.linedefs.push(Linedef {
            v1,
            v2,
            front,
            back,
            ..Linedef::default()
        });
    }

    /// One closed ring of one-sided walls (sector 0), interior right.
    fn room(ring: &[(i64, i64)]) -> Map {
        let mut map = Map::default();
        map.sectors.push(Sector::default());
        let side_index = sidedef(&mut map, 0);
        let sides = (Some(side_index), None);
        let indices: Vec<usize> = ring.iter().map(|&(x, y)| vertex(&mut map, x, y)).collect();
        let back_to_front = indices.iter().skip(1).chain(indices.iter().take(1));
        for (&from, &to) in indices.iter().zip(back_to_front) {
            linedef(&mut map, (from, to), sides);
        }
        map
    }

    /// The single convex square room, interior to the right.
    fn square_room() -> Map {
        room(&[(0, 256), (256, 256), (256, 0), (0, 0)])
    }

    /// Two `256`-unit square rooms sharing the two-sided line `x = 256`.
    fn two_rooms() -> Map {
        let mut map = Map::default();
        map.sectors.push(Sector::default());
        map.sectors.push(Sector::default());
        let west_side = sidedef(&mut map, 0);
        let east_side = sidedef(&mut map, 1);
        let shared_front = sidedef(&mut map, 1);
        let shared_back = sidedef(&mut map, 0);
        let sw = vertex(&mut map, 0, 0);
        let mw = vertex(&mut map, 256, 0);
        let se = vertex(&mut map, 512, 0);
        let ne = vertex(&mut map, 512, 256);
        let me = vertex(&mut map, 256, 256);
        let nw = vertex(&mut map, 0, 256);
        linedef(&mut map, (sw, nw), (Some(west_side), None));
        linedef(&mut map, (nw, me), (Some(west_side), None));
        linedef(&mut map, (mw, sw), (Some(west_side), None));
        linedef(&mut map, (mw, me), (Some(shared_front), Some(shared_back)));
        linedef(&mut map, (me, ne), (Some(east_side), None));
        linedef(&mut map, (ne, se), (Some(east_side), None));
        linedef(&mut map, (se, mw), (Some(east_side), None));
        map
    }

    /// The six-vertex L-shaped room, interior to the right.
    fn l_room() -> Map {
        room(&[
            (0, 256),
            (128, 256),
            (128, 128),
            (256, 128),
            (256, 0),
            (0, 0),
        ])
    }

    /// The default-options tree of `map`.
    fn tree_of(map: &Map) -> BspTree {
        build_tree(map, BuildOptions::default()).expect("the fixture builds")
    }

    /// The GL run of `map`'s tree.
    fn run_of(map: &Map) -> GlNodes {
        let tree = tree_of(map);
        to_gl(map, &tree).expect("the fixture serializes")
    }

    /// One parsed `GL_SEGS` row.
    #[derive(Clone, Copy, Debug)]
    struct Row {
        /// First endpoint word, flag included.
        v1: u16,
        /// Second endpoint word, flag included.
        v2: u16,
        /// Linedef index or the miniseg sentinel.
        linedef: u16,
        /// Side word.
        side: u16,
        /// Partner index or the sentinel.
        partner: u16,
    }

    /// Read one `GL_SEGS` record off a cursor.
    fn read_row(reader: &mut crate::wad::bytes::Reader<'_>) -> Row {
        Row {
            v1: reader.read_u16().unwrap(),
            v2: reader.read_u16().unwrap(),
            linedef: reader.read_u16().unwrap(),
            side: reader.read_u16().unwrap(),
            partner: reader.read_u16().unwrap(),
        }
    }

    /// Parse the `GL_SEGS` lump back into rows.
    fn rows(run: &GlNodes) -> Vec<Row> {
        let mut reader = crate::wad::bytes::Reader::new(&run.segs);
        let mut out = Vec::new();
        while reader.remaining() >= SEG_RECORD_LEN {
            out.push(read_row(&mut reader));
        }
        out
    }

    /// The subsector ranges `(first, count)` of a written run.
    fn subsector_ranges(run: &GlNodes) -> Vec<(usize, usize)> {
        let mut reader = crate::wad::bytes::Reader::new(&run.ssectors);
        let mut out = Vec::new();
        while reader.remaining() >= 4 {
            let count = usize::from(reader.read_u16().unwrap());
            let first = usize::from(reader.read_u16().unwrap());
            out.push((first, count));
        }
        out
    }

    /// The word after an index, wrapping around `count`.
    /// The `16.16` point a vertex word names.
    fn endpoint(map: &Map, run: &GlNodes, word: u16) -> (i64, i64) {
        if word & GL_VERTEX_FLAG == 0 {
            let v = &map.vertices[usize::from(word)];
            let x = i64::from(v.x).checked_mul(UNIT).unwrap();
            let y = i64::from(v.y).checked_mul(UNIT).unwrap();
            return (x, y);
        }
        let row = usize::from(word & !GL_VERTEX_FLAG);
        let offset = row.checked_mul(8).unwrap();
        let mut reader = crate::wad::bytes::Reader::at(&run.vertices, MAGIC_GND2.len());
        reader
            .seek(offset.checked_add(MAGIC_GND2.len()).unwrap())
            .unwrap();
        let x = i64::from(reader.read_i32().unwrap());
        let y = i64::from(reader.read_i32().unwrap());
        (x, y)
    }

    /// Require every subsector's seg loop to close and the leaves to
    /// cover the whole pool.
    fn assert_loops_close(run: &GlNodes) {
        let all = rows(run);
        let ranges = subsector_ranges(run);
        let total: usize = ranges.iter().map(|entry| entry.1).sum();
        assert_eq!(total, all.len(), "the leaves cover every seg");
        for (first, count) in ranges {
            assert_cycle_closes(&all, first, count);
        }
    }

    /// One closed cycle: rows share endpoint words, end to start.
    fn assert_cycle_closes(all: &[Row], first: usize, count: usize) {
        assert!(count >= 3, "a region needs at least three segs");
        let window = &all[first..first.checked_add(count).unwrap()];
        let ends: Vec<u16> = window.iter().map(|row| row.v2).collect();
        let starts: Vec<u16> = window
            .iter()
            .cycle()
            .skip(1)
            .take(count)
            .map(|row| row.v1)
            .collect();
        assert_eq!(ends, starts, "subsector at seg {first} closes exactly");
    }

    /// The signed `16.16` shoelace of one loop, via its endpoints.
    fn loop_area(map: &Map, run: &GlNodes, first: usize, count: usize) -> i128 {
        let all = rows(run);
        let window = &all[first..first.checked_add(count).unwrap()];
        let corners: Vec<(i64, i64)> = window
            .iter()
            .map(|row| endpoint(map, run, row.v1))
            .collect();
        let mut sum = 0_i128;
        for pair in corners
            .iter()
            .zip(corners.iter().cycle().skip(1).take(count))
        {
            let (here, next) = pair;
            let hx = i128::from(here.0);
            let hy = i128::from(here.1);
            let nx = i128::from(next.0);
            let ny = i128::from(next.1);
            sum = sum.saturating_add(hx.saturating_mul(ny).saturating_sub(nx.saturating_mul(hy)));
        }
        sum
    }

    /// Require every loop to run clockwise (negative shoelace, y up).
    fn assert_loops_clockwise(map: &Map, run: &GlNodes) {
        for (first, count) in subsector_ranges(run) {
            assert!(
                loop_area(map, run, first, count) < 0_i128,
                "subsector at seg {first} runs clockwise"
            );
        }
    }

    /// Require the partner words to be symmetric and self-consistent.
    fn assert_partner_symmetry(rows: &[Row]) {
        let matched: Vec<(usize, usize)> = rows
            .iter()
            .enumerate()
            .filter(|pair| pair.1.partner != NO_PARTNER)
            .map(|pair| (pair.0, usize::from(pair.1.partner)))
            .collect();
        for (mine, theirs) in matched {
            let here = &rows[mine];
            let other = &rows[theirs];
            assert_eq!(
                other.partner,
                u16::try_from(mine).unwrap(),
                "partners face back"
            );
            assert_eq!(here.v1, other.v2, "partners reverse v1");
            assert_eq!(here.v2, other.v1, "partners reverse v2");
        }
    }

    /// A fixed point at whole map units.
    fn point(x: i64, y: i64) -> Point {
        Point {
            x: x.checked_mul(UNIT).unwrap(),
            y: y.checked_mul(UNIT).unwrap(),
        }
    }

    /// A seg between whole-unit points, sourced at linedef `index`.
    fn seg(index: usize, side: u8, corners: (i64, i64, i64, i64)) -> BuildSeg {
        let (x1, y1, x2, y2) = corners;
        BuildSeg {
            start: point(x1, y1),
            end: point(x2, y2),
            linedef: index,
            side,
            offset: 0,
            sector: 0,
        }
    }

    #[test]
    fn a_single_convex_room_is_its_own_closed_loop() {
        let map = square_room();
        let run = run_of(&map);
        assert_eq!(run.seg_count, 4, "the ring, nothing more");
        assert_eq!(run.minisegs, 0, "every edge is a wall");
        assert_eq!(run.new_vertices, 0, "all corners are map vertices");
        assert_eq!(run.vertices, MAGIC_GND2.to_vec(), "an empty vertex lump");
        assert!(run.nodes.is_empty(), "no partition, no GL node");
        assert_eq!(subsector_ranges(&run), vec![(0, 4)], "one closed leaf");
        assert!(
            rows(&run).iter().all(|row| row.linedef != MINI),
            "no miniseg words"
        );
        assert_loops_close(&run);
        assert_loops_clockwise(&map, &run);
    }

    #[test]
    fn two_rooms_need_no_minisegs_and_partner_their_shared_segs() {
        let map = two_rooms();
        let run = run_of(&map);
        assert_eq!(run.seg_count, 8, "four walls per room, no filler");
        assert_eq!(run.minisegs, 0, "the shared line is covered on both sides");
        assert_eq!(run.new_vertices, 0, "flush corners");
        assert_eq!(run.nodes.len(), 28, "one GL node");
        assert_eq!(
            subsector_ranges(&run),
            vec![(0, 4), (4, 4)],
            "two closed leaves"
        );
        assert_loops_close(&run);
        assert_loops_clockwise(&map, &run);
        let rows = rows(&run);
        assert_partner_symmetry(&rows);
        let shared: Vec<usize> = rows
            .iter()
            .enumerate()
            .filter(|pair| pair.1.linedef == 3)
            .map(|pair| pair.0)
            .collect();
        assert_eq!(shared.len(), 2, "front and back segs of the shared line");
        assert_eq!(
            rows[shared[0]].partner,
            u16::try_from(shared[1]).unwrap(),
            "front sees back"
        );
        assert_eq!(
            rows[shared[1]].partner,
            u16::try_from(shared[0]).unwrap(),
            "and back sees front"
        );
        let sides: Vec<u16> = shared.iter().map(|&i| rows[i].side).collect();
        assert!(
            sides.contains(&0) && sides.contains(&1),
            "the paired shared segs carry front and back sides: {sides:?}"
        );
        assert!(
            rows.iter()
                .filter(|row| row.linedef != 3)
                .all(|row| row.partner == NO_PARTNER),
            "outer walls face the void: no partner"
        );
    }

    #[test]
    fn the_l_room_grows_paired_minisegs_across_the_doorway() {
        let map = l_room();
        let tree = tree_of(&map);
        let run = to_gl(&map, &tree).expect("the L fixture serializes");
        assert_eq!(tree.segs.len(), 7, "the classic run splits the bottom wall");
        assert_eq!(
            run.seg_count, 9,
            "seven real segs plus the two doorway minisegs"
        );
        assert_eq!(run.minisegs, 2, "one per side of the uncovered partition");
        assert_eq!(run.new_vertices, 1, "the doorway corner (128, 0) is new");
        let covered: usize = subsector_ranges(&run).iter().map(|entry| entry.1).sum();
        assert_eq!(covered, 9, "the leaves hold the whole pool");
        assert_loops_close(&run);
        assert_loops_clockwise(&map, &run);
        let rows = rows(&run);
        assert_partner_symmetry(&rows);
        let minis: Vec<usize> = rows
            .iter()
            .enumerate()
            .filter(|pair| pair.1.linedef == MINI)
            .map(|pair| pair.0)
            .collect();
        assert_eq!(minis.len(), 2, "two minisegs");
        assert_eq!(
            rows[minis[0]].partner,
            u16::try_from(minis[1]).unwrap(),
            "they partner each other"
        );
        let mut corners = vec![
            endpoint(&map, &run, rows[minis[0]].v1),
            endpoint(&map, &run, rows[minis[0]].v2),
        ];
        corners.sort_unstable();
        assert_eq!(
            corners,
            vec![
                (128_i64.checked_mul(UNIT).unwrap(), 0_i64),
                (
                    128_i64.checked_mul(UNIT).unwrap(),
                    128_i64.checked_mul(UNIT).unwrap()
                ),
            ],
            "the miniseg spans the uncovered doorway stretch"
        );
    }

    #[test]
    fn a_partial_edge_gets_one_filler_miniseg() {
        // Triangle (0,0),(0,256),(256,0) clockwise; one real seg covers
        // half of the bottom edge, so a miniseg fills the other half.
        let region = vec![point(0, 0), point(0, 256), point(256, 0)];
        let loop_segs = close_region(&region, &[seg(0, 0, (256, 0, 128, 0))]).unwrap();
        assert_eq!(loop_segs.len(), 4, "three edges plus the seg split");
        assert_eq!(
            loop_segs.iter().filter(|s| s.linedef.is_none()).count(),
            3,
            "two whole edges and the bottom's other half"
        );
        let ends: Vec<Point> = loop_segs.iter().map(|item| item.end).collect();
        let starts: Vec<Point> = loop_segs
            .iter()
            .cycle()
            .skip(1)
            .take(loop_segs.len())
            .map(|item| item.start)
            .collect();
        assert_eq!(ends, starts, "the loop closes end to start");
    }

    #[test]
    fn hairline_gaps_snap_and_wide_gaps_emit_minisegs() {
        let mut cursor = point(0, 0);
        let mut segs: Vec<super::GlSeg> = Vec::new();
        let hair = Point { x: 4095, y: 0 };
        push_gap(&mut segs, &mut cursor, hair);
        assert!(
            segs.is_empty(),
            "a hairline gap at the loop head snaps away"
        );
        assert_eq!(cursor, hair, "and the cursor moves to the seg end");
        let far = point(128, 0);
        push_gap(&mut segs, &mut cursor, far);
        assert_eq!(segs.len(), 1, "half a unit of stretch earns a miniseg");
        let just_far = Point {
            x: far.x.checked_add(4097).unwrap(),
            y: 0,
        };
        push_gap(&mut segs, &mut cursor, just_far);
        assert_eq!(segs.len(), 2, "one raw unit past the snap is long again");
        let hair_again = Point {
            x: just_far.x.checked_add(4095).unwrap(),
            y: 0,
        };
        push_gap(&mut segs, &mut cursor, hair_again);
        assert_eq!(segs.len(), 2, "a hairline gap joins the last miniseg");
        assert_eq!(
            segs.last().map(|tail| tail.end),
            Some(hair_again),
            "its end moves"
        );
        segs.push(super::GlSeg {
            start: hair_again,
            end: point(64, 0),
            linedef: Some(1),
            side: 0,
        });
        cursor = point(64, 0);
        let near = Point {
            x: 64_i64.checked_mul(UNIT).unwrap().checked_sub(1).unwrap(),
            y: 0,
        };
        push_gap(&mut segs, &mut cursor, near);
        assert_eq!(
            segs.len(),
            4,
            "behind a real seg even a hairline gap must close the loop"
        );
        assert!(
            segs.last().is_some_and(|tail| tail.linedef.is_none()),
            "a filler miniseg"
        );
    }

    #[test]
    fn oversized_records_and_broken_trees_refuse() {
        let mut huge = Map::default();
        for x in 0..0x8001_i64 {
            vertex(&mut huge, x, 0);
        }
        let tree = BspTree {
            segs: Vec::new(),
            subsectors: vec![(0, 0)],
            nodes: Vec::new(),
            root: Child::Subsector(0),
            warnings: Vec::new(),
        };
        let error = to_gl(&huge, &tree).unwrap_err();
        assert_eq!(
            error.code(),
            "limit_exceeded",
            "the vertex space is 15 bits"
        );
        assert!(
            error.to_string().contains("map vertices"),
            "the field: {error}"
        );

        let mut table = VertexTable {
            points: BTreeMap::new(),
            added: vec![(0_i32, 0_i32); 0x8000_usize],
        };
        let full = table.word(Point { x: 1, y: 1 }).unwrap_err();
        assert_eq!(full.code(), "limit_exceeded", "GL_VERT fills at 0x8000");

        let empty = Map::default();
        let naked = to_gl(&empty, &tree).unwrap_err();
        assert_eq!(naked.code(), "internal", "no vertices to bound");
        assert!(
            naked.to_string().contains("no vertices"),
            "the hint: {naked}"
        );

        let dangling = BspTree {
            segs: Vec::new(),
            subsectors: vec![(0, 0)],
            nodes: Vec::new(),
            root: Child::Subsector(7),
            warnings: Vec::new(),
        };
        let broken = to_gl(&square_room(), &dangling).unwrap_err();
        assert_eq!(broken.code(), "internal", "a leaf that is not there");
        assert!(broken.to_string().contains('7'), "the index: {broken}");
    }

    #[test]
    fn a_seg_outside_its_region_is_reported_not_dropped() {
        // A real seg parallel to, but away from, every edge of the
        // region cannot bound it: the builder's convexity rule excludes
        // this, and the writer says so rather than losing the seg.
        let region = vec![point(0, 0), point(0, 256), point(256, 0)];
        let stray = seg(9, 1, (128, 128, 64, 128));
        let error = close_region(&region, core::slice::from_ref(&stray)).unwrap_err();
        assert_eq!(error.code(), "internal", "unplaceable seg");
        assert!(
            error.to_string().contains("split_cost"),
            "the message names the fix: {error}"
        );
    }

    #[test]
    fn node_records_keep_the_vanilla_layout_and_flags() {
        let map = two_rooms();
        let run = run_of(&map);
        assert_eq!(run.nodes.len(), 28, "one 28-byte record");
        let fields: Vec<i16> = run
            .nodes
            .chunks_exact(2)
            .map(|pair| i16::from_le_bytes(pair[..2].try_into().unwrap()))
            .collect();
        assert_eq!(fields.first().copied().unwrap(), 256_i16, "partition x");
        let children: Vec<u16> = run
            .nodes
            .chunks_exact(2)
            .skip(12)
            .map(|pair| u16::from_le_bytes(pair[..2].try_into().unwrap()))
            .collect();
        assert_eq!(
            children,
            vec![0x8000_u16, 0x8001_u16],
            "both children are leaves, flagged"
        );
        assert!(run.pvs.is_empty(), "the GL_PVS lump stays empty");
    }

    #[test]
    fn an_oversized_child_word_refuses() {
        let tree = BspTree {
            segs: Vec::new(),
            subsectors: vec![(0, 0)],
            nodes: vec![BuiltNode {
                partition: Partition::default(),
                right_bbox: BBox::default(),
                left_bbox: BBox::default(),
                right: Child::Node(0x8000_usize),
                left: Child::Subsector(0),
            }],
            root: Child::Node(0),
            warnings: Vec::new(),
        };
        let error = super::child_word(Child::Node(0x8000_usize)).unwrap_err();
        assert_eq!(
            error.code(),
            "limit_exceeded",
            "children share the flag bit"
        );
        assert!(
            error.to_string().contains("node child"),
            "the field: {error}"
        );
        let reached = to_gl(&square_room(), &tree).unwrap_err();
        assert_eq!(
            reached.code(),
            "internal",
            "the walk stops at the missing node"
        );
    }
}
