//! The geometry an edit is compared against, and the split vertices a
//! discarded node run takes with it.
//!
//! [`signature`] folds a [`Map`] into the facts a node build reads: two maps
//! share one signature exactly as long as a rebuild would work off the same
//! geometry, which is what tells [`super::edit_in_place`] whether the
//! block's generated lumps still stand. The comparison cannot see the
//! vertices past the last one a linedef ends at, and neither does a node
//! build, so the tail holds two very different kinds of point: the split
//! vertices a classic `build_nodes` appended, which belong to the run it
//! wrote, and the unwired points a caller added, which belong to the map.
//! [`built_vertices`] and [`strip_split_vertices`] tell the two apart by the
//! run's own `SEGS` records, so invalidating a block sheds its own leftovers
//! and nothing else.

use alloc::collections::BTreeSet;
use alloc::vec::Vec;

use crate::wad::map::model::Map;
use crate::wad::map::nodes::seg_vertex_indices;

/// The geometry a node build reads from one linedef.
type LineRef = (usize, usize, Option<usize>, Option<usize>);

/// The geometry facts the generated node lumps depend on: two maps share
/// one signature exactly as long as a node rebuild would read the same
/// geometry from either.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct Signature {
    /// The coordinates of the vertices a linedef ends at, in index order:
    /// the tail past the last reference is invisible to a node build — both
    /// the split points it appended and the unwired points an `add_vertex`
    /// left behind — so it stays out of the comparison. See
    /// [`strip_split_vertices`] for the part of that tail an invalidation
    /// takes back.
    vertices: Vec<(i32, i32)>,
    /// Every linedef's endpoints and side assignments, in index order.
    linedefs: Vec<LineRef>,
    /// Every sidedef's sector index, in index order.
    sidedefs: Vec<usize>,
    /// How many sectors the map holds: the `REJECT` matrix is sized by it,
    /// so an added sector invalidates the run even with the geometry still.
    sector_count: usize,
}

/// The geometry signature of `map`.
pub(super) fn signature(map: &Map) -> Signature {
    Signature {
        vertices: map
            .vertices
            .iter()
            .take(used_vertices(map))
            .map(|vertex| (vertex.x, vertex.y))
            .collect(),
        linedefs: map
            .linedefs
            .iter()
            .map(|line| (line.v1, line.v2, line.front, line.back))
            .collect(),
        sidedefs: map.sidedefs.iter().map(|side| side.sector).collect(),
        sector_count: map.sectors.len(),
    }
}

/// How many vertices the linedefs reference: one past the highest endpoint
/// index. Vertices trailing past it are invisible to a node build
/// (`build_nodes` keeps its split points there, an `add_vertex` appends its
/// point there until a line ends at it, and removal cascades leave orphans
/// there), so they stay out of the signature.
fn used_vertices(map: &Map) -> usize {
    map.linedefs
        .iter()
        .flat_map(|line| [line.v1, line.v2])
        .max()
        .and_then(|top| top.checked_add(1))
        .unwrap_or(0_usize)
}

/// The vertex indices the block's own classic node run names, read off the
/// `SEGS` records of the map as the edit left it: a classic `build_nodes`
/// references every split vertex it appended, and only those sit past the
/// last linedef endpoint.
///
/// `None` whenever the block cannot say which vertices it grew: no `SEGS`
/// lump at all (a hand-built map), an empty one (a freshly created block, an
/// already-invalidated run, or a `ZDoom` extended build, which empties
/// `SEGS` and keeps its split points inside its own lump), a `UDMF` block,
/// or records that no longer answer to the map's present numbering — which
/// is how an edit that renumbered the vertex table (a removal cascade, an
/// overlay rebuild) keeps its leftover points out of the shed. `None` is the
/// answer that drops nothing.
pub(super) fn built_vertices(map: &Map) -> Option<BTreeSet<usize>> {
    let segs = map
        .other
        .iter()
        .find(|lump| lump.name.as_bytes() == b"SEGS")?;
    seg_vertex_indices(&segs.data, map.vertices.len(), map.linedefs.len())
}

/// Drops the vertices the block's own node run appended: the tail of `map`'s
/// vertex list that its `SEGS` records name past [`used_vertices`], which is
/// exactly where a classic `build_nodes` keeps its split points.
///
/// Call this while the run is still in `map`: the records are what says
/// which vertices grew with it, so after [`super::clear_node_lumps`] emptied
/// `SEGS` there is nothing left to read and the points stay.
///
/// The scan runs down from the end and stops at the first point the run
/// never named, because that point is the caller's: an `add_vertex` append
/// made after the build cannot be in the run's records, and neither can any
/// vertex above it go without renumbering it — the caller holds the index it
/// was told, and every kept index above a dropped one would silently mean a
/// different point afterwards. So a run whose tail the caller has built on
/// is left alone, and its leftover split points are ordinary unreferenced
/// vertices: invisible to the node builder, reported by `validate_map` as
/// info, and removable with `remove_vertex`.
pub(super) fn strip_split_vertices(map: &mut Map) {
    let Some(named) = built_vertices(map) else {
        return;
    };
    let wired = used_vertices(map);
    let mut keep = map.vertices.len();
    while let Some(top) = keep.checked_sub(1) {
        if top < wired || !named.contains(&top) {
            break;
        }
        keep = top;
    }
    map.vertices.truncate(keep);
}

#[cfg(test)]
mod tests {
    use super::{built_vertices, signature, strip_split_vertices, used_vertices};
    use crate::wad::archive::Lump;
    use crate::wad::classify::MapFormat;
    use crate::wad::map::model::{Linedef, Map, Sector, Sidedef, Vertex};
    use crate::wad::name::LumpName;
    use alloc::collections::BTreeSet;

    /// A parsed lump name.
    fn name(text: &str) -> LumpName {
        LumpName::parse(text).expect("test names are valid")
    }

    /// A vertex at `x`/`y`.
    fn vertex(x: i32, y: i32) -> Vertex {
        Vertex {
            x,
            y,
            ..Default::default()
        }
    }

    /// A one-sided line from vertex 0 to vertex `top`.
    fn line(top: usize) -> Linedef {
        Linedef {
            v1: 0,
            v2: top,
            front: Some(0),
            ..Default::default()
        }
    }

    /// A map of `total` vertices with one line spanning the first `wired`
    /// of them and a `SEGS` lump holding `segs`.
    fn map_with(wired: usize, total: usize, segs: &[u8]) -> Map {
        let mut map = Map::new_empty(name("MAP01"), MapFormat::Doom);
        map.vertices = (0..total)
            .map(|at| vertex(i32::try_from(at).expect("a test count fits i32"), 0_i32))
            .collect();
        map.sidedefs = vec![Sidedef::default()];
        if wired > 1 {
            map.linedefs = vec![line(wired.saturating_sub(1))];
        }
        map.other = vec![Lump::new(name("SEGS"), segs.to_vec())];
        map
    }

    /// One 12-byte `SEGS` record from vertex `start` to vertex `end` on
    /// linedef 0: the shape a classic build writes for one split seg.
    fn seg(start: u16, end: u16) -> Vec<u8> {
        let mut bytes = start.to_le_bytes().to_vec();
        bytes.extend_from_slice(&end.to_le_bytes());
        bytes.extend_from_slice(&[0_u8; 8]);
        bytes
    }

    #[test]
    fn used_vertices_reaches_one_past_the_last_endpoint() {
        assert_eq!(used_vertices(&map_with(0, 0, b"")), 0, "no lines");
        assert_eq!(used_vertices(&map_with(4, 4, b"")), 4, "one line 0..3");
        assert_eq!(
            used_vertices(&map_with(2, 6, b"")),
            2,
            "the four points past the line are invisible to it"
        );
    }

    #[test]
    fn built_vertices_reads_the_records_the_block_carries() {
        let named = built_vertices(&map_with(4, 5, &seg(3, 4))).expect("one record answers");
        assert_eq!(
            named,
            BTreeSet::from([3_usize, 4_usize]),
            "both endpoints of the record"
        );
        assert!(
            built_vertices(&map_with(4, 5, b"")).is_none(),
            "an empty run says nothing about the tail"
        );
        assert!(
            built_vertices(&map_with(4, 5, &[0_u8; 8])).is_none(),
            "a partial record says nothing"
        );
        assert!(
            built_vertices(&map_with(4, 5, &seg(0, 40))).is_none(),
            "a record naming a vertex the map does not hold is from an older numbering"
        );
    }

    #[test]
    fn strip_drops_the_named_tail_past_the_last_endpoint() {
        let mut map = map_with(4, 5, &seg(3, 4));
        strip_split_vertices(&mut map);
        assert_eq!(map.vertices.len(), 4, "the run's own split point went");
    }

    #[test]
    fn strip_drops_nothing_without_a_run() {
        let mut unwired = map_with(2, 4, b"");
        strip_split_vertices(&mut unwired);
        assert_eq!(
            unwired.vertices.len(),
            4,
            "with no run to shed, the caller's unwired points stay"
        );

        let mut partial = map_with(2, 4, &[0_u8; 8]);
        strip_split_vertices(&mut partial);
        assert_eq!(
            partial.vertices.len(),
            4,
            "and an unreadable run sheds nothing"
        );
    }

    #[test]
    fn strip_stops_at_a_point_the_run_never_named() {
        let mut map = map_with(4, 6, &seg(3, 4));
        strip_split_vertices(&mut map);
        assert_eq!(
            map.vertices.len(),
            6,
            "the point appended after the run holds the tail, and the split point with it"
        );
    }

    #[test]
    fn strip_never_touches_a_vertex_a_linedef_ends_at() {
        let mut map = map_with(3, 3, &seg(0, 2));
        strip_split_vertices(&mut map);
        assert_eq!(map.vertices.len(), 3, "every point is wired");
    }

    #[test]
    fn the_signature_ignores_the_tail_and_notices_a_sector() {
        let wired = map_with(4, 4, b"");
        let mut appended = map_with(4, 5, b"");
        assert_eq!(
            signature(&wired),
            signature(&appended),
            "an unwired append is not geometry a node build would see"
        );
        appended.sectors.push(Sector::default());
        assert_ne!(
            signature(&wired),
            signature(&appended),
            "the sector count sizes REJECT, so a new sector is"
        );
    }
}
