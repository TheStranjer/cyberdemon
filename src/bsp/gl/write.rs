//! The `GL_*` record writers: the vertex table that turns `16.16` points
//! into `GL_SEGS` vertex words, the seg pool every closed loop is appended
//! to, the partner pass that faces the segs across a partition and the
//! three lump serializers.
//!
//! [`VertexTable`] gives an exact map vertex its plain index and every
//! other point a fraction-precise `GL_VERT` row; [`SegPool`] keeps the
//! [`GlSeg`](super::regions::GlSeg) loops as [`SegRow`] records until
//! [`pair_faces`] links each one to the reversed-endpoint seg facing it.
//! [`write_vertices`] renders `GL_VERT`, [`SegPool::write`] the `GL_SEGS`
//! and `GL_SSECT` lumps and [`write_nodes`] `GL_NODES`, one
//! [`write_node`] and [`write_boxes`] per interior node. Every refusal
//! names the record field that overflowed, on the words
//! [`oversized`](super::oversized) builds.

use alloc::collections::{BTreeMap, BTreeSet};

use crate::bsp::gl::regions::GlSeg;
use crate::bsp::gl::{
    GL_VERTEX_FLAG, MAGIC_GND2, MAX_INDEX, MAX_LINEDEF_INDEX, MAX_SEG_INDEX, MINISEG_LINEDEF,
    NO_PARTNER, NODE_RECORD_LEN, SEG_RECORD_LEN, SSECT_RECORD_LEN, SUBSECTOR_FLAG, VERTEX_ROW_LEN,
    coordinate, oversized, shown,
};
use crate::bsp::{BspTree, BuiltNode, Child, Point};
use crate::error::CyberError;
use crate::fixed::FRACBITS;
use crate::wad::bytes::Writer;
use crate::wad::map::Map;

/// A `16.16` point to its `GL_SEGS` vertex word: an exact map vertex
/// keeps its plain index, anything else becomes a new fraction-precise
/// `GL_VERT` row indexed with the `bit 15` flag.
pub(super) struct VertexTable {
    /// Exact `16.16` point to its vertex word.
    pub(super) points: BTreeMap<(i64, i64), u16>,
    /// The new vertices, raw `16.16` pairs in lump order.
    pub(super) added: Vec<(i32, i32)>,
}

impl VertexTable {
    /// Seed the table with `map`'s own vertices scaled to `16.16`.
    ///
    /// # Errors
    ///
    /// [`CyberError::LimitExceeded`] when the map holds more vertices
    /// than the plain half of a vertex word can index.
    pub(super) fn new(map: &Map) -> Result<Self, CyberError> {
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
    pub(super) fn word(&mut self, point: Point) -> Result<u16, CyberError> {
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
    pub(super) fn added_count(&self) -> usize {
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
pub(super) struct SegPool {
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
pub(super) struct WrittenRecords {
    /// The `GL_SEGS` lump.
    pub(super) segs: Vec<u8>,
    /// The `GL_SSECT` lump.
    pub(super) ssectors: Vec<u8>,
    /// Total segs.
    pub(super) seg_count: usize,
    /// Minisegs among them.
    pub(super) minisegs: usize,
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
    pub(super) fn emit(
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
    pub(super) fn write(mut self) -> Result<WrittenRecords, CyberError> {
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
pub(super) fn write_vertices(table: &VertexTable) -> Vec<u8> {
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
pub(super) fn write_nodes(tree: &BspTree) -> Result<Vec<u8>, CyberError> {
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
pub(super) fn child_word(child: Child) -> Result<u16, CyberError> {
    let (index, flag) = match child {
        Child::Node(index) => (index, 0_u16),
        Child::Subsector(index) => (index, SUBSECTOR_FLAG),
    };
    if index > MAX_INDEX {
        return Err(oversized("node child", shown(index)));
    }
    Ok(u16::try_from(index).unwrap_or(u16::MAX) | flag)
}
