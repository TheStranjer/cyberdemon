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
//! `VERTEXES` never grow. Nodes are written children before parents —
//! the order [`build_tree`](crate::bsp::build_tree) already guarantees,
//! which makes the root the final record and leaves the node section
//! empty when the root is a subsector. Nothing here knows about
//! sessions, archives or `MCP`: the result is one complete lump payload
//! that [`crate::wad::map::nodes::parse_extended_nodes`] reads back.

use alloc::collections::BTreeMap;
use std::io::Write as _;

use crate::bsp::{BBox, BspTree, BuiltNode, Child, Point};
use crate::error::CyberError;
use crate::fixed::FRACBITS;
use crate::wad::bytes::Writer;
use crate::wad::map::Map;

/// Magic of the raw extended lump.
const MAGIC_XNOD: &[u8; 4] = b"XNOD";
/// Magic of the `zlib`-compressed extended lump.
const MAGIC_ZNOD: &[u8; 4] = b"ZNOD";
/// Subsector flag of an extended child word (`bit 31`).
const SUBSECTOR_FLAG: u32 = 0x8000_0000_u32;
/// Highest node or subsector index one extended child word holds.
const MAX_CHILD_INDEX: u32 = 0x7FFF_FFFF_u32;

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
    let mut vertices = VertexTable::new(map)?;
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
        new_vertices: vertices.added.len(),
    })
}

/// `16.16` point to lump vertex index; the map's own vertices are
/// seeded in and win ties against later duplicates.
struct VertexTable {
    /// Exact `16.16` point to vertex index.
    points: BTreeMap<(i64, i64), u32>,
    /// How many vertices the map holds (`orgVerts`).
    originals: usize,
    /// The split points appended so far, in lump order.
    added: Vec<(i32, i32)>,
}

impl VertexTable {
    /// Seed the table with `map`'s own vertices scaled to `16.16`.
    ///
    /// # Errors
    ///
    /// [`CyberError::LimitExceeded`] when the map alone passes the
    /// `u32` vertex index space.
    fn new(map: &Map) -> Result<Self, CyberError> {
        let mut points = BTreeMap::new();
        for (index, vertex) in map.vertices.iter().enumerate() {
            let at = u32::try_from(index).map_err(|_error| too_many_vertices())?;
            points
                .entry((scaled(vertex.x), scaled(vertex.y)))
                .or_insert(at);
        }
        Ok(Self {
            points,
            originals: map.vertices.len(),
            added: Vec::new(),
        })
    }

    /// The lump index of `point`: an original vertex when one sits
    /// exactly on it, otherwise a new fraction-precise entry.
    ///
    /// # Errors
    ///
    /// [`CyberError::LimitExceeded`] when the point leaves the `16.16`
    /// range or the table passes the `u32` vertex index space.
    fn locate(&mut self, point: Point) -> Result<u32, CyberError> {
        if let Some(index) = self.points.get(&(point.x, point.y)).copied() {
            return Ok(index);
        }
        let raw_x = i32::try_from(point.x).map_err(|_error| coordinate(point.x))?;
        let raw_y = i32::try_from(point.y).map_err(|_error| coordinate(point.y))?;
        let index = self
            .originals
            .checked_add(self.added.len())
            .and_then(|total| u32::try_from(total).ok())
            .ok_or_else(too_many_vertices)?;
        self.points.insert((point.x, point.y), index);
        self.added.push((raw_x, raw_y));
        Ok(index)
    }
}

/// The `16.16` form of a whole-unit vertex coordinate; the shift of an
/// `i32` into `i64` cannot fail.
fn scaled(value: i32) -> i64 {
    let widened = i64::from(value);
    widened.checked_shl(FRACBITS).unwrap_or(widened)
}

/// The vertex section: `orgVerts`, `newVerts` and the new vertices as
/// raw `16.16` `i32` pairs.
fn write_vertices(out: &mut Writer, table: &VertexTable) -> Result<(), CyberError> {
    out.put_u32(field_u32(table.originals, "vertex count")?);
    out.put_u32(field_u32(table.added.len(), "split vertex count")?);
    for &(x, y) in &table.added {
        out.put_i32(x);
        out.put_i32(y);
    }
    Ok(())
}

/// The subsector section: the count, then one seg count per subsector;
/// the segs follow consecutively in [`BspTree::segs`] order, so each
/// subsector's first seg is implicit.
fn write_subsectors(out: &mut Writer, tree: &BspTree) -> Result<(), CyberError> {
    out.put_u32(field_u32(tree.subsectors.len(), "subsector count")?);
    for &(_first, count) in &tree.subsectors {
        out.put_u32(field_u32(count, "subsector seg count")?);
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
    out.put_u32(field_u32(tree.segs.len(), "seg count")?);
    for (seg, &(v1, v2)) in tree.segs.iter().zip(endpoints) {
        out.put_u32(v1);
        out.put_u32(v2);
        out.put_u16(field_u16(seg.linedef, "linedef index")?);
        out.put_u8(seg.side);
    }
    Ok(())
}

/// The node section: the count, then 32 bytes per node, root last.
fn write_nodes(out: &mut Writer, tree: &BspTree) -> Result<(), CyberError> {
    out.put_u32(field_u32(tree.nodes.len(), "node count")?);
    for node in &tree.nodes {
        write_node(out, node)?;
    }
    Ok(())
}

/// One node: the partition line, both child boxes and the two `u32`
/// child words.
fn write_node(out: &mut Writer, node: &BuiltNode) -> Result<(), CyberError> {
    out.put_i16(field_i16(i64::from(node.partition.x), "partition x")?);
    out.put_i16(field_i16(i64::from(node.partition.y), "partition y")?);
    out.put_i16(field_i16(i64::from(node.partition.dx), "partition dx")?);
    out.put_i16(field_i16(i64::from(node.partition.dy), "partition dy")?);
    write_box(out, node.right_bbox)?;
    write_box(out, node.left_bbox)?;
    out.put_u32(child_word(node.right)?);
    out.put_u32(child_word(node.left)?);
    Ok(())
}

/// One bounding box: `top`, `bottom`, `left`, `right` as `i16`.
fn write_box(out: &mut Writer, bbox: BBox) -> Result<(), CyberError> {
    out.put_i16(field_i16(i64::from(bbox.top), "bbox top")?);
    out.put_i16(field_i16(i64::from(bbox.bottom), "bbox bottom")?);
    out.put_i16(field_i16(i64::from(bbox.left), "bbox left")?);
    out.put_i16(field_i16(i64::from(bbox.right), "bbox right")?);
    Ok(())
}

/// The extended child word: `bit 31` marks a subsector leaf.
fn child_word(child: Child) -> Result<u32, CyberError> {
    match child {
        Child::Node(index) => child_index(index, "node"),
        Child::Subsector(index) => Ok(child_index(index, "subsector")? | SUBSECTOR_FLAG),
    }
}

/// The `u32` of a child index, refusing what reaches the flag bit.
fn child_index(index: usize, what: &str) -> Result<u32, CyberError> {
    // A past-`u32` index clamps to its maximum, itself past the child
    // limit, so it is refused exactly like a merely large one.
    if u32::try_from(index).unwrap_or(u32::MAX) > MAX_CHILD_INDEX {
        return Err(CyberError::LimitExceeded(format!(
            "the extended child word cannot index {what} {index}: more than {} \
             of them do not fit below the subsector flag",
            u64::from(MAX_CHILD_INDEX).saturating_add(1)
        )));
    }
    Ok(u32::try_from(index).unwrap_or(u32::MAX))
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

/// The `i16` form of a whole value, naming the field on refusal.
fn field_i16(value: i64, what: &str) -> Result<i16, CyberError> {
    i16::try_from(value).map_err(|_error| too_big(what, value))
}

/// The `u16` form of an index, naming what it indexes on refusal.
fn field_u16(value: usize, what: &str) -> Result<u16, CyberError> {
    let shown = i64::try_from(value).unwrap_or(i64::MAX);
    u16::try_from(value).map_err(|_error| too_big(what, shown))
}

/// The `u32` form of a count, naming what it counts on refusal.
fn field_u32(value: usize, what: &str) -> Result<u32, CyberError> {
    let shown = i64::try_from(value).unwrap_or(i64::MAX);
    u32::try_from(value).map_err(|_error| too_big(what, shown))
}

/// The shared `LimitExceeded` for a value past an extended record field.
fn too_big(what: &str, value: i64) -> CyberError {
    CyberError::LimitExceeded(format!(
        "the extended node records cannot store {what} {value}: it leaves the \
         field range; no node layout holds wider fields, so keep the map \
         inside the 32767-unit box every engine enforces"
    ))
}

/// The shared `LimitExceeded` for a `16.16` coordinate past the `i32`
/// field of the extended vertex section.
fn coordinate(value: i64) -> CyberError {
    CyberError::LimitExceeded(format!(
        "the extended vertex records cannot store the 16.16 coordinate \
         {value}: it leaves the i32 field; keep the map inside the \
         32767-unit box every engine enforces"
    ))
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
    use super::{
        ExtendedNodes, VertexTable, child_word, coordinate, scaled, to_extended, too_big,
        too_many_vertices,
    };
    use crate::bsp::side::{Side, point_side};
    use crate::bsp::{
        BBox, BspTree, BuildOptions, BuildSeg, BuiltNode, Child, Partition, Point, build_tree,
    };
    use crate::fixed::Fixed;
    use crate::wad::map::Vertex;
    use crate::wad::map::{Linedef, Map, Sector, Sidedef, nodes};
    use alloc::collections::BTreeMap;

    /// The `16.16` scale factor.
    const UNIT: i64 = 0x0001_0000_i64;

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

    /// The unit square room: four walls, interior to the right.
    fn square_room() -> Map {
        room(&[(0, 256), (256, 256), (256, 0), (0, 0)])
    }

    /// Two 256-unit square rooms sharing the line `x = 256`.
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

    /// The six-vertex L-shaped room, interior to the right; its node
    /// build cuts the bottom wall at `(128, 0)`, a point no vertex of
    /// the map sits on.
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

    /// The five-vertex notched room: the wall from `(0, 256)` to
    /// `(64, 64)` extended meets the bottom wall at `x = 256 / 3`, so
    /// the node build's split point needs fractional precision.
    fn notch_room() -> Map {
        room(&[(0, 256), (64, 64), (256, 64), (256, 0), (0, 0)])
    }

    /// The default-options tree of `map`.
    fn tree_of(map: &Map) -> BspTree {
        build_tree(map, BuildOptions::default()).expect("the fixture builds")
    }

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
        (scaled(vertex.x), scaled(vertex.y))
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

    /// A fixed point at whole map units.
    fn point(x: i64, y: i64) -> Point {
        Point {
            x: x.checked_mul(UNIT).unwrap(),
            y: y.checked_mul(UNIT).unwrap(),
        }
    }

    /// A one-subsector tree of exactly `segs`, for limit tests.
    fn leaf_tree(segs: Vec<BuildSeg>) -> BspTree {
        let count = segs.len();
        BspTree {
            segs,
            subsectors: vec![(0, count)],
            nodes: Vec::new(),
            root: Child::Subsector(0),
            warnings: Vec::new(),
        }
    }

    /// A seg between raw `16.16` points with the given source.
    fn seg(from: (i64, i64), to: (i64, i64), linedef_index: usize) -> BuildSeg {
        BuildSeg {
            start: Point {
                x: from.0,
                y: from.1,
            },
            end: Point { x: to.0, y: to.1 },
            linedef: linedef_index,
            side: 1,
            offset: 0,
            sector: 0,
        }
    }

    /// One node with default everything except `edit`'s changes.
    fn node_with(edit: impl FnOnce(&mut BuiltNode)) -> BspTree {
        let mut node = BuiltNode {
            partition: Partition::default(),
            right_bbox: BBox::default(),
            left_bbox: BBox::default(),
            right: Child::Subsector(0),
            left: Child::Subsector(0),
        };
        edit(&mut node);
        BspTree {
            segs: Vec::new(),
            subsectors: vec![(0, 0)],
            nodes: vec![node],
            root: Child::Node(0),
            warnings: Vec::new(),
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

    #[test]
    fn oversized_values_refuse_with_limit_exceeded() {
        let map = two_rooms();
        let point_note = refuse(
            &map,
            &leaf_tree(vec![seg((0, 0), (0x2000_0000_0000_0000, 0), 0)]),
        );
        assert!(
            point_note.contains("coordinate"),
            "the endpoint is named: {point_note}"
        );
        let line_note = refuse(&map, &leaf_tree(vec![seg((0, 0), (UNIT, 0), 0x1_0000)]));
        assert!(
            line_note.contains("linedef"),
            "the index is named: {line_note}"
        );
        let partition_note = refuse(
            &Map::default(),
            &node_with(|node| node.partition.x = 40_000_i32),
        );
        assert!(
            partition_note.contains("partition x"),
            "the partition field is named: {partition_note}"
        );
        let box_note = refuse(
            &Map::default(),
            &node_with(|node| node.left_bbox.bottom = -40_000_i32),
        );
        assert!(
            box_note.contains("bbox bottom"),
            "the box is named: {box_note}"
        );
        let leaf_note = refuse(
            &Map::default(),
            &node_with(|node| node.left = Child::Subsector(0x8000_0000_usize)),
        );
        assert!(
            leaf_note.contains("subsector 2147483648"),
            "the child is named: {leaf_note}"
        );
    }

    #[test]
    fn the_vertex_table_refuses_a_full_index_space() {
        let mut table = VertexTable {
            points: BTreeMap::new(),
            originals: 0xFFFF_FFFF_usize,
            added: vec![(0_i32, 0_i32)],
        };
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
        assert_eq!(coordinate(-0x8000_0000_0000_i64).code(), "limit_exceeded");
        assert!(coordinate(0).to_string().contains("16.16"), "the kind");
        assert_eq!(too_big("bbox top", 40_000).code(), "limit_exceeded");
        assert!(
            too_big("bbox top", 40_000).to_string().contains("bbox top"),
            "the field is named"
        );
        assert_eq!(too_many_vertices().code(), "limit_exceeded");
    }

    #[test]
    fn the_child_word_sets_and_clears_the_flag() {
        assert_eq!(child_word(Child::Node(3)).unwrap(), 3, "a plain node");
        assert_eq!(
            child_word(Child::Subsector(3)).unwrap(),
            0x8000_0003,
            "the leaf flag rides along"
        );
    }
}
