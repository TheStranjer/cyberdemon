//! Vanilla serialization of a built [`BspTree`]: the `SEGS`, `SSECTORS`
//! and `NODES` record lumps, plus the split vertices the seg endpoints
//! need.
//!
//! [`to_vanilla`] folds a tree into the three vanilla lumps. Every seg
//! endpoint is a `16.16` point, rounded to the nearest whole unit and
//! matched against the map's own vertices: an exact hit reuses that index,
//! anything else is appended after all existing vertices (deduplicated
//! among themselves too) and reported through
//! [`VanillaNodes::new_vertices`], so the caller owns the `VERTEXES`
//! growth. Nodes are written children before parents — the order
//! [`build_tree`](crate::bsp::build_tree) already guarantees, which makes
//! the root the final record and leaves the `NODES` lump empty when the
//! root is a subsector. Nothing here knows about sessions, archives or
//! `MCP`: the result is three complete lump payloads.

use alloc::collections::BTreeMap;

use crate::bsp::{BBox, BspTree, BuildSeg, BuiltNode, Child, Point};
use crate::error::CyberError;
use crate::fixed::{FRACBITS, atan2, round_shift};
use crate::wad::bytes::Writer;
use crate::wad::map::{Map, Vertex};

/// Most vertices a vanilla run can address: the `u16` index space.
const MAX_VERTICES: usize = 0xFFFF_usize;
/// Highest node or subsector index one vanilla child word holds.
const MAX_CHILD_INDEX: u16 = 0x7FFF_u16;
/// Subsector flag of a vanilla child word (`bit 15`).
const SUBSECTOR_FLAG: u16 = 0x8000_u16;

/// The whole vanilla node run of one tree: the three record lumps and the
/// vertices to append to `VERTEXES` before they are stored.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct VanillaNodes {
    /// Vertices the seg endpoints need beyond the map's own, in append
    /// order. Their indices start right after the last existing vertex.
    pub new_vertices: Vec<Vertex>,
    /// The `SEGS` lump bytes: 12 per seg, in subsector order.
    pub segs: Vec<u8>,
    /// The `SSECTORS` lump bytes: 4 per subsector.
    pub ssectors: Vec<u8>,
    /// The `NODES` lump bytes: 28 per node, the root last; empty for a
    /// tree whose root is a subsector.
    pub nodes: Vec<u8>,
}

/// Serialize `tree` as the vanilla `SEGS`, `SSECTORS` and `NODES` lumps
/// over `map`.
///
/// # Errors
///
/// [`CyberError::LimitExceeded`] when the map already holds more vertices
/// than the vanilla index space, when the split points would take the
/// total past it (the message suggests the `extended` variant), or when a
/// coordinate, index, offset or child reference does not fit the record's
/// `i16`/`u16` field.
#[inline]
pub fn to_vanilla(map: &Map, tree: &BspTree) -> Result<VanillaNodes, CyberError> {
    let mut vertices = VertexTable::new(map)?;
    let segs = write_segs(tree, &mut vertices)?;
    Ok(VanillaNodes {
        new_vertices: vertices.added,
        segs,
        ssectors: write_ssectors(tree)?,
        nodes: write_nodes(tree)?,
    })
}

/// The vertex index of a rounded seg endpoint: an existing vertex when one
/// sits exactly on the point, otherwise a newly appended one.
struct VertexTable {
    /// Whole-unit point to total vertex index; existing vertices win.
    points: BTreeMap<(i32, i32), usize>,
    /// How many vertices the map already holds.
    originals: usize,
    /// The vertices this build appended, in order.
    added: Vec<Vertex>,
}

impl VertexTable {
    /// Seed the table with the map's own vertices (of a duplicated point
    /// the first index wins).
    ///
    /// # Errors
    ///
    /// [`CyberError::LimitExceeded`] when the map alone passes the vanilla
    /// vertex space.
    fn new(map: &Map) -> Result<Self, CyberError> {
        if map.vertices.len() > MAX_VERTICES {
            return Err(too_many_vertices());
        }
        let mut points = BTreeMap::new();
        for (index, vertex) in map.vertices.iter().enumerate() {
            points.entry((vertex.x, vertex.y)).or_insert(index);
        }
        Ok(Self {
            points,
            originals: map.vertices.len(),
            added: Vec::new(),
        })
    }

    /// The stored index of `point` rounded to the nearest unit, appending
    /// a new vertex when the map holds none on that exact point.
    fn locate(&mut self, point: Point) -> Result<u16, CyberError> {
        let x = round_unit(point.x)?;
        let y = round_unit(point.y)?;
        if let Some(index) = self.points.get(&(x, y)).copied() {
            return field_u16(index, "vertex index");
        }
        let index = self
            .originals
            .checked_add(self.added.len())
            .filter(|total| *total < MAX_VERTICES)
            .ok_or_else(too_many_vertices)?;
        let vertex = Vertex {
            x: i32::from(field_i16(i64::from(x), "split vertex x")?),
            y: i32::from(field_i16(i64::from(y), "split vertex y")?),
            ..Default::default()
        };
        self.points.insert((x, y), index);
        self.added.push(vertex);
        field_u16(index, "vertex index")
    }
}

/// The `SEGS` lump: one 12-byte record per tree seg, in subsector order.
fn write_segs(tree: &BspTree, vertices: &mut VertexTable) -> Result<Vec<u8>, CyberError> {
    let mut out = Writer::new();
    for seg in &tree.segs {
        let v1 = vertices.locate(seg.start)?;
        let v2 = vertices.locate(seg.end)?;
        out.put_u16(v1);
        out.put_u16(v2);
        out.put_i16(seg_angle(seg));
        out.put_u16(field_u16(seg.linedef, "linedef index")?);
        out.put_i16(i16::from(seg.side));
        out.put_i16(field_i16(i64::from(round_unit(seg.offset)?), "seg offset")?);
    }
    Ok(out.into_inner())
}

/// The vanilla `angle` field: the high 16 bits of the seg's `BAM`
/// direction, reinterpreted as the `i16` the record stores.
fn seg_angle(seg: &BuildSeg) -> i16 {
    let bam = atan2(
        seg.end.y.saturating_sub(seg.start.y),
        seg.end.x.saturating_sub(seg.start.x),
    )
    .0;
    let raw = bam.to_le_bytes();
    let high = raw.get(2..).unwrap_or_default();
    i16::from_le_bytes(high.try_into().unwrap_or([0_u8; 2]))
}

/// The `SSECTORS` lump: one 4-byte record per subsector.
fn write_ssectors(tree: &BspTree) -> Result<Vec<u8>, CyberError> {
    let mut out = Writer::new();
    for &(first, count) in &tree.subsectors {
        out.put_u16(field_u16(count, "subsector seg count")?);
        out.put_u16(field_u16(first, "subsector first seg")?);
    }
    Ok(out.into_inner())
}

/// The `NODES` lump: one 28-byte record per interior node, root last.
fn write_nodes(tree: &BspTree) -> Result<Vec<u8>, CyberError> {
    let mut out = Writer::new();
    for node in &tree.nodes {
        write_node(&mut out, node)?;
    }
    Ok(out.into_inner())
}

/// One node record: the partition line, both child boxes and the two
/// child words.
fn write_node(out: &mut Writer, node: &BuiltNode) -> Result<(), CyberError> {
    out.put_i16(field_i16(i64::from(node.partition.x), "partition x")?);
    out.put_i16(field_i16(i64::from(node.partition.y), "partition y")?);
    out.put_i16(field_i16(i64::from(node.partition.dx), "partition dx")?);
    out.put_i16(field_i16(i64::from(node.partition.dy), "partition dy")?);
    write_box(out, node.right_bbox)?;
    write_box(out, node.left_bbox)?;
    out.put_u16(child_word(node.right)?);
    out.put_u16(child_word(node.left)?);
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

/// The vanilla child word: `bit 15` marks a subsector leaf.
fn child_word(child: Child) -> Result<u16, CyberError> {
    match child {
        Child::Node(index) => child_index(index, "node"),
        Child::Subsector(index) => Ok(child_index(index, "subsector")? | SUBSECTOR_FLAG),
    }
}

/// The `u16` of a child index, refusing anything a plain word cannot hold
/// beside the flag bit.
fn child_index(index: usize, what: &str) -> Result<u16, CyberError> {
    // A past-`u16` index clamps to its maximum, itself past the child
    // limit, so it is refused exactly like a merely large one.
    if u16::try_from(index).unwrap_or(u16::MAX) > MAX_CHILD_INDEX {
        return Err(CyberError::LimitExceeded(format!(
            "the vanilla child word cannot index {what} {index}: more than {} \
             of them do not fit; use variant \"extended\"",
            MAX_CHILD_INDEX.saturating_add(1)
        )));
    }
    Ok(u16::try_from(index).unwrap_or(u16::MAX))
}

/// The whole-unit form of a `16.16` value, refusing what passes the map
/// model's `i32`.
fn round_unit(value: i64) -> Result<i32, CyberError> {
    let rounded = round_shift(i128::from(value), FRACBITS);
    i32::try_from(rounded).map_err(|_error| too_big("coordinate", rounded))
}

/// The `i16` form of a whole value, naming the field on refusal.
fn field_i16(value: i64, what: &str) -> Result<i16, CyberError> {
    i16::try_from(value).map_err(|_error| too_big(what, value))
}

/// The `u16` form of a count or index, naming what it counts on refusal.
fn field_u16(value: usize, what: &str) -> Result<u16, CyberError> {
    let shown = i64::try_from(value).unwrap_or(i64::MAX);
    u16::try_from(value).map_err(|_error| too_big(what, shown))
}

/// The shared `LimitExceeded` for a value past a vanilla record field.
fn too_big(what: &str, value: i64) -> CyberError {
    CyberError::LimitExceeded(format!(
        "the vanilla node records cannot store {what} {value}: it leaves the \
         i16/u16 field range; use variant \"extended\""
    ))
}

/// The shared `LimitExceeded` for passing the vanilla vertex space.
fn too_many_vertices() -> CyberError {
    CyberError::LimitExceeded(format!(
        "this map plus its seg splits need more than {MAX_VERTICES} vertices: \
         the vanilla index space is full; use variant \"extended\""
    ))
}

#[cfg(test)]
mod tests {
    use super::{MAX_VERTICES, seg_angle, to_vanilla};
    use crate::bsp::side::{Side, point_side};
    use crate::bsp::{BBox, BspTree, BuildOptions, BuildSeg, BuiltNode, Child, Partition, Point};
    use crate::wad::map::nodes;
    use crate::wad::map::{Linedef, Map, Sector, Sidedef, Vertex};

    /// The `16.16` scale factor.
    const UNIT: i64 = 0x1_0000_i64;

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

    /// A fixed point at whole map units.
    fn point(x: i64, y: i64) -> Point {
        Point {
            x: x.checked_mul(UNIT).unwrap(),
            y: y.checked_mul(UNIT).unwrap(),
        }
    }

    /// The tree of `map` and its vanilla run, in one step.
    fn run_of(map: &Map) -> (BspTree, super::VanillaNodes) {
        let tree = build_tree(map);
        let run = to_vanilla(map, &tree).unwrap();
        (tree, run)
    }

    /// The default-options tree of `map`.
    fn build_tree(map: &Map) -> BspTree {
        crate::bsp::build_tree(map, BuildOptions::default()).unwrap()
    }

    /// Parse a finished run back with the lump reader.
    fn parsed(run: &super::VanillaNodes) -> nodes::NodeSet {
        nodes::parse_vanilla_nodes(&run.segs, &run.ssectors, &run.nodes).unwrap()
    }

    /// The map point at `index`, existing or split, as whole units.
    fn vertex_at(map: &Map, run: &super::VanillaNodes, index: u32) -> (i32, i32) {
        let at = usize::try_from(index).unwrap();
        let existing = map.vertices.get(at).or_else(|| {
            run.new_vertices
                .get(at.checked_sub(map.vertices.len()).unwrap())
        });
        let vertex = existing.unwrap();
        (vertex.x, vertex.y)
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
    fn seg(from: (i64, i64), to: (i64, i64), linedef_index: usize, offset: i64) -> BuildSeg {
        BuildSeg {
            start: Point {
                x: from.0,
                y: from.1,
            },
            end: Point { x: to.0, y: to.1 },
            linedef: linedef_index,
            side: 1,
            offset,
            sector: 0,
        }
    }

    /// One node with default everything except `f`'s edits.
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
        let error = to_vanilla(map, tree).unwrap_err();
        assert_eq!(error.code(), "limit_exceeded", "limit code");
        error.to_string()
    }

    /// The whole-unit form of a fixed point, for endpoint comparisons.
    fn whole(point: Point) -> (i32, i32) {
        let x = point.x.checked_shr(16).unwrap();
        let y = point.y.checked_shr(16).unwrap();
        (i32::try_from(x).unwrap(), i32::try_from(y).unwrap())
    }

    #[test]
    fn the_two_room_run_parses_back_to_the_tree() {
        let map = two_rooms();
        let (tree, run) = run_of(&map);
        assert!(run.new_vertices.is_empty(), "flush rooms need no splits");
        assert_eq!(run.nodes.len(), 28, "one node record");
        assert_eq!(run.ssectors.len(), 8, "two subsector records");
        let records = tree.segs.len().checked_mul(12).unwrap();
        assert_eq!(run.segs.len(), records, "12 bytes per seg");
        let set = parsed(&run);
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
        assert_eq!(root.right_child, nodes::Child::Subsector(0), "leaf flag");
        assert_eq!(root.left_child, nodes::Child::Subsector(1));
        assert_eq!(set.root(), Some(0), "the last node is the root");
        assert_eq!(set.subsectors[0].seg_count, 4, "east leaf");
        assert_eq!(set.subsectors[1].first_seg, 4, "west leaf follows");
        for (index, build) in tree.segs.iter().enumerate() {
            let row = &set.segs[index];
            assert_eq!(
                usize::try_from(row.linedef).unwrap(),
                build.linedef,
                "seg {index} source linedef"
            );
            assert_eq!(row.direction, i16::from(build.side), "seg {index} side");
            assert_eq!(row.offset, 0, "flush corners: no offsets");
            assert_eq!(
                vertex_at(&map, &run, row.v1),
                whole(build.start),
                "seg {index} starts on its own vertex"
            );
        }
    }

    #[test]
    fn a_north_facing_seg_angles_at_0x4000() {
        let map = two_rooms();
        let (_, run) = run_of(&map);
        let set = parsed(&run);
        let north = set
            .segs
            .iter()
            .find(|row| row.linedef == 0 && row.direction == 0)
            .unwrap();
        assert_eq!(north.angle, 0x4000_i16, "due north in the high word");
        let south = seg_angle(&seg((0, 0), (0, -16_384_000), 0, 0));
        assert!(
            i64::from(south).abs_diff(i64::from(-0x4000_i16)) <= 1,
            "the raw high half of due south, within the CORDIC ulp: {south}"
        );
        let east = seg_angle(&seg((0, 0), (0x0100_0000, 0), 0, 0));
        assert_eq!(east, 0, "east is zero");
    }

    #[test]
    fn the_l_room_appends_one_split_vertex() {
        let map = l_room();
        let (tree, run) = run_of(&map);
        assert_eq!(run.new_vertices.len(), 1, "the bottom wall is cut once");
        let added = run.new_vertices.first().unwrap();
        assert_eq!((added.x, added.y), (128_i32, 0_i32), "the cut point");
        assert_eq!(tree.segs.len(), 7, "six sides plus the extra piece");
        let set = parsed(&run);
        assert_eq!(set.segs.len(), 7, "every seg serialized");
        let cut = 6_u32;
        let touches: usize = set
            .segs
            .iter()
            .filter(|row| row.v1 == cut || row.v2 == cut)
            .count();
        assert_eq!(touches, 2, "the split point ends one seg, starts the next");
        let total: u64 = set
            .subsectors
            .iter()
            .map(|row| u64::from(row.seg_count))
            .sum();
        assert_eq!(
            usize::try_from(total).unwrap(),
            7,
            "the leaves cover every seg"
        );
    }

    #[test]
    fn a_single_convex_room_writes_no_nodes() {
        let map = square_room();
        let (_, run) = run_of(&map);
        assert!(run.nodes.is_empty(), "the root leaf writes no node");
        assert!(run.new_vertices.is_empty(), "corners are all existing");
        let set = parsed(&run);
        assert!(set.nodes.is_empty(), "no interior nodes");
        assert_eq!(
            set.subsectors,
            vec![nodes::Subsector {
                seg_count: 4,
                first_seg: 0
            }],
            "one leaf of four segs"
        );
        assert_eq!(set.segs.len(), 4, "the ring");
    }

    #[test]
    fn oversized_values_refuse_with_limit_exceeded() {
        let map = two_rooms();
        let point_note = refuse(
            &map,
            &leaf_tree(vec![seg((0, 0), (2_621_440_000, 0), 0, 0)]),
        );
        assert!(point_note.contains("extended"), "the hint: {point_note}");
        let offset_note = refuse(
            &map,
            &leaf_tree(vec![seg((0, 0), (256, 0), 0, 2_621_440_000)]),
        );
        assert!(
            offset_note.contains("offset"),
            "the field is named: {offset_note}"
        );
        let line_note = refuse(&map, &leaf_tree(vec![seg((0, 0), (256, 0), 0x1_0000, 0)]));
        assert!(
            line_note.contains("linedef"),
            "the index is named: {line_note}"
        );
        let huge_note = refuse(
            &map,
            &leaf_tree(vec![seg((0, 0), (0x2000_0000_0000_0000, 0), 0, 0)]),
        );
        assert!(
            huge_note.contains("coordinate"),
            "the point is named: {huge_note}"
        );
    }

    #[test]
    fn oversized_nodes_and_children_refuse_with_limit_exceeded() {
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
            &node_with(|node| node.right_bbox.right = 40_000_i32),
        );
        assert!(
            box_note.contains("bbox right"),
            "the box is named: {box_note}"
        );
        let leaf_note = refuse(
            &Map::default(),
            &node_with(|node| node.right = Child::Subsector(0x8000_usize)),
        );
        assert!(
            leaf_note.contains("subsector 32768"),
            "the child is named: {leaf_note}"
        );
        let node_note = refuse(
            &Map::default(),
            &node_with(|node| node.left = Child::Node(70_000)),
        );
        assert!(
            node_note.contains("node 70000"),
            "the child is named: {node_note}"
        );
    }

    #[test]
    fn the_vertex_space_fills_to_65535_and_then_refuses() {
        let mut map = Map::default();
        for x in 0..0xFFFE_i64 {
            vertex(&mut map, x, 0);
        }
        let tree = leaf_tree(vec![seg((0, 0), (0, 327_680), 0, 0)]);
        let run = to_vanilla(&map, &tree).expect("65535 in total still fit");
        assert_eq!(run.new_vertices.len(), 1, "the point above the row");
        let index = parsed(&run).segs.first().unwrap().v2;
        assert_eq!(
            usize::try_from(index).unwrap(),
            MAX_VERTICES.checked_sub(1).unwrap(),
            "the last index is the 65535th vertex"
        );
        let overgrown = leaf_tree(vec![
            seg((0, 0), (0, 327_680), 0, 0),
            seg((0, 0), (0, 393_216), 0, 0),
        ]);
        let cap_note = refuse(&map, &overgrown);
        assert!(
            cap_note.contains("65535") && cap_note.contains("extended"),
            "the cap names the fix: {cap_note}"
        );
        let mut huge = Map::default();
        for x in 0..0x0001_0000_i64 {
            vertex(&mut huge, x, 0);
        }
        let huge_note = refuse(&huge, &leaf_tree(vec![seg((0, 0), (0x0001_0000, 0), 0, 0)]));
        assert!(huge_note.contains("extended"), "the hint: {huge_note}");
    }

    #[test]
    fn walking_the_serialized_nodes_finds_each_sector() {
        let map = two_rooms();
        let (_, run) = run_of(&map);
        let set = parsed(&run);
        assert_eq!(leaf_sector(&map, &set, point(128, 128)), 0, "west room");
        assert_eq!(leaf_sector(&map, &set, point(384, 128)), 1, "east room");
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

    /// Walk the parsed tree to `spot`'s subsector and report the sector of
    /// that leaf's first seg.
    fn leaf_sector(map: &Map, set: &nodes::NodeSet, spot: Point) -> usize {
        let root = set.root().expect("the two-room tree has a node");
        let mut child = nodes::Child::Node(u32::try_from(root).unwrap());
        while let nodes::Child::Node(index) = child {
            let node = &set.nodes[usize::try_from(index).unwrap()];
            child = descend(node, spot);
        }
        match child {
            nodes::Child::Subsector(leaf) => first_seg_sector(map, set, leaf),
            nodes::Child::Node(_) => panic!("the walk stopped on a node"),
        }
    }

    /// The sector the first seg of subsector `leaf` bounds.
    fn first_seg_sector(map: &Map, set: &nodes::NodeSet, leaf: u32) -> usize {
        let sub = &set.subsectors[usize::try_from(leaf).unwrap()];
        let seg = &set.segs[usize::try_from(sub.first_seg).unwrap()];
        let line = &map.linedefs[usize::try_from(seg.linedef).unwrap()];
        let side = if seg.direction == 0 {
            line.front
        } else {
            line.back
        };
        let sidedef = side.expect("the leaf segs come from sided lines");
        map.sidedefs[sidedef].sector
    }
}
