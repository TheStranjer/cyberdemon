//! The node record machinery the three `BSP` serializations share.
//!
//! Vanilla `NODES`, the `ZDoom` extended lump and `GL_NODES` write the same
//! node record — the partition line, the two child boxes, then the two
//! child words — and differ only in how wide those child words are (`u16`
//! with the subsector flag on `bit 15` for the classic and `GL` layouts,
//! `u32` with the flag on `bit 31` for the extended one) and in the words
//! their refusals answer with. All three also build the same vertex table:
//! the map's own vertices first, then one row per split point the seg
//! endpoints need, deduplicated by whatever key the format matches on.
//! This module holds that shared part: [`VertexTable`] with its [`Spot`],
//! [`write_node`] and [`write_node_records`] over [`write_box`] and
//! [`ChildWidth`], the [`Refusals`] makers, and [`scaled`] with
//! [`write_rows`] for `16.16` vertex rows. Each format keeps its own index
//! space, its own row type and its own lump framing on top of them.

use alloc::collections::BTreeMap;

use crate::bsp::{BBox, BspTree, BuiltNode, Child};
use crate::error::CyberError;
use crate::fixed::FRACBITS;
use crate::wad::bytes::Writer;

/// Highest node index a 16-bit child word holds below its flag bit.
const LAST_INDEX_16: u64 = 0x7FFF_u64;
/// Highest node index a 32-bit child word holds below its flag bit.
const LAST_INDEX_32: u64 = 0x7FFF_FFFF_u64;
/// Subsector flag of a 16-bit child word (`bit 15`).
const SUBSECTOR_FLAG_16: u32 = 0x8000_u32;
/// Subsector flag of a 32-bit child word (`bit 31`).
const SUBSECTOR_FLAG_32: u32 = 0x8000_0000_u32;

/// The `16.16` form of a whole-unit coordinate; the widening shift of an
/// `i32` into `i64` cannot fail.
#[must_use]
#[inline]
pub(super) fn scaled(value: i32) -> i64 {
    let widened = i64::from(value);
    widened.checked_shl(FRACBITS).unwrap_or(widened)
}

/// The `LimitExceeded` wording of one node layout: the three records say
/// the same thing about the same field and differ only in which layout is
/// named and which fix they point at.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Refusals {
    /// The layout every message names: `vanilla`, `extended`, `GL`.
    layout: &'static str,
    /// The clause after "cannot store {what} {value}: ".
    field: &'static str,
    /// The clause after "do not fit" in a child index refusal.
    children: &'static str,
}

impl Refusals {
    /// The words of the classic `SEGS`/`SSECTORS`/`NODES` records.
    pub(super) const VANILLA: Self = Self {
        layout: "vanilla",
        field: "it leaves the i16/u16 field range; use variant \"extended\"",
        children: "; use variant \"extended\"",
    };

    /// The words of the `ZDoom` extended lump's records.
    pub(super) const EXTENDED: Self = Self {
        layout: "extended",
        field: "it leaves the field range; no node layout holds wider fields, so keep \
                the map inside the 32767-unit box every engine enforces",
        children: " below the subsector flag",
    };

    /// The words of the `glBSP` v2 records.
    pub(super) const GL: Self = Self {
        layout: "GL",
        field: "the glBSP v2 words are 16 bits; shrink the map or build variant \
                \"extended\"",
        children: "; shrink the map or build variant \"extended\"",
    };

    /// The shared `LimitExceeded` for a value past one record field.
    pub(super) fn oversized(self, what: &str, value: i64) -> CyberError {
        CyberError::LimitExceeded(format!(
            "the {} node records cannot store {what} {value}: {}",
            self.layout, self.field
        ))
    }

    /// The shared `LimitExceeded` for a `16.16` coordinate past the `i32`
    /// field of a vertex row.
    pub(super) fn coordinate(self, value: i64) -> CyberError {
        CyberError::LimitExceeded(format!(
            "the {} vertex records cannot store the 16.16 coordinate {value}: it \
             leaves the i32 field; keep the map inside the 32767-unit box every \
             engine enforces",
            self.layout
        ))
    }

    /// The shared `LimitExceeded` for a child index the layout's words
    /// cannot reach below their leaf flag.
    fn too_many_children(self, width: ChildWidth, what: &str, index: usize) -> CyberError {
        let addressable = width.last_index().saturating_add(1);
        CyberError::LimitExceeded(format!(
            "the {} child word cannot index {what} {index}: more than {addressable} of \
             them do not fit{}",
            self.layout, self.children
        ))
    }

    /// The `i16` form of a whole value, naming the field on refusal.
    pub(super) fn i16(self, value: i64, what: &str) -> Result<i16, CyberError> {
        i16::try_from(value).map_err(|_error| self.oversized(what, value))
    }

    /// The `u16` form of a count or index, naming what it counts on refusal.
    pub(super) fn u16(self, value: usize, what: &str) -> Result<u16, CyberError> {
        u16::try_from(value).map_err(|_error| self.oversized(what, shown(value)))
    }

    /// The `u32` form of a count or index, naming what it counts on refusal.
    pub(super) fn u32(self, value: usize, what: &str) -> Result<u32, CyberError> {
        u32::try_from(value).map_err(|_error| self.oversized(what, shown(value)))
    }
}

/// The `i64` display form of a counted index.
#[must_use]
#[inline]
pub(super) fn shown(value: usize) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

/// The width of one node layout's child words.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ChildWidth {
    /// The classic `NODES` and `GL_NODES` words: 16 bits, `bit 15` marking
    /// a subsector leaf.
    Bits16,
    /// The extended lump's words: 32 bits, `bit 31` marking a leaf.
    Bits32,
}

impl ChildWidth {
    /// The highest node or subsector index these words address below
    /// their flag bit.
    #[must_use]
    const fn last_index(self) -> u64 {
        match self {
            Self::Bits16 => LAST_INDEX_16,
            Self::Bits32 => LAST_INDEX_32,
        }
    }

    /// The subsector flag of these words.
    #[must_use]
    const fn flag(self) -> u32 {
        match self {
            Self::Bits16 => SUBSECTOR_FLAG_16,
            Self::Bits32 => SUBSECTOR_FLAG_32,
        }
    }

    /// The flag `child` carries: its leaf flag for a subsector, nothing for
    /// a node.
    #[must_use]
    const fn leaf_flag(self, child: Child) -> u32 {
        match child {
            Child::Node(_index) => 0_u32,
            Child::Subsector(_index) => self.flag(),
        }
    }

    /// The child word of `child`: its index, with the leaf flag set for a
    /// subsector and refused past the highest index these words reach.
    pub(super) fn word(self, child: Child, refusals: Refusals) -> Result<u32, CyberError> {
        let (index, what) = match child {
            Child::Node(node) => (node, "node"),
            Child::Subsector(subsector) => (subsector, "subsector"),
        };
        // A wider-than-words index clamps to `u64::MAX`, itself past the
        // child limit, so it is refused exactly like a merely large one.
        if u64::try_from(index).unwrap_or(u64::MAX) > self.last_index() {
            return Err(refusals.too_many_children(self, what, index));
        }
        // The limit above keeps the index inside `u32`; the clamp only
        // stands in for a cast Clippy forbids.
        let word = u32::try_from(index).unwrap_or(u32::MAX);
        Ok(word | self.leaf_flag(child))
    }

    /// Appends `word` as one child of this width.
    fn write(self, out: &mut Writer, word: u32) {
        match self {
            // [`Self::word`] kept the index below `bit 15`, so the flag
            // rides inside the plain half of the word.
            Self::Bits16 => out.put_u16(u16::try_from(word).unwrap_or(u16::MAX)),
            Self::Bits32 => out.put_u32(word),
        }
    }

    /// Appends both children of `node`, right first, as this width's two
    /// child words.
    fn write_pair(
        self,
        out: &mut Writer,
        node: &BuiltNode,
        refusals: Refusals,
    ) -> Result<(), CyberError> {
        for child in [node.right, node.left] {
            let word = self.word(child, refusals)?;
            self.write(out, word);
        }
        Ok(())
    }
}

/// One bounding box: `top`, `bottom`, `left`, `right` as `i16`.
fn write_box(out: &mut Writer, bbox: BBox, refusals: Refusals) -> Result<(), CyberError> {
    for (value, what) in [
        (bbox.top, "bbox top"),
        (bbox.bottom, "bbox bottom"),
        (bbox.left, "bbox left"),
        (bbox.right, "bbox right"),
    ] {
        out.put_i16(refusals.i16(i64::from(value), what)?);
    }
    Ok(())
}

/// One node record: the partition line, both child boxes and the two child
/// words of `width`.
pub(super) fn write_node(
    out: &mut Writer,
    node: &BuiltNode,
    width: ChildWidth,
    refusals: Refusals,
) -> Result<(), CyberError> {
    for (value, what) in [
        (node.partition.x, "partition x"),
        (node.partition.y, "partition y"),
        (node.partition.dx, "partition dx"),
        (node.partition.dy, "partition dy"),
    ] {
        out.put_i16(refusals.i16(i64::from(value), what)?);
    }
    write_box(out, node.right_bbox, refusals)?;
    write_box(out, node.left_bbox, refusals)?;
    width.write_pair(out, node, refusals)
}

/// Every interior node of `tree`, children before parents: the order
/// [`build_tree`](crate::bsp::build_tree) already guarantees, which makes
/// the root the final record and writes nothing for a tree whose root is a
/// subsector.
pub(super) fn write_node_records(
    out: &mut Writer,
    tree: &BspTree,
    width: ChildWidth,
    refusals: Refusals,
) -> Result<(), CyberError> {
    for node in &tree.nodes {
        write_node(out, node, width, refusals)?;
    }
    Ok(())
}

/// The fraction-precise vertex rows of the extended and `GL` lumps: each
/// `16.16` point as a raw `i32` pair, in lump order.
pub(super) fn write_rows(out: &mut Writer, rows: &[(i32, i32)]) {
    for &(x, y) in rows {
        out.put_i32(x);
        out.put_i32(y);
    }
}

/// Where a key stands in a [`VertexTable`].
pub(super) enum Spot {
    /// The total index of a vertex the table already holds.
    Known(usize),
    /// The key is new: `total` is the index its row takes in the whole
    /// vertex space, `added` the index among the rows this build appends.
    Open {
        /// The index after the map's own vertices and every earlier row.
        total: usize,
        /// The index of the row about to be appended.
        added: usize,
    },
}

/// The vertex table every node layout builds: the map's own vertices
/// first, then one row per split point its seg endpoints need, deduplicated
/// by `K` — whole-unit points for the vanilla layout, which rounds every
/// endpoint, and exact `16.16` points for the extended and `GL` lumps,
/// which keep them at full precision. The table hands out the indices and
/// keeps the appended rows in the layout's own row type `V`; the format
/// that runs out of its index space refuses, because only it knows its
/// vertex space and what passing it means.
pub(super) struct VertexTable<K, V> {
    /// Key to total vertex index; the map's own vertices win ties.
    keys: BTreeMap<K, usize>,
    /// How many vertices the map already holds.
    originals: usize,
    /// The rows this build appended, in append order.
    rows: Vec<V>,
}

impl<K: Ord, V> VertexTable<K, V> {
    /// A table over a map of `originals` vertices, seeded with one key per
    /// map vertex in index order; of a duplicated point the first index
    /// wins.
    pub(super) fn seeded<Keys: IntoIterator<Item = K>>(originals: usize, keys: Keys) -> Self {
        let mut table = Self {
            keys: BTreeMap::new(),
            originals,
            rows: Vec::new(),
        };
        for (index, key) in keys.into_iter().enumerate() {
            table.keys.entry(key).or_insert(index);
        }
        table
    }

    /// Where `key` stands: on a vertex the table already holds, or at the
    /// free pair of indices [`Self::take`] fills.
    pub(super) fn spot(&self, key: &K) -> Spot {
        if let Some(total) = self.keys.get(key).copied() {
            return Spot::Known(total);
        }
        let added = self.rows.len();
        Spot::Open {
            total: self.originals.saturating_add(added),
            added,
        }
    }

    /// Stores `row` under `key` and reports its total index. Only a key
    /// [`Self::spot`] left [`Spot::Open`] is ever taken: a caller that
    /// refuses in between abandons the whole run, so the row it skipped
    /// costs nothing.
    pub(super) fn take(&mut self, key: K, row: V) -> usize {
        let total = self.originals.saturating_add(self.rows.len());
        self.keys.entry(key).or_insert(total);
        self.rows.push(row);
        total
    }

    /// How many vertices the map already held.
    #[must_use]
    pub(super) const fn originals(&self) -> usize {
        self.originals
    }

    /// The appended rows, in lump order.
    #[must_use]
    pub(super) fn rows(&self) -> &[V] {
        &self.rows
    }

    /// How many rows this build appended.
    #[must_use]
    pub(super) fn added(&self) -> usize {
        self.rows.len()
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ChildWidth, Refusals, Spot, VertexTable, scaled, shown, write_node, write_node_records,
        write_rows,
    };
    use crate::bsp::{BBox, BspTree, BuiltNode, Child, Partition};
    use crate::wad::bytes::{Reader, Writer};

    /// The `16.16` scale factor.
    const UNIT: i64 = 0x1_0000_i64;

    /// The shared table over exact `16.16` points and raw `i32` pairs.
    type Pairs = VertexTable<(i64, i64), (i32, i32)>;

    /// The 12 `i16` words in front of one layout's child words, as the
    /// record's field values.
    fn node(right: Child, left: Child) -> BuiltNode {
        BuiltNode {
            partition: Partition {
                x: 4,
                y: 6,
                dx: 8,
                dy: 10,
            },
            right_bbox: BBox {
                top: 12,
                bottom: 14,
                left: 16,
                right: 18,
            },
            left_bbox: BBox {
                top: 20,
                bottom: 22,
                left: 24,
                right: 26,
            },
            right,
            left,
        }
    }

    /// The `i16` words of a written record, in field order.
    fn i16_words(bytes: &[u8]) -> Vec<i16> {
        bytes
            .chunks_exact(2)
            .map(|pair| i16::from_le_bytes(pair.try_into().unwrap()))
            .collect()
    }

    /// One node record of each width, its children read back as words.
    fn written(width: ChildWidth, refusals: Refusals) -> (Vec<i16>, Vec<u32>) {
        let tree = BspTree {
            segs: Vec::new(),
            subsectors: Vec::new(),
            nodes: vec![node(Child::Node(3), Child::Subsector(5))],
            root: Child::Node(0),
            warnings: Vec::new(),
        };
        let mut out = Writer::new();
        write_node_records(&mut out, &tree, width, refusals).unwrap();
        let bytes = out.into_inner();
        let fields = i16_words(bytes.get(..24).unwrap_or_default());
        let mut reader = Reader::new(bytes.get(24..).unwrap_or_default());
        let children = match width {
            ChildWidth::Bits16 => vec![
                u32::from(reader.read_u16().unwrap()),
                u32::from(reader.read_u16().unwrap()),
            ],
            ChildWidth::Bits32 => vec![reader.read_u32().unwrap(), reader.read_u32().unwrap()],
        };
        (fields, children)
    }

    #[test]
    fn scaled_shifts_a_whole_unit_into_16_16() {
        assert_eq!(scaled(1), UNIT, "one unit");
        assert_eq!(scaled(256), 256_i64.checked_mul(UNIT).unwrap(), "a room");
        assert_eq!(scaled(-3), (-3_i64).checked_mul(UNIT).unwrap(), "negative");
        assert_eq!(scaled(0), 0_i64, "zero stays zero");
    }

    #[test]
    fn shown_widens_a_counted_index_for_a_message() {
        assert_eq!(shown(7_usize), 7_i64, "an ordinary count");
        assert_eq!(shown(usize::MAX), i64::MAX, "and the widest one");
    }

    #[test]
    fn both_child_widths_flag_a_leaf_and_leave_a_node_plain() {
        assert_eq!(
            ChildWidth::Bits16
                .word(Child::Node(3), Refusals::VANILLA)
                .unwrap(),
            3_u32,
            "a vanilla node child"
        );
        assert_eq!(
            ChildWidth::Bits16
                .word(Child::Subsector(3), Refusals::VANILLA)
                .unwrap(),
            0x8003_u32,
            "the vanilla leaf flag rides on bit 15"
        );
        assert_eq!(
            ChildWidth::Bits32
                .word(Child::Subsector(3), Refusals::EXTENDED)
                .unwrap(),
            0x8000_0003_u32,
            "the extended leaf flag rides on bit 31"
        );
        assert_eq!(
            ChildWidth::Bits32
                .word(Child::Node(0x7FFF_FFFF), Refusals::EXTENDED)
                .unwrap(),
            0x7FFF_FFFF_u32,
            "the last extended index still fits"
        );
    }

    #[test]
    fn a_child_past_its_width_refuses_naming_the_index_and_fix() {
        let vanilla = ChildWidth::Bits16
            .word(Child::Subsector(0x8000), Refusals::VANILLA)
            .unwrap_err();
        assert_eq!(vanilla.code(), "limit_exceeded", "vanilla leaf flag");
        assert!(
            vanilla.to_string().contains("subsector 32768"),
            "the child is named: {vanilla}"
        );
        assert!(
            vanilla.to_string().contains("extended"),
            "the fix is named: {vanilla}"
        );
        let extended = ChildWidth::Bits32
            .word(Child::Subsector(0x8000_0000), Refusals::EXTENDED)
            .unwrap_err();
        assert_eq!(extended.code(), "limit_exceeded", "extended leaf flag");
        assert!(
            extended
                .to_string()
                .contains("more than 2147483648 of them do not fit below the subsector flag"),
            "the extended wording: {extended}"
        );
        let wide = ChildWidth::Bits16
            .word(Child::Node(usize::MAX), Refusals::GL)
            .unwrap_err();
        assert!(
            wide.to_string().contains(&usize::MAX.to_string()),
            "a wider-than-words index is refused too: {wide}"
        );
    }

    #[test]
    fn one_node_record_holds_the_partition_boxes_and_children() {
        let (fields, children) = written(ChildWidth::Bits16, Refusals::VANILLA);
        assert_eq!(
            fields,
            vec![
                4_i16, 6, 8, 10, // the partition line
                12, 14, 16, 18, // the right box, top first
                20, 22, 24, 26, // the left box
            ],
            "the shared field order"
        );
        assert_eq!(
            children,
            vec![3_u32, 0x8005_u32],
            "one node child, one flagged leaf"
        );
        let (_, wide) = written(ChildWidth::Bits32, Refusals::EXTENDED);
        assert_eq!(wide, vec![3_u32, 0x8000_0005_u32], "the same pair, 32 bits");
    }

    #[test]
    fn a_node_record_refuses_naming_the_field_that_overflowed() {
        let mut broken = node(Child::Subsector(0), Child::Subsector(1));
        broken.partition.dy = 40_000_i32;
        let mut out = Writer::new();
        let note = write_node(&mut out, &broken, ChildWidth::Bits16, Refusals::VANILLA)
            .unwrap_err()
            .to_string();
        assert!(note.contains("partition dy"), "the line is named: {note}");
        broken.partition = Partition::default();
        broken.left_bbox.left = -40_000_i32;
        let wide = write_node(&mut out, &broken, ChildWidth::Bits32, Refusals::EXTENDED)
            .unwrap_err()
            .to_string();
        assert!(wide.contains("bbox left"), "the box is named: {wide}");
    }

    #[test]
    fn each_layout_words_an_oversized_field_its_own_way() {
        let record = Refusals::GL.i16(40_000, "bbox right").unwrap_err();
        assert_eq!(record.code(), "limit_exceeded", "a GL box field");
        assert!(
            record
                .to_string()
                .contains("the GL node records cannot store bbox right 40000"),
            "the GL wording: {record}"
        );
        let classic = Refusals::VANILLA
            .i16(40_000, "bbox right")
            .unwrap_err()
            .to_string();
        assert!(
            classic.contains("the vanilla node records cannot store bbox right 40000")
                && classic.contains("i16/u16 field range"),
            "the vanilla wording: {classic}"
        );
    }

    #[test]
    fn a_narrowed_count_names_what_it_counts() {
        assert_eq!(
            Refusals::VANILLA.u16(7, "seg count").unwrap(),
            7_u16,
            "fits"
        );
        let wide = Refusals::VANILLA
            .u16(0x1_0000, "linedef index")
            .unwrap_err();
        assert!(
            wide.to_string()
                .contains("cannot store linedef index 65536"),
            "the vanilla wording: {wide}"
        );
        assert_eq!(
            Refusals::EXTENDED.u32(7, "node count").unwrap(),
            7_u32,
            "a u32 count"
        );
        let huge = Refusals::EXTENDED.u32(usize::MAX, "seg count").unwrap_err();
        assert!(
            huge.to_string().contains("cannot store seg count"),
            "the count is named: {huge}"
        );
        let vertex = Refusals::GL.coordinate(0x8000_0000_0000_i64);
        assert_eq!(vertex.code(), "limit_exceeded", "a 16.16 coordinate");
        assert!(
            vertex.to_string().contains("GL vertex records"),
            "the layout is named: {vertex}"
        );
    }

    #[test]
    fn the_vertex_table_hands_out_total_and_append_indices() {
        let mut table: Pairs = Pairs::seeded(2, [(0_i64, 0_i64), (UNIT, 0_i64)]);
        assert_eq!(table.originals(), 2, "the map's own two");
        assert_eq!(table.added(), 0, "nothing appended yet");
        assert!(
            matches!(table.spot(&(UNIT, 0_i64)), Spot::Known(1)),
            "an existing vertex answers its own index"
        );
        let Spot::Open { total, added } = table.spot(&(128_i64, 0_i64)) else {
            panic!("a point the map does not hold");
        };
        assert_eq!((total, added), (2_usize, 0_usize), "the first row");
        assert_eq!(table.take((128_i64, 0_i64), (128_i32, 0_i32)), 2);
        assert!(
            matches!(table.spot(&(128_i64, 0_i64)), Spot::Known(2)),
            "the taken key dedupes to its own row"
        );
        assert_eq!(table.added(), 1, "one row");
        assert_eq!(table.rows(), &[(128_i32, 0_i32)], "the row is stored");
    }

    #[test]
    fn a_duplicated_map_key_keeps_its_first_index() {
        let table: VertexTable<(i32, i32), u8> =
            VertexTable::seeded(3, [(0_i32, 0_i32), (1_i32, 1_i32), (0_i32, 0_i32)]);
        assert!(
            matches!(table.spot(&(0_i32, 0_i32)), Spot::Known(0)),
            "the first of two identical points wins"
        );
    }

    #[test]
    fn write_rows_lays_each_point_out_as_a_raw_i32_pair() {
        let mut out = Writer::new();
        write_rows(&mut out, &[(1_i32, -2_i32), (0x0100_0000_i32, 0_i32)]);
        let bytes = out.into_inner();
        assert_eq!(bytes.len(), 16, "two 8-byte rows");
        let mut reader = Reader::new(&bytes);
        assert_eq!(reader.read_i32().unwrap(), 1_i32, "first x");
        assert_eq!(reader.read_i32().unwrap(), -2_i32, "first y");
        assert_eq!(reader.read_i32().unwrap(), 0x0100_0000_i32, "second x");
        assert_eq!(reader.read_i32().unwrap(), 0_i32, "second y");
    }
}
