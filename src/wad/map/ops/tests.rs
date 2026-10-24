//! Shared builders for the linedef geometry engine's tests: the error-code
//! reader, the point, sidedef and linedef builders, and the two fixtures
//! the split and join topics cut and weld.
//!
//! [`split`] measures the cut, its plan and every refusal; [`join`] welds
//! pairs back, reads the removal engine's report and checks the split and
//! join round-trip each other.

mod join;
mod split;

use crate::error::CyberError;
use crate::wad::classify::MapFormat;
use crate::wad::map::common::fixtures::{name, vertex};
use crate::wad::map::{Linedef, Map, Sector, Sidedef};

use super::Point;

/// The error code of `error`, for compact asserts.
pub(super) fn code(error: &CyberError) -> &str {
    error.code()
}

/// A point at `x`/`y`.
pub(super) const fn point(x: i32, y: i32) -> Point {
    Point { x, y }
}

/// A sidedef with `x_offset`, a `middle` texture and a sector.
pub(super) fn side(x_offset: i32, middle: &str, sector: usize) -> Sidedef {
    Sidedef {
        x_offset,
        upper: "-".to_owned(),
        lower: "-".to_owned(),
        middle: middle.to_owned(),
        sector,
        ..Default::default()
    }
}

/// A line from `v1` to `v2` with the given sides and flags.
pub(super) fn line(v1: usize, v2: usize, front: Option<usize>, back: Option<usize>) -> Linedef {
    Linedef {
        v1,
        v2,
        flags: 1_u32,
        front,
        back,
        ..Default::default()
    }
}

/// The split fixture: one line `(0,0)` to `(256,0)`, front sidedef 0
/// (`x_offset` 10, sector 0) and back sidedef 1 (`x_offset` 5,
/// sector 1), with a tag and a special to copy.
pub(super) fn split_fixture(format: MapFormat) -> Map {
    Map {
        name: name("MAP01"),
        format,
        vertices: vec![vertex(0, 0), vertex(256, 0)],
        linedefs: vec![Linedef {
            special: 9,
            tag: 3,
            flags: 0x104,
            ..line(0, 1, Some(0), Some(1))
        }],
        sidedefs: vec![side(10, "WALL1", 0), side(5, "-", 1)],
        sectors: vec![Sector::default(), Sector::default()],
        ..Default::default()
    }
}

/// A map for joining: vertices `(0,0)`, `(128,0)`, `(256,0)` and lines
/// head-to-tail along the run, each fronted by its own sector-0 side.
pub(super) fn join_fixture(format: MapFormat) -> Map {
    Map {
        name: name("MAP01"),
        format,
        vertices: vec![vertex(0, 0), vertex(128, 0), vertex(256, 0)],
        linedefs: vec![line(0, 1, Some(0), None), line(1, 2, Some(1), None)],
        sidedefs: vec![side(0, "WALL1", 0), side(40, "WALL1", 0)],
        sectors: vec![Sector::default()],
        ..Default::default()
    }
}
