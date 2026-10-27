//! Initial segs: every linedef side becomes one [`BuildSeg`].
//!
//! A front side runs `v1` to `v2` (`side` `0`), a back side runs `v2` to
//! `v1` (`side` `1`), each carrying the sector of its sidedef. Vertex,
//! sidedef and sector references are validated up front: a dangling
//! reference is a [`CyberError::Format`]. Zero-length linedefs produce
//! no segs.

use crate::bsp::{Partition, Point};
use crate::error::CyberError;
use crate::wad::map::{Map, Vertex};

/// One seg of the builder: a directed piece of a linedef side in 16.16
/// coordinates.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BuildSeg {
    /// Start endpoint.
    pub start: Point,
    /// End endpoint.
    pub end: Point,
    /// Index of the source linedef in [`Map::linedefs`].
    pub linedef: usize,
    /// Source side: `0` front (ran `v1` to `v2`), `1` back.
    pub side: u8,
    /// 16.16 distance from the start of this linedef side, where the
    /// initial seg of a side starts at `0` and split pieces inherit
    /// their place along it.
    pub offset: i64,
    /// Index of the sector this side bounds in [`Map::sectors`].
    pub sector: usize,
}

/// A seg under construction plus the partition line of its source
/// linedef side, inherited unchanged by split pieces.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct WorkSeg {
    /// The seg itself.
    pub(crate) seg: BuildSeg,
    /// The integer partition line it sits on.
    pub(crate) partition: Partition,
}

/// The initial seg set of `map`, in linedef order (front side before
/// back side), or a `Format` error on a dangling reference. Zero-length
/// linedefs are skipped after validation.
pub(crate) fn initial_segs(map: &Map) -> Result<Vec<WorkSeg>, CyberError> {
    let mut segs = Vec::new();
    for (index, linedef) in map.linedefs.iter().enumerate() {
        let v1 = vertex_at(map, linedef.v1, index, "vertex")?;
        let v2 = vertex_at(map, linedef.v2, index, "vertex")?;
        let front = side_sector(map, linedef.front, index)?;
        let back = side_sector(map, linedef.back, index)?;
        if v1.x == v2.x && v1.y == v2.y {
            continue;
        }
        if let Some(sector) = front {
            segs.push(side_seg(&v1, &v2, (index, 0, sector)));
        }
        if let Some(sector) = back {
            segs.push(side_seg(&v1, &v2, (index, 1, sector)));
        }
    }
    Ok(segs)
}

/// The vertex `index`, or a `Format` error naming its owner.
fn vertex_at(map: &Map, index: usize, owner: usize, what: &str) -> Result<Vertex, CyberError> {
    map.vertices
        .get(index)
        .cloned()
        .ok_or_else(|| dangling(what, index, "linedef", owner))
}

/// The sector behind one sidedef side of `linedef`, `None` when the side
/// is absent, or a `Format` error on a dangling sidedef or sector.
fn side_sector(
    map: &Map,
    sidedef: Option<usize>,
    linedef: usize,
) -> Result<Option<usize>, CyberError> {
    let Some(sidedef_index) = sidedef else {
        return Ok(None);
    };
    let side = map
        .sidedefs
        .get(sidedef_index)
        .ok_or_else(|| dangling("sidedef", sidedef_index, "linedef", linedef))?;
    if side.sector >= map.sectors.len() {
        return Err(dangling("sector", side.sector, "sidedef", sidedef_index));
    }
    Ok(Some(side.sector))
}

/// The work seg of one side: its endpoints follow the side's direction,
/// its partition follows the linedef. `source` bundles
/// `(linedef, side, sector)`.
fn side_seg(v1: &Vertex, v2: &Vertex, source: (usize, u8, usize)) -> WorkSeg {
    let (linedef, side, sector) = source;
    let (from, to) = if side == 0 { (v1, v2) } else { (v2, v1) };
    WorkSeg {
        seg: BuildSeg {
            start: Point::from_vertex(from),
            end: Point::from_vertex(to),
            linedef,
            side,
            offset: 0_i64,
            sector,
        },
        partition: Partition::from_vertices(v1, v2, side),
    }
}

/// The dangling-reference error naming the index and its owner.
fn dangling(what: &str, index: usize, owner: &str, owner_index: usize) -> CyberError {
    CyberError::Format(format!(
        "{owner} {owner_index} references {what} {index}, which does not exist: \
         fix the map's element indices before building"
    ))
}

#[cfg(test)]
mod tests {
    use super::{WorkSeg, initial_segs};
    use crate::bsp::Partition;
    use crate::bsp::fixtures::{linedef, point, sector, sidedef, vertex};
    use crate::error::CyberError;
    use crate::wad::map::Map;

    /// A two-sector wall: shared side 0 is front and 1 is back.
    fn wall_map() -> Map {
        let mut map = Map::default();
        let low = sector(&mut map);
        let high = sector(&mut map);
        let front = sidedef(&mut map, low);
        let back = sidedef(&mut map, high);
        vertex(&mut map, 0, 0);
        vertex(&mut map, 64, 0);
        linedef(&mut map, (0, 1), (Some(front), Some(back)));
        map
    }

    #[test]
    fn a_two_sided_line_yields_front_and_back_segs() {
        let segs = initial_segs(&wall_map()).unwrap();
        assert_eq!(segs.len(), 2, "one seg per side");
        let front: &WorkSeg = &segs[0];
        assert_eq!(front.seg.start, point(0, 0), "front runs v1 to v2");
        assert_eq!(front.seg.end, point(64, 0), "front end");
        assert_eq!(front.seg.linedef, 0, "source linedef");
        assert_eq!(front.seg.side, 0, "front side number");
        assert_eq!(front.seg.sector, 0, "front sector");
        assert_eq!(front.seg.offset, 0, "initial offset is zero");
        assert_eq!(
            front.partition,
            Partition {
                x: 0,
                y: 0,
                dx: 64,
                dy: 0
            },
            "front partition"
        );
        let back: &WorkSeg = &segs[1];
        assert_eq!(back.seg.start, point(64, 0), "back runs v2 to v1");
        assert_eq!(back.seg.end, point(0, 0), "back end");
        assert_eq!(back.seg.side, 1, "back side number");
        assert_eq!(back.seg.sector, 1, "back sector");
        assert_eq!(
            back.partition,
            Partition {
                x: 64,
                y: 0,
                dx: -64,
                dy: 0
            },
            "back partition is reversed"
        );
    }

    #[test]
    fn zero_length_linedefs_are_skipped() {
        let mut map = wall_map();
        let extra = sidedef(&mut map, 0);
        linedef(&mut map, (0, 0), (Some(extra), None));
        let segs = initial_segs(&map).unwrap();
        assert_eq!(segs.len(), 2, "the degenerate line adds nothing");
    }

    #[test]
    fn linedefs_without_sides_add_nothing() {
        let mut map = Map::default();
        vertex(&mut map, 0, 0);
        vertex(&mut map, 32, 0);
        linedef(&mut map, (0, 1), (None, None));
        let segs = initial_segs(&map).unwrap();
        assert!(segs.is_empty(), "no sidedefs, no segs");
    }

    #[test]
    fn a_dangling_vertex_is_a_format_error() {
        let mut map = Map::default();
        vertex(&mut map, 0, 0);
        linedef(&mut map, (0, 99), (None, None));
        let error = initial_segs(&map).unwrap_err();
        assert!(
            matches!(&error, CyberError::Format(message)
                if message.contains("linedef 0") && message.contains("vertex 99")),
            "dangling v2: {error}"
        );
    }

    #[test]
    fn a_dangling_sidedef_is_a_format_error() {
        let mut map = Map::default();
        vertex(&mut map, 0, 0);
        vertex(&mut map, 8, 0);
        linedef(&mut map, (0, 1), (Some(5), None));
        let error = initial_segs(&map).unwrap_err();
        assert!(
            matches!(&error, CyberError::Format(message)
                if message.contains("sidedef 5")),
            "dangling sidedef: {error}"
        );
    }

    #[test]
    fn a_dangling_sector_is_a_format_error() {
        let mut map = Map::default();
        vertex(&mut map, 0, 0);
        vertex(&mut map, 8, 0);
        let broken = sidedef(&mut map, 7);
        linedef(&mut map, (0, 1), (Some(broken), None));
        let error = initial_segs(&map).unwrap_err();
        assert!(
            matches!(&error, CyberError::Format(message)
                if message.contains("sector 7") && message.contains("sidedef 0")),
            "dangling sector: {error}"
        );
    }
}
