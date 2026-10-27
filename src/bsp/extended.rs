//! `ZDoom` extended serialization of a built [`BspTree`]: one lump with
//! fraction-precise split vertices and `u32` indices and child words.
//!
//! [`to_extended`] folds a tree into the single extended lump `ZDoom`
//! reads: the magic (`XNOD` raw, `ZNOD` with the payload behind a `zlib`
//! stream), the map's own vertex count, the split vertices at full
//! `16.16` precision (never rounded, unlike the vanilla writer),
//! per-subsector seg counts, wider seg records (no angle, no offset, a
//! side byte) and node records whose child words reach `bit 31` for the
//! subsector flag. Seg endpoints that sit exactly on a map vertex (its
//! `16.16` form) reuse that index; every other endpoint is appended to
//! the lump's own vertex table, deduplicated among itself, so the map's
//! `VERTEXES` never grow. The node record itself is the one shared
//! `bsp/records.rs` writer, at the 32-bit child width and with the
//! extended refusals. Nodes are written children before parents —
//! the order [`build_tree`](crate::bsp::build_tree) already guarantees,
//! which makes the root the final record and leaves the node section
//! empty when the root is a subsector. Nothing here knows about
//! sessions, archives or `MCP`: the result is one complete lump payload
//! that [`crate::wad::map::nodes::parse_extended_nodes`] reads back.

use std::io::Write as _;

use crate::bsp::records::{self, ChildWidth, Refusals, Spot, VertexTable};
use crate::bsp::{BspTree, Point};
use crate::error::CyberError;
use crate::wad::bytes::Writer;
use crate::wad::map::Map;

/// Magic of the raw extended lump.
const MAGIC_XNOD: &[u8; 4] = b"XNOD";
/// Magic of the `zlib`-compressed extended lump.
const MAGIC_ZNOD: &[u8; 4] = b"ZNOD";

/// The words every extended refusal answers with.
const REFUSALS: Refusals = Refusals::EXTENDED;

/// The whole extended node run of one tree: the lump bytes and how many
/// fraction-precise vertices they add beyond `VERTEXES`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExtendedNodes {
    /// The magic written: `"ZNOD"` for a compressed lump, `"XNOD"` raw.
    pub format: &'static str,
    /// The complete lump payload, magic first.
    pub bytes: Vec<u8>,
    /// Split vertices stored in the lump beyond the map's own
    /// `VERTEXES` (which the build never modifies).
    pub new_vertices: usize,
}

/// Serialize `tree` as one `ZDoom` extended node lump over `map`.
///
/// `compress` picks the magic: `true` writes `ZNOD` with the sections
/// behind a `zlib` stream, `false` the raw `XNOD` form. The map's
/// `VERTEXES` stay untouched: split endpoints live in the lump's own
/// vertex section.
///
/// # Errors
///
/// [`CyberError::LimitExceeded`] when a coordinate, index or count does
/// not fit the record's `i16`/`u16`/`u32` field, or when the vertex
/// table would pass the `u32` index space.
#[inline]
pub fn to_extended(map: &Map, tree: &BspTree, compress: bool) -> Result<ExtendedNodes, CyberError> {
    let mut vertices = LumpVertices::new(map);
    let mut endpoints = Vec::with_capacity(tree.segs.len());
    for seg in &tree.segs {
        endpoints.push((vertices.locate(seg.start)?, vertices.locate(seg.end)?));
    }
    let mut out = Writer::new();
    write_vertices(&mut out, &vertices)?;
    write_subsectors(&mut out, tree)?;
    write_segs(&mut out, tree, &endpoints)?;
    write_nodes(&mut out, tree)?;
    let payload = out.into_inner();
    let (format, bytes) = if compress {
        ("ZNOD", wrap_zlib(&payload)?)
    } else {
        ("XNOD", wrap_raw(&payload))
    };
    Ok(ExtendedNodes {
        format,
        bytes,
        new_vertices: vertices.table.added(),
    })
}

/// The shared endpoint table over the exact `16.16` point, whose rows are
/// the lump's own raw `16.16` pairs.
type Endpoints = VertexTable<(i64, i64), (i32, i32)>;

/// The exact `16.16` endpoint table over the shared index bookkeeping: the
/// map's own vertices are seeded in and win ties against later duplicates,
/// and its rows are the lump's own raw `16.16` pairs.
struct LumpVertices {
    /// `16.16` point to total vertex index.
    table: Endpoints,
}

impl LumpVertices {
    /// Seed the table with `map`'s own vertices scaled to `16.16`.
    fn new(map: &Map) -> Self {
        Self {
            table: VertexTable::seeded(
                map.vertices.len(),
                map.vertices
                    .iter()
                    .map(|vertex| (records::scaled(vertex.x), records::scaled(vertex.y))),
            ),
        }
    }

    /// A table over a map of `originals` vertices and no keys at all, for
    /// the full-index-space test no real map can supply.
    #[cfg(test)]
    fn over(originals: usize) -> Self {
        Self {
            table: VertexTable::seeded(originals, []),
        }
    }

    /// The lump index of `point`: an original vertex when one sits
    /// exactly on it, otherwise a new fraction-precise entry.
    ///
    /// # Errors
    ///
    /// [`CyberError::LimitExceeded`] when the point leaves the `16.16`
    /// range or the table passes the `u32` vertex index space.
    fn locate(&mut self, point: Point) -> Result<u32, CyberError> {
        let key = (point.x, point.y);
        let total = match self.table.spot(&key) {
            Spot::Known(index) => index,
            Spot::Open { .. } => self.append(key, point)?,
        };
        u32::try_from(total).map_err(|_error| too_many_vertices())
    }

    /// The total index of a new row for `point`, refusing a coordinate past
    /// the lump's `i32` fields.
    fn append(&mut self, key: (i64, i64), point: Point) -> Result<usize, CyberError> {
        let row = (
            i32::try_from(point.x).map_err(|_error| REFUSALS.coordinate(point.x))?,
            i32::try_from(point.y).map_err(|_error| REFUSALS.coordinate(point.y))?,
        );
        Ok(self.table.take(key, row))
    }
}

/// The vertex section: `orgVerts`, `newVerts` and the new vertices as
/// raw `16.16` `i32` pairs.
fn write_vertices(out: &mut Writer, table: &LumpVertices) -> Result<(), CyberError> {
    out.put_u32(REFUSALS.u32(table.table.originals(), "vertex count")?);
    out.put_u32(REFUSALS.u32(table.table.added(), "split vertex count")?);
    records::write_rows(out, table.table.rows());
    Ok(())
}

/// The subsector section: the count, then one seg count per subsector;
/// the segs follow consecutively in [`BspTree::segs`] order, so each
/// subsector's first seg is implicit.
fn write_subsectors(out: &mut Writer, tree: &BspTree) -> Result<(), CyberError> {
    out.put_u32(REFUSALS.u32(tree.subsectors.len(), "subsector count")?);
    for &(_first, count) in &tree.subsectors {
        out.put_u32(REFUSALS.u32(count, "subsector seg count")?);
    }
    Ok(())
}

/// The seg section: the count, then 11 bytes per seg — both vertex
/// indices, the `u16` linedef and the side byte.
fn write_segs(
    out: &mut Writer,
    tree: &BspTree,
    endpoints: &[(u32, u32)],
) -> Result<(), CyberError> {
    out.put_u32(REFUSALS.u32(tree.segs.len(), "seg count")?);
    for (seg, &(v1, v2)) in tree.segs.iter().zip(endpoints) {
        out.put_u32(v1);
        out.put_u32(v2);
        out.put_u16(REFUSALS.u16(seg.linedef, "linedef index")?);
        out.put_u8(seg.side);
    }
    Ok(())
}

/// The node section: the count, then 32 bytes per node, root last.
fn write_nodes(out: &mut Writer, tree: &BspTree) -> Result<(), CyberError> {
    out.put_u32(REFUSALS.u32(tree.nodes.len(), "node count")?);
    records::write_node_records(out, tree, ChildWidth::Bits32, REFUSALS)
}

/// Prefixes `payload` with the raw `XNOD` magic.
fn wrap_raw(payload: &[u8]) -> Vec<u8> {
    let mut bytes = MAGIC_XNOD.to_vec();
    bytes.extend_from_slice(payload);
    bytes
}

/// Prefixes `payload` with the `ZNOD` magic and `zlib`-compresses
/// everything behind it.
fn wrap_zlib(payload: &[u8]) -> Result<Vec<u8>, CyberError> {
    let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
    let deflated = encoder
        .write_all(payload)
        .and_then(|()| encoder.finish())
        .map_err(|error| {
            CyberError::Internal(format!("the extended node lump does not compress: {error}"))
        })?;
    let mut bytes = MAGIC_ZNOD.to_vec();
    bytes.extend_from_slice(&deflated);
    Ok(bytes)
}

/// The shared `LimitExceeded` for passing the extended vertex index space.
fn too_many_vertices() -> CyberError {
    CyberError::LimitExceeded(
        "this map plus its seg splits pass the extended lump's 4294967295-vertex \
         index space: no node layout can address more"
            .to_owned(),
    )
}

#[cfg(test)]
mod tests {
    use super::{ExtendedNodes, LumpVertices, REFUSALS, to_extended, too_many_vertices};
    use crate::bsp::fixtures::{
        UNIT, l_room, leaf_tree, node_tree, notch_room, point, seg, square_room, tree_of, two_rooms,
    };
    use crate::bsp::records::{self, ChildWidth};
    use crate::bsp::side::{Side, point_side};
    use crate::bsp::{BspTree, BuildSeg, Child, Partition, Point};
    use crate::fixed::Fixed;
    use crate::wad::map::Map;
    use crate::wad::map::nodes;

    /// The raw extended run of `map`'s tree, in one step.
    fn run_of(map: &Map) -> ExtendedNodes {
        let tree = tree_of(map);
        to_extended(map, &tree, false).expect("the fixture serializes")
    }

    /// Parse a finished run back with the lump reader.
    fn parsed(run: &ExtendedNodes) -> nodes::NodeSet {
        nodes::parse_extended_nodes(&run.bytes).expect("the lump parses back")
    }

    /// The map vertex at `index` (the lump reuses all `orgVerts` as-is),
    /// scaled to `16.16` for endpoint comparisons.
    fn original_at(map: &Map, index: u32) -> (i64, i64) {
        let vertex = &map.vertices[usize::try_from(index).unwrap()];
        (records::scaled(vertex.x), records::scaled(vertex.y))
    }

    /// The `16.16` point a lump vertex index names: the map's own vertex
    /// below `orgVerts`, the added fraction-precise one above.
    fn endpoint_at(map: &Map, set: &nodes::NodeSet, index: u32) -> (i64, i64) {
        let at = usize::try_from(index).unwrap();
        at.checked_sub(map.vertices.len()).map_or_else(
            || original_at(map, index),
            |extra| {
                let pair = &set.vertices_extra[extra];
                (i64::from(pair.0.0), i64::from(pair.1.0))
            },
        )
    }

    /// A seg on the back side with no offset, between raw `16.16` points.
    fn back_seg(from: (i64, i64), to: (i64, i64), linedef: usize) -> BuildSeg {
        seg(from, to, (linedef, 1_u8), 0_i64)
    }

    /// The child of `node` that `spot` descends into.
    fn descend(node: &nodes::Node, spot: Point) -> nodes::Child {
        let partition = Partition {
            x: i32::from(node.x),
            y: i32::from(node.y),
            dx: i32::from(node.dx),
            dy: i32::from(node.dy),
        };
        match point_side(partition, spot) {
            Side::Right => node.right_child,
            Side::Left => node.left_child,
        }
    }

    /// Walk the parsed tree to the subsector `spot` lands in.
    fn leaf_subsector(set: &nodes::NodeSet, spot: Point) -> Option<usize> {
        let root = set.root()?;
        let mut child = nodes::Child::Node(u32::try_from(root).unwrap());
        while let nodes::Child::Node(index) = child {
            let node = set.nodes.get(usize::try_from(index).unwrap())?;
            child = descend(node, spot);
        }
        match child {
            nodes::Child::Subsector(leaf) => usize::try_from(leaf).ok(),
            nodes::Child::Node(_) => None,
        }
    }

    /// The message of a refused serialization, code checked.
    fn refuse(map: &Map, tree: &BspTree) -> String {
        let error = to_extended(map, tree, false).unwrap_err();
        assert_eq!(error.code(), "limit_exceeded", "limit code");
        error.to_string()
    }

    #[test]
    fn the_two_room_run_parses_back_with_original_vertices() {
        let map = two_rooms();
        let tree = tree_of(&map);
        let run = to_extended(&map, &tree, false).expect("the fixture serializes");
        assert_eq!(run.format, "XNOD", "the raw magic");
        assert_eq!(run.new_vertices, 0, "flush corners need no splits");
        assert!(run.bytes.starts_with(b"XNOD"), "the lump is tagged");
        let set = parsed(&run);
        assert_eq!(set.format, nodes::NodesFormat::Xnod, "the raw layout");
        assert!(set.vertices_extra.is_empty(), "no added vertices");
        assert_eq!(set.segs.len(), tree.segs.len(), "every seg stored");
        assert_eq!(set.subsectors.len(), tree.subsectors.len(), "both leaves");
        assert_eq!(set.nodes.len(), 1, "one node");
        assert!(
            set.segs
                .iter()
                .all(|row| usize::try_from(row.v1).unwrap() < map.vertices.len()
                    && usize::try_from(row.v2).unwrap() < map.vertices.len()),
            "every flush endpoint is an original vertex"
        );
        let root = set.nodes.first().unwrap();
        assert_eq!(
            (root.x, root.y, root.dx, root.dy),
            (256, 0, 0, 256),
            "the shared line"
        );
        assert_eq!(root.right.top, 256, "east box top");
        assert_eq!(root.right.left, 256, "east box left");
        assert_eq!(root.right.right, 512, "east box far right");
        assert_eq!(root.left.right, 256, "west box right edge");
        assert_eq!(root.right_child, nodes::Child::Subsector(0), "bit 31 leaf");
        assert_eq!(root.left_child, nodes::Child::Subsector(1));
        assert_eq!(set.root(), Some(0), "the only node is the root");
        assert_eq!(set.depth(), Some(1), "one level");
    }

    #[test]
    fn the_l_room_split_gets_its_own_vertex_index() {
        let map = l_room();
        let run = run_of(&map);
        assert_eq!(run.new_vertices, 1, "the bottom wall is cut once");
        let set = parsed(&run);
        assert_eq!(
            set.vertices_extra,
            vec![(Fixed(128_i32.checked_mul(0x1_0000).unwrap()), Fixed(0))],
            "the cut point is stored at full 16.16"
        );
        let cut = u32::try_from(map.vertices.len()).unwrap();
        let touches: usize = set
            .segs
            .iter()
            .filter(|row| row.v1 == cut || row.v2 == cut)
            .count();
        assert_eq!(touches, 2, "the split point ends one seg, starts the next");
        let total: usize = set
            .subsectors
            .iter()
            .map(|row| usize::try_from(row.seg_count).unwrap())
            .sum();
        assert_eq!(total, set.segs.len(), "the leaves cover every seg");
    }

    #[test]
    fn a_fractional_split_vertex_keeps_its_fixed_point() {
        let map = notch_room();
        let run = run_of(&map);
        assert_eq!(run.new_vertices, 1, "the diagonal cut is the only split");
        let set = parsed(&run);
        // 85.333... units: 256 * 65536 / 3 rounded to nearest.
        let cut = 5_592_405_i32;
        assert_eq!(
            set.vertices_extra,
            vec![(Fixed(cut), Fixed(0))],
            "x stays fractional (85 whole units would be 5_570_560)"
        );
        let index = u32::try_from(map.vertices.len()).unwrap();
        assert!(
            set.segs
                .iter()
                .any(|row| row.v1 == index || row.v2 == index),
            "segs reach the lump's own vertex"
        );
    }

    #[test]
    fn the_compressed_lump_inflates_to_the_same_records() {
        let map = l_room();
        let tree = tree_of(&map);
        let packed = to_extended(&map, &tree, true).expect("the fixture compresses");
        assert_eq!(packed.format, "ZNOD", "the compressed magic");
        assert!(packed.bytes.starts_with(b"ZNOD"), "the lump is tagged");
        let raw = to_extended(&map, &tree, false).expect("the fixture stays raw");
        assert_ne!(packed.bytes, raw.bytes, "the payloads differ");
        let zipped = parsed(&packed);
        let plain = parsed(&raw);
        assert_eq!(zipped.format, nodes::NodesFormat::Znod, "the znod layout");
        assert_eq!(zipped.vertices_extra, plain.vertices_extra, "same vertices");
        assert_eq!(zipped.subsectors, plain.subsectors, "same subsectors");
        assert_eq!(zipped.segs, plain.segs, "same segs");
        assert_eq!(zipped.nodes, plain.nodes, "same nodes");
        assert_eq!(packed.new_vertices, raw.new_vertices, "same split count");
    }

    #[test]
    fn a_single_convex_room_writes_no_nodes() {
        let map = square_room();
        let run = run_of(&map);
        let set = parsed(&run);
        assert!(set.nodes.is_empty(), "the root leaf writes no node");
        assert_eq!(
            set.subsectors,
            vec![nodes::Subsector {
                seg_count: 4,
                first_seg: 0
            }],
            "one leaf of four segs"
        );
        assert_eq!(set.segs.len(), 4, "the ring");
        assert_eq!(set.root(), None, "no node to root");
    }

    #[test]
    fn seg_vertex_indices_point_at_the_vertex_the_builder_cut() {
        let map = l_room();
        let tree = tree_of(&map);
        let run = to_extended(&map, &tree, false).expect("serializes");
        let set = parsed(&run);
        for (index, build) in tree.segs.iter().enumerate() {
            let row = &set.segs[index];
            assert_eq!(
                endpoint_at(&map, &set, row.v1),
                (build.start.x, build.start.y),
                "seg {index} keeps its start at full 16.16"
            );
            assert_eq!(
                endpoint_at(&map, &set, row.v2),
                (build.end.x, build.end.y),
                "seg {index} keeps its end at full 16.16"
            );
            assert_eq!(
                usize::try_from(row.linedef).unwrap(),
                build.linedef,
                "seg {index} source linedef"
            );
            assert_eq!(
                u8::try_from(row.direction).unwrap(),
                build.side,
                "seg {index} side byte"
            );
        }
    }

    #[test]
    fn walking_the_serialized_nodes_finds_each_room() {
        let map = two_rooms();
        let set = parsed(&run_of(&map));
        assert_eq!(leaf_subsector(&set, point(128, 128)), Some(1), "west room");
        assert_eq!(leaf_subsector(&set, point(384, 128)), Some(0), "east room");
    }

    #[test]
    fn oversized_values_refuse_with_limit_exceeded() {
        let map = two_rooms();
        let point_note = refuse(
            &map,
            &leaf_tree(vec![back_seg((0, 0), (0x2000_0000_0000_0000, 0), 0)]),
        );
        assert!(
            point_note.contains("coordinate"),
            "the endpoint is named: {point_note}"
        );
        let line_note = refuse(
            &map,
            &leaf_tree(vec![back_seg((0, 0), (UNIT, 0), 0x1_0000)]),
        );
        assert!(
            line_note.contains("linedef"),
            "the index is named: {line_note}"
        );
        let partition_note = refuse(
            &Map::default(),
            &node_tree(|node| node.partition.x = 40_000_i32),
        );
        assert!(
            partition_note.contains("partition x"),
            "the partition field is named: {partition_note}"
        );
        let box_note = refuse(
            &Map::default(),
            &node_tree(|node| node.left_bbox.bottom = -40_000_i32),
        );
        assert!(
            box_note.contains("bbox bottom"),
            "the box is named: {box_note}"
        );
        let leaf_note = refuse(
            &Map::default(),
            &node_tree(|node| node.left = Child::Subsector(0x8000_0000_usize)),
        );
        assert!(
            leaf_note.contains("subsector 2147483648"),
            "the child is named: {leaf_note}"
        );
    }

    #[test]
    fn the_vertex_table_refuses_a_full_index_space() {
        let mut table = LumpVertices::over(0x1_0000_0000_usize);
        let error = table
            .locate(Point { x: 1_i64, y: 1_i64 })
            .expect_err("the next index is past u32");
        assert_eq!(error.code(), "limit_exceeded", "vertex space code");
        assert!(
            error.to_string().contains("4294967295"),
            "the cap is named: {error}"
        );
    }

    #[test]
    fn helper_errors_keep_their_codes_and_names() {
        assert_eq!(
            REFUSALS.coordinate(-0x8000_0000_0000_i64).code(),
            "limit_exceeded"
        );
        assert!(
            REFUSALS.coordinate(0).to_string().contains("16.16"),
            "the kind"
        );
        assert!(
            REFUSALS.coordinate(0).to_string().contains("extended"),
            "the layout is named"
        );
        assert_eq!(
            REFUSALS.oversized("bbox top", 40_000).code(),
            "limit_exceeded"
        );
        assert!(
            REFUSALS
                .oversized("bbox top", 40_000)
                .to_string()
                .contains("bbox top"),
            "the field is named"
        );
        assert_eq!(too_many_vertices().code(), "limit_exceeded");
    }

    #[test]
    fn the_wide_child_word_sets_and_clears_the_flag() {
        assert_eq!(
            ChildWidth::Bits32.word(Child::Node(3), REFUSALS).unwrap(),
            3_u32,
            "a plain node"
        );
        assert_eq!(
            ChildWidth::Bits32
                .word(Child::Subsector(3), REFUSALS)
                .unwrap(),
            0x8000_0003,
            "the leaf flag rides along"
        );
    }
}
