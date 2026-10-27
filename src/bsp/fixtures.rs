//! The `BSP` builders every tree and serialization test shares: the same
//! small maps ([`square_room`], [`two_rooms`], [`l_room`], [`notch_room`],
//! assembled element by element with [`vertex`], [`sector`], [`sidedef`]
//! and [`linedef`], or all at once from a wall ring with [`room`]), the
//! fixed points and segs they are walked with ([`point`], [`seg`],
//! [`whole_seg`]) and the trees they are read back from ([`tree_of`],
//! [`leaf_tree`], [`node_tree`], [`dangling_tree`]).
//!
//! Test-only: the module is compiled under `cfg(test)` alone, so none of it
//! reaches the binary. The fixture maps are deliberately the same ones the
//! builder tests trace, which is the point: a tree built here and
//! serialized by the vanilla, extended or `GL` writer is the same geometry
//! measured three ways, so a claim that holds in one topic holds about the
//! very same map in the next.

use crate::bsp::{
    BBox, BspTree, BuildOptions, BuildSeg, BuiltNode, Child, Partition, Point, build_tree,
};
use crate::wad::map::{Linedef, Map, Sector, Sidedef, Vertex};

/// The `16.16` scale factor.
pub(super) const UNIT: i64 = 0x0001_0000_i64;

/// Append a vertex at whole units, returning its index.
pub(super) fn vertex(map: &mut Map, x: i64, y: i64) -> usize {
    let vx = i32::try_from(x).unwrap();
    let vy = i32::try_from(y).unwrap();
    map.vertices.push(Vertex {
        x: vx,
        y: vy,
        ..Default::default()
    });
    map.vertices.len().checked_sub(1).unwrap()
}

/// Append a sector, returning its index.
pub(super) fn sector(map: &mut Map) -> usize {
    map.sectors.push(Sector::default());
    map.sectors.len().checked_sub(1).unwrap()
}

/// Append a sidedef onto `sector`, returning its index.
pub(super) fn sidedef(map: &mut Map, sector: usize) -> usize {
    map.sidedefs.push(Sidedef {
        sector,
        ..Sidedef::default()
    });
    map.sidedefs.len().checked_sub(1).unwrap()
}

/// Append a linedef from `ends.0` to `ends.1` with the given sides.
pub(super) fn linedef(map: &mut Map, ends: (usize, usize), sides: (Option<usize>, Option<usize>)) {
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

/// One closed ring of one-sided walls (sector 0), interior to the right of
/// each wall as given.
pub(super) fn room(ring: &[(i64, i64)]) -> Map {
    let mut map = Map::default();
    let only = sector(&mut map);
    let side_index = sidedef(&mut map, only);
    let sides = (Some(side_index), None);
    let indices: Vec<usize> = ring.iter().map(|&(x, y)| vertex(&mut map, x, y)).collect();
    let back_to_front = indices.iter().skip(1).chain(indices.iter().take(1));
    for (&from, &to) in indices.iter().zip(back_to_front) {
        linedef(&mut map, (from, to), sides);
    }
    map
}

/// The unit square room: four walls, interior to the right.
pub(super) fn square_room() -> Map {
    room(&[(0, 256), (256, 256), (256, 0), (0, 0)])
}

/// Two 256-unit square rooms sharing the line `x = 256`.
pub(super) fn two_rooms() -> Map {
    let mut map = Map::default();
    let west = sector(&mut map);
    let east = sector(&mut map);
    let west_side = sidedef(&mut map, west);
    let east_side = sidedef(&mut map, east);
    let shared_front = sidedef(&mut map, east);
    let shared_back = sidedef(&mut map, west);
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

/// The six-vertex `L`-shaped room, interior to the right; its node build
/// cuts the bottom wall at `(128, 0)`, a point no vertex of the map sits on.
pub(super) fn l_room() -> Map {
    room(&[
        (0, 256),
        (128, 256),
        (128, 128),
        (256, 128),
        (256, 0),
        (0, 0),
    ])
}

/// The five-vertex notched room: the wall from `(0, 256)` to `(64, 64)`
/// extended meets the bottom wall at `x = 256 / 3`, so the node build's
/// split point needs fractional precision.
pub(super) fn notch_room() -> Map {
    room(&[(0, 256), (64, 64), (256, 64), (256, 0), (0, 0)])
}

/// A fixed point at whole map units.
pub(super) fn point(x: i64, y: i64) -> Point {
    Point::from_vertex(&Vertex {
        x: i32::try_from(x).unwrap(),
        y: i32::try_from(y).unwrap(),
        ..Default::default()
    })
}

/// A seg between raw `16.16` points, on side `source.1` of linedef
/// `source.0` and carrying `offset`.
pub(super) fn seg(from: (i64, i64), to: (i64, i64), source: (usize, u8), offset: i64) -> BuildSeg {
    BuildSeg {
        start: Point {
            x: from.0,
            y: from.1,
        },
        end: Point { x: to.0, y: to.1 },
        linedef: source.0,
        side: source.1,
        offset,
        sector: 0,
    }
}

/// A seg between whole-unit points, sourced at linedef `index`.
pub(super) fn whole_seg(index: usize, side: u8, corners: (i64, i64, i64, i64)) -> BuildSeg {
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

/// The default-options tree of `map`.
pub(super) fn tree_of(map: &Map) -> BspTree {
    build_tree(map, BuildOptions::default()).expect("the fixture builds")
}

/// A one-subsector tree of exactly `segs`, for limit tests.
pub(super) fn leaf_tree(segs: Vec<BuildSeg>) -> BspTree {
    let count = segs.len();
    BspTree {
        segs,
        subsectors: vec![(0, count)],
        nodes: Vec::new(),
        root: Child::Subsector(0),
        warnings: Vec::new(),
    }
}

/// A tree of one node and no segs, whose single leaf pair `edit` changes.
pub(super) fn node_tree(edit: impl FnOnce(&mut BuiltNode)) -> BspTree {
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

/// A tree whose root is the subsector `leaf` it does not hold.
pub(super) fn dangling_tree(leaf: usize) -> BspTree {
    BspTree {
        segs: Vec::new(),
        subsectors: vec![(0, 0)],
        nodes: Vec::new(),
        root: Child::Subsector(leaf),
        warnings: Vec::new(),
    }
}
