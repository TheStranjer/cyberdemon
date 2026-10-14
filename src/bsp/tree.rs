//! The recursive tree: convexity, region splitting, bounding boxes and
//! the assembled [`BspTree`].
//!
//! [`build_tree`] seeds one [`BuildSeg`] per linedef side and recurses:
//! a seg set that satisfies the convexity rule becomes a subsector,
//! otherwise [`choose`](super::choose) picks a partition line, the segs
//! are placed on its sides ([`split`](super::split)), and each child is
//! built deeper. Nodes are pushed after their children, so every node's
//! `Child` index points at an already-written entry and the root is the
//! final element of [`BspTree::nodes`].

use crate::bsp::BuildOptions;
use crate::bsp::Partition;
use crate::bsp::Point;
use crate::bsp::choose::choose_partition;
use crate::bsp::seg::{BuildSeg, WorkSeg, initial_segs};
use crate::bsp::side::{FixedLine, SegPlace, Side, classify_seg, point_side};
use crate::bsp::split::{Placement, place_seg};
use crate::error::CyberError;
use crate::fixed::FRACBITS;
use crate::wad::map::Map;

/// Recursion depth beyond which building gives up.
const MAX_DEPTH: usize = 512;

/// Maximum number of nodes, and of subsectors, in one tree.
const MAX_CHILDREN: usize = 0x7FFF;

/// An inclusive bounding box in whole map units: the floors of the
/// seg minima and the ceils of the seg maxima of one child.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BBox {
    /// Greatest `Y`, rounded up.
    pub top: i32,
    /// Smallest `Y`, rounded down.
    pub bottom: i32,
    /// Smallest `X`, rounded down.
    pub left: i32,
    /// Greatest `X`, rounded up.
    pub right: i32,
}

/// One side of a node: either another node or a subsector.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Child {
    /// Index into [`BspTree::nodes`].
    Node(usize),
    /// Index into [`BspTree::subsectors`].
    Subsector(usize),
}

/// One interior node: its partition line, both children and the
/// bounding box of each child's segs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BuiltNode {
    /// The partition line, from the source linedef of one of the
    /// region's segs.
    pub partition: Partition,
    /// Box of the segs right of the partition.
    pub right_bbox: BBox,
    /// Box of the segs left of the partition.
    pub left_bbox: BBox,
    /// Child holding the segs right (front) of the partition.
    pub right: Child,
    /// Child holding the segs left (back) of the partition.
    pub left: Child,
}

/// The finished tree: every leaf seg in subsector order, the subsector
/// ranges, the nodes (children before parents, root last) and the
/// warnings collected while building.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BspTree {
    /// The segs of every subsector, contiguous per subsector.
    pub segs: Vec<BuildSeg>,
    /// Each subsector as `(first seg, count)` into [`BspTree::segs`].
    pub subsectors: Vec<(usize, usize)>,
    /// The interior nodes, children before parents; the root is last.
    pub nodes: Vec<BuiltNode>,
    /// Which child the whole map hangs from (usually the final node).
    pub root: Child,
    /// Human-readable notes, one per subsector spanning several
    /// sectors.
    pub warnings: Vec<String>,
}

impl BspTree {
    /// Walk the tree to the subsector containing `point`, applying
    /// [`point_side`] at every node: right (front) descends to
    /// [`BuiltNode::right`], on-the-line and left to [`BuiltNode::left`].
    /// `None` only if the tree is malformed (an index that is not
    /// there).
    #[must_use]
    #[inline]
    pub fn locate(&self, point: Point) -> Option<usize> {
        let mut current = Some(self.root);
        while let Some(child) = current {
            match child {
                Child::Subsector(index) => return Some(index),
                Child::Node(index) => current = self.walk_step(index, point),
            }
        }
        None
    }

    /// The child of node `index` that `point` descends into.
    fn walk_step(&self, index: usize, point: Point) -> Option<Child> {
        let node = self.nodes.get(index)?;
        Some(match point_side(node.partition, point) {
            Side::Right => node.right,
            Side::Left => node.left,
        })
    }

    /// The segs of one subsector, or `None` when `index` is out of
    /// range.
    #[must_use]
    #[inline]
    pub fn subsector_segs(&self, index: usize) -> Option<&[BuildSeg]> {
        let (start, count) = *self.subsectors.get(index)?;
        let end = start.checked_add(count)?;
        self.segs.get(start..end)
    }
}

/// Build the `BSP` tree of `map` under `opts`.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] when the map yields no segs;
/// [`CyberError::Format`] on a dangling vertex, sidedef or sector
/// reference; [`CyberError::LimitExceeded`] past the depth or node
/// limits; [`CyberError::Internal`] when a non-convex region has no
/// valid partition candidate or a split degenerates.
#[inline]
pub fn build_tree(map: &Map, opts: BuildOptions) -> Result<BspTree, CyberError> {
    let initial = initial_segs(map)?;
    if initial.is_empty() {
        return Err(CyberError::InvalidArgument(
            "map has no linedefs with sidedefs: give the geometry sides before building".to_owned(),
        ));
    }
    let mut builder = Builder {
        opts,
        segs: Vec::new(),
        subsectors: Vec::new(),
        nodes: Vec::new(),
        warnings: Vec::new(),
    };
    let root = builder.build(&initial, 0)?;
    Ok(builder.finish(root))
}

/// The recursion state: options plus the tree being filled in.
struct Builder {
    /// Cost and candidate knobs.
    opts: BuildOptions,
    /// Final seg pool, extended by every leaf.
    segs: Vec<BuildSeg>,
    /// Final subsector ranges.
    subsectors: Vec<(usize, usize)>,
    /// Final nodes, children before parents.
    nodes: Vec<BuiltNode>,
    /// Leaf warnings.
    warnings: Vec<String>,
}

impl Builder {
    /// Hand the finished parts over to a [`BspTree`] rooted at `root`.
    fn finish(self, root: Child) -> BspTree {
        BspTree {
            segs: self.segs,
            subsectors: self.subsectors,
            nodes: self.nodes,
            root,
            warnings: self.warnings,
        }
    }

    /// Build one region: a leaf when convex, otherwise split on the
    /// chosen partition and recurse.
    fn build(&mut self, set: &[WorkSeg], depth: usize) -> Result<Child, CyberError> {
        check_limit(depth, MAX_DEPTH, "recursion depth")?;
        if is_convex(set) {
            return self.leaf(set);
        }
        let Some(partition) = choose_partition(set, self.opts) else {
            return Err(CyberError::Internal(format!(
                "no valid BSP partition for a region of {} segs: \
                 every candidate line leaves one side empty",
                set.len()
            )));
        };
        let line = FixedLine::from_partition(partition);
        let (right, left) = split_region(set, &line);
        if right.is_empty() || left.is_empty() {
            return Err(empty_side_error(set.len(), &right));
        }
        let right_bbox = region_bbox(&right);
        let left_bbox = region_bbox(&left);
        let deeper = depth.saturating_add(1);
        let right_child = self.build(&right, deeper)?;
        let left_child = self.build(&left, deeper)?;
        check_limit(self.nodes.len().saturating_add(1), MAX_CHILDREN, "nodes")?;
        let node_index = self.nodes.len();
        self.nodes.push(BuiltNode {
            partition,
            right_bbox,
            left_bbox,
            right: right_child,
            left: left_child,
        });
        Ok(Child::Node(node_index))
    }

    /// Close `set` as a subsector, warning when it spans sectors.
    fn leaf(&mut self, set: &[WorkSeg]) -> Result<Child, CyberError> {
        let index = self.subsectors.len();
        check_limit(index.saturating_add(1), MAX_CHILDREN, "subsectors")?;
        let start = self.segs.len();
        self.segs.extend(set.iter().map(|work| work.seg));
        self.subsectors.push((start, set.len()));
        let sectors = sector_count(set);
        if sectors > 1_usize {
            self.warnings.push(format!(
                "subsector {index} holds segs from {sectors} sectors"
            ));
        }
        Ok(Child::Subsector(index))
    }
}

/// The `Internal` error for a split that emptied one child.
fn empty_side_error(segs: usize, right: &[WorkSeg]) -> CyberError {
    let side = if right.is_empty() { "right" } else { "left" };
    CyberError::Internal(format!(
        "the BSP partition of a {segs}-seg region emptied the {side} child: \
         the splitting segs could not be cut at their endpoints"
    ))
}

/// Error when `count` grows past `limit`, naming what was counted.
fn check_limit(count: usize, limit: usize, what: &str) -> Result<(), CyberError> {
    if count > limit {
        return Err(CyberError::LimitExceeded(format!(
            "BSP {what} reached {count}, above the limit of {limit}: \
             simplify the map or raise the build limit"
        )));
    }
    Ok(())
}

/// Whether every seg of the set lies right of (or along, same
/// direction) every other seg's line: the convexity rule that ends
/// recursion.
fn is_convex(set: &[WorkSeg]) -> bool {
    set.iter().enumerate().all(|(here, work)| {
        let line = FixedLine::through_seg(&work.seg);
        set.iter().enumerate().all(|(there, other)| {
            here == there || classify_seg(&line, &other.seg) == SegPlace::Right
        })
    })
}

/// Distribute a region's segs to the two sides of `line`, cutting the
/// crossing ones and carrying each piece's partition along.
fn split_region(set: &[WorkSeg], line: &FixedLine) -> (Vec<WorkSeg>, Vec<WorkSeg>) {
    let mut right: Vec<WorkSeg> = Vec::new();
    let mut left: Vec<WorkSeg> = Vec::new();
    for work in set {
        match place_seg(line, &work.seg) {
            Placement::Right(seg) => right.push(WorkSeg {
                seg,
                partition: work.partition,
            }),
            Placement::Left(seg) => left.push(WorkSeg {
                seg,
                partition: work.partition,
            }),
            Placement::Split {
                right: head,
                left: tail,
            } => {
                right.push(WorkSeg {
                    seg: head,
                    partition: work.partition,
                });
                left.push(WorkSeg {
                    seg: tail,
                    partition: work.partition,
                });
            }
        }
    }
    (right, left)
}

/// The integer bounding box of a region's seg endpoints. An empty
/// region bounds nothing and gets the all-zero box.
fn region_bbox(set: &[WorkSeg]) -> BBox {
    if set.is_empty() {
        return BBox::default();
    }
    let mut low = Point {
        x: i64::MAX,
        y: i64::MAX,
    };
    let mut high = Point {
        x: i64::MIN,
        y: i64::MIN,
    };
    for work in set {
        for point in [work.seg.start, work.seg.end] {
            low.x = low.x.min(point.x);
            low.y = low.y.min(point.y);
            high.x = high.x.max(point.x);
            high.y = high.y.max(point.y);
        }
    }
    BBox {
        top: ceil_unit(high.y),
        bottom: floor_unit(low.y),
        left: floor_unit(low.x),
        right: ceil_unit(high.x),
    }
}

/// The distinct sectors a region touches.
fn sector_count(set: &[WorkSeg]) -> usize {
    let mut sectors: Vec<usize> = set.iter().map(|work| work.seg.sector).collect();
    sectors.sort_unstable();
    sectors.dedup();
    sectors.len()
}

/// The floor of a 16.16 value in whole units, saturating to `i32`.
fn floor_unit(value: i64) -> i32 {
    let shifted = value.checked_shr(FRACBITS).unwrap_or(0_i64);
    i32::try_from(shifted).unwrap_or(if shifted >= 0_i64 { i32::MAX } else { i32::MIN })
}

/// The ceiling of a 16.16 value in whole units, saturating to `i32`.
fn ceil_unit(value: i64) -> i32 {
    let bias = value.saturating_add(i64::from(crate::fixed::FRACUNIT).saturating_sub(1_i64));
    floor_unit(bias)
}

#[cfg(test)]
mod tests {
    use super::{
        BBox, Builder, Child, MAX_CHILDREN, ceil_unit, check_limit, floor_unit, region_bbox,
    };
    use crate::bsp::seg::WorkSeg;
    use crate::bsp::side::Side;
    use crate::bsp::{BspTree, BuildOptions, BuildSeg, Partition, Point, build_tree, side};
    use crate::fixed::hypot;
    use crate::wad::map::{Linedef, Map, Sector, Sidedef, Vertex};

    /// Append a vertex, returning its index.
    fn vertex(map: &mut Map, x: i64, y: i64) -> usize {
        let vx = i32::try_from(x).unwrap();
        let vy = i32::try_from(y).unwrap();
        map.vertices.push(Vertex { x: vx, y: vy });
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

    /// One closed ring of one-sided walls (sector 0), in the order
    /// given, with the interior to the right of each wall.
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
        Point::from_vertex(Vertex {
            x: i32::try_from(x).unwrap(),
            y: i32::try_from(y).unwrap(),
        })
    }

    /// Wrap a seg into a work item with a default partition.
    fn work_of(seg: BuildSeg) -> WorkSeg {
        WorkSeg {
            seg,
            partition: Partition::default(),
        }
    }

    /// A seg between raw fixed coordinates, for box tests.
    fn fixed_work(x1: i64, y1: i64, x2: i64, y2: i64) -> WorkSeg {
        work_of(BuildSeg {
            start: Point { x: x1, y: y1 },
            end: Point { x: x2, y: y2 },
            linedef: 0,
            side: 0,
            offset: 0,
            sector: 0,
        })
    }

    /// Every tree subsector must pass the convexity rule.
    fn assert_all_convex(tree: &BspTree) {
        for index in 0..tree.subsectors.len() {
            let segs = tree.subsector_segs(index).unwrap();
            let works: Vec<WorkSeg> = segs.iter().copied().map(work_of).collect();
            assert!(super::is_convex(&works), "subsector {index} is not convex");
        }
    }

    /// The summed 16.16 lengths of one linedef side's segs.
    fn side_length(tree: &BspTree, linedef: usize, side: u8) -> u64 {
        tree.segs
            .iter()
            .filter(|seg| seg.linedef == linedef && seg.side == side)
            .map(|seg| {
                hypot(
                    seg.end.x.saturating_sub(seg.start.x),
                    seg.end.y.saturating_sub(seg.start.y),
                )
            })
            .fold(0_u64, u64::saturating_add)
    }

    /// The true 16.16 length of one map side.
    fn true_length(map: &Map, linedef: usize, side: u8) -> u64 {
        let line: &Linedef = &map.linedefs[linedef];
        let (from, to) = if side == 0 {
            (line.v1, line.v2)
        } else {
            (line.v2, line.v1)
        };
        let va = map.vertices[from];
        let vb = map.vertices[to];
        hypot(
            i64::from(vb.x.saturating_sub(va.x))
                .checked_mul(0x1_0000)
                .unwrap(),
            i64::from(vb.y.saturating_sub(va.y))
                .checked_mul(0x1_0000)
                .unwrap(),
        )
    }

    #[test]
    fn a_square_room_is_one_convex_subsector() {
        let tree = build_tree(&square_room(), BuildOptions::default()).unwrap();
        assert!(tree.nodes.is_empty(), "no split was needed");
        assert_eq!(tree.root, Child::Subsector(0), "the root is the leaf");
        assert_eq!(tree.subsectors, vec![(0, 4)], "one leaf, four segs");
        assert!(tree.warnings.is_empty(), "single sector");
        assert_all_convex(&tree);
    }

    #[test]
    fn two_rooms_split_once_on_their_shared_line() {
        let tree = build_tree(&two_rooms(), BuildOptions::default()).unwrap();
        assert_eq!(tree.nodes.len(), 1, "one node separates the rooms");
        let root = tree.nodes.first().unwrap();
        assert_eq!(
            root.partition,
            Partition {
                x: 256,
                y: 0,
                dx: 0,
                dy: 256
            },
            "the shared front side's line"
        );
        assert_eq!(root.right, Child::Subsector(0), "east room (right side)");
        assert_eq!(root.left, Child::Subsector(1), "west room");
        assert_eq!(
            root.right_bbox,
            BBox {
                top: 256,
                bottom: 0,
                left: 256,
                right: 512
            },
            "east box"
        );
        assert_eq!(
            root.left_bbox,
            BBox {
                top: 256,
                bottom: 0,
                left: 0,
                right: 256
            },
            "west box"
        );
        assert!(tree.subsectors.len() >= 2, "two rooms, two subsectors");
        assert!(tree.segs.len() >= 8, "8 sides, at least 8 segs");
        assert_all_convex(&tree);
        assert!(tree.warnings.is_empty(), "each room stays one sector");
        let east_sectors: Vec<usize> = tree
            .subsector_segs(0)
            .unwrap()
            .iter()
            .map(|seg| seg.sector)
            .collect();
        assert_eq!(east_sectors, vec![1, 1, 1, 1], "east segs bound sector 1");
    }

    #[test]
    fn the_l_room_splits_into_two_convex_subsectors() {
        let tree = build_tree(&l_room(), BuildOptions::default()).unwrap();
        assert!(tree.subsectors.len() >= 2, "concave needs at least two");
        assert_eq!(tree.nodes.len(), 1, "one cut suffices");
        let root = tree.nodes.first().unwrap();
        assert_eq!(
            root.partition,
            Partition {
                x: 128,
                y: 256,
                dx: 0,
                dy: -128
            },
            "the inner corner line: cheapest split, earliest at the tie"
        );
        assert_all_convex(&tree);
    }

    /// Both sides of one linedef: seg lengths must sum to the side.
    fn check_conserved(map: &Map, tree: &BspTree, index: usize, line: &Linedef) {
        let unit = 0x1_0000_u64;
        let sides = [(0_u8, line.front), (1_u8, line.back)];
        for (side, present) in sides {
            let want = present.map_or(0_u64, |_| true_length(map, index, side));
            let got = side_length(tree, index, side);
            let diff = got.saturating_sub(want).max(want.saturating_sub(got));
            assert!(diff < unit, "linedef {index} side {side}: {got} vs {want}");
        }
    }

    /// Every seg of `index` must bound sector `want`.
    fn check_sector(tree: &BspTree, index: usize, want: usize) {
        for seg in tree.subsector_segs(index).unwrap() {
            assert_eq!(seg.sector, want, "subsector {index} bounds sector {want}");
        }
    }

    /// Build one fixture map and check every side's seg conservation.
    fn check_map_conservation(map: &Map) {
        let tree = build_tree(map, BuildOptions::default()).unwrap();
        for (index, line) in map.linedefs.iter().enumerate() {
            check_conserved(map, &tree, index, line);
        }
    }

    #[test]
    fn segs_are_conserved_per_linedef_side() {
        for map in [square_room(), two_rooms(), l_room()] {
            check_map_conservation(&map);
        }
    }

    #[test]
    fn walking_the_tree_finds_each_room() {
        let tree = build_tree(&two_rooms(), BuildOptions::default()).unwrap();
        let west = tree.locate(point(128, 128)).unwrap();
        let east = tree.locate(point(384, 128)).unwrap();
        assert_ne!(west, east, "the rooms land in different subsectors");
        check_sector(&tree, west, 0);
        check_sector(&tree, east, 1);
        assert_eq!(
            tree.locate(point(256, 128)),
            Some(west),
            "exactly on the partition walks left"
        );

        let l = build_tree(&l_room(), BuildOptions::default()).unwrap();
        let north_arm = l.locate(point(64, 192)).unwrap();
        let east_arm = l.locate(point(192, 64)).unwrap();
        assert_ne!(north_arm, east_arm, "the two arms land apart");
        check_sector(&l, north_arm, 0);
        check_sector(&l, east_arm, 0);
    }

    #[test]
    fn a_leaf_spanning_sectors_warns_but_builds() {
        let mut map = Map::default();
        map.sectors.push(Sector::default());
        map.sectors.push(Sector::default());
        let low = sidedef(&mut map, 0);
        let high = sidedef(&mut map, 1);
        let a = vertex(&mut map, 0, 0);
        let b = vertex(&mut map, 100, 0);
        let c = vertex(&mut map, 200, 0);
        linedef(&mut map, (a, b), (Some(low), None));
        linedef(&mut map, (b, c), (Some(high), None));
        let tree = build_tree(&map, BuildOptions::default()).unwrap();
        assert!(tree.nodes.is_empty(), "collinear same direction: convex");
        assert_eq!(tree.root, Child::Subsector(0), "one leaf");
        assert_eq!(tree.warnings.len(), 1, "the mixed leaf warned");
        assert!(
            tree.warnings
                .first()
                .is_some_and(|warning| warning.contains("2 sectors")),
            "warning names the sector count: {:?}",
            tree.warnings
        );
    }

    #[test]
    fn an_empty_map_is_an_invalid_argument() {
        let error = build_tree(&Map::default(), BuildOptions::default()).unwrap_err();
        assert_eq!(error.code(), "invalid_argument", "stable code");
        assert!(
            error
                .to_string()
                .starts_with("map has no linedefs with sidedefs"),
            "message names the problem: {error}"
        );
    }

    #[test]
    fn dangling_vertices_keep_failing_as_format_errors() {
        let mut map = square_room();
        map.linedefs.first_mut().unwrap().v1 = 999;
        let error = build_tree(&map, BuildOptions::default()).unwrap_err();
        assert_eq!(error.code(), "format_error", "dangling vertex code");
    }

    #[test]
    fn no_valid_candidate_is_internal_and_names_the_segs() {
        // The L room with a single candidate slot: only the first
        // distinct line is seen and it leaves one side empty.
        let error = build_tree(
            &l_room(),
            BuildOptions {
                max_candidates: 1,
                ..BuildOptions::default()
            },
        )
        .unwrap_err();
        assert_eq!(error.code(), "internal", "a stuck region is internal");
        assert!(
            error.to_string().contains("6 segs"),
            "the error names the region: {error}"
        );
    }

    #[test]
    fn a_degenerate_split_that_empties_a_child_is_internal() {
        // Two segs left of a giant north-facing partition; the crossing
        // one cannot actually be cut (the products leave i128), so both
        // segs fall back left and the right child comes out empty even
        // though the candidate scored valid.
        let giant = Partition {
            x: 0,
            y: 0,
            dx: 0,
            dy: i32::MAX,
        };
        let shift = |bits: u32| 1_i64.checked_shl(bits).unwrap();
        let crossing = BuildSeg {
            start: Point {
                x: shift(47).checked_neg().unwrap(),
                y: 0x1_0000,
            },
            end: Point {
                x: shift(40),
                y: 0x1_0000,
            },
            linedef: 0,
            side: 0,
            offset: 0,
            sector: 0,
        };
        let west = BuildSeg {
            start: Point {
                x: shift(44).checked_neg().unwrap(),
                y: 0x1_0000,
            },
            end: Point {
                x: shift(44).checked_neg().unwrap(),
                y: 0x2_0000,
            },
            linedef: 1,
            side: 0,
            offset: 0,
            sector: 0,
        };
        let set = [
            WorkSeg {
                seg: crossing,
                partition: giant,
            },
            WorkSeg {
                seg: west,
                partition: giant,
            },
        ];
        let opts = BuildOptions::default();
        let mut builder = Builder {
            opts,
            segs: Vec::new(),
            subsectors: Vec::new(),
            nodes: Vec::new(),
            warnings: Vec::new(),
        };
        let error = builder.build(&set, 0).unwrap_err();
        assert_eq!(error.code(), "internal", "degenerate split is internal");
        assert!(
            error.to_string().contains("right"),
            "the error names the emptied side: {error}"
        );
    }

    #[test]
    fn limits_report_themselves_before_being_crossed() {
        assert!(
            check_limit(512, 512, "recursion depth").is_ok(),
            "at the limit is fine"
        );
        let error = check_limit(513, 512, "recursion depth").unwrap_err();
        assert_eq!(error.code(), "limit_exceeded", "past it is a limit");
        assert!(
            error.to_string().contains("recursion depth reached 513"),
            "the message names the counter: {error}"
        );
        assert!(
            check_limit(MAX_CHILDREN, MAX_CHILDREN, "nodes").is_ok(),
            "the node cap itself is allowed"
        );
        assert_eq!(
            check_limit(MAX_CHILDREN.saturating_add(1), MAX_CHILDREN, "nodes")
                .unwrap_err()
                .code(),
            "limit_exceeded",
            "one past the node cap"
        );
    }

    #[test]
    fn integer_boxes_floor_and_ceil_across_quadrants() {
        assert_eq!(floor_unit(0x1_0000), 1_i32, "exact");
        assert_eq!(floor_unit(0x1_0001), 1_i32, "floor drops the remainder");
        assert_eq!(floor_unit(-1_i64), -1_i32, "a hair below zero floors to -1");
        assert_eq!(ceil_unit(1_i64), 1_i32, "a hair above zero ceils to 1");
        assert_eq!(ceil_unit(-1_i64), 0_i32, "a hair below zero ceils to 0");
        assert_eq!(ceil_unit(-0x1_0000), -1_i32, "exact negative");
        let quarter = 0x1_0000_i64.checked_div(4).unwrap();
        let set = [
            fixed_work(
                quarter.checked_mul(-14).unwrap(),
                quarter.checked_mul(-8).unwrap(),
                quarter.checked_mul(9).unwrap(),
                quarter.checked_mul(2).unwrap(),
            ),
            fixed_work(0_i64, 0_i64, 0_i64, 0_i64),
        ];
        assert_eq!(
            region_bbox(&set),
            BBox {
                top: 1,
                bottom: -2,
                left: -4,
                right: 3
            },
            "floor of -14/4 is -4, ceil of 9/4 is 3"
        );
    }

    #[test]
    fn an_empty_region_bounds_nothing_and_walks_survive_bad_trees() {
        assert_eq!(
            region_bbox(&[]),
            BBox::default(),
            "the empty box is all zero"
        );
        let broken = BspTree {
            segs: Vec::new(),
            subsectors: Vec::new(),
            nodes: Vec::new(),
            root: Child::Node(7),
            warnings: Vec::new(),
        };
        assert_eq!(
            broken.locate(point(0, 0)),
            None,
            "a missing node ends the walk"
        );
        assert_eq!(broken.subsector_segs(0), None, "no subsectors, no segs");
        let dangling_range = BspTree {
            segs: Vec::new(),
            subsectors: vec![(0, 4)],
            nodes: Vec::new(),
            root: Child::Subsector(0),
            warnings: Vec::new(),
        };
        assert_eq!(
            dangling_range.subsector_segs(0),
            None,
            "a range past the seg pool reads as nothing"
        );
        assert_eq!(
            side::point_side(
                Partition {
                    x: 0,
                    y: 0,
                    dx: 1,
                    dy: 0
                },
                point(5, 0)
            ),
            Side::Left,
            "the on-line convention holds through the tree's import"
        );
    }
}
