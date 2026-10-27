//! The `GL_*` record writers: the vertex table that turns `16.16` points
//! into `GL_SEGS` vertex words, the seg pool every closed loop is appended
//! to, the partner pass that faces the segs across a partition and the
//! three lump serializers.
//!
//! [`VertexWords`] gives an exact map vertex its plain index and every
//! other point a fraction-precise `GL_VERT` row; its dedupe and index
//! bookkeeping are the shared [`crate::bsp::records::VertexTable`], its own
//! part the `bit 15` vertex word. [`SegPool`] keeps the
//! [`GlSeg`](super::regions::GlSeg) loops as [`SegRow`] records until
//! [`pair_faces`] links each one to the reversed-endpoint seg facing it.
//! [`write_vertices`] renders `GL_VERT`, [`SegPool::write`] the `GL_SEGS`
//! and `GL_SSECT` lumps and [`write_nodes`] `GL_NODES` on the one shared
//! node record writer. Every refusal names the record field that
//! overflowed, on the words [`Refusals::GL`](crate::bsp::records::Refusals::GL)
//! builds.

use alloc::collections::{BTreeMap, BTreeSet};

use crate::bsp::gl::regions::GlSeg;
use crate::bsp::gl::{
    GL_VERTEX_FLAG, MAGIC_GND2, MAX_INDEX, MAX_LINEDEF_INDEX, MAX_SEG_INDEX, MINISEG_LINEDEF,
    NO_PARTNER, NODE_RECORD_LEN, SEG_RECORD_LEN, SSECT_RECORD_LEN, VERTEX_ROW_LEN,
};
use crate::bsp::records::{self, ChildWidth, Refusals, Spot, VertexTable};
use crate::bsp::{BspTree, Point};
use crate::error::CyberError;
use crate::wad::bytes::Writer;
use crate::wad::map::Map;

/// The words every `GL` refusal answers with.
const REFUSALS: Refusals = Refusals::GL;

/// The shared endpoint table over the exact `16.16` point, whose rows are
/// the `GL_VERT` lump's own raw `16.16` pairs.
type Endpoints = VertexTable<(i64, i64), (i32, i32)>;

/// A `16.16` point to its `GL_SEGS` vertex word: an exact map vertex
/// keeps its plain index, anything else becomes a new fraction-precise
/// `GL_VERT` row indexed with the `bit 15` flag.
pub(super) struct VertexWords {
    /// Total vertex index per exact point, with the `GL_VERT` rows under it.
    table: Endpoints,
}

impl VertexWords {
    /// Seed the table with `map`'s own vertices scaled to `16.16`.
    ///
    /// # Errors
    ///
    /// [`CyberError::LimitExceeded`] when the map holds more vertices
    /// than the plain half of a vertex word can index.
    pub(super) fn new(map: &Map) -> Result<Self, CyberError> {
        if map.vertices.len() > MAX_INDEX.saturating_add(1) {
            return Err(REFUSALS.oversized("map vertices", records::shown(map.vertices.len())));
        }
        Ok(Self {
            table: VertexTable::seeded(
                map.vertices.len(),
                map.vertices
                    .iter()
                    .map(|vertex| (records::scaled(vertex.x), records::scaled(vertex.y))),
            ),
        })
    }

    /// A table of `added` fraction-precise rows over a map with none of its
    /// own, for the full-`GL_VERT` test no real map can supply.
    #[cfg(test)]
    pub(super) fn over(added: usize) -> Self {
        let mut table = Self {
            table: VertexTable::seeded(0_usize, []),
        };
        for index in 0..added {
            let key = (i64::try_from(index).unwrap(), 0_i64);
            table.table.take(key, (0_i32, 0_i32));
        }
        table
    }

    /// The word for `point`, appending a `GL_VERT` row on a miss.
    ///
    /// # Errors
    ///
    /// [`CyberError::LimitExceeded`] when the `GL_VERT` table is full or
    /// the point leaves the `i32` range of the vertex lump.
    pub(super) fn word(&mut self, point: Point) -> Result<u16, CyberError> {
        let key = (point.x, point.y);
        let total = match self.table.spot(&key) {
            Spot::Known(index) => index,
            Spot::Open { added, .. } => self.append(key, point, added)?,
        };
        Ok(flagged(total, self.table.originals()))
    }

    /// The total index of a new `GL_VERT` row for `point` at append index
    /// `added`, refusing a full table or a coordinate past `i32`.
    ///
    /// # Errors
    ///
    /// [`CyberError::LimitExceeded`] when `GL_VERT` is full or the point
    /// leaves the `i32` range.
    fn append(&mut self, key: (i64, i64), point: Point, added: usize) -> Result<usize, CyberError> {
        if added > MAX_INDEX {
            return Err(REFUSALS.oversized("GL_VERT entries", records::shown(added)));
        }
        let row = (
            i32::try_from(point.x).map_err(|_error| REFUSALS.coordinate(point.x))?,
            i32::try_from(point.y).map_err(|_error| REFUSALS.coordinate(point.y))?,
        );
        Ok(self.table.take(key, row))
    }

    /// Rows written to `GL_VERT`.
    pub(super) fn added_count(&self) -> usize {
        self.table.added()
    }

    /// The `GL_VERT` rows, in lump order.
    pub(super) fn rows(&self) -> &[(i32, i32)] {
        self.table.rows()
    }
}

/// The `GL_SEGS` vertex word of a total vertex index: one of the map's own
/// vertices keeps its plain index, an appended row is numbered from zero
/// among themselves and flagged as a `GL_VERT` lookup.
fn flagged(total: usize, originals: usize) -> u16 {
    let (index, from_lump) = total
        .checked_sub(originals)
        .map_or((total, false), |row| (row, true));
    // Both tables are capped at `MAX_INDEX` before a row is taken, so the
    // index always fits the plain half of the word; the clamp only stands
    // in for a cast Clippy forbids.
    let word = u16::try_from(index).unwrap_or(u16::MAX);
    if from_lump {
        word | GL_VERTEX_FLAG
    } else {
        word
    }
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
        vertices: &mut VertexWords,
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
            ssectors.put_u16(REFUSALS.u16(count, "subsector seg count")?);
            ssectors.put_u16(REFUSALS.u16(first, "subsector first seg")?);
            first = first
                .checked_add(count)
                .ok_or_else(|| REFUSALS.oversized("GL_SEGS entries", records::shown(first)))?;
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
    vertices: &mut VertexWords,
) -> Result<usize, CyberError> {
    let mut count = 0_usize;
    for seg in loop_segs {
        count = count.saturating_add(1);
        if pool.rows.len() > MAX_SEG_INDEX {
            return Err(REFUSALS.oversized("GL_SEGS entries", records::shown(pool.rows.len())));
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
        Some(index) if index <= MAX_LINEDEF_INDEX => u16::try_from(index)
            .map_err(|_error| REFUSALS.oversized("linedef index", records::shown(index))),
        Some(index) => Err(REFUSALS.oversized("linedef index", records::shown(index))),
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

/// The `GL_VERT` lump: magic plus the new fraction-precise vertices.
pub(super) fn write_vertices(table: &VertexWords) -> Vec<u8> {
    let mut out = Writer::with_capacity(
        MAGIC_GND2
            .len()
            .saturating_add(table.added_count().saturating_mul(VERTEX_ROW_LEN)),
    );
    out.put_bytes(MAGIC_GND2);
    records::write_rows(&mut out, table.rows());
    out.into_inner()
}

/// The `GL_NODES` lump: one 28-byte record per interior node, root last.
pub(super) fn write_nodes(tree: &BspTree) -> Result<Vec<u8>, CyberError> {
    let mut out = Writer::with_capacity(tree.nodes.len().saturating_mul(NODE_RECORD_LEN));
    records::write_node_records(&mut out, tree, ChildWidth::Bits16, REFUSALS)?;
    Ok(out.into_inner())
}
